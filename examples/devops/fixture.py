#!/usr/bin/env python3
"""Trusted, bounded local fixtures for the DevOps examples; no third-party modules.

This helper is host process execution, not a security sandbox. Workflows grant
the interpreter explicitly. It never invokes cloud CLIs or production services.
"""
import datetime as dt
import difflib
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import traceback
import xml.etree.ElementTree as ET

PROTOCOL = "agentctl.dev/process-extension/v1"
SCHEMA = {"type": "object"}
CAPABILITIES = ["devops.fixture"]
ROOT = Path.cwd().resolve()


def path(name):
    candidate = (ROOT / name).resolve()
    if not candidate.is_relative_to(ROOT):
        raise ValueError("fixture path escapes workspace")
    return candidate


def read(name):
    data = path(name).read_bytes()
    if len(data) > 1024 * 1024:
        raise ValueError("fixture exceeds 1 MiB")
    return data.decode("utf-8")


def document(name):
    return json.loads(read(name))


def write(name, value):
    target = path(name)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes((value if isinstance(value, str) else json.dumps(value, indent=2) + "\n").encode("utf-8"))


def write_atomic(name, value):
    target = path(name)
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=".agentctl-report-", dir=target.parent, delete=False) as staged:
            temporary = Path(staged.name)
            staged.write(value.encode("utf-8"))
        temporary.replace(target)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def command(argv, cwd=None, expected=0):
    if argv[0] == "git":
        argv = [git_executable()] + argv[1:]
    result = subprocess.run(argv, cwd=cwd or ROOT, capture_output=True, text=True, timeout=30)
    if result.returncode != expected:
        raise ValueError(f"{argv[0]} returned {result.returncode}: {result.stderr[:2000]}")
    if len(result.stdout) + len(result.stderr) > 1024 * 1024:
        raise ValueError("fixture subprocess output exceeds 1 MiB")
    return result


def git_executable():
    # Published setup stages this beside the helper, outside workflow inputs.
    # Cleared child environments cannot discover Git for Windows through PATH.
    config_path = Path(__file__).resolve().with_name("fixture-tools.json")
    if not config_path.is_file():
        executable = shutil.which("git")
        if not executable:
            raise FileNotFoundError("run setup.py to record the trusted Git installation")
        return executable
    if config_path.stat().st_size > 16384:
        raise ValueError("fixture tool configuration exceeds 16 KiB")
    configured = json.loads(config_path.read_text(encoding="utf-8"))["git"]
    executable = Path(configured["executable"])
    if not executable.is_absolute() or not executable.is_file():
        raise ValueError("fixture Git executable must be an existing absolute path")
    if hashlib.sha256(executable.read_bytes()).hexdigest() != configured["sha256"]:
        raise ValueError("fixture Git installation changed after setup preflight")
    return str(executable)


def python_executable():
    # An env-cleared Linux process launched with argv0=python3 can have no
    # sys.executable. Resolve the same installation without importing host PATH.
    candidates = [Path(sys.executable)] if sys.executable else []
    candidates += [Path(sys.exec_prefix) / "bin/python3", Path(sys.exec_prefix) / "python.exe"]
    for candidate in candidates:
        if candidate.is_absolute() and candidate.is_file():
            return str(candidate)
    raise FileNotFoundError("cannot resolve Python interpreter in its installation directory")


def record_failure(error):
    # Exception values and source lines can include input data. Preserve only
    # stack locations and the exception class in this local fixture diagnostic.
    diagnostic = {"errorType": type(error).__name__, "frames": [
        {"file": Path(frame.filename).name, "line": frame.lineno, "function": frame.name}
        for frame in traceback.extract_tb(error.__traceback__)[-12:]],
        "exceptionValuesRedacted": True}
    write("evidence/fixture-error.json", diagnostic)


def patch(relative, before, after):
    """Check and apply a real unified patch in a disposable checkout."""
    patch_text = "".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                            fromfile="a/" + relative, tofile="b/" + relative))
    work = path("artifacts/patch-workspace")
    work.mkdir(parents=True, exist_ok=True)
    target = work / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(before.encode("utf-8"))
    patch_file = path("artifacts/proposed.patch")
    write("artifacts/proposed.patch", patch_text)
    # Apply the reviewed bytes exactly, regardless of Git for Windows defaults.
    command(["git", "-c", "core.autocrlf=false", "apply", "--check", str(patch_file)], cwd=work)
    command(["git", "-c", "core.autocrlf=false", "apply", str(patch_file)], cwd=work)
    if target.read_bytes().decode("utf-8") != after:
        raise ValueError("applied patch differs from validated proposal")
    return {"path": "artifacts/proposed.patch", "appliedAndVerified": True,
            "sha256": hashlib.sha256(patch_text.encode()).hexdigest()}


