"""Input-derived change review and remediation; model claims are never completion evidence."""
import copy
import hashlib
import json
import re


def schema(fields):
    return {"type": "object", "required": list(fields), "additionalProperties": False,
            "properties": {name: {"type": kind} for name, kind in fields.items()}}


INPUT_SCHEMAS = {
    "configuration-drift": schema({"actualPath": "string", "desired": "object", "environment": "string"}),
    "plan-change": schema({"configurationPath": "string", "proposedTimeout": "integer", "maxTimeout": "integer"}),
    "review-change": schema({"context": "object", "proposal": "object", "review": "object"}),
    "apply-change": schema({"context": "object", "review": "object", "requireStagedValue": "boolean"}),
    "prepare-remediation": schema({"configurationPath": "string", "maxTimeout": "integer", "maxReduction": "integer"}),
    "validate-remediation-target": schema({"context": "object", "proposal": "object"}),
    "remediation-step": schema({"configurationPath": "string", "maxTimeout": "integer", "maxReduction": "integer", "iteration": "integer", "previous": "object"}),
    "remediation-context": schema({"configurationPath": "string", "maxTimeout": "integer", "maxReduction": "integer", "iteration": "integer", "previous": "object"}),
    "apply-remediation": schema({"context": "object", "proposal": "object", "requireStagedValue": "boolean"}),
    "verify-remediation": schema({"configurationPath": "string", "maxTimeout": "integer"}),
}


_CONTEXT_FIELDS = {"configurationPath": "string", "configuration": "object", "sourceSha256": "string",
                   "current": "object", "expectedTimeout": "integer", "maxTimeout": "integer",
                   "maxReduction": "integer", "iteration": "integer"}
_STEP_FIELDS = {"done": "boolean", "timeoutSeconds": "integer", "iteration": "integer", "artifactSha256": "string"}
OUTPUT_SCHEMAS = {
    "configuration-drift": schema({"desired": "object", "actual": "object", "drift": "array", "inputEnvironment": "string"}),
    "plan-change": schema({"configuration": "object", "sourceSha256": "string", "configurationPath": "string",
                           "proposedTimeout": "integer", "maxTimeout": "integer", "allowed": "boolean", "scope": "string"}),
    "review-change": schema({"approved": "boolean", "timeoutSeconds": "integer", "sourceSha256": "string", "decision": "string", "validation": "string"}),
    "apply-change": schema({"executed": "boolean", "configuration": "object", "artifactSha256": "string", "originalPreserved": "boolean", "scope": "string"}),
    "prepare-remediation": schema({**_CONTEXT_FIELDS, "targetTimeout": "integer"}),
    "validate-remediation-target": schema({"validated": "boolean", "targetTimeout": "integer", "sourceSha256": "string", "validation": "string"}),
    "remediation-context": schema(_CONTEXT_FIELDS),
    "apply-remediation": schema(_STEP_FIELDS),
    "remediation-step": schema(_STEP_FIELDS),
    "verify-remediation": schema({"validated": "boolean", "configuration": "object", "maxTimeout": "integer", "artifactSha256": "string", "completionEvidence": "string"}),
}
for _output in OUTPUT_SCHEMAS.values():
    _output["properties"]["reportText"] = {"type": "string"}
    _output["required"].append("reportText")


def integer(value, name, minimum=1, maximum=3600):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be an integer from {minimum} through {maximum}")
    return value


def configuration(common, filename):
    value = common.document(filename)
    if not isinstance(value, dict):
        raise ValueError("configuration must be an object")
    integer(value.get("timeoutSeconds"), "timeoutSeconds")
    return value


def source(common, filename):
    value = configuration(common, filename)
    return value, hashlib.sha256(common.path(filename).read_bytes()).hexdigest()


def plan_change(common, payload):
    value, digest = source(common, payload["configurationPath"])
    proposed = integer(payload["proposedTimeout"], "proposedTimeout")
    limit = integer(payload["maxTimeout"], "maxTimeout")
    return {"configuration": value, "sourceSha256": digest, "configurationPath": payload["configurationPath"],
            "proposedTimeout": proposed, "maxTimeout": limit,
            "allowed": proposed <= limit, "scope": "review an output copy; never mutate the supplied source configuration"}


