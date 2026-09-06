"""Named, bounded format operations for examples 03–07.

These trusted host helpers are not a sandbox or a replacement for cluster,
Terraform-provider, package-registry, or GitHub-runner validation.
"""
import copy
import difflib
import hashlib
import json
from pathlib import Path
import re

from yaml_io import loads, dumps

ACTIONLINT_VERSION = "1.7.7"
KUBERNETES_SCHEMA_SHA256 = "7ecfcb16d4e530e985608d4b1e0ad08e802f15f8a4004ab47ec86d3f64ef42fb"


def schema(fields):
    return {"type": "object", "additionalProperties": False, "required": list(fields),
            "properties": {name: {"type": kind} for name, kind in fields.items()}}


INPUT_SCHEMAS = {
    "pipeline-propose": schema({"sourcePath": "string", "maxTimeoutMinutes": "integer"}),
    "pipeline-validate": schema({"sourcePath": "string", "proposalPath": "string", "maxTimeoutMinutes": "integer", "requireActionlint": "boolean", "actionlintPath": "string"}),
    "dockerfile-propose": schema({"sourcePath": "string"}),
    "dockerfile-validate": schema({"sourcePath": "string", "proposalPath": "string", "appPath": "string"}),
    "kubernetes-propose": schema({"sourcePath": "string"}),
    "kubernetes-validate": schema({"sourcePath": "string", "proposalPath": "string", "schemaPath": "string"}),
    "terraform-analyze": schema({"planPath": "string", "allowedFilename": "string"}),
    "vendor-prepare": schema({"targetPath": "string", "testPath": "string", "patchPath": "string"}),
    "vendor-test-before": schema({"targetName": "string", "sourceSha256": "string", "testName": "string", "testSha256": "string"}),
    "vendor-apply": schema({"targetName": "string", "sourceSha256": "string", "proposedSha256": "string", "patchSha256": "string"}),
    "vendor-test-after": schema({"targetName": "string", "proposedSha256": "string", "baselineTests": "integer", "testName": "string", "testSha256": "string"}),
}


_PROPOSAL_FIELDS = {"proposalPath": "string", "patchPath": "string", "sourceSha256": "string", "proposedSha256": "string", "violations": "array"}
OUTPUT_SCHEMAS = {
    "pipeline-propose": schema(_PROPOSAL_FIELDS),
    "pipeline-validate": schema({"violations": "array", "patch": "object", "validated": "boolean", "validationScope": "string", "actionlint": "object"}),
    "dockerfile-propose": schema({**{key: value for key, value in _PROPOSAL_FIELDS.items() if key != "violations"}, "improvements": "array"}),
    "dockerfile-validate": schema({"improvements": "array", "patch": "object", "syntaxValidated": "boolean", "validationScope": "string", "containerBuild": "string", "performanceClaim": "null"}),
    "kubernetes-propose": schema(_PROPOSAL_FIELDS),
    "kubernetes-validate": schema({"manifest": "object", "violations": "array", "schemaScope": "string", "schemaSha256": "string", "validated": "boolean"}),
    "terraform-analyze": schema({"allowed": "boolean", "denied": "array", "reasons": "object", "changes": "array", "desiredContent": "string", "targetPath": "string", "scope": "string"}),
    "vendor-prepare": schema({"targetName": "string", "sourceSha256": "string", "proposedSha256": "string", "patchSha256": "string", "testName": "string", "testSha256": "string", "scope": "string"}),
    "vendor-test-before": schema({"baselineExit": "integer", "tests": "integer"}),
    "vendor-apply": schema({"appliedAndVerified": "boolean", "reused": "boolean", "sha256": "string"}),
    "vendor-test-after": schema({"baselineExit": "integer", "updatedExit": "integer", "tests": "integer", "scope": "string"}),
}
for _output in OUTPUT_SCHEMAS.values():
    _output["properties"]["reportText"] = {"type": "string"}
    _output["required"].append("reportText")


def sha(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def proposal(common, filename, before, after):
    patch_text = "".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                            fromfile="a/" + filename, tofile="b/" + filename))
    common.write("artifacts/proposed.patch", patch_text)
    common.write("artifacts/proposed/" + filename, after)
    return {"proposalPath": "artifacts/proposed/" + filename,
            "patchPath": "artifacts/proposed.patch", "sourceSha256": sha(before), "proposedSha256": sha(after)}


