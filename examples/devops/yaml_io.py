"""Strict YAML 1.2 I/O for authored examples, separate from JSON evidence files."""
from io import StringIO
import json
from pathlib import Path

try:
    from ruamel.yaml import YAML
    from ruamel.yaml.scalarstring import LiteralScalarString, SingleQuotedScalarString
except ImportError as error:
    raise ImportError("Install the pinned example dependencies: python -m pip install -r examples/devops/requirements.txt") from error


def loads(text):
    parser = YAML(typ="safe", pure=True)
    parser.allow_duplicate_keys = False
    value = parser.load(text)
    # Workflow data uses JSON value types. Reject YAML-only timestamps, sets,
    # non-string mapping keys and non-finite values before downstream consumers.
    def validate(item):
        if isinstance(item, dict):
            if any(not isinstance(key, str) for key in item):
                raise ValueError("YAML mapping keys must be strings")
            for child in item.values():
                validate(child)
        elif isinstance(item, list):
            for child in item:
                validate(child)
    validate(value)
    json.dumps(value, allow_nan=False)
    return value


def load(path):
    return loads(Path(path).read_text(encoding="utf-8"))


def dumps(value):
    def styled(item):
        if isinstance(item, dict):
            return {key: styled(child) for key, child in item.items()}
        if isinstance(item, list):
            return [styled(child) for child in item]
        if isinstance(item, str):
            if "\n" in item:
                return LiteralScalarString(item)
            if "${{" in item:
                return SingleQuotedScalarString(item)
        return item
    writer = YAML()
    writer.default_flow_style = False
    writer.allow_unicode = True
    writer.width = 100
    writer.indent(mapping=2, sequence=4, offset=2)
    writer.representer.ignore_aliases = lambda data: True
    stream = StringIO()
    writer.dump(styled(value), stream)
    return stream.getvalue()


def dump(path, value, comment=None):
    prefix = "# " + comment + "\n" if comment else ""
    Path(path).write_bytes((prefix + dumps(value)).encode("utf-8"))
