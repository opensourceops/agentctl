"""Named disposable-service and matrix operations for cookbook examples 14–18.

These reviewed host Python integrations are not an OS sandbox. HTTP probes only
access the separately started loopback fixture; no production deployment occurs.
"""
import hashlib
import json
import re
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, build_opener


def schema(fields):
    return {"type": "object", "additionalProperties": False, "required": list(fields),
            "properties": {name: {"type": kind} for name, kind in fields.items()}}


INPUT_SCHEMAS = {
    "service-snapshot": schema({"desiredPath": "string"}),
    "service-probe": schema({"endpointPath": "string", "expectedText": "string", "requireHealthy": "boolean"}),
    "service-deploy": schema({"desiredPath": "string"}),
    "service-check": schema({"expectedText": "string", "requireReady": "boolean"}),
    "service-check-item": schema({"service": "string", "check": "string", "index": "integer", "maxReplicas": "integer", "maxTimeoutSeconds": "integer"}),
    "aggregate-services": schema({"items": "array"}),
}


OUTPUT_SCHEMAS = {
    "service-snapshot": schema({"priorText": "string", "priorSha256": "string", "priorVersion": "string", "desiredText": "string", "desiredSha256": "string", "desiredVersion": "string", "scope": "string"}),
    "service-probe": schema({"verified": "boolean", "reason": "string", "httpStatus": "integer", "version": "string", "healthy": "boolean", "sha256": "string", "url": "string", "scope": "string"}),
    "service-deploy": schema({"mutation": "integer", "version": "string", "desiredText": "string", "desiredSha256": "string", "scope": "string"}),
    "service-check": schema({"verified": "boolean", "version": "string", "healthy": "boolean", "mutation": "integer", "sha256": "string", "dependencyChecked": "boolean", "reason": "string", "scope": "string"}),
    "service-check-item": schema({"service": "string", "check": "string", "passed": "boolean", "index": "integer", "source": "string", "sha256": "string"}),
    "aggregate-services": schema({"items": "array", "checks": "array", "passed": "boolean", "count": "integer", "scope": "string"}),
}
for _output in OUTPUT_SCHEMAS.values():
    _output["properties"]["reportText"] = {"type": "string"}
    _output["required"].append("reportText")


def service_config(text):
    if not isinstance(text, str) or len(text.encode("utf-8")) > 65536:
        raise ValueError("service configuration must be at most 64 KiB of JSON text")
    value = json.loads(text)
    if not isinstance(value, dict) or not isinstance(value.get("version"), str) or not value["version"].strip() or type(value.get("healthy")) is not bool:
        raise ValueError("service configuration requires a nonempty version and boolean healthy field")
    return value


def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def service_snapshot(common, payload):
    prior = common.read("artifacts/service.json")
    desired = common.read(payload["desiredPath"])
    prior_value, desired_value = service_config(prior), service_config(desired)
    common.write_atomic("artifacts/prior-service.json", prior)
    return {"priorText": prior, "priorSha256": digest(prior), "priorVersion": prior_value["version"],
            "desiredText": desired, "desiredSha256": digest(desired), "desiredVersion": desired_value["version"],
            "scope": "captured local service bytes; no deployment performed by snapshot"}


class NoRedirects(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        raise ValueError("local service probes never follow redirects")


def service_probe(common, payload):
    endpoint = common.document(payload["endpointPath"])
    url = endpoint.get("url")
    if not isinstance(url, str):
        raise ValueError("service endpoint requires one loopback URL")
    parsed = urlsplit(url)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.username is not None
            or parsed.password is not None or parsed.port is None or not 1 <= parsed.port <= 65535
            or parsed.path != "/service.json" or parsed.query or parsed.fragment):
        raise ValueError("service probe requires http://127.0.0.1:PORT/service.json without credentials, query or fragment")
    expected = service_config(payload["expectedText"])
    if type(payload["requireHealthy"]) is not bool:
        raise ValueError("requireHealthy must be a boolean")
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with opener.open(url, timeout=5) as response:
        if response.status != 200:
            raise ValueError("local service probe did not return HTTP 200")
        body = response.read(65537)
    if len(body) > 65536:
        raise ValueError("local service response exceeds 64 KiB")
    actual_text = body.decode("utf-8")
    actual = service_config(actual_text)
    matched = actual_text == payload["expectedText"] and actual == expected
    verified = matched and (not payload["requireHealthy"] or actual["healthy"] is True)
    reason = "verified" if verified else "observed configuration differs from expected bytes" if not matched else "observed service is unhealthy"
    return {"verified": verified, "reason": reason, "httpStatus": 200, "version": actual["version"], "healthy": actual["healthy"],
            "sha256": digest(actual_text), "url": url, "scope": "real HTTP probe of disposable loopback state"}


