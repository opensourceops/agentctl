#!/usr/bin/env python3
"""Preallocate a release task and CI jobs inside an EXISTING paid-test ledger.

The original ledger remains the authority: init reserves the whole task envelope
there; each job lease reserves part of that envelope in the task ledger. CI gets
only its nonsecret lease, never the original ledger or its remaining allowance.
execute uses live_command.py and its durable-usage checks, without making its own
provider requests. receipt/reconcile release a job reservation only after every
child reservation is complete. close releases the original envelope only when
all jobs have complete receipts; uncertainty retains the entire envelope.

A lease binds repository, source, workflow ref, run ID OR workflow run number,
attempt, and job. Run-number binding permits preallocation before workflow_dispatch
creates a run ID. A concurrent dispatch fails closed; reruns require a NEW lease.
See https://docs.github.com/en/actions/reference/workflows-and-actions/variables.
Use one paid job (no matrix) per binding. The trusted workflow must preserve the
execution ledger in RUNNER_TEMP, and retrieve receipts from the exact leased run.
Each lease permits one fresh agentctl run, followed only by resumes of that same
durably paused run. Repeating a completed run requires a new reserved lease.
Leases/receipts are not signatures: untrusted model output is never a receipt.

Typical sequence (all paths absolute):
  init --original-ledger OLD --task-ledger TASK --task-id release-20260906
  lease --task-ledger TASK --repo OWNER/REPO --source SHA --workflow-ref REF \
    --run-number N --run-attempt 1 --job-id remediate --max-requests 12 \
    --max-tokens 18000 --max-wall-seconds 420 --max-cost-microusd 900000
  execute --lease LEASE.json --execution-ledger JOB -- /abs/agentctl run ...
  receipt --lease LEASE.json --execution-ledger JOB
  reconcile --task-ledger TASK --receipt RECEIPT.json
  close --task-ledger TASK

No command resets an existing allowance. Missing receipts, timeouts, outstanding
runtime reservations, unknown prices, and uncertain effects keep charges intact.
Reasoning tokens are already included in outputTokens and are never added twice.
"""
import argparse
from contextlib import closing, contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import time


_SPEC = importlib.util.spec_from_file_location("release_live_command", Path(__file__).with_name("live_command.py"))
wrapper = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(wrapper)
LiveBudget = wrapper.LiveBudget
BudgetError = wrapper.BudgetError
FIELDS = {"providerRequests": "maxProviderRequests", "totalTokens": "maxTotalTokens",
          "wallTimeSeconds": "maxWallTimeSeconds", "costMicrousd": "maxCostMicrousd"}
TASK_MAXIMUM = {"providerRequests": 30, "totalTokens": 40000,
                "wallTimeSeconds": 900, "costMicrousd": 2000000}
MODEL = "gpt-6-astra"
VERSION = "agentctl.dev/release-live-budget/v1"


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def digest(value):
    return hashlib.sha256(encoded(value).encode()).hexdigest()


def absolute(value):
    path = Path(value)
    if not path.is_absolute():
        raise BudgetError("ledger paths must be absolute")
    return path.resolve()


def counters(value, maximum=None):
    if not isinstance(value, dict) or set(value) != set(FIELDS):
        raise BudgetError("all four budget dimensions are required")
    if any(not wrapper.nonnegative(number) or not number for number in value.values()):
        raise BudgetError("budget dimensions must be positive integers")
    if maximum and any(value[key] > maximum[key] for key in FIELDS):
        raise BudgetError("requested allowance exceeds the task envelope")
    return value


def runtime_limits(value):
    return {field: value[key] for key, field in FIELDS.items()}


@contextmanager
def locked(path):
    """Serialize coordinator metadata with existing LiveBudget transactions.

    A bounded local file lock also protects crash recovery between the original
    and task databases. CI never mounts or concurrently copies either authority.
    """
    lock = absolute(path).with_name(Path(path).name + ".release.lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("a+b") as handle:
        handle.seek(0, os.SEEK_END)
        if handle.tell() == 0:
            handle.write(b"0")
            handle.flush()
        deadline = time.monotonic() + 30
        while True:
            try:
                if os.name == "nt":
                    import msvcrt
                    handle.seek(0)
                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except (BlockingIOError, OSError):
                if time.monotonic() >= deadline:
                    raise BudgetError("budget coordinator lock timed out") from None
                time.sleep(0.05)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle, fcntl.LOCK_UN)


