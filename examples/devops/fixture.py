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
    # The trusted runner stages this beside the helper, outside workflow data.
    # Cleared child environments cannot discover Git for Windows through PATH.
    config_path = Path(__file__).resolve().with_name("fixture-tools.json")
    if not config_path.is_file():
        executable = shutil.which("git")
        if not executable:
            raise FileNotFoundError("use the suite runner to stage the trusted Git installation")
        return executable
    if config_path.stat().st_size > 16384:
        raise ValueError("fixture tool configuration exceeds 16 KiB")
    configured = json.loads(config_path.read_text(encoding="utf-8"))["git"]
    executable = Path(configured["executable"])
    if not executable.is_absolute() or not executable.is_file():
        raise ValueError("fixture Git executable must be an existing absolute path")
    if hashlib.sha256(executable.read_bytes()).hexdigest() != configured["sha256"]:
        raise ValueError("fixture Git installation changed after runner preflight")
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
    command(["git", "apply", "--check", str(patch_file)], cwd=work)
    command(["git", "apply", str(patch_file)], cwd=work)
    if target.read_bytes().decode("utf-8") != after:
        raise ValueError("applied patch differs from validated proposal")
    return {"path": "artifacts/proposed.patch", "appliedAndVerified": True,
            "sha256": hashlib.sha256(patch_text.encode()).hexdigest()}


