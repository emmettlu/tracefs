"""Check results logged by the SDK smoke tests against the protocol schema.
Usage: schema_check.py SCHEMA LOG, where LOG has one JSON [op, result] per line.
Covers the keywords schemars emits: $ref, anyOf/oneOf, type, const, enum,
properties/required, additionalProperties false and items."""

import json
import sys

schema = json.load(open(sys.argv[1]))
defs = schema["$defs"]
TYPES = {"object": dict, "array": list, "string": str, "boolean": bool, "null": type(None)}


def is_type(v, t):
    if t in ("integer", "number"):
        return not isinstance(v, bool) and isinstance(v, int if t == "integer" else (int, float))
    return isinstance(v, TYPES[t])


def check(v, s, at):
    if s is True:
        return
    if "$ref" in s:
        check(v, defs[s["$ref"].rsplit("/", 1)[1]], at)
    for key in ("anyOf", "oneOf"):
        if key in s:
            errors = []
            for sub in s[key]:
                try:
                    check(v, sub, at)
                    break
                except AssertionError as e:
                    errors.append(str(e))
            else:
                raise AssertionError(f"{at}: no {key} branch matches: {errors}")
    if "const" in s:
        assert v == s["const"], f"{at}: {v!r} != {s['const']!r}"
    if "enum" in s:
        assert v in s["enum"], f"{at}: {v!r} not in {s['enum']}"
    if "type" in s:
        ts = s["type"] if isinstance(s["type"], list) else [s["type"]]
        assert any(is_type(v, t) for t in ts), f"{at}: {v!r} is not {s['type']}"
    if isinstance(v, dict):
        for k in s.get("required", []):
            assert k in v, f"{at}: missing {k}"
        props = s.get("properties", {})
        for k, x in v.items():
            if k in props:
                check(x, props[k], f"{at}.{k}")
            else:
                assert s.get("additionalProperties") is not False, f"{at}: unexpected {k}"
    if isinstance(v, list) and "items" in s:
        for i, x in enumerate(v):
            check(x, s["items"], f"{at}[{i}]")


results = schema["properties"]["results"]["properties"]
ops = set()
for line in open(sys.argv[2]):
    op, result = json.loads(line)
    check(result, results[op], op)
    ops.add(op)
assert ops, "no results logged"
print(f"{len(ops)} ops match the schema: {' '.join(sorted(ops))}")