CI_ACTIONS = {
    "install_declared_dependency": "Install the declared missing dependency, then rerun the failed build.",
    "inspect_test_failure": "Inspect the cited failing test and repair its cause before rerunning the build.",
    "repair_configuration": "Repair the cited configuration error, validate it, then rerun the build.",
    "investigate_build": "Investigate the cited build evidence before deciding whether to rerun or release.",
}
INCIDENT_ACTIONS = {
    "inspect_connection_pool": "Inspect connection-pool capacity and saturation using the cited incident evidence.",
    "check_service_connectivity": "Check connectivity to the affected service using the cited incident evidence.",
    "investigate_incident": "Investigate the cited incident timeline before selecting a remediation.",
}


def ci_diagnose(payload):
    filename = payload.get("logPath", "fixtures/build.log")
    lines = read(filename).splitlines()
    if not any(line.strip() for line in lines):
        raise ValueError("CI log contains no evidence")
    patterns = [
        (r"ModuleNotFoundError|ImportError:.*(?:No module|cannot import)", "missing_dependency", "install_declared_dependency"),
        (r"AssertionError|FAILED\s+\S+|assertion failed", "test_failure", "inspect_test_failure"),
        (r"(?:invalid|malformed) configuration|(?:YAML|yaml).*parse error|SyntaxError", "configuration_error", "repair_configuration"),
    ]
    findings, supported = [], {}
    for number, line in enumerate(lines, 1):
        for pattern, category, action in patterns:
            if re.search(pattern, line):
                source = f"{filename}:{number}"
                findings.append({"source": source, "message": line, "category": category})
                supported.setdefault(action, []).append(source)
    evidence = list(dict.fromkeys(item["source"] for item in findings))
    if not evidence:
        evidence = [f"{filename}:{number}" for number, line in enumerate(lines, 1) if line.strip()][-1:]
    supported["investigate_build"] = evidence
    categories = {item["category"] for item in findings}
    root_cause = next(iter(categories)) if len(categories) == 1 else "unknown"
    selected = next(action for action in CI_ACTIONS if action in supported)
    return {"findings": findings, "rootCause": root_cause,
            "evidence": evidence, "lineCount": len(lines), "supportedActions": supported,
            "suggestedDecision": {"rootCause": root_cause, "action": selected, "evidence": supported[selected]},
            "validationScope": "bounded log-pattern observations and supported decisions; not proof of arbitrary advice"}


def junit_triage(payload):
    filename = payload.get("reportPath", "fixtures/junit.xml")
    root = ET.fromstring(read(filename))
    if root.tag.rsplit("}", 1)[-1] not in {"testsuite", "testsuites"}:
        raise ValueError("JUnit root must be testsuite or testsuites")
    results, cases = [], []
    for index, test in enumerate((node for node in root.iter() if node.tag.rsplit("}", 1)[-1] == "testcase"), 1):
        name = ".".join(value for value in [test.get("classname"), test.get("name")] if value) or f"testcase-{index}"
        issues = [node for node in test if node.tag.rsplit("}", 1)[-1] in {"failure", "error"}]
        skipped = any(node.tag.rsplit("}", 1)[-1] == "skipped" for node in test)
        if skipped and issues:
            raise ValueError("JUnit testcase cannot be both skipped and failed")
        cases.append({"test": name, "status": "failed" if issues else "skipped" if skipped else "passed"})
        for issue_index, node in enumerate(issues, 1):
            kind = node.tag.rsplit("}", 1)[-1]
            message, detail = node.get("message", ""), "".join(node.itertext()).strip()
            kind_type = node.get("type", "")
            combined = f"{kind_type} {message} {detail}"
            infrastructure = re.search(r"connection refused|connection timed out|name resolution|host unreachable|database unavailable", combined, re.IGNORECASE)
            application = re.search(r"(?:^|[.\s])(ValueError|TypeError|KeyError|IndexError|NullPointerException)(?:$|[\s:])", combined)
            classification = "assertion" if kind == "failure" else "infrastructure" if infrastructure else "application" if application else "unknown"
            recommendation = ("Investigate the cited dependency connectivity or availability failure before retrying."
                              if classification == "infrastructure" else
                              "Inspect expected versus actual behavior before changing the test."
                              if classification == "assertion" else
                              "Inspect the cited application exception and stack trace; do not assume an infrastructure cause.")
            results.append({"test": name, "kind": kind, "type": kind_type, "classification": classification,
                            "evidence": f"{filename}#testcase={index}&issue={issue_index}",
                            "message": message, "detail": detail, "recommendation": recommendation})
    return {"tests": len(cases), "testCases": cases, "failures": results,
            "failedCount": sum(item["status"] == "failed" for item in cases),
            "issueCount": len(results), "skippedCount": sum(item["status"] == "skipped" for item in cases),
            "status": "no-results" if not cases else "failures" if results else "passed"}


