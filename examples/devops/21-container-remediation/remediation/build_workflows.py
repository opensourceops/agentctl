#!/usr/bin/env python3
"""Maintainer generation of readable authored YAML; runtime reads those YAML files."""
from pathlib import Path
from ruamel.yaml.scalarstring import LiteralScalarString
import schemas
from adapter import expected_files
from preflight import expected_echo_input
from yaml_io import dump

ROOT = Path(__file__).resolve().parent.parent


def action(name):
    return {"kind": "extension.process", "command": "/usr/bin/python3", "args": ["source/remediation/adapter.py", name],
            "idempotency": "idempotent", "protocolVersion": "agentctl.dev/process-extension/v1", "inputSchema": schemas.INPUTS[name],
            "outputSchema": schemas.OUTPUTS[name], "capabilities": ["remediation.validate"], "timeoutSeconds": 30,
            "stdoutLimitBytes": 262144, "stderrLimitBytes": 8192, "combinedOutputLimitBytes": 270336}


def write_tool(filename, content):
    decoded = content.decode('utf-8')
    # Escape only ECMA-262 syntax characters; Python's re.escape also escapes
    # spaces and hyphens, which are invalid identity escapes in Unicode mode.
    literal = ''.join('\\' + char if char in r'\^$.*+?()[]{}|' else
                      {'\n': r'\n', '\r': r'\r', '\t': r'\t'}.get(char, char)
                      for char in decoded)
    # Pinned jsonschema 0.37 / fancy-regex 0.16 maps `$` to absolute EndText
    # without multiline flags. Real CLI rejection fixtures prove this boundary;
    # Python search / ECMA soft-end semantics are not the runtime contract.
    # The provider supports basic anchors but rejects lookaround assertions.
    pattern = LiteralScalarString('^' + literal + '$')
    return {"kind": "builtin.workspace.write", "description": "Write only the reviewed dependency bytes to the bounded staging file",
            "inputSchema": schemas.obj({"path": {"type": "string", "enum": ["patch/" + filename]},
                                        "content": {"type": "string", "pattern": pattern,
                                                    "minLength": len(decoded), "maxLength": len(decoded)}}),
            "outputSchema": {"type": "object"}, "capability": "filesystem.write", "effectClass": "workspace_mutate", "risk": "medium",
            "idempotency": "idempotent", "retrySafe": True, "timeoutSeconds": 5, "approval": "policy"}


