#!/usr/bin/env python3
"""Compare `acfs diff` output with a ground truth computed from two fingerprints.

usage: cmpdiff.py FP_BEFORE FP_AFTER DIFF_JSON [WS]
Fingerprint lines are "<repr(path)> <mode> <uid:gid> <hash/target> <xattrs>".
"""
import ast, base64, json, sys

def load(p):
    out = {}
    for line in open(p, encoding="utf-8", errors="surrogateescape"):
        line = line.rstrip("\n")
        if not line:
            continue
        # repr() of a str path, then the rest
        end = line.index("' ") + 1 if line.startswith("'") else line.index('" ') + 1
        path = ast.literal_eval(line[:end])
        out[path.encode("utf-8", "surrogateescape")] = line[end + 1:]
    return out

def kind_of(a, b):
    ta, tb = a.split()[0], b.split()[0]
    return ta[:-4] != tb[:-4]

a, b = load(sys.argv[1]), load(sys.argv[2])
truth = {}
for p in a.keys() - b.keys():
    truth[p] = "delete"
for p in b.keys() - a.keys():
    truth[p] = "add"
for p in a.keys() & b.keys():
    if a[p] != b[p]:
        truth[p] = "modify"
items = json.load(open(sys.argv[3]))
if len(sys.argv) > 4:
    items = [i for i in items if i["ws"] == sys.argv[4]]
got = {}
renames = 0
for it in items:
    for c in it["changes"]:
        p = c["path"]
        p = p.encode("utf-8", "surrogateescape") if isinstance(p, str) else base64.b64decode(p["base64"])
        if p == b"":
            continue
        k = c["kind"]
        if k == "rename":
            renames += 1
            k = "add"
        got[p] = k
missing = {p: k for p, k in truth.items() if got.get(p) != k}
extra = {p: k for p, k in got.items() if p not in truth}
print("truth=%d got=%d renames=%d missing=%d extra=%d" % (len(truth), len(got), renames, len(missing), len(extra)))
for p, k in list(missing.items())[:8]:
    print("  missing/wrong", k, p, "got", got.get(p))
for p, k in list(extra.items())[:8]:
    print("  extra", k, p)
sys.exit(1 if missing or extra else 0)
