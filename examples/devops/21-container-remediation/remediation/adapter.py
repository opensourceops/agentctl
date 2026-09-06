#!/usr/bin/env python3
"""Bounded deterministic adapters. No network, subprocesses, Git or credentials."""
import hashlib
import json
from pathlib import Path
import re
import sys
from packaging.version import InvalidVersion, Version

PROTOCOL = "agentctl.dev/process-extension/v1"
ROOT = Path.cwd().resolve()
CONTRACT = Path(__file__).resolve().with_name("contract.json")
MAX_BYTES = 16 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def parse(data):
    if len(data) > MAX_BYTES:
        raise ValueError("input exceeds 16 MiB")
    return json.loads(data, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON")))


def path(name):
    if not isinstance(name, str) or Path(name).is_absolute() or ".." in Path(name).parts:
        raise ValueError("path must be relative and traversal-free")
    candidate = ROOT / name
    if any(parent.is_symlink() for parent in [candidate, *candidate.parents] if parent != ROOT.parent):
        raise ValueError("symlinks are not accepted")
    resolved = candidate.resolve()
    if not resolved.is_relative_to(ROOT):
        raise ValueError("path leaves workspace")
    return resolved


def read(name):
    p = path(name)
    if not p.is_file():
        raise ValueError("required regular input file is missing")
    value = p.read_bytes()
    if len(value) > MAX_BYTES:
        raise ValueError("input exceeds 16 MiB")
    return value


def document(name):
    return parse(read(name))


def write_record(name, value):
    p = path(name)
    p.parent.mkdir(parents=True, exist_ok=True)
    temporary = p.with_suffix(p.suffix + ".tmp")
    if temporary.exists() or temporary.is_symlink():
        raise ValueError("unfinished record requires explicit cleanup")
    temporary.write_bytes(canonical(value) + b"\n")
    temporary.replace(p)


def contract():
    return parse(CONTRACT.read_bytes())


def require_digest(value, length=64):
    if not isinstance(value, str) or not re.fullmatch("[0-9a-f]{" + str(length) + "}", value):
        raise ValueError("invalid content identity")
    return value


def image_id(value):
    if not isinstance(value, str) or not value.startswith("sha256:"):
        raise ValueError("image must have an actual sha256 identity")
    require_digest(value[7:])
    return value


def version(value):
    if not isinstance(value, str):
        raise ValueError("package version must be text")
    try:
        parsed = Version(value)
    except InvalidVersion as error:
        raise ValueError("unsupported Python version syntax") from error
    if parsed.is_prerelease or parsed.is_devrelease or parsed.local:
        raise ValueError("prerelease, development and local versions require manual review")
    return parsed


def expected_files(selected, policy=None):
    policy = policy or contract()
    if selected not in policy["wheels"]:
        raise ValueError("package release is not in the reviewed wheel inventory")
    wheel = policy["wheels"][selected]
    require_digest(wheel["sha256"])
    return {
        "requirements.in": ("# Deliberately vulnerable sample dependency; never install into the tooling environment.\n"
                            f"urllib3=={selected}\n").encode(),
        "requirements.lock": ("# Hash-locked pure Python wheel; application dependencies only.\n"
                              f"urllib3=={selected} --hash=sha256:{wheel['sha256']}\n").encode(),
    }


def findings(report, expected_image):
    if not isinstance(report, dict) or report.get("SchemaVersion") != 2 or report.get("ArtifactType") != "container_image":
        raise ValueError("expected a Trivy schema2 container image report")
    if report.get("Metadata", {}).get("ImageID") != image_id(expected_image):
        raise ValueError("Trivy report does not identify the captured image")
    results = report.get("Results")
    if not isinstance(results, list) or len(results) > 10000:
        raise ValueError("invalid Trivy results")
    rows, packages, seen = [], [], set()
    for index, result in enumerate(results):
        if not isinstance(result, dict) or not all(isinstance(result.get(key), str) for key in ["Target", "Class", "Type"]):
            raise ValueError("invalid Trivy result metadata")
        for package in result.get("Packages", []) or []:
            if isinstance(package, dict) and isinstance(package.get("Name"), str) and isinstance(package.get("Version"), str):
                packages.append((result["Type"], package["Name"], package["Version"]))
        entries = result.get("Vulnerabilities", []) or []
        if not isinstance(entries, list):
            raise ValueError("invalid vulnerability list")
        for offset, item in enumerate(entries):
            if not isinstance(item, dict) or not all(isinstance(item.get(key), str) and item[key] for key in ["VulnerabilityID", "PkgName", "InstalledVersion", "Severity"]):
                raise ValueError("malformed vulnerability entry")
            if item["Severity"] not in {"UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL"}:
                raise ValueError("unknown severity")
            fixed = item.get("FixedVersion", "")
            if not isinstance(fixed, str):
                raise ValueError("fixed version must be text")
            row = {"id": item["VulnerabilityID"], "package": item["PkgName"], "installedVersion": item["InstalledVersion"],
                   "fixedVersions": [v.strip() for v in fixed.split(",") if v.strip()], "severity": item["Severity"],
                   "class": result["Class"], "type": result["Type"], "target": result["Target"],
                   "evidence": f"#/Results/{index}/Vulnerabilities/{offset}"}
            identity = tuple(row[k] for k in ["id", "package", "installedVersion", "class", "type", "target"])
            if identity in seen:
                raise ValueError("duplicate finding identity")
            seen.add(identity)
            rows.append(row)
            if len(rows) > 20000:
                raise ValueError("too many findings")
    return rows, packages


def capture(_):
    policy = contract()
    metadata = document("inputs/scan-context.json")
    for field in ["sourceSha", "sourceTree"]:
        require_digest(metadata[field], 40)
    for field in ["reportSha256", "databaseSha256", "databaseMetadataSha256"]:
        require_digest(metadata[field])
    if metadata["repository"] != policy["repository"] or metadata["baseBranch"] != policy["baseBranch"]:
        raise ValueError("repository scope differs from trusted contract")
    if metadata["scannerImage"] != policy["scannerImage"]:
        raise ValueError("scanner image differs from the reviewed immutable pin")
    if sha(read("inputs/before.json")) != metadata["reportSha256"]:
        raise ValueError("before report bytes changed")
    source_files = {name: read("source/" + name) for name in policy["allowedFiles"]}
    selected = next((v for v in [policy["installedVersion"], policy["targetVersion"]] if source_files == expected_files(v, policy)), None)
    if selected is None:
        raise ValueError("unsupported manifest/lockfile or direct dependency mapping")
    rows, packages = findings(document("inputs/before.json"), metadata["imageId"])
    if ("python-pkg", "urllib3", selected) not in packages:
        raise ValueError("Trivy --list-all-pkgs must identify the direct dependency")
    targets = [row for row in rows if row["class"] == "lang-pkgs" and row["type"] == "python-pkg" and row["package"] == "urllib3"]
    residual = [row for row in rows if row not in targets]
    decision, reason = "remediate", "reviewed direct dependency has a supported upgrade"
    if not targets:
        decision, reason = ("manual", "only unsupported package classes or dependencies have findings") if rows else ("no_change", "scan contains no vulnerabilities")
    else:
        for row in targets:
            if selected == policy["targetVersion"] or row["installedVersion"] != selected or not row["fixedVersions"]:
                decision, reason = "manual", "installed version mismatch or no available fixed version"
                break
            try:
                if any(version(policy["targetVersion"]) < version(fixed) for fixed in row["fixedVersions"]):
                    decision, reason = "manual", "reviewed package release does not cover every required fix"
            except ValueError:
                decision, reason = "manual", "unsupported advisory version syntax"
    if len(targets) > 16:
        decision, reason = "manual", "too many targeted advisories for the bounded planner"
    output = {"decision": decision, "reason": reason, "repository": metadata["repository"], "sourceSha": metadata["sourceSha"],
              "sourceTree": metadata["sourceTree"], "reportSha256": metadata["reportSha256"], "imageId": metadata["imageId"],
              "databaseSha256": metadata["databaseSha256"], "databaseMetadataSha256": metadata["databaseMetadataSha256"],
              "scannerImage": metadata["scannerImage"], "package": "urllib3", "installedVersion": selected,
              "targetVersion": policy["targetVersion"], "files": policy["allowedFiles"], "targets": targets,
              "sourceFiles": {name: {"text": data.decode(), "sha256": sha(data)} for name, data in source_files.items()},
              "residualCount": len(residual), "residualPolicy": policy["residualPolicy"], "contractSha256": sha(CONTRACT.read_bytes())}
    write_record("state/context.json", output)
    return output


def validate_plan(payload):
    context, plan = payload["context"], payload["plan"]
    if context != document("state/context.json") or context["decision"] != "remediate":
        raise ValueError("planner context differs from captured remediation inputs")
    if set(plan) != {"decision", "sourceSha", "reportSha256", "package", "installedVersion", "targetVersion", "files", "advisories", "compatibility", "manualItems"}:
        raise ValueError("unexpected remediation plan fields")
    if plan["decision"] != "remediate" or not isinstance(plan["compatibility"], str) or not plan["compatibility"].strip():
        raise ValueError("plan did not request a supported edit with compatibility notes")
    for name in ["sourceSha", "reportSha256", "package", "installedVersion", "targetVersion", "files"]:
        if plan[name] != context[name]:
            raise ValueError("plan widened or changed captured authority: " + name)
    expected = [{name: row[name] for name in ["id", "evidence", "fixedVersions"]} for row in context["targets"]]
    if plan["advisories"] != expected:
        raise ValueError("plan invented, omitted, reordered or changed advisory evidence")
    if not isinstance(plan["manualItems"], list) or not all(isinstance(v, str) for v in plan["manualItems"]):
        raise ValueError("manual concerns must be plain advisory text")
    approved = {**plan, "validated": True, "planSha256": sha(canonical(plan)),
                "expectedFiles": {name: data.decode() for name, data in expected_files(context["targetVersion"]).items()}}
    write_record("state/validated-plan.json", approved)
    return approved


def patch_identity(files):
    return sha(canonical({name: sha(content) for name, content in sorted(files.items())}))


def validate_patch(payload):
    context, approved, implementation = payload["context"], payload["plan"], payload["implementation"]
    if context != document("state/context.json") or approved != document("state/validated-plan.json"):
        raise ValueError("patch input differs from captured validated handoff")
    if implementation != {"applied": True, "package": "urllib3", "targetVersion": context["targetVersion"], "files": context["files"]}:
        raise ValueError("implementation did not confirm the exact bounded edit")
    present = {p.relative_to(path("patch")).as_posix() for p in path("patch").rglob("*") if p.is_file() or p.is_symlink()}
    if present != set(context["files"]):
        raise ValueError("patch contains unauthorized or missing files")
    files = {name: read("patch/" + name) for name in context["files"]}
    if files != expected_files(context["targetVersion"]):
        raise ValueError("patched bytes differ from the approved dependency and lock hashes")
    if {name: sha(read("source/" + name)) for name in context["files"]} != {name: context["sourceFiles"][name]["sha256"] for name in context["files"]}:
        raise ValueError("original source changed during agent execution")
    digest = patch_identity(files)
    output = {"ready": True, "sourceSha": context["sourceSha"], "sourceTree": context["sourceTree"], "planSha256": approved["planSha256"],
              "patchDigest": digest, "files": {name: sha(data) for name, data in files.items()},
              "fingerprint": sha(canonical({"repository": context["repository"], "sourceSha": context["sourceSha"], "patchDigest": digest}))}
    write_record("state/patch-ready.json", output)
    return output


def finding_key(row):
    return tuple(row[key] for key in ["id", "package", "installedVersion", "class", "type"])


def eligibility(_):
    context = document("state/context.json")
    gate = document("inputs/gates.json")
    policy = contract()
    if context["contractSha256"] != sha(CONTRACT.read_bytes()) or context["repository"] != policy["repository"]:
        raise ValueError("captured policy identity changed")
    if context["files"] != policy["allowedFiles"] or context["targetVersion"] != policy["targetVersion"] or context["scannerImage"] != policy["scannerImage"]:
        raise ValueError("captured remediation scope changed")
    if sha(read("inputs/before.json")) != context["reportSha256"]:
        raise ValueError("captured before report changed")
    before, _ = findings(document("inputs/before.json"), context["imageId"])
    actual_targets = [row for row in before if row["class"] == "lang-pkgs" and row["type"] == "python-pkg" and row["package"] == "urllib3"]
    if actual_targets != context["targets"]:
        raise ValueError("captured advisory set changed")
    if gate["sourceSha"] != context["sourceSha"] or gate["sourceTree"] != context["sourceTree"]:
        raise ValueError("external gates belong to a different source")
    if context["decision"] != "remediate":
        output = {"eligible": False, "decision": context["decision"], "reason": context["reason"], "sourceSha": context["sourceSha"]}
        write_record("state/publication.json", output)
        return output
    patch = document("state/patch-ready.json")
    for flag in ["testsPassed", "buildSucceeded", "scanSucceeded"]:
        if gate.get(flag) is not True:
            raise ValueError("trusted outer gate did not pass: " + flag)
    for name in ["databaseSha256", "databaseMetadataSha256", "scannerImage", "reportSha256"]:
        if gate[name] != context[name]:
            raise ValueError("scan context changed between before and after: " + name)
    if gate["patchDigest"] != patch["patchDigest"] or patch_identity({name: read("patch/" + name) for name in context["files"]}) != patch["patchDigest"]:
        raise ValueError("rebuild did not use the validated patch")
    require_digest(gate["validatedTree"], 40)
    if gate["beforeImageId"] != context["imageId"] or image_id(gate["afterImageId"]) == context["imageId"]:
        raise ValueError("before and changed application image identities are invalid")
    if sha(read("inputs/after.json")) != gate["afterReportSha256"]:
        raise ValueError("rescan bytes changed")
    after, packages = findings(document("inputs/after.json"), gate["afterImageId"])
    if ("python-pkg", "urllib3", context["targetVersion"]) not in packages:
        raise ValueError("rescanned image does not contain the approved fixed dependency")
    if any(row["package"] == "urllib3" and row["type"] == "python-pkg" for row in after):
        raise ValueError("target package findings remain after remediation")
    previous = {finding_key(row) for row in before}
    if any(row["severity"] in contract()["blockingSeverities"] and finding_key(row) not in previous for row in after):
        raise ValueError("rescan introduced a new blocking finding")
    output = {"eligible": True, "decision": "publish", "sourceSha": context["sourceSha"], "sourceTree": context["sourceTree"],
              "validatedTree": gate["validatedTree"], "patchDigest": patch["patchDigest"], "fingerprint": patch["fingerprint"],
              "files": patch["files"], "beforeImageId": context["imageId"], "afterImageId": gate["afterImageId"],
              "removedAdvisories": [row["id"] for row in context["targets"]], "residualFindings": after,
              "databaseSha256": context["databaseSha256"], "beforeReportSha256": context["reportSha256"],
              "afterReportSha256": gate["afterReportSha256"], "gateSha256": sha(read("inputs/gates.json"))}
    write_record("state/publication.json", output)
    return output


OPERATIONS = {"capture": capture, "validate-plan": validate_plan, "validate-patch": validate_patch, "eligibility": eligibility}


def main():
    from schemas import INPUTS, OUTPUTS
    operation, mode = sys.argv[1:3]
    if mode == "--agentctl-handshake":
        print(json.dumps({"protocolVersion": PROTOCOL, "name": "bounded-python-remediation", "inputSchema": INPUTS[operation],
                          "outputSchema": OUTPUTS[operation], "capabilities": ["remediation.validate"]}))
    elif mode == "--agentctl-invoke":
        request = parse(sys.stdin.buffer.read(MAX_BYTES + 1))
        output = OPERATIONS[operation](request["input"])
        print(json.dumps({"protocolVersion": PROTOCOL, "effectId": request["effectId"], "output": output}))
    else:
        raise ValueError("unknown adapter mode")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(type(error).__name__ + ": " + str(error), file=sys.stderr)
        sys.exit(1)
