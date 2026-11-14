"""Drive the Python SDK through a workspace cycle, logging [op, result] lines.
Usage: sdk_smoke.py LOG FILE. Env: ACFS_SOCKET, W (workspace "w" path)."""

import base64
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk/python/src"))
from acfs_sdk import AcfsError, Client  # noqa: E402

c = Client()
log = open(sys.argv[1], "w")
name = sys.argv[2]


def call(op, args=None):
    r = c.call(op, args)
    log.write(json.dumps([op, r]) + "\n")
    return r


assert call("ping")["protocol"] == 2
call("ws.list")
s = call("session.create", {"ws": ["w"]})["id"]
content = base64.b64encode(b"sdk\n").decode()
cs = call("fs.edit", {"ws": "w", "ops": [{"op": "write", "path": name, "content": content}], "author": "sdk", "message": f"sdk {name}"})["changeset"]
changes = [ch for d in call("diff", {"a": cs}) for ch in d["changes"]]
assert {"path": name, "kind": "add"} in changes, changes
assert call("fs.read", {"ws": "w", "path": name})["content"] == content
call("changeset", {"session": s, "author": "sdk", "message": "nothing"})
call("log", {"session": s})
call("proc.list")
for op, args, code in [("proc.show", {"proc_id": "nope"}, "not_found"), ("proc.run", {"cmd": ["true"], "cwd": "/"}, "invalid")]:
    try:
        c.call(op, args)
        raise SystemExit(f"{op}: expected an error")
    except AcfsError as e:
        assert e.code == code, (op, e.code)
assert call("session.merge", {"session": s})["state"] == "merged"
call("session.list")
print("ok")