def apply_proposal(common, filename, before, after):
    if before == after:
        common.write("artifacts/patch-workspace/" + filename, after)
        return {"path": "artifacts/proposed.patch", "appliedAndVerified": True,
                "changed": False, "sha256": sha("")}
    return common.patch(filename, before, after)


def pipeline_review(value, limit):
    if type(limit) is not int or not 1 <= limit <= 60:
        raise ValueError("maxTimeoutMinutes must be an integer from 1 to 60")
    if not isinstance(value, dict) or "on" not in value or not isinstance(value.get("jobs"), dict) or not value["jobs"]:
        raise ValueError("expected a GitHub Actions workflow with on and nonempty jobs")
    reviewed, violations = copy.deepcopy(value), []
    # This recipe intentionally grants only repository-content read access.
    # Deployment/publishing workflows need a separately reviewed permission rule.
    permissions = {"contents": "read"}
    if reviewed.get("permissions") != permissions:
        violations.append({"rule": "least-privilege", "location": "/permissions"})
        reviewed["permissions"] = permissions
    for name, job in reviewed["jobs"].items():
        if not isinstance(job, dict) or "uses" in job:
            raise ValueError("this patch recipe supports ordinary jobs, not reusable-workflow call jobs")
        if not isinstance(job.get("steps"), list) or not job["steps"] or "runs-on" not in job:
            raise ValueError("each job requires runs-on and nonempty steps")
        timeout = job.get("timeout-minutes")
        if type(timeout) is not int or not 1 <= timeout <= limit:
            violations.append({"rule": "bounded-job", "location": f"/jobs/{name}/timeout-minutes"})
            job["timeout-minutes"] = limit
        if "permissions" in job and job["permissions"] != permissions:
            violations.append({"rule": "least-privilege", "location": f"/jobs/{name}/permissions"})
            job["permissions"] = permissions.copy()
    return reviewed, violations


def pipeline_propose(common, payload):
    before = common.read(payload["sourcePath"])
    reviewed, violations = pipeline_review(loads(before), payload["maxTimeoutMinutes"])
    return {**proposal(common, "pipeline.yaml", before, dumps(reviewed)), "violations": violations}


def pipeline_validate(common, payload):
    before, after = common.read(payload["sourcePath"]), common.read(payload["proposalPath"])
    expected, violations = pipeline_review(loads(before), payload["maxTimeoutMinutes"])
    if loads(after) != expected:
        raise ValueError("pipeline proposal changed unrelated fields or failed the reviewed rules")
    actionlint = {"status": "unverified", "version": None,
                  "scope": "local permission/timeout rules only; GitHub Actions syntax and expressions require actionlint"}
    if payload["requireActionlint"]:
        executable = str(common.path(payload["actionlintPath"]))
        version = common.command([executable, "-version"]).stdout.splitlines()
        if not version or version[0].strip() != ACTIONLINT_VERSION:
            raise ValueError("install the pinned actionlint 1.7.7 binary; no tools are downloaded by this workflow")
        # Disable optional shellcheck/pyflakes discovery so the evidence scope
        # and dependency set are stable across developer machines.
        checked = common.command([executable, "-shellcheck=", "-pyflakes=", str(common.path(payload["proposalPath"]))])
        common.write("artifacts/actionlint.txt", checked.stdout + checked.stderr)
        actionlint = {"status": "passed", "version": ACTIONLINT_VERSION,
                      "scope": "actionlint syntax, expressions and built-in semantic checks; shellcheck and pyflakes disabled"}
    applied = apply_proposal(common, "pipeline.yaml", before, after)
    return {"violations": violations, "patch": applied, "validated": True,
            "validationScope": "exact allowed field changes and local permission/timeout rules", "actionlint": actionlint}


def dockerfile_review(before):
    if len(re.findall(r"(?im)^FROM\s", before)) != 1 or not re.search(r"(?im)^WORKDIR\s+/app\s*$", before):
        raise ValueError("this recipe supports one Python image stage with WORKDIR /app")
    after = re.sub(r"(?im)^COPY\s+\.\s+/app\s*$", "COPY app.py /app/app.py", before)
    after = re.sub(r"(?im)^USER\s+(?:root|0)(?::(?:root|0))?\s*$", "USER 65534:65534", after)
    users = re.findall(r"(?im)^USER\s+([^\n]+)$", after)
    if not users:
        after += "\nUSER 65534:65534\n"
    if not re.search(r"(?m)^COPY app\.py /app/app\.py$", after):
        raise ValueError("this recipe requires COPY . /app or COPY app.py /app/app.py")
    improvements = []
    if re.search(r"(?im)^COPY\s+\.\s+/app\s*$", before):
        improvements.append("explicit-copy")
    if not users or re.search(r"(?im)^USER\s+(?:root|0)(?::(?:root|0))?\s*$", before):
        improvements.append("non-root-user")
    return after, improvements


