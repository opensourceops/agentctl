#!/usr/bin/env python3
"""Forward CLI arguments through the acceptance container's existing mounts.

The JSON configuration contains an engine and mount arguments, never credentials.
Metadata and replay commands run without network or provider environment access.
This adapter lets live_command.py enforce the same shared reservation as local CLI.
"""
import json
import os
import sys

from live_command import DISPATCH, parsed_arguments


def command(arguments, configuration):
    positional, flags = parsed_arguments(arguments)
    dispatch = bool(positional and positional[0] in DISPATCH
                    and "--check" not in flags and "--plan" not in flags)
    result = [configuration["program"], *configuration["args"]]
    result.extend(["--env", "OPENAI_API_KEY"] if dispatch else ["--network", "none"])
    return [*result, "agentctl-acceptance:local", *arguments]


if __name__ == "__main__":
    invocation = command(sys.argv[1:], json.loads(os.environ["AGENTCTL_LIVE_CONTAINER_COMMAND"]))
    os.execvp(invocation[0], invocation)
