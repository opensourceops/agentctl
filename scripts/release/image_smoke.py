#!/usr/bin/env python3
"""Execute the maintained image guide natively, then replay offline without input files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess

from artifacts import SHA
from bundle import write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", default="docker")
    parser.add_argument("--image", required=True)
    parser.add_argument("--variant", choices=["minimal", "tooling"], required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if os.getuid() == 0 or not SHA.fullmatch(args.source):
        raise ValueError("use a non-root native host and an exact reviewed source SHA")
    args.work.mkdir(parents=True, exist_ok=False)
    for name in ["config", "state", "artifacts"]:
        (args.work / name).mkdir(mode=0o700)
    guide = Path("docs/CONTAINER.md").read_text()
    blocks = re.findall(r"```yaml\n(.*?)\n```", guide, re.S)
    (args.work / "config/workflow.yaml").write_text(blocks[0] + "\n")
    (args.work / "config/defaults.yaml").write_text(blocks[1] + "\n")
    info = json.loads(subprocess.check_output([args.engine, "image", "inspect", args.image]))[0]
    architecture = {"aarch64": "arm64", "arm64": "arm64", "x86_64": "amd64"}.get(platform.machine())
    if platform.system() != "Linux" or info["Os"] != "linux" or info["Architecture"] != architecture:
        raise ValueError("release evidence requires native Linux execution matching the image architecture")
    config = info["Config"]
    if config["User"] not in {"65532:65532", "nonroot:nonroot"} or config["Entrypoint"] != ["/usr/local/bin/agentctl"]:
        raise ValueError("non-root image entrypoint contract changed")
    if config["Labels"]["org.opencontainers.image.revision"] != args.source or config["Labels"]["org.opencontainers.image.version"] != args.version:
        raise ValueError("image metadata does not match reviewed source")
    base = [args.engine, "run", "--rm", "--read-only", "--network", "none", "--user", f"{os.getuid()}:{os.getgid()}",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--tmpfs", "/tmp:rw,noexec,nosuid,size=16m"]
    for name, destination, readonly in [("config", "/workspace/config", True), ("state", "/state", False), ("artifacts", "/artifacts", False)]:
        base += ["--mount", f"type=bind,src={(args.work / name).resolve()},dst={destination}" + (",readonly" if readonly else "")]
    def cli(name, arguments):
        completed = subprocess.run(base + [args.image] + arguments + ["--output", "json", "--color", "never"], capture_output=True, text=True, timeout=180)
        (args.work / (name + ".json")).write_text(completed.stdout)
        (args.work / (name + ".stderr")).write_text(completed.stderr)
        completed.check_returncode()
        envelope = json.loads(completed.stdout)
        if envelope.get("apiVersion") != "agentctl.dev/cli/v1" or envelope.get("ok") is not True:
            raise ValueError("invalid stable CLI envelope")
        return envelope["data"]
    if cli("version", ["version"])["version"] != args.version:
        raise ValueError("executable version and source disagree")
    for command in ["check", "plan"]:
        cli(command, [command, "/workspace/config/workflow.yaml", "--workspace", "/workspace"])
    run = cli("run", ["run", "/workspace/config/workflow.yaml", "--workspace", "/workspace", "--db", "/state/run.db"])
    if run["state"] != "succeeded" or run["output"]["greeting"] != "hello, world":
        raise ValueError("bounded image workflow failed its semantic assertion")
    cli("inspect", ["inspect", run["runId"], "--db", "/state/run.db"])
    artifact = args.work / "artifacts/greeting.txt"
    before = (hashlib.sha256(artifact.read_bytes()).hexdigest(), artifact.stat().st_mtime_ns)
    (args.work / "config/defaults.yaml").unlink()
    replay = cli("replay", ["replay", run["runId"], "--db", "/state/run.db"])
    inspection = cli("replay-inspect", ["inspect", replay["runId"], "--db", "/state/run.db"])
    if replay["state"] != "succeeded" or replay["output"] != run["output"] or before != (hashlib.sha256(artifact.read_bytes()).hexdigest(), artifact.stat().st_mtime_ns):
        raise ValueError("replay changed recorded output or rewrote the artifact")
    if any(inspection[field] for field in ["effects", "providerSessions", "toolCalls"]):
        raise ValueError("replay created fresh effects or provider/tool requests")
    if args.variant == "tooling":
        script = 'test "$(id -u)" -ne 0 && test ! -w / && test ! -w /workspace/config && test -w /state && test ! -e /workspace/state && command -v git && /usr/bin/python3 -c \'import packaging, ruamel.yaml\' && /usr/bin/python3 /opt/agentctl/release-budget/scripts/release_live_budget.py --help'
        subprocess.run(base + ["--entrypoint", "/bin/sh", args.image, "-c", script], check=True, capture_output=True, timeout=30)
    write_json(args.output, {"passed": True, "sourceSha": args.source, "version": args.version, "variant": args.variant,
                            "imageId": info["Id"], "platform": "linux/" + architecture, "runId": run["runId"],
                            "replayRunId": replay["runId"], "artifactSha256": before[0], "freshReplayEffects": 0,
                            "guideSha256": hashlib.sha256(guide.encode()).hexdigest()})


if __name__ == "__main__":
    main()