def build():
    workflow = {"apiVersion": "agentctl.dev/v1", "kind": "Workflow", "metadata": {"name": "container-dependency-remediation",
                "description": "Two bounded model roles stage a dependency fix for trusted CI rebuild and rescan."}, "spec": {
        "policy": {"workspaceRoot": ".", "writableRoots": ["patch", "state"], "processAllowlist": ["python3"], "networkAllowlist": ["api.openai.com"], "approval": "never"},
        "providers": {"openai": {"kind": "openai", "credential": {"env": "OPENAI_API_KEY"}}},
        "runtime": {"maxConcurrency": 1, "budgets": {"maxProviderRequests": 4, "maxTurns": 4, "maxToolCalls": 2, "maxTotalTokens": 20000,
                     "maxWallTimeSeconds": 600, "maxCostMicrousd": 1000000, "maxProcessOutputBytes": 1048576, "maxArtifactBytes": 1048576},
                    "pricing": {"version": "openai-public-2026-09-08-estimate", "models": {"openai/gpt-6-astra": {
                        "inputMicrousdPerMillionTokens": 10000000, "outputMicrousdPerMillionTokens": 50000000,
                        "cacheReadMicrousdPerMillionTokens": 1000000, "cacheWriteMicrousdPerMillionTokens": 12500000}}}},
        "actions": {name: action(name) for name in ["capture", "validate-plan", "validate-patch"]},
        "tools": {name: write_tool(filename, content) for name, (filename, content) in zip(["write_manifest", "write_lock"], expected_files("2.7.0").items())},
        "agents": {}, "tasks": [
            {"id": "capture", "uses": "action:capture", "with": {}},
            {"id": "analyze", "uses": "agent:analyzer", "needs": ["capture"], "with": {"prompt": "${{ tasks.capture.output }}"}},
            {"id": "validate-plan", "uses": "action:validate-plan", "needs": ["capture", "analyze"],
             "with": {"context": "${{ tasks.capture.output }}", "plan": "${{ tasks.analyze.output }}"}},
            {"id": "implement", "uses": "agent:implementer", "needs": ["validate-plan"], "with": {"prompt": "${{ tasks.validate-plan.output }}"}},
            {"id": "validate-patch", "uses": "action:validate-patch", "needs": ["capture", "validate-plan", "implement"],
             "with": {"context": "${{ tasks.capture.output }}", "plan": "${{ tasks.validate-plan.output }}", "implementation": "${{ tasks.implement.output }}"}}],
        "outputs": {"patch": "${{ tasks.validate-patch.output }}"}}}
    for name, instruction, turns, tools, output, output_tokens in [("analyzer", "analyze", 1, [], schemas.PLAN, 2048),
                                                                  ("implementer", "implement", 3, ["write_manifest", "write_lock"], schemas.IMPLEMENTATION, 4096)]:
        workflow["spec"]["agents"][name] = {"provider": "openai", "model": "gpt-6-astra", "reasoning": {"effort": "high"},
            "instructionsFile": "instructions/" + instruction + ".md", "tools": tools, "maxTurns": turns, "maxToolCalls": len(tools),
            "maxOutputTokens": output_tokens, "timeoutSeconds": 180, "structuredOutput": output, "providerOptions": {"store": False}}
    dump(ROOT / "agentctl/remediate.yaml", workflow)
    eligibility = {"apiVersion": "agentctl.dev/v1", "kind": "Workflow", "metadata": {"name": "container-remediation-publication-eligibility",
                   "description": "Validate exact trusted build/test/rescan evidence without a provider or GitHub credential."}, "spec": {
        "policy": {"workspaceRoot": ".", "writableRoots": ["state"], "processAllowlist": ["python3"], "approval": "never"},
        "runtime": {"budgets": {"maxWallTimeSeconds": 60, "maxProcessOutputBytes": 1048576}},
        "actions": {"eligibility": action("eligibility")}, "tasks": [{"id": "eligibility", "uses": "action:eligibility", "with": {}}],
        "outputs": {"publication": "${{ tasks.eligibility.output }}"}}}
    dump(ROOT / "agentctl/eligibility.yaml", eligibility)
    probe_schema = schemas.obj({"echo": {"type": "string", "enum": ["ok"]}})
    echo_schema = schemas.obj({name: workflow['spec']['tools'][tool]['inputSchema']
                              for name, tool in [('manifest', 'write_manifest'), ('lock', 'write_lock')]})
    preflight = {"apiVersion": "agentctl.dev/v1", "kind": "Workflow", "metadata": {
        "name": "responses-tool-preflight", "description": "Echo both exact multiline tool inputs before the full live journey."}, "spec": {
        "policy": {"workspaceRoot": ".", "writableRoots": ["state"], "networkAllowlist": ["api.openai.com"], "approval": "never"},
        "providers": {"openai": {"kind": "openai", "credential": {"env": "OPENAI_API_KEY"}}},
        "runtime": {"maxConcurrency": 1, "budgets": {"maxProviderRequests": 3, "maxTurns": 3, "maxToolCalls": 1,
                    "maxTotalTokens": 16000, "maxWallTimeSeconds": 90, "maxCostMicrousd": 600000},
                    "pricing": workflow["spec"]["runtime"]["pricing"]},
        "tools": {"echo": {"kind": "builtin.echo", "description": "Echo the exact reviewed manifest and lock inputs without writing files", "inputSchema": echo_schema, "outputSchema": echo_schema,
                  "capability": "internal", "effectClass": "pure", "risk": "low", "idempotency": "pure", "retrySafe": True,
                  "timeoutSeconds": 5, "approval": "policy"}},
        "agents": {"probe": {"provider": "openai", "model": "gpt-6-astra", "reasoning": {"effort": "high"},
                   "instructionsFile": "instructions/preflight.md", "tools": ["echo"], "maxTurns": 3, "maxToolCalls": 1,
                   "maxOutputTokens": 4096, "timeoutSeconds": 90, "structuredOutput": probe_schema, "providerOptions": {"store": False}}},
        "actions": {"verify": {"kind": "builtin.assert"}},
        "tasks": [{"id": "probe", "uses": "agent:probe", "with": {"prompt": expected_echo_input()}},
                  {"id": "verify", "uses": "action:verify", "needs": ["probe"],
                   "with": {"that": '${{ tasks.probe.output.echo == "ok" }}', "message": "Preflight structured echo differs"}}],
        "outputs": {"preflight": "${{ tasks.probe.output }}"}}}
    dump(ROOT / "agentctl/preflight.yaml", preflight)



if __name__ == "__main__":
    build()