def existing(path):
    path = absolute(path)
    if not path.is_file():
        raise BudgetError("existing ledger is required; refusing a fresh independent allowance")
    try:
        with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as connection:
            row = connection.execute("SELECT limits_json FROM budget_config WHERE id=1").fetchone()
            counters(json.loads(row[0]) if row else None)
    except (sqlite3.Error, ValueError, TypeError) as error:
        raise BudgetError("existing ledger must contain its original configured allowance") from error
    return path


def config(ledger):
    try:
        row = ledger.connection.execute("SELECT value FROM release_config WHERE id=1").fetchone()
    except sqlite3.OperationalError as error:
        raise BudgetError("task ledger has not been initialized") from error
    if not row:
        raise BudgetError("task ledger has not been initialized")
    return json.loads(row[0])


def save_config(ledger, value):
    ledger.connection.execute("CREATE TABLE IF NOT EXISTS release_config (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
    ledger.connection.execute("INSERT OR REPLACE INTO release_config VALUES (1, ?)", (encoded(value),))


def reservation_rows(ledger):
    return [{"id": row[0], "status": row[1], "charged": json.loads(row[2]),
             "reserved": json.loads(row[3]), "metadata": json.loads(row[4])}
            for row in ledger.connection.execute("SELECT id,status,charged_json,reserved_json,metadata_json FROM reservations ORDER BY created,id")]


def initialize(original_path, task_path, task_id, limits=None):
    original_path, task_path = existing(original_path), absolute(task_path)
    if original_path == task_path or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,100}", task_id):
        raise BudgetError("use a distinct task ledger and a stable simple task ID")
    limits = counters(limits or TASK_MAXIMUM, TASK_MAXIMUM)
    with locked(original_path), locked(task_path), LiveBudget(original_path) as original:
        matches = [row for row in reservation_rows(original)
                   if row["metadata"].get("kind") == "release-envelope" and row["metadata"].get("taskId") == task_id]
        if len(matches) > 1:
            raise BudgetError("duplicate original task reservations require reconciliation")
        if matches:
            envelope = matches[0]
            if envelope["reserved"] != limits or envelope["metadata"].get("taskLedger") != str(task_path):
                raise BudgetError("task identity is already bound to different limits or ledger")
            if envelope["status"] != "reserved":
                raise BudgetError("task envelope has already been closed; it cannot be reallocated")
            envelope_id = envelope["id"]
        else:
            if task_path.exists():
                raise BudgetError("unrecognized existing task ledger; refusing to adopt or reset it")
            envelope_id = original.reserve("release:" + task_id, runtime_limits(limits),
                {"kind": "release-envelope", "taskId": task_id, "taskLedger": str(task_path)})
        # An interruption before this write leaves the original reservation fully
        # charged. Repeating init locates it by task ID and never reserves twice.
        with LiveBudget(task_path, limits) as task:
            value = {"version": VERSION, "taskId": task_id, "originalLedger": str(original_path),
                     "envelopeId": envelope_id, "limits": limits, "state": "active", "model": MODEL}
            try:
                previous = config(task)
            except BudgetError:
                previous = None
            if previous is not None and previous != value:
                raise BudgetError("existing task configuration differs")
            save_config(task, value)
            return {key: val for key, val in value.items() if key != "originalLedger"}


def binding(repo, source, workflow_ref, run_id, run_number, run_attempt, job_id):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise BudgetError("repository must be OWNER/REPO")
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise BudgetError("source must be an exact lowercase Git SHA")
    if not workflow_ref.startswith(repo + "/.github/workflows/") or "@refs/heads/" not in workflow_ref:
        raise BudgetError("workflow ref must name this repository's exact branch workflow")
    if bool(run_id) == bool(run_number) or not re.fullmatch(r"[1-9][0-9]*", str(run_id or run_number)):
        raise BudgetError("bind exactly one positive CI run ID or workflow run number")
    if not re.fullmatch(r"[1-9][0-9]*", str(run_attempt)) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", job_id):
        raise BudgetError("run attempt and job ID are required")
    return {"repository": repo, "sourceSha": source, "workflowRef": workflow_ref,
            "runId": str(run_id) if run_id else None, "runNumber": str(run_number) if run_number else None,
            "runAttempt": str(run_attempt), "jobId": job_id}