def unchanged(common, context):
    original, digest = source(common, context["configurationPath"])
    if digest != context["sourceSha256"] or original != context["configuration"]:
        raise ValueError("source configuration changed after planning; review a fresh plan")
    return original


def review_change(common, payload):
    context, proposal, review = payload["context"], payload["proposal"], payload["review"]
    unchanged(common, context)
    desired = integer(context["proposedTimeout"], "proposedTimeout")
    limit = integer(context["maxTimeout"], "maxTimeout")
    if proposal.get("timeoutSeconds") != desired or type(proposal.get("timeoutSeconds")) is not int:
        raise ValueError("planner proposal differs from the requested bounded change")
    allowed = desired <= limit
    if review.get("approved") is not allowed or review.get("timeoutSeconds") != desired:
        raise ValueError("review decision does not match deterministic threshold validation")
    return {"approved": allowed, "timeoutSeconds": desired, "sourceSha256": context["sourceSha256"],
            "decision": "approve" if allowed else "reject", "validation": "source digest, exact proposal and configured maximum"}


def staged_timeout(common, filename, expected):
    token = common.path(filename).read_bytes()
    if not re.fullmatch(rb"[1-9][0-9]{0,3}", token) or int(token) != expected:
        raise ValueError("staged timeout must exactly match the validated decimal proposal, without whitespace")


def verified_write(common, filename, value):
    text = json.dumps(value, sort_keys=True, indent=2) + "\n"
    common.write_atomic(filename, text)
    if common.document(filename) != value:
        raise ValueError("written configuration does not match the validated change")
    return hashlib.sha256(common.path(filename).read_bytes()).hexdigest()


def apply_change(common, payload):
    context, review = payload["context"], payload["review"]
    original = unchanged(common, context)
    desired = integer(context["proposedTimeout"], "proposedTimeout")
    if (review.get("approved") is not True or desired > integer(context["maxTimeout"], "maxTimeout")
            or review.get("timeoutSeconds") != desired or review.get("sourceSha256") != context["sourceSha256"]):
        raise ValueError("change has no matching deterministic approval")
    if payload["requireStagedValue"]:
        staged_timeout(common, "artifacts/role-timeout.txt", desired)
    changed = copy.deepcopy(original)
    changed["timeoutSeconds"] = desired
    digest = verified_write(common, "artifacts/reviewed-configuration.json", changed)
    return {"executed": True, "configuration": changed, "artifactSha256": digest,
            "originalPreserved": True, "scope": "validated local output copy"}


def prepare_remediation(common, payload):
    context = remediation_context(common, {**payload, "iteration": 0, "previous": {"done": False}})
    context["targetTimeout"] = min(context["configuration"]["timeoutSeconds"], context["maxTimeout"])
    return context


def validate_remediation_target(common, payload):
    context = payload["context"]
    original = unchanged(common, context)
    target = min(original["timeoutSeconds"], integer(context["maxTimeout"], "maxTimeout"))
    proposal = payload["proposal"]
    if "items" in proposal:
        completed = [item for item in proposal["items"] if item.get("state") == "succeeded"]
        if not completed:
            raise ValueError("proposal loop has no completed attempt")
        proposal = completed[-1]["output"]
    if type(proposal.get("timeoutSeconds")) is not int or proposal["timeoutSeconds"] != target:
        raise ValueError("model target differs from the input-derived maximum; done is not validation")
    staged_timeout(common, "artifacts/timeout-seconds.txt", target)
    return {"validated": True, "targetTimeout": target, "sourceSha256": context["sourceSha256"],
            "validation": "actual staged decimal bytes, unchanged source and exact requested target"}


def remediation_step(common, payload):
    context = remediation_context(common, payload)
    return apply_remediation(common, {"context": context, "proposal": {"timeoutSeconds": context["expectedTimeout"]},
                                      "requireStagedValue": False})