def dockerfile_propose(common, payload):
    before = common.read(payload["sourcePath"])
    after, improvements = dockerfile_review(before)
    return {**proposal(common, "Dockerfile", before, after), "improvements": improvements}


def dockerfile_validate(common, payload):
    before, after = common.read(payload["sourcePath"]), common.read(payload["proposalPath"])
    expected, improvements = dockerfile_review(before)
    if after != expected:
        raise ValueError("Dockerfile proposal differs from the reviewed narrow recipe")
    app = common.read(payload["appPath"])
    compile(app, payload["appPath"], "exec")  # Parse only; never run supplied Python here.
    applied = apply_proposal(common, "Dockerfile", before, after)
    common.write("artifacts/Dockerfile", after)
    common.write("artifacts/app.py", app)
    return {"improvements": improvements, "patch": applied, "syntaxValidated": True,
            "validationScope": "Python syntax and narrow Dockerfile recipe; actual image build/run is separate",
            "containerBuild": "not-executed; use the documented optional image gate", "performanceClaim": None}


def kubernetes_review(value):
    if not isinstance(value, dict) or value.get("apiVersion") != "apps/v1" or value.get("kind") != "Deployment":
        raise ValueError("expected one apps/v1 Deployment")
    reviewed, violations = copy.deepcopy(value), []
    spec = reviewed["spec"]
    replicas = spec.get("replicas", 1)
    if isinstance(replicas, str) and re.fullmatch(r"[0-9]+", replicas):
        violations.append({"rule": "replicas-integer", "location": "/spec/replicas"})
        spec["replicas"] = replicas = int(replicas)
    if type(replicas) is not int or not 0 <= replicas <= 100:
        raise ValueError("replicas must be an integer from 0 to 100 for this local review")
    pod = spec["template"]["spec"]
    for collection in ("containers", "initContainers"):
        for index, container in enumerate(pod.get(collection, [])):
            context = container.setdefault("securityContext", {})
            effective_user = context.get("runAsUser", pod.get("securityContext", {}).get("runAsUser"))
            root_user = type(effective_user) is int and effective_user == 0
            changed = context.get("runAsNonRoot") is not True or context.get("allowPrivilegeEscalation") is not False or context.get("privileged") is True or root_user
            if changed:
                violations.append({"rule": "non-root", "location": f"/spec/template/spec/{collection}/{index}/securityContext"})
            context["runAsNonRoot"] = True
            context["allowPrivilegeEscalation"] = False
            if context.get("privileged") is True:
                context["privileged"] = False
            if root_user:
                context["runAsUser"] = 65534
    return reviewed, violations


def kubernetes_propose(common, payload):
    before = common.read(payload["sourcePath"])
    reviewed, violations = kubernetes_review(loads(before))
    return {**proposal(common, "deployment.yaml", before, dumps(reviewed)), "violations": violations}


def kubernetes_validate(common, payload):
    from jsonschema import Draft4Validator
    schema_text = common.read(payload["schemaPath"])
    if sha(schema_text) != KUBERNETES_SCHEMA_SHA256:
        raise ValueError("Kubernetes schema does not match the bundled v1.35.0 pin")
    schema_value = json.loads(schema_text)
    Draft4Validator.check_schema(schema_value)
    manifest = loads(common.read(payload["proposalPath"]))
    expected, violations = kubernetes_review(loads(common.read(payload["sourcePath"])))
    if manifest != expected:
        raise ValueError("Deployment proposal changed unrelated fields or failed the security recipe")
    Draft4Validator(schema_value).validate(manifest)
    labels = manifest["spec"]["template"]["metadata"]["labels"]
    if any(labels.get(key) != value for key, value in manifest["spec"]["selector"].get("matchLabels", {}).items()):
        raise ValueError("Deployment selector does not match template labels")
    common.write("artifacts/deployment.json", manifest)
    common.write("artifacts/deployment.yaml", dumps(manifest))
    return {"manifest": manifest, "violations": violations,
            "schemaScope": "all 120 referenced upstream Kubernetes v1.35.0 Deployment definitions; IntOrString translated to JSON Schema; no cluster admission, CEL, server defaulting or image validation",
            "schemaSha256": KUBERNETES_SCHEMA_SHA256, "validated": True}


