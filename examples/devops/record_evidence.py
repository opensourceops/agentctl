#!/usr/bin/env python3
"""Record compact, source-labeled evidence from actual DevOps runner reports."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read_report(filename):
    data = json.loads(filename.read_text())
    if data.get("schemaVersion") != "agentctl.dev/devops-evidence/v1":
        raise ValueError("input is not a DevOps runner report")
    source = {name: data.get(name) for name in ["sourceSha", "sourceWorktreeDirty", "binarySha256",
                                               "catalogSha256", "supportSha256", "mode"]}
    source["reportSha256"] = hashlib.sha256(filename.read_bytes()).hexdigest()
    source["reportName"] = filename.name
    results = []
    for result in data["results"]:
        summary = {key: result[key] for key in ["id", "status", "runId", "workflowSha256",
                   "wallSeconds", "usage", "assertions", "containerBuild", "error"] if key in result}
        summary["exitCodes"] = [command["exitCode"] for command in result["commands"]]
        results.append(summary)
    return {"source": source, "results": results, "liveBudget": data.get("liveBudget")}



def live_coverage(reports):
    """Describe source-labeled union coverage without turning failed runs green."""
    required = ["01", "12", "19", "20"]
    cases = {identifier: {"passed": False, "attempts": []} for identifier in required}
    for report in reports:
        if report.get("source", {}).get("mode") != "openai":
            continue
        for result in report["results"]:
            if result["id"] not in cases:
                raise ValueError("unexpected live DevOps example ID")
            case = cases[result["id"]]
            case["attempts"].append({"source": report["source"], "result": result})
            case["passed"] = case["passed"] or result["status"] == "passed"
    return {"scope": "Union of individually passed cases at their recorded sources; failures remain in attempts. This is not a whole-invocation or final-source release verdict.",
            "requiredIds": required, "allRequiredIdsHavePassed": all(case["passed"] for case in cases.values()),
            "cases": cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--deterministic", type=Path, required=True)
    parser.add_argument("--container", type=Path)
    parser.add_argument("--focused", type=Path, action="append", default=[])
    parser.add_argument("--live", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "validation.json")
    args = parser.parse_args()
    evidence = {"schemaVersion": "agentctl.dev/devops-validation/v1",
                "recordedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "scope": "Local execution evidence; final code-commit and hosted release gates must be assessed separately",
                "deterministic": read_report(args.deterministic),
                "focused": [read_report(filename) for filename in args.focused],
                "live": read_report(args.live) if args.live else {"status": "not-executed", "requiredIds": ["01", "12", "19", "20"]}}
    if {result["id"] for result in evidence["deterministic"]["results"]} != {f"{i:02}" for i in range(1, 21)}:
        raise ValueError("deterministic evidence must include all twenty IDs, including failures")
    if args.container:
        evidence["container"] = read_report(args.container)
        raw = json.loads(args.container.read_text())
        for result in raw["results"]:
            filename = Path(result["workspace"]) / "artifacts/container-build.json"
            expected = result.get("artifacts", {}).get("artifacts/container-build.json")
            if filename.is_file() and hashlib.sha256(filename.read_bytes()).hexdigest() == expected:
                image = json.loads(filename.read_text())
                evidence["container"]["image"] = {key: image[key] for key in ["baseImageId", "baseReference",
                                                                           "imageId", "response", "performanceClaim"]}
    else:
        evidence["container"] = {"status": "not-executed"}
    evidence["liveCoverage"] = live_coverage(evidence["focused"] + [evidence["live"]])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