def remediation_context(common, payload):
    original, digest = source(common, payload["configurationPath"])
    iteration = integer(payload["iteration"], "iteration", 0, 2)
    limit = integer(payload["maxTimeout"], "maxTimeout")
    reduction = integer(payload["maxReduction"], "maxReduction")
    current = original
    if iteration:
        current = configuration(common, "artifacts/remediation.json")
        if payload["previous"].get("timeoutSeconds") != current["timeoutSeconds"]:
            raise ValueError("recorded previous iteration differs from the actual artifact")
        untouched = dict(current)
        untouched["timeoutSeconds"] = original["timeoutSeconds"]
        if untouched != original:
            raise ValueError("remediation changed unrelated configuration fields")
    expected = current["timeoutSeconds"] if current["timeoutSeconds"] <= limit else max(limit, current["timeoutSeconds"] - reduction)
    return {"configurationPath": payload["configurationPath"], "configuration": original, "sourceSha256": digest,
            "current": current, "expectedTimeout": expected, "maxTimeout": limit, "maxReduction": reduction,
            "iteration": iteration}


def apply_remediation(common, payload):
    context = payload["context"]
    original = unchanged(common, context)
    current = context["current"]
    limit = integer(context["maxTimeout"], "maxTimeout")
    reduction = integer(context["maxReduction"], "maxReduction")
    expected = current["timeoutSeconds"] if current["timeoutSeconds"] <= limit else max(limit, current["timeoutSeconds"] - reduction)
    if context["expectedTimeout"] != expected or type(payload["proposal"].get("timeoutSeconds")) is not int or payload["proposal"].get("timeoutSeconds") != expected:
        raise ValueError("proposal does not implement the bounded input-derived repair step")
    if payload["requireStagedValue"]:
        staged_timeout(common, "artifacts/timeout-seconds.txt", expected)
    changed = copy.deepcopy(original)
    changed["timeoutSeconds"] = expected
    digest = verified_write(common, "artifacts/remediation.json", changed)
    # Completion is computed only after rereading the actual written artifact.
    actual = configuration(common, "artifacts/remediation.json")
    return {"done": actual["timeoutSeconds"] <= limit, "timeoutSeconds": actual["timeoutSeconds"],
            "iteration": context["iteration"], "artifactSha256": digest}


def verify_remediation(common, payload):
    original = configuration(common, payload["configurationPath"])
    actual = configuration(common, "artifacts/remediation.json")
    limit = integer(payload["maxTimeout"], "maxTimeout")
    if actual["timeoutSeconds"] > limit:
        raise ValueError("actual configuration has not converged within the requested maximum")
    preserved = dict(actual)
    preserved["timeoutSeconds"] = original["timeoutSeconds"]
    if preserved != original:
        raise ValueError("repair modified unrelated fields")
    return {"validated": True, "configuration": actual, "maxTimeout": limit,
            "artifactSha256": hashlib.sha256(common.path("artifacts/remediation.json").read_bytes()).hexdigest(),
            "completionEvidence": "actual artifact schema, threshold, preserved fields and digest; no model done flag"}


def configuration_drift(common, payload):
    actual = common.document(payload["actualPath"])
    if not isinstance(actual, dict):
        raise ValueError("actual configuration must be an object")
    desired = payload["desired"]
    return {"desired": desired, "actual": actual,
            "drift": [{"key": key, "desired": value, "actual": actual.get(key)} for key, value in desired.items() if key not in actual or actual[key] != value],
            "inputEnvironment": payload["environment"]}


def register(common):
    return {name: (lambda payload, function=function: function(common, payload)) for name, function in {
        "configuration-drift": configuration_drift, "plan-change": plan_change, "review-change": review_change, "apply-change": apply_change,
        "prepare-remediation": prepare_remediation, "validate-remediation-target": validate_remediation_target,
        "remediation-step": remediation_step, "remediation-context": remediation_context, "apply-remediation": apply_remediation,
        "verify-remediation": verify_remediation}.items()}
