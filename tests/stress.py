#!/usr/bin/env python3
"""Deterministic random file operations, like fsstress, for acfs consistency checks.

    stress.py ROOT SEED N [--protect LIST] [--map OUT]

Runs N operations below ROOT chosen by SEED. Every choice depends only on SEED and the
sorted state of the tree, so the same run on two equal trees leaves equal trees (one
native, one through acfs). Operations: create, append, overwrite in place, truncate,
save as a new file renamed over, mkdir, rename files and directories (also across
directories and over existing files), delete files and trees, chmod, symlinks, user
xattrs, fifos.

--protect names files (relative to ROOT, comma separated) whose content and metadata
are left alone: they are only renamed, or moved with their directory. --map writes
where each of them ended up, as JSON, found by inode.
"""

import json
import os
import random
import stat
import sys

MAX_DEPTH = 5


def walk(root):
    files, dirs = [], []
    for d, ds, fs in os.walk(root):
        ds.sort()
        for n in sorted(ds):
            p = os.path.join(d, n)
            if os.path.islink(p):
                files.append(p)
            else:
                dirs.append(p)
        for n in sorted(fs):
            files.append(os.path.join(d, n))
    return sorted(files), sorted(dirs)


def protected_by(path, prot_inos):
    try:
        return os.lstat(path).st_ino in prot_inos
    except OSError:
        return False


def contains_protected(path, prot_inos):
    if protected_by(path, prot_inos):
        return True
    if os.path.isdir(path) and not os.path.islink(path):
        for d, ds, fs in os.walk(path):
            for n in ds + fs:
                if protected_by(os.path.join(d, n), prot_inos):
                    return True
    return False


def main():
    args = sys.argv[1:]
    protect, out_map = [], None
    if "--protect" in args:
        i = args.index("--protect")
        protect = [p for p in args[i + 1].split(",") if p]
        del args[i : i + 2]
    if "--map" in args:
        i = args.index("--map")
        out_map = args[i + 1]
        del args[i : i + 2]
    root, seed, n = args[0], int(args[1]), int(args[2])
    rng = random.Random(seed)

    # Shallow trees keep overlay redirects short: past `redirect_max` a directory rename
    # fails with EXDEV through the overlay but not natively.
    def depth(p):
        return 0 if p == root else os.path.relpath(p, root).count("/") + 1

    def subdepth(p):
        return 1 + max((depth(os.path.join(d, x)) - depth(p) for d, ds, _ in os.walk(p) for x in ds), default=0)

    prot_inos = {os.lstat(os.path.join(root, p)).st_ino: p for p in protect}
    name_counter = [0]

    def new_name(prefix):
        name_counter[0] += 1
        return "%s%d_%d" % (prefix, seed, name_counter[0])

    def data():
        k = rng.choice([0, 1, 10, 100, 4096, 20000, 65536])
        if rng.random() < 0.7:
            return "".join(rng.choice("abcdefgh\n") for _ in range(k)).encode()
        return bytes(rng.getrandbits(8) for _ in range(k))

    for _ in range(n):
        files, dirs = walk(root)
        # Protected files are only moved.
        regular = [f for f in files if os.path.isfile(f) and not os.path.islink(f) and not protected_by(f, prot_inos)]
        alldirs = [root] + dirs
        op = rng.choice(
            ["create", "create", "append", "overwrite", "truncate", "save", "mkdir", "rename", "rename", "rendir", "delete", "rmtree",
             "chmod", "symlink", "retarget", "xattr", "fifo", "over"]
        )
        try:
            if op == "create" or (op in ("append", "overwrite", "truncate", "save", "chmod", "xattr") and not regular):
                with open(os.path.join(rng.choice(alldirs), new_name("f")), "wb") as f:
                    f.write(data())
            elif op == "append":
                with open(rng.choice(regular), "ab") as f:
                    f.write(data())
            elif op == "overwrite":
                p = rng.choice(regular)
                with open(p, "r+b") as f:
                    f.seek(rng.randrange(0, max(1, os.path.getsize(p))))
                    f.write(data()[:4096])
            elif op == "truncate":
                p = rng.choice(regular)
                os.truncate(p, rng.randrange(0, os.path.getsize(p) + 1))
            elif op == "save":
                p = rng.choice(regular)
                tmp = p + ".save"
                with open(tmp, "wb") as f:
                    f.write(open(p, "rb").read() + data()[:100])
                os.rename(tmp, p)
            elif op == "mkdir":
                parent = rng.choice(alldirs)
                if depth(parent) >= MAX_DEPTH:
                    continue
                os.mkdir(os.path.join(parent, new_name("d")))
            elif op == "rename" and files:
                src = rng.choice(files)
                dst = os.path.join(rng.choice(alldirs), rng.choice([os.path.basename(src) + "r", new_name("r")]))
                if not os.path.lexists(dst):
                    os.rename(src, dst)
            elif op == "rendir" and dirs:
                src = rng.choice(dirs)
                parent = rng.choice(alldirs)
                if parent == src or parent.startswith(src + "/") or depth(parent) + subdepth(src) >= MAX_DEPTH:
                    continue
                dst = os.path.join(parent, new_name("m"))
                os.rename(src, dst)
            elif op == "over" and len(regular) > 1:
                a, b = rng.sample(regular, 2)
                os.rename(a, b)
            elif op == "delete" and files:
                p = rng.choice(files)
                if protected_by(p, prot_inos):
                    continue
                os.unlink(p)
            elif op == "rmtree" and dirs:
                p = rng.choice(dirs)
                if contains_protected(p, prot_inos) or rng.random() < 0.5:
                    continue
                for d, ds, fs in os.walk(p, topdown=False):
                    for x in fs:
                        os.unlink(os.path.join(d, x))
                    for x in ds:
                        q = os.path.join(d, x)
                        os.unlink(q) if os.path.islink(q) else os.rmdir(q)
                os.rmdir(p)
            elif op == "chmod":
                os.chmod(rng.choice(regular), rng.choice([0o600, 0o640, 0o644, 0o755, 0o4755]))
            elif op == "symlink":
                os.symlink(rng.choice(["x", "../y", "/etc/hostname", "a/b/c"]), os.path.join(rng.choice(alldirs), new_name("l")))
            elif op == "retarget":
                links = [f for f in files if os.path.islink(f)]
                if links:
                    p = rng.choice(links)
                    os.unlink(p)
                    os.symlink(rng.choice(["z", "../w", "q/r"]), p)
            elif op == "xattr":
                os.setxattr(rng.choice(regular), "user." + rng.choice(["a", "b"]), data()[:64] or b"v", follow_symlinks=False)
            elif op == "fifo":
                os.mkfifo(os.path.join(rng.choice(alldirs), new_name("p")))
        except OSError as e:
            # The same error on both trees; keep going.
            print("op %s: %s" % (op, e), file=sys.stderr)

    if out_map:
        where = {}
        for d, ds, fs in os.walk(root):
            for n in ds + fs:
                p = os.path.join(d, n)
                st = os.lstat(p)
                if st.st_ino in prot_inos and stat.S_ISREG(st.st_mode):
                    where[prot_inos[st.st_ino]] = os.path.relpath(p, root)
        with open(out_map, "w") as f:
            json.dump(where, f, sort_keys=True)


if __name__ == "__main__":
    main()
