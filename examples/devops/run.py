#!/usr/bin/env python3
"""Execute the catalog through the real CLI from isolated clean directories."""
import argparse
import copy
from contextlib import closing
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

from live_budget import LiveBudget

ROOT = Path(__file__).resolve().parent


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def save(filename, value):
    filename.parent.mkdir(parents=True, exist_ok=True)
    filename.write_text(json.dumps(value, indent=2) + "\n")


def digest(filename):
    return hashlib.sha256(filename.read_bytes()).hexdigest()


def artifact_digests(workspace):
    return {str(file.relative_to(workspace)): digest(file) for file in sorted((workspace / "artifacts").rglob("*"))
            if file.is_file()}


def run_id(envelope):
    return envelope.get("data", {}).get("runId") or envelope.get("error", {}).get("runId")


def latest_run_id(database):
    # sqlite3's connection context manages transactions; it does not close the
    # handle. Explicit close is required before Windows can remove runtime.db.
    with closing(sqlite3.connect(database, timeout=1)) as connection:
        row = connection.execute("SELECT run_id FROM runs ORDER BY created_at DESC LIMIT 1").fetchone()
        return row[0] if row else None


def cleanup_workspace(base):
    root = Path(base).resolve()
    def remove_readonly(function, filename, exc_info):
        error = exc_info[1]
        target = Path(filename)
        if not isinstance(error, PermissionError) or target.is_symlink() or not target.resolve().is_relative_to(root):
            raise error
        target.chmod(target.stat().st_mode | stat.S_IWRITE)
        function(filename)
    shutil.rmtree(base, onerror=remove_readonly)