def contains_true(value):
    if isinstance(value, dict):
        return any(contains_true(child) for child in value.values())
    if isinstance(value, list):
        return any(contains_true(child) for child in value)
    return value is True


def terraform_analyze(common, payload):
    plan = common.document(payload["planPath"])
    if not isinstance(plan, dict) or not isinstance(plan.get("format_version"), str) or not re.fullmatch(r"1\.[0-9]+", plan["format_version"]):
        raise ValueError("expected supported terraform show -json format_version 1.x")
    if not isinstance(plan.get("terraform_version"), str) or not isinstance(plan.get("resource_changes"), list):
        raise ValueError("plan requires terraform_version and resource_changes")
    allowed_filename = payload["allowedFilename"]
    common.path(allowed_filename)  # Explicitly confined disposable fixture target.
    changes, denied, reasons, content = [], [], {}, None
    for item in plan["resource_changes"]:
        address, change = item["address"], item["change"]
        if not isinstance(address, str) or not address or not isinstance(change, dict):
            raise ValueError("resource change requires a nonempty address and a change object")
        actions = change["actions"]
        after = change.get("after") or {}
        violations = []
        if item.get("mode") != "managed" or item.get("type") != "local_file" or item.get("provider_name") != "registry.terraform.io/hashicorp/local":
            violations.append("only the explicit managed local_file provider is allowed")
        if actions not in [["create"], ["update"], ["no-op"]]:
            violations.append("deletion, replacement, reading and unknown action sequences are denied")
        if after.get("filename") != allowed_filename:
            violations.append("filename differs from the explicit disposable target")
        for marker in ("after_unknown", "after_sensitive"):
            value = change.get(marker, {})
            if not isinstance(value, (dict, bool)):
                raise ValueError("unknown and sensitive markers must be object masks or booleans")
            if value is True or isinstance(value, dict) and any(contains_true(value.get(key)) for key in ("filename", "content")):
                violations.append("unknown or sensitive target/content cannot authorize this fixture mutation")
        if not isinstance(after.get("content"), str) or len(after.get("content", "").encode()) > 65536:
            violations.append("local content must be a known string of at most 64 KiB")
        if violations:
            denied.append(address)
            reasons[address] = violations
        else:
            content = after["content"]
        changes.append({"address": address, "actions": actions})
    if len(changes) != 1:
        denied.append("plan")
        reasons["plan"] = ["this governed demonstration requires exactly one resource change"]
    return {"allowed": not denied, "denied": denied, "reasons": reasons, "changes": changes,
            "desiredContent": content if not denied else "", "targetPath": allowed_filename,
            "scope": "analysis of terraform show -json; approval can authorize a builtin.write disposable local fixture only; terraform apply is never executed"}


def target_path(common, name, prefix="artifacts/patch-workspace"):
    if not isinstance(name, str) or not name or Path(name).name != name or name in {".", ".."}:
        raise ValueError("targetName must be one filename")
    return common.path(prefix + "/" + name)


def vendor_prepare(common, payload):
    before, tests, patch_text = (common.read(payload[key]) for key in ("targetPath", "testPath", "patchPath"))
    target_name, test_name = Path(payload["targetPath"]).name, Path(payload["testPath"]).name
    if not target_name.endswith(".py") or not test_name.startswith("test_") or not test_name.endswith(".py") or target_name == test_name:
        raise ValueError("supply one vendored Python module and a separate test_*.py unittest file")
    patch_file = common.path("artifacts/proposed.patch")
    common.write("artifacts/proposed.patch", patch_text)
    staging = common.path("artifacts/proposal-workspace")
    staging.mkdir(parents=True, exist_ok=True)
    # Keep Git's path reporting independent of a caller's enclosing checkout.
    # This initializes only the disposable proposal directory and makes no commits.
    common.command(["git", "init", "--quiet"], cwd=staging)
    staged = target_path(common, target_name, "artifacts/proposal-workspace")
    staged.write_bytes(before.encode("utf-8"))
    stats = common.command(["git", "apply", "--numstat", str(patch_file)], cwd=staging).stdout.strip().splitlines()
    if len(stats) != 1 or len(stats[0].split("\t")) != 3 or stats[0].split("\t")[-1] != target_name:
        raise ValueError("supplied patch must touch only the named vendored module")
    common.command(["git", "-c", "core.autocrlf=false", "apply", "--check", str(patch_file)], cwd=staging)
    common.command(["git", "-c", "core.autocrlf=false", "apply", str(patch_file)], cwd=staging)
    after = staged.read_bytes().decode("utf-8")
    work = common.path("artifacts/patch-workspace")
    work.mkdir(parents=True, exist_ok=True)
    target_path(common, target_name).write_bytes(before.encode("utf-8"))
    target_path(common, test_name).write_bytes(tests.encode("utf-8"))
    return {"targetName": target_name, "sourceSha256": sha(before), "proposedSha256": sha(after),
            "patchSha256": sha(patch_text), "testName": test_name, "testSha256": sha(tests),
            "scope": "supplied single-file vendored Python code patch; no package-manager or registry upgrade"}