def analyze(case, payload):
    if case == "01":
        lines = read("fixtures/build.log").splitlines()
        findings = [{"source": f"fixtures/build.log:{i}", "message": line,
                     "category": "missing_dependency"} for i, line in enumerate(lines, 1)
                    if "ModuleNotFoundError" in line]
        if len(findings) != 1:
            raise ValueError("expected exactly one actionable missing dependency")
        return {"findings": findings, "rootCause": "missing_dependency",
                "evidence": [f["source"] for f in findings], "lineCount": len(lines)}
    if case == "02":
        root = ET.fromstring(read("fixtures/junit.xml"))
        results = []
        for test in root.iter("testcase"):
            failure = test.find("failure")
            error = test.find("error")
            if failure is not None or error is not None:
                node = error if error is not None else failure
                results.append({"test": test.attrib["classname"] + "." + test.attrib["name"],
                    "classification": "infrastructure" if error is not None else "assertion",
                    "evidence": "fixtures/junit.xml#" + test.attrib["name"],
                    "message": node.attrib.get("message", ""),
                    "recommendation": "retry after restoring the fixture database" if error is not None
                    else "inspect expected versus actual values before changing the test"})
        return {"tests": len(list(root.iter("testcase"))), "failures": results,
                "failedCount": len(results)}
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
        sbom = document("fixtures/sbom.json")
        vulnerabilities = document("fixtures/vulnerabilities.json")
        rules = document("fixtures/rules.json")
        components = {c["bom-ref"] for c in sbom["components"]}
        findings = []
        for vuln in vulnerabilities:
            if vuln["component"] not in components:
                raise ValueError("vulnerability component absent from SBOM")
            exception = next((e for e in rules["exceptions"] if e["id"] == vuln["id"]
                              and e["expires"] >= rules["asOf"]), None)
            findings.append({**vuln, "disposition": "excepted" if exception else
                "block" if vuln["severity"] in rules["blockingSeverities"] else "track",
                "exception": exception})
        return {"findings": findings, "decision": "no-go" if any(f["disposition"] == "block" for f in findings)
                else "go", "asOf": rules["asOf"]}
    if case == "09":
        history = document("fixtures/history.json")
        repository = path("artifacts/history")
        repository.mkdir(parents=True, exist_ok=True)
        command(["git", "init", "--quiet"], cwd=repository)
        for row in history:
            (repository / row["path"]).write_text(row["content"])
            command(["git", "add", "--", row["path"]], cwd=repository)
            command(["git", "-c", "user.name=DevOps Fixture", "-c", "user.email=fixture@example.invalid",
                     "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", row["message"]], cwd=repository)
        shas = command(["git", "log", "--reverse", "--format=%H"], cwd=repository).stdout.splitlines()
        notes = []
        for sha, row in zip(shas, history):
            files = command(["git", "diff-tree", "--root", "--no-commit-id", "--name-only", "-r", sha],
                            cwd=repository).stdout.splitlines()
            if row["path"] not in files:
                raise ValueError("release note has no corresponding Git change")
            notes.append({"commit": sha, "summary": row["message"], "files": files})
        write("artifacts/CHANGELOG.md", "# Fixture release\n\n" + "".join(
            f"- {n['summary']} (`{n['commit'][:12]}`; {', '.join(n['files'])})\n" for n in notes))
        return {"notes": notes, "commitsVerified": len(notes), "scope": "actual disposable Git repository"}
    if case == "10":
        gates = document("fixtures/gates.json")
        package = path("fixtures/package.txt").read_bytes()
        actual = hashlib.sha256(package).hexdigest()
        digest_gate = {"name": "package-digest", "passed": actual == gates["packageSha256"], "sha256": actual}
        checks = gates["checks"] + [digest_gate]
        return {"checks": checks, "decision": "go" if all(c["passed"] for c in checks) else "no-go",
                "blocking": [c["name"] for c in checks if not c["passed"]]}
    if case == "11":
        actual = document("fixtures/actual.json")
        desired = payload["desired"]
        drift = [{"key": key, "desired": value, "actual": actual.get(key)}
                 for key, value in desired.items() if actual.get(key) != value]
        return {"desired": desired, "actual": actual, "drift": drift, "inputEnvironment": payload["environment"]}
    if case == "12":
        events = []
        for number, line in enumerate(read("fixtures/incident.log").splitlines(), 1):
            timestamp, service, message = line.split(" ", 2)
            instant = dt.datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
            events.append({"timestamp": timestamp, "service": service, "message": message,
                           "source": f"fixtures/incident.log:{number}", "seconds": instant.timestamp()})
        events.sort(key=lambda item: item["seconds"])
        duration = int(events[-1]["seconds"] - events[0]["seconds"])
        for event in events:
            del event["seconds"]
        return {"timeline": events, "durationSeconds": duration,
                "recommendations": [{"action": "check connection-pool capacity before restarting API",
                                     "evidence": [events[0]["source"], events[1]["source"]]}]}
    if case == "13":
        samples = document("fixtures/metrics.json")
        errors = sum(s["errors"] for s in samples)
        requests = sum(s["requests"] for s in samples)
        if requests <= 0:
            raise ValueError("empty canary sample fails closed")
        ratio = errors / requests
        limit = payload.get("errorLimit", 0.01)
        return {"requests": requests, "errors": errors, "errorRatio": ratio,
                "errorLimit": limit, "route": "promote" if ratio <= limit else "rollback"}
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
        result = document("artifacts/remediation.json")
        if result != {"timeoutSeconds": 30}:
            raise ValueError("remediation output failed final semantic validation")
        return {"validated": True, "configuration": result, "scope": "local configuration remediation"}
    raise ValueError(f"unknown fixture case {case}")


def verify_model(case, payload):
    report, analysis = payload["report"], payload["analysis"]
    if case == "01":
        assert analysis["rootCause"] == report["rootCause"]
        assert analysis["evidence"] and set(analysis["evidence"]).issubset(report["evidence"])
        assert analysis["recommendation"].strip()
    elif case == "12":
        assert analysis["durationSeconds"] == report["durationSeconds"]
        assert analysis["evidence"] and set(analysis["evidence"]).issubset(
            {event["source"] for event in report["timeline"]})
        assert analysis["recommendation"].strip()
    else:
        raise ValueError("no model semantic contract for this case")
    return {"report": report, "analysis": analysis, "semanticValidation": True}


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
                          "inputSchema": SCHEMA, "outputSchema": SCHEMA, "capabilities": CAPABILITIES}))
        return 0
    if mode != "--agentctl-invoke":
        raise ValueError("unknown extension invocation")
    request = json.load(sys.stdin)
    payload = request["input"]
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