class Case:
    def __init__(self, args, entry):
        self.args, self.entry = args, entry
        self.base = Path(tempfile.mkdtemp(prefix="agentctl-devops-" + entry["id"] + "-"))
        self.workspace = self.base / "workflow"
        self.cwd = self.base / "unrelated-invoking-directory"
        self.cwd.mkdir()
        shutil.copytree(ROOT / entry["directory"], self.workspace)
        shutil.copy2(ROOT / "fixture.py", self.base / "fixture.py")
        self.fixture_tools = {}
        if "git" in entry["dependencies"]:
            executable = shutil.which("git")
            require(executable, "Git is required by this fixture")
            executable = Path(executable).resolve()
            self.fixture_tools["git"] = {"executable": str(executable), "sha256": digest(executable)}
            save(self.base / "fixture-tools.json", self.fixture_tools)
        self.db = self.workspace / "runtime.db"
        self.evidence = self.workspace / "evidence"
        self.evidence.mkdir()
        self.commands = []
        self.child_runs = []
        self.environment = os.environ.copy()
        if args.mode == "deterministic":
            self.environment.pop("OPENAI_API_KEY", None)
        self.workflow = self.workspace / (entry["openaiWorkflow"] if args.mode == "openai" else "workflow.yaml")
        self.workflow_value = json.loads(self.workflow.read_text())
        if args.mode == "openai":
            for definition in self.workflow_value["spec"]["agents"].values():
                require(definition["model"] == args.model, "model override needs a matching reviewed pricing entry; regenerate or explicitly edit the live fixture")
        self.start = time.monotonic()
        self.reservation = None

    def cli(self, command, expected=0, filename=None, credential_free=False):
        argv = [str(self.args.agentctl)] + [str(value) for value in command] + ["--output", "json", "--color", "never"]
        environment = self.environment.copy()
        if credential_free:
            environment.pop("OPENAI_API_KEY", None)
            environment["HTTP_PROXY"] = "http://127.0.0.1:1"
            environment["HTTPS_PROXY"] = "http://127.0.0.1:1"
        completed = subprocess.run(argv, cwd=self.cwd, env=environment,
                                   capture_output=True, text=True, timeout=150)
        text = completed.stdout.strip() or completed.stderr.strip()
        try:
            envelope = json.loads(text)
        except ValueError as error:
            raise AssertionError(f"CLI returned no JSON envelope: {completed.returncode}: {text[:2000]}") from error
        require(envelope.get("apiVersion") == "agentctl.dev/cli/v1", "unstable CLI envelope version")
        item = {"argv": argv, "exitCode": completed.returncode, "envelope": envelope}
        self.commands.append(item)
        save(self.evidence / (filename or f"command-{len(self.commands):03}.json"), item)
        if expected is not None:
            codes = expected if isinstance(expected, tuple) else (expected,)
            require(completed.returncode in codes, f"expected exit {codes}, got {completed.returncode}: {text[:2400]}")
        return envelope

    def inspect(self, identifier, filename="inspect.json"):
        return self.cli(["inspect", identifier, "--db", self.db], filename=filename, credential_free=True)["data"]

    def run(self, extra=None, expected=0, workflow=None, database=None):
        return self.cli(["run", workflow or self.workflow, "--workspace", self.workspace,
                         "--db", database or self.db] + (extra or []), expected, "run.json")

    def approve(self, outcome):
        identifier = run_id(outcome)
        for _ in range(12):
            if outcome.get("data", {}).get("state") != "paused":
                return outcome
            approvals = self.cli(["approvals", "--db", self.db, "list", identifier])["data"]
            require(approvals, "paused run has no durable approval")
            for approval in approvals:
                if approval["taskId"] == "apply-local":
                    require(not (self.workspace / "artifacts/local-state.json").exists(), "mutation happened before approval")
                if approval["taskId"] == "deploy":
                    require(json.loads((self.workspace / "artifacts/service.json").read_text())["version"] == "1.0.0",
                            "service changed before deployment approval")
                self.cli(["approvals", "--db", self.db, "approve", approval["approvalId"],
                          "--actor", "devops-fixture-reviewer", "--reason",
                          "Reviewed the bounded local fixture effect, path, and content digest"])
            outcome = self.cli(["resume", identifier, "--db", self.db, "--workspace", self.workspace], (0, 3))
        raise AssertionError("approval loop exceeded 12 reviewed effects")

    def crash_recovery(self):
        argv = [str(self.args.agentctl), "run", str(self.workflow), "--workspace", str(self.workspace),
                "--db", str(self.db), "--output", "json", "--color", "never"]
        child = subprocess.Popen(argv, cwd=self.cwd, env=self.environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        identifier = None
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and child.poll() is None:
                if self.db.exists():
                    try:
                        identifier = latest_run_id(self.db)
                        if identifier:
                            inspection = self.inspect(identifier, "before-crash.json")
                            if any(task["taskId"] == "deploy" and task["state"] == "succeeded" for task in inspection["tasks"]) and any(
                                    effect["request"]["operation"] == "fake" and effect["status"] == "started"
                                    for effect in inspection["effects"]):
                                break
                    except sqlite3.OperationalError:
                        pass
                time.sleep(0.05)
            else:
                raise AssertionError("failed to reach durable deployment interruption boundary")
            child.kill()
            child.communicate(timeout=10)
            save(self.evidence / "crash.json", {"argv": argv, "processExit": child.returncode,
                 "runId": identifier, "boundary": "confirmed deploy followed by delayed fake provider"})
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate(timeout=10)
        outcome = self.cli(["resume", identifier, "--db", self.db, "--workspace", self.workspace], expected=None)
        inspection = self.inspect(identifier, "after-first-resume.json")
        require((self.workspace / "artifacts/mutations.txt").read_text() == "1", "confirmed mutation repeated after interruption")
        require(outcome.get("error", {}).get("exitCode") == 3,
                "resume must refuse the deliberately interrupted at-most-once provider request")
        uncertain = [effect for effect in inspection["effects"] if effect["status"] in {"uncertain", "started"}]
        require(uncertain and all(effect["request"]["operation"] == "fake" for effect in uncertain),
                "unexpected recovery boundary; cannot reconcile a real mutation automatically")
        for effect in uncertain:
            self.cli(["effects", "--db", self.db, "reconcile", effect["request"]["id"],
                      "--status", "not-applied", "--actor", "devops-fixture-reviewer",
                      "--reason", "Killed the in-process fake delay; fixture has no remote service or remote mutation", "--approved"])
        if inspection["run"]["state"] == "failed":
            outcome = self.cli(["retry", self.workflow, identifier, "--failed", "--db", self.db,
                                "--workspace", self.workspace])
        else:
            outcome = self.cli(["resume", identifier, "--db", self.db, "--workspace", self.workspace])
        require((self.workspace / "artifacts/mutations.txt").read_text() == "1", "mutation repeated during reconciled recovery")
        return outcome

    def retry_repair(self):
        failed = self.run(expected=4)
        source_id = run_id(failed)
        require(source_id, "terminal failure lost its run id")
        self.inspect(source_id, "source-failure.json")
        require((self.workspace / "artifacts/mutations.txt").read_text() == "1", "source build did not run once")
        (self.workspace / "fixtures/retry-ready.txt").write_text("available\n")
        self.cli(["retry", self.workflow, source_id, "--failed", "--plan", "--db", self.db])
        retried = self.cli(["retry", self.workflow, source_id, "--failed", "--db", self.db, "--workspace", self.workspace])
        retry_inspection = self.inspect(run_id(retried), "retry-inspect.json")
        require(any(task["taskId"] == "build" and task["disposition"] == "reused" for task in retry_inspection["tasks"]),
                "retry failed to reuse upstream build")
        repaired = self.workspace / "repaired.workflow.yaml"
        self.cli(["repair", repaired, source_id, "--from", "test", "--plan", "--db", self.db])
        result = self.cli(["repair", repaired, source_id, "--from", "test", "--db", self.db, "--workspace", self.workspace])
        require((self.workspace / "artifacts/mutations.txt").read_text() == "1", "retry or repair duplicated upstream mutation")
        self.child_runs.append(run_id(retried))
        return result

    def compensation(self):
        failed = self.run(expected=4)
        source_id = run_id(failed)
        plan = self.cli(["compensate", source_id, "--plan", "--db", self.db, "--workspace", self.workspace])
        require(plan["data"]["executable"], "inverse plan is not executable")
        outcome = self.cli(["compensate", source_id, "--db", self.db, "--workspace", self.workspace])
        source = self.inspect(source_id, "source-reconciled.json")
        require(any(row["status"] == "compensated" for row in source["effectReconciliations"]), "missing compensated reconciliation")
        require(json.loads((self.workspace / "artifacts/service.json").read_text())["version"] == "restored", "inverse did not restore service")
        return outcome

    def replay(self, identifier):
        before = self.inspect(identifier, "before-replay.json")
        hashes = artifact_digests(self.workspace)
        # Remove mutable instruction/variable sources to prove snapshots drive replay.
        moved = []
        for name in ("instructions", "vars"):
            source = self.workspace / name
            if source.exists():
                renamed = source.with_name(name + ".hidden-for-replay")
                source.rename(renamed)
                moved.append((source, renamed))
        try:
            replayed = self.cli(["replay", identifier, "--db", self.db], credential_free=True, filename="replay.json")
        finally:
            for source, renamed in moved:
                renamed.rename(source)
        replay_inspection = self.inspect(run_id(replayed), "replay-inspect.json")
        require(replay_inspection["effects"] == [], "replay dispatched fresh effects")
        require(artifact_digests(self.workspace) == hashes, "replay changed artifact bytes")
        after = self.inspect(identifier, "after-replay.json")
        require(len(before["effects"]) == len(after["effects"]), "replay changed source effects")

    def denial(self):
        value = json.loads((self.workspace / "workflow.yaml").read_text())
        value["spec"]["policy"]["toolsDeny"] = ["extension.process", "filesystem.write"] + list(value["spec"].get("tools", {}))
        value["spec"]["policy"]["approval"] = "never"
        denied_root = self.base / "denied"
        shutil.copytree(ROOT / self.entry["directory"], denied_root)
        filename = denied_root / "denied.workflow.yaml"
        save(filename, value)
        outcome = self.cli(["run", filename, "--workspace", denied_root, "--db", denied_root / "runtime.db"],
                           expected=4, credential_free=True, filename="denial.json")
        require(not any(file.is_file() and file.name != ".lock" for file in (denied_root / "artifacts").rglob("*")), "denied workflow wrote forbidden artifacts")
        require(run_id(outcome), "denial did not retain auditable run identity")
        inspection = self.cli(["inspect", run_id(outcome), "--db", denied_root / "runtime.db"],
                              credential_free=True, filename="denial-inspect.json")["data"]
        denied_events = [row for row in inspection["audit"] if row["eventType"] == "task.transition"
                         and row["payload"].get("to") == "failed"
                         and "denied" in (row["payload"].get("error") or "").lower()]
        trace_ids = {row["traceId"] for row in inspection["traces"]}
        require(denied_events and all(row["traceId"] in trace_ids for row in denied_events),
                "denial lost correlated failure audit/trace evidence")

    def semantics(self, inspection):
        case = self.entry["id"]
        output = inspection["run"].get("output") or {}
        filename = self.workspace / "artifacts/report.json"
        report = json.loads(filename.read_text()) if filename.exists() else {}
        checks = {
            "01": lambda: report["semanticValidation"] and report["analysis"]["rootCause"] == "missing_dependency",
            "02": lambda: report["tests"] == 3 and {item["classification"] for item in report["failures"]} == {"assertion", "infrastructure"},
            "03": lambda: report["validated"] and len(report["violations"]) == 2 and report["patch"]["appliedAndVerified"],
            "04": lambda: report["syntaxValidated"] and report["patch"]["appliedAndVerified"] and report["performanceClaim"] is None,
            "05": lambda: report["manifest"]["spec"]["replicas"] == 2 and len(report["violations"]) == 2,
            "06": lambda: report["allowed"] and json.loads((self.workspace / "artifacts/local-state.json").read_text())["applied"],
            "07": lambda: report["baselineExit"] == 1 and report["updatedExit"] == 0 and report["tests"] == 3,
            "08": lambda: report["decision"] == "no-go" and [row["disposition"] for row in report["findings"]] == ["block", "excepted", "track"],
            "09": lambda: report["commitsVerified"] == 3 and all(len(row["commit"]) == 40 for row in report["notes"]),
            "10": lambda: report["decision"] == "no-go" and report["blocking"] == ["security-review"],
            "11": lambda: report["desired"] == {"replicas": 4, "region": "fixture", "settings": {"timeout": 20}} and report["inputEnvironment"] == "disposable",
            "12": lambda: report["semanticValidation"] and report["analysis"]["durationSeconds"] == 120,
            "13": lambda: report["route"] == "promote" and (self.workspace / "artifacts/promotion.json").exists() and not (self.workspace / "artifacts/rollback.json").exists(),
            "14": lambda: json.loads((self.workspace / "artifacts/service.json").read_text())["version"] == "2.0.0",
            "15": lambda: (self.workspace / "artifacts/mutations.txt").read_text() == "1",
            "16": lambda: (self.workspace / "artifacts/mutations.txt").read_text() == "1" and any(task["taskId"] == "build" and task["disposition"] == "reused" for task in inspection["tasks"]),
            "17": lambda: json.loads((self.workspace / "artifacts/service.json").read_text())["version"] == "restored",
            "18": lambda: len(output["items"]) == 4,
            "19": lambda: report["executed"] and len(inspection["toolCalls"]) == 2,
            "20": lambda: report["validated"] and report["configuration"] == {"timeoutSeconds": 30},
        }
        require(checks[case](), "case-specific semantic assertion failed: " + case)
        require(inspection["audit"] and inspection["traces"], "missing audit/trace evidence")
        require(all(row["traceId"] for row in inspection["audit"]), "audit event lost trace correlation")
        if case == "18":
            items = output["items"]
            require([item["output"]["index"] for item in items] == list(range(4)), "matrix aggregation order is unstable")
        if case == "20":
            require((self.workspace / "artifacts/timeout-seconds.txt").read_bytes() == b"30",
                    "model tool staged bytes outside the approved decimal contract")
            require((self.workspace / "artifacts/remediation.json").read_bytes() == b'{"timeoutSeconds":30}\n',
                    "deterministic remediation serialization changed the approved configuration")
            loop = next(task for task in inspection["tasks"] if task["taskId"] == "remediate")
            require(loop["output"]["iterations"] == 1, "completion guard failed to stop bounded loop")
            require(inspection["budget"]["usage"]["providerRequests"] == 2, "unexpected remediation requests")

    def alternate_cases(self):
        case = self.entry["id"]
        if case in {"01", "12"}:
            malformed = json.loads((self.workspace / "workflow.yaml").read_text())
            response = json.loads(malformed["spec"]["agents"]["analyst"]["providerOptions"]["finalText"])
            label = "classification" if case == "01" else "citations"
            response["rootCause" if case == "01" else "evidence"] = "The requests package is missing." if case == "01" else []
            malformed["spec"]["agents"]["analyst"]["providerOptions"]["finalText"] = json.dumps(response)
            filename = self.workspace / f"malformed-{label}.workflow.yaml"
            database = self.workspace / f"malformed-{label}.db"
            save(filename, malformed)
            failed = self.cli(["run", filename, "--workspace", self.workspace, "--db", database],
                              expected=4, credential_free=True, filename=f"malformed-{label}.json")
            inspection = self.cli(["inspect", run_id(failed), "--db", database], credential_free=True,
                                  filename=f"malformed-{label}-inspect.json")["data"]
            require(any(task["taskId"] == "advise" and task["state"] == "failed" for task in inspection["tasks"])
                    and "provider structured output failed its contract" in failed["error"]["message"],
                    f"invalid {label} were not rejected by the agent schema")
            require(not any(effect["request"]["taskId"] == "verify" for effect in inspection["effects"]),
                    f"malformed {label} reached the downstream verification action")
        if case == "19":
            malformed = json.loads((self.workspace / "workflow.yaml").read_text())
            reviewer = malformed["spec"]["agents"]["reviewer"]
            reviewer["providerOptions"]["finalText"] = json.dumps({"approved": True, "payload": "unreviewed-change"})
            # Negative fixture only: permit malformed fake model data through
            # its schema to test the independently enforced typed handoff.
            reviewer["structuredOutput"]["properties"]["payload"].pop("enum")
            filename = self.workspace / "invalid-handoff.workflow.yaml"
            database = self.workspace / "invalid-handoff.db"
            save(filename, malformed)
            before = artifact_digests(self.workspace)
            failed = self.cli(["run", filename, "--workspace", self.workspace, "--db", database],
                              expected=4, credential_free=True, filename="invalid-handoff.json")
            inspection = self.cli(["inspect", run_id(failed), "--db", database], credential_free=True,
                                  filename="invalid-handoff-inspect.json")["data"]
            require(any(task["taskId"] == "roles--handoff" and task["state"] == "failed" for task in inspection["tasks"]),
                    "invalid reviewer payload did not reach and fail the typed handoff")
            require(not any(effect["request"]["taskId"] == "roles--execute" for effect in inspection["effects"]),
                    "invalid handoff dispatched an executor effect")
            require(artifact_digests(self.workspace) == before, "invalid handoff changed artifact bytes")
        if case == "11":
            explanation = self.cli(["explain", self.workflow, "--workspace", self.workspace],
                                   filename="explain-defaults.json")["data"]
            effective = explanation["effectiveOrigins"]["spec.tasks.analyze"]
            require(effective["replicas"] == "spec.tasks.analyze.vars" and
                    effective["settings"] == "spec.varsFiles[1]:vars/environment.json",
                    "winning variable-source diagnostics lost scope or file order")
            require(all(isinstance(source, str) for scope in explanation["effectiveOrigins"].values()
                        for source in scope.values()), "variable diagnostics exposed values instead of source labels")
            canary = "NON_SECRET_DIAGNOSTIC_VALUE_MUST_BE_REDACTED"
            explained_override = self.cli(["explain", self.workflow, "--workspace", self.workspace,
                "--var", "replicas=6", "--var", "diagnosticProbe=" + json.dumps(canary)],
                filename="explain-invocation.json")
            require(canary not in json.dumps(explained_override), "explain exposed a variable value")
            require(explained_override["data"]["effectiveOrigins"]["spec.tasks.analyze"]["replicas"] == "invocation:--var",
                    "explain lost explicit invocation precedence")
            save(self.workspace / "invocation-vars.json", {"replicas": 5})
            self.run(["--vars-file", self.workspace / "invocation-vars.json", "--var", "replicas=6",
                      "--input", "environment=invoked"], database=self.workspace / "overrides.db")
            report = json.loads((self.workspace / "artifacts/report.json").read_text())
            require(report["desired"]["replicas"] == 6 and report["inputEnvironment"] == "invoked",
                    "explicit vars and typed input namespaces did not remain distinct")
        if case == "13":
            (self.workspace / "artifacts/promotion.json").unlink()
            self.run(["--input", "errorLimit=0.001"], database=self.workspace / "rollback.db")
            require((self.workspace / "artifacts/rollback.json").exists() and not (self.workspace / "artifacts/promotion.json").exists(),
                    "typed rollback route dispatched the wrong branch")

        if case == "20":
            nonconverging = json.loads((self.workspace / "workflow.yaml").read_text())
            nonconverging["spec"]["agents"]["remediator"]["providerOptions"]["finalText"] = json.dumps({"done": False, "timeoutSeconds": 30})
            filename = self.workspace / "nonconverging.workflow.yaml"
            database = self.workspace / "nonconverging.db"
            save(filename, nonconverging)
            failed = self.run(expected=4, workflow=filename, database=database)
            require("loop" in failed["error"]["message"].lower(), "nonconverging loop failed for an unrelated reason")
            inspection = self.cli(["inspect", run_id(failed), "--db", database], credential_free=True,
                                  filename="nonconverging-inspect.json")["data"]
            usage = inspection["budget"]["usage"]
            require(usage["providerRequests"] == 6 and usage["toolCalls"] <= 3,
                    "nonconverging loop did not stop at its declared iteration/request limits")

    def container_build(self):
        engine = self.args.container_engine or shutil.which("docker") or shutil.which("podman")
        require(engine, "--container-build requires usable Docker or Podman")
        tag = "agentctl-devops-fixture:" + self.base.name.lower()
        base_image = self.args.container_base
        records = []
        def command(arguments, timeout=150):
            result = subprocess.run([engine] + arguments, capture_output=True, text=True, timeout=timeout)
            records.append({"argv": [engine] + arguments, "exitCode": result.returncode,
                            "stdout": result.stdout[-10000:], "stderr": result.stderr[-10000:]})
            require(result.returncode == 0, "container gate failed: " + result.stderr[-3000:])
            return result.stdout
        try:
            command(["info"])
            info = json.loads(command(["image", "inspect", base_image]))[0]
            require(info.get("RepoDigests"), "container base needs an immutable repository digest; pull the documented base image first")
            pinned = info["RepoDigests"][0]
            require("@sha256:" in pinned, "container base has no immutable digest reference")
            command(["build", "--network=none", "--build-arg", "BASE_IMAGE=" + pinned, "-t", tag,
                     "-f", str(self.workspace / "artifacts/Dockerfile"), str(self.workspace / "artifacts")])
            built = json.loads(command(["image", "inspect", tag]))[0]
            require(built["Config"]["User"] == "65534:65534", "built image is not non-root")
            response = json.loads(command(["run", "--rm", "--network=none", "--read-only", "--cap-drop=ALL",
                                           "--security-opt=no-new-privileges", built["Id"]]))
            require(response == {"service": "fixture", "healthy": True}, "container health artifact is malformed")
            save(self.workspace / "artifacts/container-build.json", {"baseImageId": info["Id"], "baseReference": pinned, "imageId": built["Id"],
                 "response": response, "performanceClaim": None, "commands": records})
        finally:
            subprocess.run([engine, "image", "rm", tag], capture_output=True, timeout=30)

    def execute(self, ledger):
        server = None
        identifier = None
        live_start = None
        result = {"id": self.entry["id"], "directory": self.entry["directory"], "mode": self.args.mode,
                  "workspace": str(self.workspace), "workflowSha256": digest(self.workflow),
                  "fixtureTools": self.fixture_tools}
        try:
            self.cli(["check", self.workflow], filename="check.json")
            self.cli(["plan", self.workflow], filename="plan.json")
            if self.entry["id"] == "14":
                (self.workspace / "artifacts").mkdir()
                shutil.copy2(self.workspace / "fixtures/service.json", self.workspace / "artifacts/service.json")
                folder = str(self.workspace / "artifacts")
                class Handler(http.server.SimpleHTTPRequestHandler):
                    def __init__(self, *args, **kwargs):
                        super().__init__(*args, directory=folder, **kwargs)
                    def log_message(self, *args):
                        pass
                server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
                threading.Thread(target=server.serve_forever, daemon=True).start()
            if ledger:
                limits = self.workflow_value["spec"]["runtime"]["budgets"]
                self.reservation = ledger.reserve("devops-" + self.entry["id"], limits,
                    {"workflowSha256": digest(self.workflow), "model": self.args.model, "workspace": str(self.workspace)})
                live_start = time.monotonic()
            case = self.entry["id"]
            if case == "15":
                outcome = self.crash_recovery()
            elif case == "16":
                outcome = self.retry_repair()
            elif case == "17":
                outcome = self.compensation()
            else:
                outcome = self.run(expected=(0, 3) if case in {"06", "14"} else 0)
                if case in {"06", "14"}:
                    require(outcome["data"]["state"] == "paused", "governed mutation failed to pause")
                    if server:
                        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                        with opener.open(f"http://127.0.0.1:{server.server_port}/service.json", timeout=3) as response:
                            require(json.load(response)["version"] == "1.0.0", "local HTTP service changed before approval")
                    outcome = self.approve(outcome)
            identifier = run_id(outcome)
            require(outcome.get("data", {}).get("state") == "succeeded", "final workflow outcome is not success")
            inspection = self.inspect(identifier)
            if ledger:
                ledger.reconcile(self.reservation, inspection["budget"]["usage"], time.monotonic() - live_start)
                self.reservation = None
            self.semantics(inspection)
            if server:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                with opener.open(f"http://127.0.0.1:{server.server_port}/service.json", timeout=3) as response:
                    require(json.load(response)["version"] == "2.0.0", "local HTTP service did not serve deployed version")
            self.replay(identifier)
            self.denial()
            self.alternate_cases()
            if case == "04" and self.args.container_build:
                self.container_build()
            result.update({"status": "passed", "runId": identifier, "usage": inspection["budget"]["usage"],
                           "assertions": ["case-specific-semantics", "audit-trace-correlation", "zero-fresh-effect-replay",
                                          "denial-without-forbidden-artifacts"], "artifacts": artifact_digests(self.workspace)})
            if case == "04":
                result["containerBuild"] = "passed" if self.args.container_build else "not-executed"
        except Exception as error:
            identifier = identifier or next((run_id(item["envelope"]) for item in reversed(self.commands)
                                            if run_id(item["envelope"])), None)
            result.update({"status": "failed", "error": str(error), "runId": identifier,
                           "reservationRetained": self.reservation})
            diagnostic = self.evidence / "fixture-error.json"
            if diagnostic.is_file() and diagnostic.stat().st_size < 16384:
                result["fixtureDiagnostic"] = json.loads(diagnostic.read_text())
            # Partial provider usage can be reconciled only when no model effect is uncertain.
            if ledger and self.reservation:
                candidates = [run_id(item["envelope"]) for item in self.commands if run_id(item["envelope"])]
                if candidates:
                    try:
                        partial = self.inspect(candidates[-1], "partial-failure-inspect.json")
                        if partial["run"]["state"] in {"failed", "succeeded", "cancelled"} and not any(
                            effect["request"]["effectClass"] == "model" and effect["status"] in {"started", "uncertain", "requested"}
                            for effect in partial["effects"]):
                            ledger.reconcile(self.reservation, partial["budget"]["usage"], time.monotonic() - live_start)
                            result["reservationRetained"] = None
                    except Exception as reconciliation_error:
                        result["reconciliationError"] = str(reconciliation_error)
        finally:
            if server:
                server.shutdown()
                server.server_close()
        result["wallSeconds"] = round(time.monotonic() - self.start, 3)
        result["commands"] = [{"argv": item["argv"], "exitCode": item["exitCode"]} for item in self.commands]
        if not self.args.keep and result["status"] == "passed":
            try:
                cleanup_workspace(self.base)
                result["workspaceRetained"] = False
            except OSError as error:
                result.update({"status": "failed", "error": "fixture cleanup failed: " + str(error),
                               "workspaceRetained": True})
        else:
            result["workspaceRetained"] = True
        if result["workspaceRetained"]:
            save(self.evidence / "result.json", result)
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agentctl", type=Path, default=ROOT.parent.parent / ("target/debug/agentctl.exe" if os.name == "nt" else "target/debug/agentctl"))
    parser.add_argument("--only", action="append", help="Two-digit catalog ID; repeat to select cases")
    parser.add_argument("--mode", choices=["deterministic", "openai"], default="deterministic")
    parser.add_argument("--model", default="gpt-5-mini")
    parser.add_argument("--live-budget", type=Path, help="Required persistent aggregate budget shared with all other paid gates")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--container-build", action="store_true")
    parser.add_argument("--container-engine")
    parser.add_argument("--container-base", default="python:3.12-slim", help="Already-present Python image; resolved to immutable local ID before build")
    args = parser.parse_args()
    args.agentctl = args.agentctl.resolve()
    require(args.agentctl.is_file(), "build agentctl before running the suite")
    catalog = json.loads((ROOT / "catalog.json").read_text())["examples"]
    require(len(catalog) == 20 and [entry["id"] for entry in catalog] == [f"{n:02}" for n in range(1, 21)],
            "catalog must contain exactly the expected 20 distinct examples")
    disk = {directory.name for directory in ROOT.glob("[0-9][0-9]-*") if directory.is_dir()}
    require(disk == {entry["directory"] for entry in catalog}, "catalog and example inventory differ")
    selected = set(args.only or [])
    require(not selected - {entry["id"] for entry in catalog}, "unknown example ID")
    ledger = None
    if args.mode == "openai":
        require(args.live_budget, "paid mode requires an explicit --live-budget shared across the entire launch suite")
        require(bool(os.environ.get("OPENAI_API_KEY")), "OPENAI_API_KEY unavailable at runtime")
        ledger = LiveBudget(args.live_budget)
    try:
        source = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True)
        report = {"schemaVersion": "agentctl.dev/devops-evidence/v1", "sourceSha": source.stdout.strip(),
                  "sourceWorktreeDirty": bool(dirty.stdout.strip()),
                  "supportSha256": {name: digest(ROOT / name) for name in ["run.py", "fixture.py", "live_budget.py"]},
                  "binarySha256": digest(args.agentctl), "catalogSha256": digest(ROOT / "catalog.json"),
                  "mode": args.mode, "results": []}
        for entry in catalog:
            if selected and entry["id"] not in selected:
                continue
            if args.mode == "openai" and not entry["openaiWorkflow"]:
                continue
            result = Case(args, entry).execute(ledger)
            report["results"].append(result)
            if ledger:
                report["liveBudget"] = ledger.summary()
            save(args.report.resolve(), report)
            print(f"{entry['id']} {result['status']}: {result.get('error', entry['title'])}", flush=True)
            if result.get("fixtureDiagnostic"):
                print("  local fixture diagnostic: " + json.dumps(result["fixtureDiagnostic"]), flush=True)
            if result["workspaceRetained"]:
                print("  " + result["workspace"], flush=True)
            if args.mode == "openai" and result["status"] != "passed":
                break
        require(report["results"], "no executable cases selected")
        return 1 if any(result["status"] != "passed" for result in report["results"]) else 0
    finally:
        if ledger:
            ledger.close()


if __name__ == "__main__":
    sys.exit(main())