def iso_date(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", value):
        raise ValueError(f"{label} must be an ISO YYYY-MM-DD date")
    return dt.date.fromisoformat(value)


def sbom_triage(payload):
    sbom = document(payload.get("sbomPath", "fixtures/sbom.json"))
    vulnerabilities = document(payload.get("vulnerabilitiesPath", "fixtures/vulnerabilities.json"))
    rules = document(payload.get("rulesPath", "fixtures/rules.json"))
    components = [item["bom-ref"] for item in sbom["components"]]
    if any(not isinstance(item, str) or not item for item in components) or len(components) != len(set(components)):
        raise ValueError("SBOM component identities must be nonempty and unique")
    severities = {"critical", "high", "medium", "low", "info", "none", "unknown"}
    blocking = rules["blockingSeverities"]
    if not isinstance(blocking, list) or not blocking or set(blocking) - severities:
        raise ValueError("blockingSeverities must contain recognized severities")
    as_of = iso_date(rules["asOf"], "asOf")
    exceptions = rules.get("exceptions", [])
    scoped = set()
    for exception in exceptions:
        identity = (exception["id"], exception["component"])
        if identity in scoped or exception["component"] not in components:
            raise ValueError("exception scope must be unique and name a known SBOM component")
        scoped.add(identity)
        if exception.get("status") not in {"approved", "revoked"}:
            raise ValueError("exception status must explicitly be approved or revoked")
        if not all(isinstance(exception.get(key), str) and exception[key].strip() for key in ["id", "owner", "reason"]):
            raise ValueError("exception requires an ID, accountable owner and reason")
        iso_date(exception["expires"], "exception expires")
    findings, seen = [], set()
    for vuln in vulnerabilities:
        identity = (vuln["id"], vuln["component"])
        if identity in seen or vuln["component"] not in components:
            raise ValueError("vulnerability scope must be unique and name a known SBOM component")
        seen.add(identity)
        if vuln["severity"] not in severities:
            raise ValueError("vulnerability has an unrecognized severity")
        candidates = [item for item in exceptions if (item["id"], item["component"]) == identity]
        exception = next((item for item in candidates if item["status"] == "approved" and iso_date(item["expires"], "expires") >= as_of), None)
        findings.append({**vuln, "disposition": "excepted" if exception else "block" if vuln["severity"] in blocking or vuln["severity"] == "unknown" else "track",
                         "exception": exception, "inactiveExceptions": candidates if exception is None else []})
    return {"findings": findings, "decision": "no-go" if any(item["disposition"] == "block" for item in findings) else "go",
            "asOf": rules["asOf"], "mode": "analysis-only", "requiresExplicitCiGate": True}


def release_notes(payload):
    repository = path(payload["repositoryPath"])
    refs = [payload["fromRef"], payload["toRef"]]
    resolved = []
    for ref in refs:
        if not isinstance(ref, str) or not ref or ref.startswith("-"):
            raise ValueError("release range requires explicit non-option Git refs")
        resolved.append(command(["git", "rev-parse", "--verify", "--end-of-options", ref + "^{commit}"], cwd=repository).stdout.strip())
    shas = command(["git", "rev-list", "--reverse", resolved[0] + ".." + resolved[1]], cwd=repository).stdout.splitlines()
    if len(shas) > 500:
        raise ValueError("release range exceeds 500 commits")
    notes = []
    for sha in shas:
        summary = command(["git", "show", "-s", "--format=%s", sha], cwd=repository).stdout.rstrip("\n")
        files = sorted(set(command(["git", "diff-tree", "--root", "-m", "--no-commit-id", "--name-only", "-z", "-r", sha], cwd=repository).stdout.rstrip("\0").split("\0")) - {""})
        notes.append({"commit": sha, "summary": summary, "files": files})
    output = payload.get("outputPath", "artifacts/CHANGELOG.md")
    if not path(output).is_relative_to(path("artifacts")):
        raise ValueError("release-note output must remain under artifacts")
    write_atomic(output, "# Release notes\n\n" + "".join(f"- {note['summary']} (`{note['commit'][:12]}`; {', '.join(note['files'])})\n" for note in notes))
    return {"notes": notes, "commitsVerified": len(notes), "fromCommit": resolved[0], "toCommit": resolved[1],
            "outputPath": output, "scope": "read existing repository and explicit exclusive-from/inclusive-to Git range; no commits created"}


def prepare_fixture_history(payload):
    """Explicit setup operation; never invoked by release-note analysis."""
    history_path = payload.get("historyPath", "fixtures/history.json")
    history = document(history_path)
    if not isinstance(history, list) or not 1 <= len(history) <= 50:
        raise ValueError("fixture history requires one to fifty commits")
    for row in history:
        relative = Path(row["path"])
        if relative.is_absolute() or ".." in relative.parts or ".git" in relative.parts:
            raise ValueError("fixture history file must stay outside Git metadata and inside its repository")
        if not all(isinstance(row.get(key), str) and row[key] for key in ["path", "content", "message"]):
            raise ValueError("fixture history requires nonempty path, content and message strings")
    repository = path(payload.get("repositoryPath", "fixtures/repository"))
    fingerprint = hashlib.sha256(read(history_path).encode()).hexdigest()
    marker = repository / ".git/agentctl-fixture.json"
    if repository.exists():
        if not marker.is_file():
            raise ValueError("fixture setup will not modify an existing user repository")
        retained = json.loads(marker.read_text())
        head = command(["git", "rev-parse", "HEAD"], cwd=repository).stdout.strip()
        base = command(["git", "rev-parse", "fixture-base"], cwd=repository).stdout.strip()
        dirty = command(["git", "status", "--porcelain"], cwd=repository).stdout.strip()
        if retained["historySha256"] != fingerprint or retained["toRef"] != head or retained["fromRef"] != base or dirty:
            raise ValueError("prepared fixture history changed; use a new directory")
        return retained
    repository.parent.mkdir(parents=True, exist_ok=True)
    staging_owner = tempfile.TemporaryDirectory(prefix=".agentctl-history-", dir=repository.parent)
    staging = Path(staging_owner.name)
    try:
        command(["git", "init", "--quiet"], cwd=staging)
        commit = ["git", "-c", "user.name=DevOps Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "commit", "--quiet"]
        command(commit + ["--allow-empty", "-m", "fixture: initial boundary"], cwd=staging)
        base = command(["git", "rev-parse", "HEAD"], cwd=staging).stdout.strip()
        command(["git", "tag", "fixture-base", base], cwd=staging)
        for row in history:
            target = staging / row["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(row["content"].encode("utf-8"))
            command(["git", "add", "--", row["path"]], cwd=staging)
            command(commit + ["-m", row["message"]], cwd=staging)
        head = command(["git", "rev-parse", "HEAD"], cwd=staging).stdout.strip()
        prepared = {"fromRef": base, "toRef": head, "historySha256": fingerprint}
        (staging / ".git/agentctl-fixture.json").write_text(json.dumps(prepared), encoding="utf-8")
        staging.rename(repository)
        return prepared
    finally:
        staging_owner.cleanup()


def release_readiness(payload):
    gates = document(payload.get("gatesPath", "fixtures/gates.json"))
    checks = gates["checks"]
    if not isinstance(checks, list) or not checks:
        raise ValueError("release readiness requires at least one recorded gate")
    names = set()
    for check in checks:
        if not isinstance(check.get("name"), str) or not check["name"].strip() or check["name"] in names or check["name"] == "package-digest":
            raise ValueError("gate names must be nonempty and unique; package-digest is reserved")
        names.add(check["name"])
        if type(check.get("passed")) is not bool:
            raise ValueError("each gate passed value must be a JSON boolean")
    expected = gates["packageSha256"]
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("packageSha256 must be a lowercase SHA-256 digest")
    actual = hashlib.sha256(path(payload.get("packagePath", "fixtures/package.txt")).read_bytes()).hexdigest()
    checks = checks + [{"name": "package-digest", "passed": actual == expected, "sha256": actual}]
    return {"checks": checks, "decision": "go" if all(item["passed"] for item in checks) else "no-go",
            "blocking": [item["name"] for item in checks if not item["passed"]],
            "mode": "analysis-only", "requiresExplicitCiGate": True}


def instant(value, label):
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a timestamp with a timezone")
    parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError(f"{label} must include a timezone")
    return parsed.astimezone(dt.timezone.utc)


def incident_timeline(payload):
    filename = payload.get("logPath", "fixtures/incident.log")
    start = instant(payload["windowStart"], "windowStart") if payload.get("windowStart") else None
    end = instant(payload["windowEnd"], "windowEnd") if payload.get("windowEnd") else None
    if start and end and start > end:
        raise ValueError("incident window start must not follow its end")
    events, supported = [], {}
    for number, line in enumerate(read(filename).splitlines(), 1):
        if not line.strip():
            continue
        fields = line.split(" ", 2)
        if len(fields) != 3 or not fields[1] or not fields[2]:
            raise ValueError(f"incident log line {number} requires timestamp, service and message")
        timestamp, service, message = fields
        when = instant(timestamp, f"incident line {number}")
        if start and when < start or end and when > end:
            continue
        source = f"{filename}:{number}"
        events.append({"timestamp": when.isoformat().replace("+00:00", "Z"), "service": service, "message": message,
                       "source": source, "instant": when})
        if re.search(r"connection.pool.*(?:exhaust|saturat)|pool exhausted", message, re.IGNORECASE):
            supported.setdefault("inspect_connection_pool", []).append(source)
        if re.search(r"connection refused|connection timed out|unreachable", message, re.IGNORECASE):
            supported.setdefault("check_service_connectivity", []).append(source)
    events.sort(key=lambda event: event["instant"])
    if not events:
        raise ValueError("incident window contains no events")
    duration = (events[-1]["instant"] - events[0]["instant"]).total_seconds()
    for event in events:
        del event["instant"]
    supported["investigate_incident"] = [event["source"] for event in events]
    selected = next(action for action in INCIDENT_ACTIONS if action in supported)
    return {"timeline": events, "durationSeconds": duration, "supportedActions": supported,
            "suggestedDecision": {"durationSeconds": duration, "action": selected, "evidence": supported[selected]},
            "window": {"start": start.isoformat() if start else None, "end": end.isoformat() if end else None},
            "validationScope": "observed timestamp interval and source-supported actions; not causal proof or arbitrary advice validation"}


def canary_evaluate(payload):
    samples = document(payload.get("metricsPath", "fixtures/metrics.json"))
    limit = payload.get("errorLimit", 0.01)
    minimum, minimum_samples = payload.get("minRequests", 100), payload.get("minSamples", 2)
    if type(limit) not in {int, float} or not 0 <= limit <= 1:
        raise ValueError("errorLimit must be a finite number between zero and one")
    if type(minimum) is not int or minimum <= 0 or type(minimum_samples) is not int or minimum_samples <= 0:
        raise ValueError("minRequests and minSamples must be positive integers")
    if not isinstance(samples, list):
        raise ValueError("metrics must be a list of request/error samples")
    for sample in samples:
        if any(type(sample.get(key)) is not int for key in ["requests", "errors"]) or not 0 <= sample["errors"] <= sample["requests"]:
            raise ValueError("each sample requires integer 0 <= errors <= requests")
    requests, errors = sum(item["requests"] for item in samples), sum(item["errors"] for item in samples)
    sufficient = requests >= minimum and len(samples) >= minimum_samples
    ratio = errors / requests if requests else None
    return {"requests": requests, "errors": errors, "errorRatio": ratio, "errorLimit": limit,
            "sampleCount": len(samples), "minRequests": minimum, "minSamples": minimum_samples,
            "sufficientData": sufficient, "route": "hold" if not sufficient else "promote" if ratio <= limit else "rollback"}


def analyze(case, payload):
    if case == "01":
        return ci_diagnose(payload)
    if case == "02":
        return junit_triage(payload)
    if case == "03":
        before = read("fixtures/pipeline.json")
        pipeline = json.loads(before)
        violations = []
        if pipeline["permissions"] != "read-all":
            violations.append({"rule": "least-privilege", "location": "/permissions"})
            pipeline["permissions"] = "read-all"
        if pipeline["jobs"]["test"]["timeout-minutes"] > 15:
            violations.append({"rule": "bounded-job", "location": "/jobs/test/timeout-minutes"})
            pipeline["jobs"]["test"]["timeout-minutes"] = 15
        after = json.dumps(pipeline, indent=2) + "\n"
        evidence = patch("pipeline.json", before, after)
        assert pipeline["permissions"] == "read-all" and pipeline["jobs"]["test"]["timeout-minutes"] == 15
        return {"violations": violations, "patch": evidence, "validated": True}
    if case == "04":
        before = read("fixtures/Dockerfile")
        after = before.replace("COPY . /app", "COPY app.py /app/app.py").replace("USER root", "USER 65534:65534")
        evidence = patch("Dockerfile", before, after)
        write("artifacts/Dockerfile", after)
        shutil.copy2(path("fixtures/app.py"), path("artifacts/app.py"))
        command([python_executable(), "-m", "py_compile", str(path("artifacts/app.py"))])
        assert "USER 65534:65534" in after and "COPY ." not in after
        return {"improvements": ["explicit-copy", "non-root-user"], "patch": evidence,
                "syntaxValidated": True, "containerBuild": "requires --container-build",
                "performanceClaim": None}
    if case == "05":
        manifest = document("fixtures/deployment.json")
        violations = []
        if not isinstance(manifest["spec"]["replicas"], int):
            violations.append({"rule": "replicas-integer", "location": "/spec/replicas"})
            manifest["spec"]["replicas"] = int(manifest["spec"]["replicas"])
        container = manifest["spec"]["template"]["spec"]["containers"][0]
        if not container.get("securityContext", {}).get("runAsNonRoot"):
            violations.append({"rule": "non-root", "location": "/spec/template/spec/containers/0/securityContext"})
            container["securityContext"] = {"runAsNonRoot": True, "allowPrivilegeEscalation": False}
        write("artifacts/deployment.json", manifest)
        return {"manifest": manifest, "violations": violations,
                "schemaScope": "bundled apps/v1 Deployment subset; no cluster or full Kubernetes conformance"}
    if case == "06":
        plan = document("fixtures/plan.json")
        changes = plan["resource_changes"]
        denied = [c["address"] for c in changes if "delete" in c["change"]["actions"]
                  or not c["address"].startswith("local_file.")]
        return {"allowed": not denied, "denied": denied,
                "changes": [{"address": c["address"], "actions": c["change"]["actions"]} for c in changes],
                "scope": "isolated local-file plan fixture; no terraform apply"}
    if case == "07":
        before = read("fixtures/vendor_version.py")
        after = read("fixtures/vendor_version.updated.py")
        test = read("fixtures/test_dependency.py")
        work = path("artifacts/patch-workspace")
        work.mkdir(parents=True, exist_ok=True)
        (work / "vendor_version.py").write_text(before)
        (work / "test_dependency.py").write_text(test)
        baseline = command([python_executable(), "-B", "-m", "unittest", "-v"], cwd=work, expected=1)
        evidence = patch("vendor_version.py", before, after)
        changed = command([python_executable(), "-B", "-m", "unittest", "-v"], cwd=work)
        write("artifacts/test-before.txt", baseline.stderr)
        write("artifacts/test-after.txt", changed.stderr)
        return {"dependency": "local-vendor-version", "from": "1.0.0", "to": "1.1.0",
                "baselineExit": 1, "updatedExit": 0, "tests": 3, "patch": evidence,
                "scope": "vendored dependency fixture; no registry or supply-chain attestation"}
    if case == "08":
        return sbom_triage(payload)
    if case == "09":
        return release_notes(payload)
    if case == "10":
        return release_readiness(payload)
    if case == "11":
        actual = document("fixtures/actual.json")
        desired = payload["desired"]
        drift = [{"key": key, "desired": value, "actual": actual.get(key)}
                 for key, value in desired.items() if actual.get(key) != value]
        return {"desired": desired, "actual": actual, "drift": drift, "inputEnvironment": payload["environment"]}
    if case == "12":
        return incident_timeline(payload)
    if case == "13":
        return canary_evaluate(payload)
    if case in {"15", "16"}:
        counter = path("artifacts/mutations.txt")
        before = int(counter.read_text()) if counter.exists() else 0
        write("artifacts/mutations.txt", str(before + 1))
        desired = document("fixtures/desired-service.json")
        write("artifacts/service.json", {**desired, "mutation": before + 1})
        return {"mutation": before + 1, "version": desired["version"]}
    if case == "18":
        service = payload["service"]
        check = payload["check"]
        if service not in {"api", "worker"} or check not in {"syntax", "config"}:
            raise ValueError("unknown static matrix item")
        config = document(f"fixtures/{service}.json")
        if check == "syntax":
            compile(read(f"fixtures/{service}.py"), f"fixtures/{service}.py", "exec")
        else:
            if not 1 <= config["replicas"] <= 3 or config["timeoutSeconds"] > 30:
                raise ValueError("service configuration violates fixture limits")
        return {"service": service, "check": check, "passed": True, "index": payload["index"]}
    if case == "19":
        value = read("artifacts/role-change.txt")
        if value != "reviewed-local-change":
            raise ValueError("executor artifact differs from reviewed payload")
        return {"executed": True, "content": value, "scope": "local role handoff fixture"}
    if case == "20":
        token = path("artifacts/timeout-seconds.txt").read_bytes()
        if token != b"30":
            raise ValueError("remediation token must be exactly the approved decimal bytes")
        approved = {"timeoutSeconds": int(token)}
        write("artifacts/remediation.json", json.dumps(approved, separators=(",", ":")) + "\n")
        result = document("artifacts/remediation.json")
        if result != {"timeoutSeconds": 30}:
            raise ValueError("remediation output failed final semantic validation")
        return {"validated": True, "configuration": result, "scope": "local configuration remediation"}
    raise ValueError(f"unknown fixture case {case}")


def verify_model(case, payload):
    report, analysis = payload["report"], payload["analysis"]
    actions = CI_ACTIONS if case == "01" else INCIDENT_ACTIONS if case == "12" else None
    if actions is None:
        raise ValueError("no model semantic contract for this case")
    field = "rootCause" if case == "01" else "durationSeconds"
    if analysis.get(field) != report[field]:
        raise ValueError("model changed the deterministic observation")
    action = analysis.get("action")
    if action not in report["supportedActions"] or action not in actions:
        raise ValueError("model action is not supported by the observed evidence")
    evidence = analysis.get("evidence")
    if not isinstance(evidence, list) or not evidence or any(item not in report["supportedActions"][action] for item in evidence):
        raise ValueError("model action must cite its exact supporting evidence")
    recommendation = actions[action]
    if "recommendation" in analysis and analysis["recommendation"] != recommendation:
        raise ValueError("recommendation contradicts the bounded decision contract; free text is advisory only")
    validated = {field: analysis[field], "action": action, "evidence": evidence, "recommendation": recommendation}
    return {"report": report, "analysis": validated, "semanticValidation": True,
            "validationScope": "structured observation, allowed action, exact supporting citations and generated recommendation",
            "advisory": {"text": analysis.get("advisory", ""), "validated": False}}


OPERATIONS = {
    "ci-diagnose": ci_diagnose,
    "junit-triage": junit_triage,
    "sbom-triage": sbom_triage,
    "release-notes": release_notes,
    "release-readiness": release_readiness,
    "incident-timeline": incident_timeline,
    "canary-evaluate": canary_evaluate,
    "verify-ci-decision": lambda payload: verify_model("01", payload),
    "verify-incident-decision": lambda payload: verify_model("12", payload),
}


def operation_input_schema(operation):
    fields = {
        "ci-diagnose": {"logPath": "string"},
        "junit-triage": {"reportPath": "string"},
        "sbom-triage": {"sbomPath": "string", "vulnerabilitiesPath": "string", "rulesPath": "string"},
        "release-notes": {"repositoryPath": "string", "fromRef": "string", "toRef": "string", "outputPath": "string"},
        "release-readiness": {"gatesPath": "string", "packagePath": "string"},
        "incident-timeline": {"logPath": "string", "windowStart": "string", "windowEnd": "string"},
        "canary-evaluate": {"metricsPath": "string", "errorLimit": "number", "minRequests": "integer", "minSamples": "integer"},
        "verify-ci-decision": {"report": "object", "analysis": "object"},
        "verify-incident-decision": {"report": "object", "analysis": "object"},
    }
    if operation not in fields:
        return EXTRA_INPUT_SCHEMAS.get(operation, SCHEMA)
    required = list(fields[operation])
    if operation == "incident-timeline":
        required = ["logPath"]
    return {"type": "object", "additionalProperties": False, "required": required,
            "properties": {name: {"type": kind} for name, kind in fields[operation].items()}}

from format_operations import INPUT_SCHEMAS as FORMAT_SCHEMAS, OUTPUT_SCHEMAS as FORMAT_OUTPUT_SCHEMAS, register as register_formats
from operations import INPUT_SCHEMAS as CHANGE_SCHEMAS, OUTPUT_SCHEMAS as CHANGE_OUTPUT_SCHEMAS, register as register_changes
from service_operations import INPUT_SCHEMAS as SERVICE_SCHEMAS, OUTPUT_SCHEMAS as SERVICE_OUTPUT_SCHEMAS, register as register_services

EXTRA_INPUT_SCHEMAS = {**FORMAT_SCHEMAS, **CHANGE_SCHEMAS, **SERVICE_SCHEMAS}
OPERATIONS.update(register_formats(sys.modules[__name__]))
OPERATIONS.update(register_changes(sys.modules[__name__]))
OPERATIONS.update(register_services(sys.modules[__name__]))


# Required output fields reject silently missing or misspelled helper results.
# Nested source documents retain their own variable shape and semantic validators.
_OUTPUT_FIELDS = {
    "ci-diagnose": {"findings": "array", "rootCause": "string", "evidence": "array", "lineCount": "integer", "supportedActions": "object", "suggestedDecision": "object", "validationScope": "string"},
    "junit-triage": {"tests": "integer", "testCases": "array", "failures": "array", "failedCount": "integer", "issueCount": "integer", "skippedCount": "integer", "status": "string"},
    "sbom-triage": {"findings": "array", "decision": "string", "asOf": "string", "mode": "string", "requiresExplicitCiGate": "boolean"},
    "release-notes": {"notes": "array", "commitsVerified": "integer", "fromCommit": "string", "toCommit": "string", "outputPath": "string", "scope": "string"},
    "release-readiness": {"checks": "array", "decision": "string", "blocking": "array", "mode": "string", "requiresExplicitCiGate": "boolean"},
    "incident-timeline": {"timeline": "array", "durationSeconds": "number", "supportedActions": "object", "suggestedDecision": "object", "window": "object", "validationScope": "string"},
    "canary-evaluate": {"requests": "integer", "errors": "integer", "errorRatio": ["number", "null"], "errorLimit": "number", "sampleCount": "integer", "minRequests": "integer", "minSamples": "integer", "sufficientData": "boolean", "route": "string"},
}
for _operation in ["verify-ci-decision", "verify-incident-decision"]:
    _OUTPUT_FIELDS[_operation] = {"report": "object", "analysis": "object", "semanticValidation": "boolean", "validationScope": "string", "advisory": "object"}
NAMED_OUTPUT_SCHEMAS = {
    name: {"type": "object", "additionalProperties": False, "required": [*fields, "reportText"],
           "properties": {**{key: {"type": kind} for key, kind in fields.items()}, "reportText": {"type": "string"}}}
    for name, fields in _OUTPUT_FIELDS.items()
}


def operation_output_schema(operation):
    return {**FORMAT_OUTPUT_SCHEMAS, **CHANGE_OUTPUT_SCHEMAS, **SERVICE_OUTPUT_SCHEMAS, **NAMED_OUTPUT_SCHEMAS}.get(operation, SCHEMA)


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "terminal-check":
        if not path("fixtures/retry-ready.txt").exists():
            print("fixture test dependency is unavailable", file=sys.stderr)
            return 1
        print("fixture tests passed")
        return 0
    case, mode = sys.argv[1:3]
    if mode == "--agentctl-handshake":
        print(json.dumps({"protocolVersion": PROTOCOL, "name": "devops-local-fixture",
                          "inputSchema": operation_input_schema(case), "outputSchema": operation_output_schema(case), "capabilities": CAPABILITIES}))
        return 0
    if mode != "--agentctl-invoke":
        raise ValueError("unknown extension invocation")
    request = json.load(sys.stdin)
    payload = request["input"]
    if case in OPERATIONS:
        report = OPERATIONS[case](payload)
    else:
        report = verify_model(case, payload) if payload.get("phase") == "verify-model" else analyze(case, payload)
    report_text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    print(json.dumps({"protocolVersion": PROTOCOL, "effectId": request["effectId"],
                      "output": {**report, "reportText": report_text}}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        record_failure(error)
        print(f"{type(error).__name__}: see evidence/fixture-error.json (exception values redacted)", file=sys.stderr)
        sys.exit(1)