def service_deploy(common, payload):
    desired = common.read(payload["desiredPath"])
    configuration = service_config(desired)
    journal_path = common.path("artifacts/deployment-journal.json")
    if journal_path.exists() and common.document("artifacts/deployment-journal.json").get("phase") != "completed":
        raise ValueError("an incomplete local deployment journal requires explicit reconciliation before another dispatch")
    counter = common.path("artifacts/mutations.txt")
    previous = int(counter.read_text()) if counter.exists() else 0
    if previous < 0:
        raise ValueError("local deployment counter is invalid")
    journal = {"phase": "intent", "previousMutation": previous, "nextMutation": previous + 1,
               "desiredSha256": digest(desired)}
    common.write_atomic("artifacts/deployment-journal.json", json.dumps(journal) + "\n")
    # These writes are separate effects of the trusted process, not a transaction.
    # A crash between them leaves intent evidence and must not trigger blind retry.
    common.write_atomic("artifacts/service.json", desired)
    common.write_atomic("artifacts/mutations.txt", str(previous + 1))
    common.write_atomic("artifacts/deployment-journal.json", json.dumps({**journal, "phase": "completed"}) + "\n")
    return {"mutation": previous + 1, "version": configuration["version"], "desiredText": desired,
            "desiredSha256": digest(desired), "scope": "non-idempotent local deployment; journal does not provide transactional rollback"}


def service_check(common, payload):
    expected = service_config(payload["expectedText"])
    actual_text = common.read("artifacts/service.json")
    actual = service_config(actual_text)
    if actual_text != payload["expectedText"] or actual != expected or actual["healthy"] is not True:
        raise ValueError("deployed local configuration differs from the expected healthy configuration")
    if type(payload["requireReady"]) is not bool:
        raise ValueError("requireReady must be a boolean")
    ready = not payload["requireReady"] or common.path("fixtures/retry-ready.txt").is_file()
    counter = common.path("artifacts/mutations.txt")
    mutations = int(counter.read_text()) if counter.exists() else 0
    return {"verified": ready, "version": actual["version"], "healthy": actual["healthy"],
            "mutation": mutations, "sha256": digest(actual_text), "dependencyChecked": payload["requireReady"],
            "reason": "validated" if ready else "fixture test dependency is unavailable; restore it before retry",
            "scope": "actual local configuration validation; this check does not claim HTTP or external deployment health"}


def service_check_item(common, payload):
    service, check, index = payload["service"], payload["check"], payload["index"]
    if not isinstance(service, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", service):
        raise ValueError("service must be a bounded filename-safe identifier")
    if check not in {"syntax", "config"} or type(index) is not int or not 0 <= index < 16:
        raise ValueError("check must be syntax/config and index must be within the bounded matrix")
    for name in ["maxReplicas", "maxTimeoutSeconds"]:
        if type(payload[name]) is not int or not 1 <= payload[name] <= 300:
            raise ValueError("matrix thresholds must be integers from 1 to 300")
    filename = f"fixtures/{service}.py" if check == "syntax" else f"fixtures/{service}.json"
    text = common.read(filename)
    if check == "syntax":
        compile(text, filename, "exec")  # Parse without executing supplied code.
    else:
        config = json.loads(text)
        if type(config.get("replicas")) is not int or not 1 <= config["replicas"] <= payload["maxReplicas"]:
            raise ValueError("service replicas exceed the configured matrix limit")
        if type(config.get("timeoutSeconds")) is not int or not 1 <= config["timeoutSeconds"] <= payload["maxTimeoutSeconds"]:
            raise ValueError("service timeout exceeds the configured matrix limit")
    return {"service": service, "check": check, "passed": True, "index": index,
            "source": filename, "sha256": digest(text)}


def aggregate_services(common, payload):
    items = payload["items"]
    if not isinstance(items, list) or not 1 <= len(items) <= 16:
        raise ValueError("aggregation requires one to sixteen completed matrix results")
    checks = []
    for index, item in enumerate(items):
        value = item.get("output") if isinstance(item, dict) else None
        if not isinstance(value, dict) or value.get("index") != index or value.get("passed") is not True:
            raise ValueError("matrix child results are missing, failed or out of order")
        checks.append(value)
    if len({(value["service"], value["check"]) for value in checks}) != len(checks):
        raise ValueError("matrix contains duplicate service/check pairs")
    return {"items": items, "checks": checks, "passed": True, "count": len(checks),
            "scope": "ordered aggregation of completed syntax/config checks; no service code was executed"}


def register(common):
    operations = {"service-snapshot": service_snapshot, "service-probe": service_probe,
                  "service-deploy": service_deploy, "service-check": service_check,
                  "service-check-item": service_check_item, "aggregate-services": aggregate_services}
    return {name: (lambda payload, operation=operation: operation(common, payload)) for name, operation in operations.items()}