def run_vendor_tests(common, expected, payload):
    test = target_path(common, payload["testName"])
    if not test.name.startswith("test_") or test.suffix != ".py" or sha(test.read_bytes().decode("utf-8")) != payload["testSha256"]:
        raise ValueError("test bytes changed after preparation")
    result = common.command([common.python_executable(), "-B", "-m", "unittest", "-v", test.stem],
                            cwd=common.path("artifacts/patch-workspace"), expected=expected)
    output = result.stdout + result.stderr
    match = re.search(r"Ran (\d+) tests? in", output)
    if not match or int(match.group(1)) < 1 or "ERROR:" in output:
        raise ValueError("vendored-code evidence requires discovered behavioral tests, without import or execution errors")
    if expected == 1 and "FAIL:" not in output:
        raise ValueError("baseline must show an actual failing assertion")
    common.write("artifacts/test-before.txt" if expected else "artifacts/test-after.txt", output)
    return int(match.group(1))


def vendor_test_before(common, payload):
    if sha(target_path(common, payload["targetName"]).read_bytes().decode("utf-8")) != payload["sourceSha256"]:
        raise ValueError("vendored baseline differs from the prepared source")
    return {"baselineExit": 1, "tests": run_vendor_tests(common, 1, payload)}


def vendor_apply(common, payload):
    target = target_path(common, payload["targetName"])
    current = sha(target.read_bytes().decode("utf-8"))
    patch_text = common.read("artifacts/proposed.patch")
    if sha(patch_text) != payload["patchSha256"]:
        raise ValueError("patch changed after preparation")
    if current == payload["proposedSha256"]:
        return {"appliedAndVerified": True, "reused": True, "sha256": payload["patchSha256"]}
    if current != payload["sourceSha256"]:
        raise ValueError("target changed after the failing baseline")
    command = ["git", "-c", "core.autocrlf=false", "apply"]
    patch_file = str(common.path("artifacts/proposed.patch"))
    common.command(command + ["--check", patch_file], cwd=target.parent)
    common.command(command + [patch_file], cwd=target.parent)
    if sha(target.read_bytes().decode("utf-8")) != payload["proposedSha256"]:
        raise ValueError("applied bytes differ from the prepared proposal")
    return {"appliedAndVerified": True, "reused": False, "sha256": payload["patchSha256"]}


def vendor_test_after(common, payload):
    if sha(target_path(common, payload["targetName"]).read_bytes().decode("utf-8")) != payload["proposedSha256"]:
        raise ValueError("vendored candidate differs from the prepared proposal")
    count = run_vendor_tests(common, 0, payload)
    if count != payload["baselineTests"]:
        raise ValueError("test inventory changed between baseline and candidate")
    return {"baselineExit": 1, "updatedExit": 0, "tests": count,
            "scope": "same behavioral tests failed before and passed after a supplied vendored-code patch; no registry or supply-chain attestation"}


def register(common):
    operations = {
        "pipeline-propose": pipeline_propose, "pipeline-validate": pipeline_validate,
        "dockerfile-propose": dockerfile_propose, "dockerfile-validate": dockerfile_validate,
        "kubernetes-propose": kubernetes_propose, "kubernetes-validate": kubernetes_validate,
        "terraform-analyze": terraform_analyze, "vendor-prepare": vendor_prepare,
        "vendor-test-before": vendor_test_before, "vendor-apply": vendor_apply, "vendor-test-after": vendor_test_after,
    }
    return {name: (lambda payload, operation=operation: operation(common, payload)) for name, operation in operations.items()}