def lease(task_path, context, limits):
    task_path = existing(task_path)
    with locked(task_path), LiveBudget(task_path) as task:
        task_config = config(task)
        if task_config["state"] != "active":
            raise BudgetError("task is closed")
        with LiveBudget(existing(task_config["originalLedger"])) as original:
            envelopes = [row for row in reservation_rows(original) if row["id"] == task_config["envelopeId"]]
            if len(envelopes) != 1 or envelopes[0]["status"] != "reserved":
                raise BudgetError("original task envelope is no longer reserved")
        limits = counters(limits, task_config["limits"])
        matches = [row for row in reservation_rows(task) if row["metadata"].get("binding") == context]
        if matches:
            row = matches[0]
            if row["reserved"] != limits or row["status"] != "reserved":
                raise BudgetError("existing job lease cannot be resized or dispatched again after reconciliation")
            lease_id = row["id"]
        else:
            lease_id = task.reserve("release-job:" + context["jobId"], runtime_limits(limits),
                                    {"kind": "release-job", "binding": context})
        return {"version": VERSION, "taskId": task_config["taskId"], "envelopeId": task_config["envelopeId"],
                "leaseId": lease_id, "binding": context, "limits": limits, "model": task_config["model"]}


def validate_lease(value):
    if not isinstance(value, dict) or set(value) != {"version", "taskId", "envelopeId", "leaseId", "binding", "limits", "model"}:
        raise BudgetError("invalid lease contract")
    if value["version"] != VERSION or value["model"] != MODEL:
        raise BudgetError("unsupported lease version or model")
    counters(value["limits"], TASK_MAXIMUM)
    b = value["binding"]
    if not isinstance(b, dict):
        raise BudgetError("invalid lease binding")
    expected = binding(b.get("repository", ""), b.get("sourceSha", ""), b.get("workflowRef", ""),
                       b.get("runId"), b.get("runNumber"), b.get("runAttempt"), b.get("jobId", ""))
    if expected != b:
        raise BudgetError("invalid lease binding")
    return value


def verify_ci(value, environment):
    b = validate_lease(value)["binding"]
    names = {"repository": "GITHUB_REPOSITORY", "sourceSha": "GITHUB_SHA", "workflowRef": "GITHUB_WORKFLOW_REF",
             "runId": "GITHUB_RUN_ID", "runNumber": "GITHUB_RUN_NUMBER", "runAttempt": "GITHUB_RUN_ATTEMPT", "jobId": "GITHUB_JOB"}
    for key, name in names.items():
        if b[key] is not None and environment.get(name) != b[key]:
            raise BudgetError(f"lease does not authorize this CI context: {name}")
    if not re.fullmatch(r"[1-9][0-9]*", environment.get("GITHUB_RUN_ID", "")):
        raise BudgetError("actual CI run identity is required")


