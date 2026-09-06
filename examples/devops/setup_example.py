#!/usr/bin/env python3
"""Configure a copied example for this interpreter; prepare explicitly labeled fixtures only."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import sys

from yaml_io import load, dump


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
    for source in sorted(root.glob("*.yaml")):
        if source.name.startswith("local."):
            continue
        value = load(source)
        if not isinstance(value, dict) or value.get("kind") != "Workflow":
            continue
        configure(value)
        destination = root / ("local." + source.name)
        dump(destination, value, "Configured from " + source.name + "; review before check, plan and run.")
        workflows.append(destination.name)
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
              "fixtureSetup": fixture_setup, "authority": "The command selects an absolute interpreter; agentctl processAllowlist authorizes its basename. The trusted Python helper can access host resources; this is not an OS sandbox."}
    (root / "setup-report.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    print(json.dumps(prepare(Path(__file__).resolve().parent), indent=2))
