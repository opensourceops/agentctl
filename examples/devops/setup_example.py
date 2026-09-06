#!/usr/bin/env python3
"""Configure a copied example for this interpreter; prepare explicitly labeled fixtures only."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys

from yaml_io import load, dump


def configure_service_environment(workflow, *, platform=None, environment=None):
    """Declare the Windows socket startup prerequisite only at reviewed probes."""
    if (sys.platform if platform is None else platform) != "win32":
        return []
    spec = workflow["spec"]
    probes = [action for action in spec.get("actions", {}).values()
              if action.get("kind") == "extension.process"
              and action.get("args") == ["helper.py", "service-probe"]]
    if not probes:
        return []
    environment = os.environ if environment is None else environment
    if not environment.get("SYSTEMROOT", "").strip():
        raise ValueError("Windows Python socket probes require SYSTEMROOT at setup and execution; its value is never captured")
    reference = {"env": "SYSTEMROOT"}
    for action in probes:
        for name, value in action.get("env", {}).items():
            if name.upper() == "SYSTEMROOT" and (name != "SYSTEMROOT" or value != reference):
                raise ValueError("Conflicting explicit SYSTEMROOT probe setting; review the authored environment reference")
    policy = spec.setdefault("policy", {})
    if "environmentAllowlist" in policy and "SYSTEMROOT" not in policy["environmentAllowlist"]:
        raise ValueError("Explicit environmentAllowlist excludes SYSTEMROOT; review the Windows probe prerequisite before granting it")
    # Existing explicit grants, references and policy rules remain intact.
    policy.setdefault("environmentAllowlist", ["SYSTEMROOT"])
    for action in probes:
        action.setdefault("env", {}).setdefault("SYSTEMROOT", reference.copy())
    return ["SYSTEMROOT"]


def prepare(root):
    root = Path(root).absolute()
    metadata = json.loads((root / "example.json").read_text(encoding="utf-8"))
    interpreter = Path(sys.executable).absolute()  # Preserve the venv path, not its symlink target.
    tools = {}
    for name in metadata["dependencies"]:
        if name in {"git", "actionlint"}:
            executable = shutil.which(name)
            if executable is None:
                raise ValueError(f"Install the declared {name} prerequisite before setup")
            executable = Path(executable).resolve()
            tools[name] = {"executable": str(executable), "sha256": hashlib.sha256(executable.read_bytes()).hexdigest()}
    (root / "fixture-tools.json").write_text(json.dumps(tools, indent=2) + "\n", encoding="utf-8")

    def configure(value):
        if isinstance(value, dict):
            if value.get("command") in {"python", "python3"}:
                value["command"] = str(interpreter)
            if "processAllowlist" in value:
                value["processAllowlist"] = [interpreter.name if name in {"python", "python3"} else name for name in value["processAllowlist"]]
            for child in value.values():
                configure(child)
        elif isinstance(value, list):
            for child in value:
                configure(child)

    workflows = []
    environment_requirements = {}
    for source in sorted(root.glob("*.yaml")):
        if source.name.startswith("local."):
            continue
        value = load(source)
        if not isinstance(value, dict) or value.get("kind") != "Workflow":
            continue
        configure(value)
        requirements = configure_service_environment(value)
        destination = root / ("local." + source.name)
        dump(destination, value, "Configured from " + source.name + "; review before check, plan and run.")
        workflows.append(destination.name)
        if requirements:
            environment_requirements[destination.name] = requirements
    fixture_setup = None
    if metadata["exampleId"] == "09":
        spec = importlib.util.spec_from_file_location("cookbook_helper", root / "helper.py")
        helper = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = helper
        spec.loader.exec_module(helper)
        helper.ROOT = root.resolve()
        fixture_setup = helper.prepare_fixture_history({"historyPath": "fixtures/history.json", "repositoryPath": "fixtures/repository"})
    if metadata["exampleId"] in {"14", "17"}:
        service = root / "artifacts/service.json"
        if not service.exists():
            service.parent.mkdir(exist_ok=True)
            shutil.copy2(root / "fixtures/service.json", service)
    result = {"exampleId": metadata["exampleId"], "interpreter": str(interpreter), "workflows": workflows,
              "environmentRequirements": environment_requirements,
              "fixtureSetup": fixture_setup, "authority": "The command selects an absolute interpreter; agentctl processAllowlist authorizes its basename. The trusted Python helper can access host resources; this is not an OS sandbox."}
    (root / "setup-report.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    print(json.dumps(prepare(Path(__file__).resolve().parent), indent=2))
