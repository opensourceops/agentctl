"""Strict YAML 1.2 for trusted, editable workflow source."""
from pathlib import Path
import json
from ruamel.yaml import YAML


def load(path):
    parser = YAML(typ="safe", pure=True)
    parser.allow_duplicate_keys = False
    value = parser.load(Path(path).read_text(encoding="utf-8"))
    json.dumps(value, allow_nan=False)
    return value


def dump(path, value):
    writer = YAML()
    writer.default_flow_style = False
    writer.width = 120
    writer.indent(mapping=2, sequence=4, offset=2)
    with Path(path).open("w", encoding="utf-8", newline="\n") as output:
        writer.dump(value, output)