def claim(value, execution_path, environment):
    verify_ci(value, environment)
    execution_path = absolute(execution_path)
    runner_temp = absolute(environment.get("RUNNER_TEMP", ""))
    if not execution_path.is_relative_to(runner_temp):
        raise BudgetError("execution ledger must stay in trusted RUNNER_TEMP outside the agent workspace")
    registry_path = runner_temp / "agentctl-release-claims.sqlite3"
    runner_temp.mkdir(parents=True, exist_ok=True)
    with locked(registry_path), closing(sqlite3.connect(registry_path, isolation_level=None)) as registry:
        registry.execute("CREATE TABLE IF NOT EXISTS claims (lease_id TEXT PRIMARY KEY, digest TEXT NOT NULL, ledger TEXT NOT NULL)")
        previous = registry.execute("SELECT digest,ledger FROM claims WHERE lease_id=?", (value["leaseId"],)).fetchone()
        if previous and (previous != (digest(value), str(execution_path)) or not execution_path.is_file()):
            raise BudgetError("lease is already claimed; restore its original execution ledger instead of resetting it")
        if not previous and execution_path.exists():
            raise BudgetError("execution ledger already exists without this lease claim")
        with LiveBudget(execution_path, value["limits"]) as execution:
            execution.connection.execute("CREATE TABLE IF NOT EXISTS release_lease (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
            row = execution.connection.execute("SELECT value FROM release_lease WHERE id=1").fetchone()
            if row and json.loads(row[0]) != value:
                raise BudgetError("execution ledger belongs to another lease")
            execution.connection.execute("INSERT OR IGNORE INTO release_lease VALUES (1, ?)", (encoded(value),))
        registry.execute("INSERT OR IGNORE INTO claims VALUES (?, ?, ?)", (value["leaseId"], digest(value), str(execution_path)))
    return execution_path


def execute(value, execution_path, argv, environment=None):
    environment = os.environ if environment is None else environment
    execution_path = claim(value, execution_path, environment)
    if not argv or not Path(argv[0]).is_absolute():
        raise BudgetError("supply an absolute agentctl binary")
    positional, flags = wrapper.parsed_arguments(argv[1:])
    if len(positional) < 2 or positional[0] not in wrapper.DISPATCH:
        raise BudgetError("lease execution accepts only explicit live dispatch commands")
    baseline = {key: 0 for key in wrapper.USAGE}
    if positional[0] in {"resume", "fork"}:
        source = wrapper.inspect(argv[0], positional[1], flags.get("--db", ".agentctl/runtime.db"))
        workflow = source.get("run", {}).get("workflow", {})
        if positional[0] == "resume":
            baseline = {key: wrapper.complete_usage(source)[key] for key in wrapper.USAGE}
    else:
        workflow = wrapper.read_data(argv[0], ["migrate", positional[1]]).get("workflow", {})
    agents = workflow.get("spec", {}).get("agents", {})
    if not agents or any(agent.get("model") != value["model"] or agent.get("reasoning", {}).get("effort") != "high" for agent in agents.values()):
        raise BudgetError("leased agent workflows require gpt-6-astra with explicit high reasoning")
    with locked(execution_path):
        with LiveBudget(execution_path) as execution:
            rows = reservation_rows(execution)
            previous = {row["id"] for row in rows}
            execution.connection.execute("CREATE TABLE IF NOT EXISTS release_dispatch (id INTEGER PRIMARY KEY, started INTEGER NOT NULL)")
            started = execution.connection.execute("SELECT started FROM release_dispatch WHERE id=1").fetchone()
            if positional[0] == "resume":
                if not rows or any(row["metadata"].get("runId") != positional[1] for row in rows):
                    raise BudgetError("lease can only resume its own recorded run")
                if source.get("run", {}).get("state") != "paused":
                    raise BudgetError("lease only resumes a durably paused run")
            elif started:
                raise BudgetError("lease already dispatched a run; reuse its evidence or reserve a new lease")
            execution.connection.execute("INSERT OR IGNORE INTO release_dispatch VALUES (1, 1)")
        try:
            return wrapper.execute(execution_path, value["model"], argv)
        finally:
            # This extra redacted baseline makes receipt arithmetic checkable
            # even when the command resumes a run with previous paid usage.
            with LiveBudget(execution_path) as execution:
                for row in reservation_rows(execution):
                    if row["id"] not in previous:
                        metadata = {**row["metadata"], "releaseUsageBefore": baseline}
                        execution.connection.execute("UPDATE reservations SET metadata_json=? WHERE id=?", (encoded(metadata), row["id"]))


def checked_records(ledger):
    records = reservation_rows(ledger)
    if not records:
        raise BudgetError("no dispatched command evidence; retain the job reservation")
    for row in records:
        if row["status"] != "reconciled":
            raise BudgetError("child command has incomplete or exceeded usage; retain the job reservation")
        metadata = row["metadata"]
        if not metadata.get("runId") or not metadata.get("workflowModels") or any(model != MODEL for model in metadata["workflowModels"]):
            raise BudgetError("child command lacks the exact model or durable run identity")
        wrapper.complete_usage({"run": {"state": metadata.get("runState")}, "budget": metadata.get("observedBudget", {}),
            "effects": [{"request": {"effectClass": "model"}, "status": status} for status in metadata.get("effectStatuses", [])]})
        if not isinstance(metadata.get("effectStatuses"), list) or not metadata["effectStatuses"]:
            raise BudgetError("child effect evidence is missing")
        if set(row["charged"]) != set(FIELDS) or any(not wrapper.nonnegative(n) for n in row["charged"].values()):
            raise BudgetError("invalid charged child counters")
        verify_charge(row["charged"], metadata.get("releaseUsageBefore"), metadata["observedBudget"]["usage"])
    return records


def verify_charge(charged, before, after):
    if not isinstance(before, dict) or set(before) != set(wrapper.USAGE) or any(not wrapper.nonnegative(n) for n in before.values()):
        raise BudgetError("command lacks a complete trusted pre-dispatch usage baseline")
    delta = wrapper.delta_usage(after, before)
    expected = {"providerRequests": delta["providerRequests"], "totalTokens": delta["inputTokens"] + delta["outputTokens"],
                "costMicrousd": delta["costMicrousd"]}
    if any(charged[key] != number for key, number in expected.items()):
        raise BudgetError("charged usage differs from durable command usage; retain reservation")


def receipt(value, execution_path, environment=None):
    environment = os.environ if environment is None else environment
    verify_ci(value, environment)
    with LiveBudget(existing(execution_path), value["limits"]) as execution:
        row = execution.connection.execute("SELECT value FROM release_lease WHERE id=1").fetchone()
        if row is None or json.loads(row[0]) != value:
            raise BudgetError("receipt ledger does not belong to this lease")
        records = checked_records(execution)
        # Numeric observations only: never export workflow, prompts, effects,
        # secrets, provider output, local paths or arbitrary metadata fields.
        public = [{"reservationId": row["id"], "charged": row["charged"], "runId": row["metadata"]["runId"],
                   "runState": row["metadata"]["runState"], "observedBudget": row["metadata"]["observedBudget"],
                   "effectStatuses": row["metadata"]["effectStatuses"], "usageBefore": row["metadata"]["releaseUsageBefore"]} for row in records]
        return {"version": VERSION, "lease": value, "actualRunId": environment["GITHUB_RUN_ID"],
                "complete": True, "charged": execution.summary()["charged"], "commands": public}


def validate_receipt(value):
    if not isinstance(value, dict) or value.get("version") != VERSION or value.get("complete") is not True:
        raise BudgetError("receipt is incomplete; retain reservation")
    leased = validate_lease(value.get("lease"))
    if leased["binding"]["runId"] and value.get("actualRunId") != leased["binding"]["runId"]:
        raise BudgetError("receipt actual run differs from leased run")
    if not re.fullmatch(r"[1-9][0-9]*", value.get("actualRunId", "")):
        raise BudgetError("receipt actual run identity is missing")
    commands = value.get("commands")
    if not isinstance(commands, list) or not commands:
        raise BudgetError("receipt has no command evidence")
    total = {key: 0 for key in FIELDS}
    identities = set()
    for command in commands:
        identity = command.get("reservationId")
        if not isinstance(identity, str) or not identity or identity in identities or not command.get("runId"):
            raise BudgetError("receipt contains duplicate or missing command identities")
        identities.add(identity)
        if not isinstance(command.get("effectStatuses"), list) or not command["effectStatuses"]:
            raise BudgetError("receipt lacks effect completion evidence")
        wrapper.complete_usage({"run": {"state": command.get("runState")}, "budget": command.get("observedBudget", {}),
            "effects": [{"request": {"effectClass": "model"}, "status": state} for state in command["effectStatuses"]]})
        charged = command.get("charged", {})
        if set(charged) != set(FIELDS) or any(not wrapper.nonnegative(n) for n in charged.values()):
            raise BudgetError("receipt contains invalid actual charges")
        verify_charge(charged, command.get("usageBefore"), command["observedBudget"]["usage"])
        for key in total:
            total[key] += charged[key]
    if total != value.get("charged") or any(total[key] > leased["limits"][key] for key in total):
        raise BudgetError("receipt arithmetic exceeds or differs from leased allowance")
    return leased, total


def reconcile(task_path, value):
    leased, charged = validate_receipt(value)
    with locked(existing(task_path)), LiveBudget(task_path) as task:
        cfg = config(task)
        if cfg["taskId"] != leased["taskId"] or cfg["envelopeId"] != leased["envelopeId"]:
            raise BudgetError("receipt belongs to another task envelope")
        rows = [row for row in reservation_rows(task) if row["id"] == leased["leaseId"]]
        if len(rows) != 1 or rows[0]["metadata"].get("binding") != leased["binding"] or rows[0]["reserved"] != leased["limits"]:
            raise BudgetError("receipt does not match a reserved job lease")
        row = rows[0]
        if row["status"] != "reserved":
            if row["status"] == "reconciled" and row["charged"] == charged and row["metadata"].get("receiptSha256") == digest(value):
                return {"status": "already-reconciled", "leaseId": leased["leaseId"], "charged": charged}
            raise BudgetError("lease was already reconciled with different evidence")
        if row["metadata"].get("receiptSha256", digest(value)) != digest(value):
            raise BudgetError("pending reconciliation already binds different receipt evidence")
        metadata = {**row["metadata"], "receiptSha256": digest(value), "actualRunId": value["actualRunId"]}
        task.connection.execute("UPDATE reservations SET metadata_json=? WHERE id=?", (encoded(metadata), leased["leaseId"]))
        task.reconcile(leased["leaseId"], {"providerRequests": charged["providerRequests"], "inputTokens": charged["totalTokens"],
                       "outputTokens": 0, "costMicrousd": charged["costMicrousd"], "unpricedProviderRequests": 0}, charged["wallTimeSeconds"])
        return {"status": "reconciled", "leaseId": leased["leaseId"], "charged": charged}


def close(task_path):
    with locked(existing(task_path)), LiveBudget(task_path) as task:
        cfg = config(task)
        rows = reservation_rows(task)
        if any(row["status"] != "reconciled" or not row["metadata"].get("receiptSha256") for row in rows):
            raise BudgetError("unreconciled jobs remain; entire original task envelope retained")
        charged = task.summary()["charged"]
        with LiveBudget(existing(cfg["originalLedger"])) as original:
            row = next(row for row in reservation_rows(original) if row["id"] == cfg["envelopeId"])
            if row["status"] == "reserved":
                original.reconcile(cfg["envelopeId"], {"providerRequests": charged["providerRequests"],
                    "inputTokens": charged["totalTokens"], "outputTokens": 0, "costMicrousd": charged["costMicrousd"],
                    "unpricedProviderRequests": 0}, charged["wallTimeSeconds"])
            elif row["status"] != "reconciled" or row["charged"] != charged:
                raise BudgetError("original envelope has conflicting reconciliation")
            save_config(task, {**cfg, "state": "closed"})
            return {"status": "closed", "taskCharged": charged, "original": original.summary()}


def read_json(path):
    with Path(path).open("rb") as handle:
        raw = handle.read(2 * 1024 * 1024 + 1)
    if len(raw) > 2 * 1024 * 1024:
        raise BudgetError("metadata artifact exceeds 2 MiB")
    return json.loads(raw)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    init = sub.add_parser("init")
    init.add_argument("--original-ledger", required=True)
    init.add_argument("--task-ledger", required=True)
    init.add_argument("--task-id", required=True)
    allocate = sub.add_parser("lease")
    allocate.add_argument("--task-ledger", required=True)
    for key in ["repo", "source", "workflow-ref", "run-attempt", "job-id"]:
        allocate.add_argument("--" + key, required=True)
    run = allocate.add_mutually_exclusive_group(required=True)
    run.add_argument("--run-id")
    run.add_argument("--run-number")
    for key in ["max-requests", "max-tokens", "max-wall-seconds", "max-cost-microusd"]:
        allocate.add_argument("--" + key, type=int, required=True)
    for name in ["execute", "receipt"]:
        command = sub.add_parser(name)
        command.add_argument("--lease", required=True)
        command.add_argument("--execution-ledger", required=True)
        if name == "execute":
            command.add_argument("argv", nargs=argparse.REMAINDER)
    resolve = sub.add_parser("reconcile")
    resolve.add_argument("--task-ledger", required=True)
    resolve.add_argument("--receipt", required=True)
    for name in ["status", "close"]:
        command = sub.add_parser(name)
        command.add_argument("--task-ledger", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "init":
            result = initialize(args.original_ledger, args.task_ledger, args.task_id)
        elif args.command == "lease":
            context = binding(args.repo, args.source, args.workflow_ref, args.run_id, args.run_number, args.run_attempt, args.job_id)
            result = lease(args.task_ledger, context, dict(zip(FIELDS, [args.max_requests, args.max_tokens, args.max_wall_seconds, args.max_cost_microusd])))
        elif args.command == "execute":
            arguments = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
            return execute(read_json(args.lease), args.execution_ledger, arguments)
        elif args.command == "receipt":
            result = receipt(read_json(args.lease), args.execution_ledger)
        elif args.command == "reconcile":
            result = reconcile(args.task_ledger, read_json(args.receipt))
        elif args.command == "close":
            result = close(args.task_ledger)
        else:
            with LiveBudget(existing(args.task_ledger)) as task:
                cfg = config(task)
                result = {"taskId": cfg["taskId"], "state": cfg["state"], **task.summary()}
        print(json.dumps(result, sort_keys=True))
        return 0
    except (BudgetError, ValueError, OSError, sqlite3.Error, KeyError, TypeError) as error:
        wrapper.warning(str(error))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
