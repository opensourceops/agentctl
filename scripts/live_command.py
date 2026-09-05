#!/usr/bin/env python3
"""Reserve shared paid-test allowance around an explicitly selected agentctl CLI.

This wrapper never rewrites a workflow or performs a provider preflight. Metadata
reads use the credential-free migrate/inspect commands. Unknown/uncertain usage
keeps the entire reservation charged. Provider credentials are never persisted.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time


_MODULE = Path(__file__).resolve().parents[1] / "examples/devops/live_budget.py"
_SPEC = importlib.util.spec_from_file_location("agentctl_shared_live_budget", _MODULE)
_BUDGET = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_BUDGET)
LiveBudget = _BUDGET.LiveBudget

DISPATCH = {"run", "fork", "resume", "retry", "repair"}
VALUE_FLAGS = {"--output", "--color", "--db", "--workspace", "--timeout-seconds",
               "--inputs", "--inputs-file", "--input", "--var", "--vars-file",
               "--from", "--reason"}
LIMITS = ("maxProviderRequests", "maxTotalTokens", "maxWallTimeSeconds", "maxCostMicrousd")
USAGE = ("providerRequests", "inputTokens", "outputTokens", "wallTimeSeconds", "costMicrousd")


class BudgetError(ValueError):
    """Fail closed before dispatch, or retain a reservation after dispatch."""


def redact(value):
    value = value or b""
    for name, secret in os.environ.items():
        if secret and (name.endswith("_API_KEY") or name in {"OPENAI_API_KEY", "AZURE_OPENAI_API_KEY"}):
            value = value.replace(secret.encode(), b"[REDACTED]")
    return value


def emit(stdout=b"", stderr=b""):
    sys.stdout.buffer.write(redact(stdout))
    sys.stderr.buffer.write(redact(stderr))
    sys.stdout.buffer.flush()
    sys.stderr.buffer.flush()


def warning(message):
    sys.stderr.buffer.write(redact(f"live budget: {message}\n".encode()))
    sys.stderr.buffer.flush()


def parsed_arguments(argv):
    """Read only known CLI syntax; the selected executable validates everything."""
    positional, flags = [], {}
    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument == "--":
            positional.extend(argv[index + 1:])
            break
        name, separator, value = argument.partition("=")
        if name in VALUE_FLAGS:
            if not separator:
                index += 1
                if index >= len(argv):
                    raise BudgetError(f"missing value for {name}")
                value = argv[index]
            flags[name] = value
        elif argument.startswith("-"):
            flags[name] = True
        else:
            positional.append(argument)
        index += 1
    return positional, flags


def documents(content):
    try:
        value = json.loads(content)
        if isinstance(value, dict):
            return [value]
    except (ValueError, UnicodeError):
        pass
    result = []
    for line in content.splitlines():
        try:
            value = json.loads(line)
            if isinstance(value, dict):
                result.append(value)
        except (ValueError, UnicodeError):
            continue
    return result


def read_data(binary, arguments):
    result = subprocess.run([binary, *arguments, "--output", "json", "--color", "never"],
                            capture_output=True, timeout=30, check=False)
    envelopes = documents(result.stdout)
    if result.returncode or len(envelopes) != 1 or not envelopes[0].get("ok"):
        raise BudgetError(f"credential-free {arguments[0]} failed; live command was not dispatched")
    data = envelopes[0].get("data")
    if not isinstance(data, dict):
        raise BudgetError("metadata command returned an invalid envelope")
    return data


def inspect(binary, run_id, database):
    return read_data(binary, ["inspect", run_id, "--db", database])


def nonnegative(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def complete_usage(inspection, require_stopped=True):
    if require_stopped and inspection.get("run", {}).get("state") not in {"paused", "succeeded", "failed", "cancelled"}:
        raise BudgetError("run is not durably stopped; full reservation retained")
    budget = inspection.get("budget", {})
    usage, reserved = budget.get("usage"), budget.get("reserved")
    if not isinstance(usage, dict) or any(not nonnegative(usage.get(key)) for key in USAGE):
        raise BudgetError("durable usage is incomplete; full reservation retained")
    if not isinstance(reserved, dict) or not reserved or any(not nonnegative(v) or v != 0 for v in reserved.values()):
        raise BudgetError("runtime has outstanding budget reservations; full reservation retained")
    if any(key not in reserved for key in USAGE):
        raise BudgetError("durable reserved counters are incomplete; full reservation retained")
    if not nonnegative(usage.get("unpricedProviderRequests")) or usage["unpricedProviderRequests"]:
        raise BudgetError("provider usage is unpriced or unknown; full reservation retained")
    effects = inspection.get("effects")
    if not isinstance(effects, list):
        raise BudgetError("durable effects are unavailable; full reservation retained")
    providers = set(inspection.get("run", {}).get("workflow", {}).get("spec", {}).get("providers", {}))
    for effect in effects:
        request = effect.get("request", {})
        model = request.get("effectClass") == "model" or request.get("operation") in providers
        if model and effect.get("status") not in {"succeeded", "failed", "requested", "waiting_for_approval"}:
            raise BudgetError("provider effect is uncertain or unfinished; full reservation retained")
    return usage


def runtime_limits(workflow):
    if not isinstance(workflow, dict):
        raise BudgetError("source workflow metadata is invalid")
    limits = workflow.get("spec", {}).get("runtime", {}).get("budgets", {})
    if any(not nonnegative(limits.get(key)) or not limits[key] for key in LIMITS):
        raise BudgetError("live execution requires explicit positive request, total-token, wall-time, and cost budgets")
    return {key: limits[key] for key in LIMITS}


def delta_usage(after, before):
    delta = {}
    for key in USAGE:
        if after[key] < before[key]:
            raise BudgetError("durable usage decreased; full reservation retained")
        delta[key] = after[key] - before[key]
    delta["unpricedProviderRequests"] = 0
    return delta


def retain_inspection_metadata(ledger, reservation, run_id, inspection):
    """Keep audit identity and numeric counters even when reconciliation refuses.

    Do not retain workflow, prompt, effect payload, provider error text or output.
    """
    row = ledger.connection.execute("SELECT metadata_json FROM reservations WHERE id=?", (reservation,)).fetchone()
    metadata = json.loads(row[0])
    budget = inspection.get("budget", {})
    metadata["runId"] = run_id
    metadata["runState"] = inspection.get("run", {}).get("state")
    metadata["observedBudget"] = {section: {key: value for key, value in budget.get(section, {}).items()
                                           if nonnegative(value)} for section in ["usage", "reserved"]}
    exceeded = budget.get("exceeded")
    if isinstance(exceeded, dict):
        metadata["budgetExceeded"] = {key: exceeded[key] for key in ["dimension", "limit", "attempted"] if key in exceeded}
    metadata["effectStatuses"] = [effect.get("status") for effect in inspection.get("effects", [])]
    ledger.connection.execute("UPDATE reservations SET metadata_json=? WHERE id=?", (json.dumps(metadata), reservation))


def exit_status(code):
    return 128 - code if code < 0 else code


def execute(budget_path, model, argv):
    if not argv or not Path(argv[0]).is_absolute():
        raise BudgetError("supply an absolute agentctl executable after --")
    binary, arguments = argv[0], argv[1:]
    positional, flags = parsed_arguments(arguments)
    command = positional[0] if positional else ""
    if command not in DISPATCH or any(flag in flags for flag in {"--help", "-h", "--version"}) or (
        command == "run" and "--check" in flags
    ) or (command in {"retry", "repair"} and "--plan" in flags):
        result = subprocess.run(argv, capture_output=True, check=False)
        emit(result.stdout, result.stderr)
        return exit_status(result.returncode)
    if len(positional) < 2:
        raise BudgetError("live command is missing its workflow or source run")
    database = flags.get("--db", ".agentctl/runtime.db")
    before = {key: 0 for key in USAGE}
    if command in {"fork", "resume"}:
        source = inspect(binary, positional[1], database)
        workflow = source.get("run", {}).get("workflow", {})
        if command == "resume":
            before = complete_usage(source, require_stopped=False)
    else:
        workflow = read_data(binary, ["migrate", positional[1]]).get("workflow", {})
    limits = runtime_limits(workflow)
    if command == "resume":
        for name, consumed in zip(LIMITS, [before["providerRequests"], before["inputTokens"] + before["outputTokens"],
                                          before["wallTimeSeconds"], before["costMicrousd"]]):
            limits[name] -= consumed
        if any(value <= 0 for value in limits.values()):
            raise BudgetError("source run has no remaining live allowance")
    timeout = limits["maxWallTimeSeconds"]
    if "--timeout-seconds" in flags:
        try:
            requested = int(flags["--timeout-seconds"])
        except (TypeError, ValueError) as error:
            raise BudgetError("invalid command timeout") from error
        if requested <= 0:
            raise BudgetError("command timeout must be positive")
        timeout = min(timeout, requested)
    ledger = LiveBudget(budget_path)
    actual_models = sorted({agent.get("model", "") for agent in workflow.get("spec", {}).get("agents", {}).values()})
    try:
        reservation = ledger.reserve(f"legacy:{command}", limits, {"command": command, "model": model, "workflowModels": actual_models})
    except BaseException:
        ledger.connection.close()
        raise
    start = time.monotonic()
    try:
        result = subprocess.run(argv, capture_output=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        emit(error.stdout, error.stderr)
        warning("command timed out; full reservation retained")
        ledger.connection.close()
        return 124
    except (OSError, KeyboardInterrupt):
        warning("command interrupted or failed to start; full reservation retained")
        ledger.connection.close()
        return 130
    elapsed = time.monotonic() - start
    emit(result.stdout, result.stderr)
    run_ids = set()
    for envelope in documents(result.stdout) + documents(result.stderr):
        for field in ("data", "error"):
            item = envelope.get(field)
            run_id = item.get("runId") if isinstance(item, dict) else None
            if isinstance(run_id, str) and run_id:
                run_ids.add(run_id)
    if command == "resume":
        run_ids.add(positional[1])
    result_code = exit_status(result.returncode)
    try:
        if result.returncode < 0 or len(run_ids) != 1:
            raise BudgetError("no unique completed run identity; full reservation retained")
        run_id = next(iter(run_ids))
        inspection = inspect(binary, run_id, database)
        retain_inspection_metadata(ledger, reservation, run_id, inspection)
        usage = complete_usage(inspection)
        ledger.reconcile(reservation, delta_usage(usage, before), elapsed)
    except (BudgetError, ValueError, OSError, sqlite3.Error, subprocess.TimeoutExpired) as error:
        warning(str(error))
        if result_code == 0:
            result_code = 2
    finally:
        ledger.connection.close()
    return result_code


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budget", required=True, type=Path)
    parser.add_argument("--model", required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if not args.budget.is_absolute():
        parser.error("--budget must be an absolute shared SQLite path")
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    try:
        return execute(args.budget, args.model, command)
    except (BudgetError, ValueError, OSError, sqlite3.Error, subprocess.TimeoutExpired) as error:
        warning(str(error))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
