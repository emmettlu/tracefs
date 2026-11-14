import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { Store } from "../index.js";
import { WIN, canSymlink, fresh, read, tmp, write } from "./helpers.js";

function project(t) {
  const dir = tmp("acfs-lite-src-");
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  write(dir, "a.txt", "v0\n");
  write(dir, "src/lib.js", "export const x = 1;\n");
  return dir;
}

test("register copies the project in and records a baseline", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  assert.equal(read(main, "a.txt"), "v0\n");
  assert.equal(read(main, "src/lib.js"), "export const x = 1;\n");
  const log = s.log({ ws: "app" });
  assert.equal(log.length, 1);
  assert.equal(log[0].message, "register");
  assert.throws(() => s.register("app"), { code: "exists" });
  assert.throws(() => s.register("../evil"), { code: "invalid" });
});

test("checkpoint records nothing when the tree did not change", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  assert.equal(s.checkpoint("app", "main", { message: "noop" }), null);
  write(main, "a.txt", "v1\n");
  const id = s.checkpoint("app", "main", { message: "edit", toolCall: "call-1", author: "pi" });
  assert.ok(id);
  const c = s.log({ ws: "app", n: 1 })[0];
  assert.deepEqual([c.id, c.author, c.message, c.tool_call], [id, "pi", "edit", "call-1"]);
  assert.equal(s.checkpoint("app", "main"), null);
});

test("same-size edits within one millisecond are still seen", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  for (let i = 0; i < 20; i++) {
    write(main, "f", `v${i % 10}`);
    const id = s.checkpoint("app", "main", { message: String(i) });
    if (i > 0) assert.ok(id, `edit ${i} was missed`);
  }
});

test("an edit is seen when the file clock lags the wall clock", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  const f = path.join(main, "f");
  fs.writeFileSync(f, "v0");
  const t0 = new Date(Date.now() - 500);
  fs.utimesSync(f, t0, t0);
  const a = s.checkpoint("app", "main");
  fs.writeFileSync(f, "v1");
  fs.utimesSync(f, t0, t0);
  const b = s.checkpoint("app", "main");
  assert.ok(b, "the edit was missed");
  assert.deepEqual(s.diff(a, b).changes, [{ kind: "modify", path: "f" }]);
});

test("diff lists adds, deletes and modifications", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const a = s.head("app", "main");
  write(main, "a.txt", "v1\n");
  write(main, "new/b.txt", "b\n");
  fs.rmSync(path.join(main, "src"), { recursive: true });
  const b = s.checkpoint("app", "main");
  const want = [
    { kind: "modify", path: "a.txt" },
    { kind: "add", path: "new" },
    { kind: "add", path: "new/b.txt" },
    { kind: "delete", path: "src" },
    { kind: "delete", path: "src/lib.js" },
  ];
  assert.deepEqual(s.diff(a, b).changes, want);
  assert.deepEqual(s.diff(b).changes, []);
  write(main, "a.txt", "v2\n");
  assert.deepEqual(s.diff(b).changes, [{ kind: "modify", path: "a.txt" }]);
});

test("exec bit changes are changes", { skip: WIN && "no exec bit on Windows" }, (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const a = s.head("app", "main");
  fs.chmodSync(path.join(main, "a.txt"), 0o755);
  const b = s.checkpoint("app", "main");
  assert.deepEqual(s.diff(a, b).changes, [{ kind: "modify", path: "a.txt" }]);
  s.revert({ changeset: b });
  assert.equal(fs.statSync(path.join(main, "a.txt")).mode & 0o111, 0);
});

test("symlinks are kept as links", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  if (!canSymlink(main)) return t.skip("no symlink permission");
  fs.symlinkSync("a.txt", path.join(main, "link"));
  const id = s.checkpoint("app", "main");
  assert.equal(fs.readlinkSync(path.join(s.snapPath(id), "link")), "a.txt");
  fs.unlinkSync(path.join(main, "link"));
  fs.symlinkSync("src", path.join(main, "link"));
  const id2 = s.checkpoint("app", "main");
  assert.deepEqual(s.diff(id, id2).changes, [{ kind: "modify", path: "link" }]);
  s.reset(id);
  assert.equal(fs.readlinkSync(path.join(main, "link")), "a.txt");
});

test("binary files compare by content", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  const blob = Buffer.alloc(3 << 20, 7);
  fs.writeFileSync(path.join(main, "blob.bin"), blob);
  const a = s.checkpoint("app", "main");
  blob[blob.length - 1] = 8;
  fs.writeFileSync(path.join(main, "blob.bin"), blob);
  const b = s.checkpoint("app", "main");
  assert.ok(b);
  assert.deepEqual(s.diff(a, b).changes, [{ kind: "modify", path: "blob.bin" }]);
});

test("undo reverts one changeset on top of later work", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  write(main, "a.txt", "v1\n");
  const c1 = s.checkpoint("app", "main");
  write(main, "b.txt", "b\n");
  s.checkpoint("app", "main");
  const r = s.revert({ changeset: c1 });
  assert.equal(r.conflict, false);
  assert.ok(r.changeset);
  assert.equal(read(main, "a.txt"), "v0\n");
  assert.equal(read(main, "b.txt"), "b\n");
});

test("undo conflicts when the path changed since, and changes nothing", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  write(main, "a.txt", "v1\n");
  const c1 = s.checkpoint("app", "main");
  write(main, "a.txt", "v2\n");
  write(main, "z.txt", "z\n");
  const r = s.revert({ changeset: c1 });
  assert.equal(r.conflict, true);
  assert.deepEqual(r.conflicts, [{ path: "a.txt", reason: "changed since" }]);
  assert.equal(read(main, "a.txt"), "v2\n");
});

test("undoing an added directory refuses to drop files added in it later", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  write(main, "d/x", "x");
  const c1 = s.checkpoint("app", "main");
  write(main, "d/y", "y");
  const r = s.revert({ changeset: c1 });
  assert.equal(r.conflict, true);
  assert.deepEqual(r.conflicts, [{ path: "d/y", reason: "added since" }]);
  assert.equal(read(main, "d/y"), "y");
});

test("revert of a range and reset", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const c0 = s.head("app", "main");
  write(main, "a.txt", "v1\n");
  s.checkpoint("app", "main");
  write(main, "b.txt", "b\n");
  const c2 = s.checkpoint("app", "main");
  assert.equal(s.revert({ from: c0, to: c2 }).conflict, false);
  assert.equal(read(main, "a.txt"), "v0\n");
  assert.equal(read(main, "b.txt"), undefined);
  assert.ok(s.reset(c2).changeset);
  assert.deepEqual([read(main, "a.txt"), read(main, "b.txt")], ["v1\n", "b\n"]);
  assert.equal(s.reset(c2).changeset, null);
});

test("a branch merges back, with main's other work brought into it", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const br = s.branch("app", "agent1");
  assert.equal(read(br, "a.txt"), "v0\n");
  write(br, "a.txt", "agent\n");
  write(br, "new.txt", "n\n");
  write(main, "src/lib.js", "export const x = 2;\n");
  const r = s.merge("app", "agent1");
  assert.equal(r.status, "merged");
  for (const d of [main, br]) {
    assert.deepEqual([read(d, "a.txt"), read(d, "new.txt"), read(d, "src/lib.js")], ["agent\n", "n\n", "export const x = 2;\n"]);
  }
  assert.equal(s.merge("app", "agent1").status, "up-to-date");
  write(br, "a.txt", "agent2\n");
  assert.equal(s.merge("app", "agent1").status, "merged");
  assert.equal(read(main, "a.txt"), "agent2\n");
});

test("merge combines text edits of different lines with git merge-file", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  write(main, "f.txt", "1\n2\n3\n4\n5\n6\n7\n");
  s.checkpoint("app", "main");
  const br = s.branch("app", "b");
  write(main, "f.txt", "one\n2\n3\n4\n5\n6\n7\n");
  write(br, "f.txt", "1\n2\n3\n4\n5\n6\nseven\n");
  const r = s.merge("app", "b");
  assert.equal(r.status, "merged");
  assert.equal(read(main, "f.txt"), "one\n2\n3\n4\n5\n6\nseven\n");
  assert.equal(read(br, "f.txt"), "one\n2\n3\n4\n5\n6\nseven\n");
});

test("conflicting edits leave both trees as they were", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const br = s.branch("app", "b");
  write(main, "a.txt", "main\n");
  write(br, "a.txt", "branch\n");
  fs.rmSync(path.join(br, "src/lib.js"));
  write(main, "src/lib.js", "changed\n");
  const r = s.merge("app", "b");
  assert.equal(r.status, "conflict");
  assert.deepEqual(r.conflicts, [
    { path: "a.txt", reason: "both modified" },
    { path: "src/lib.js", reason: "modified/deleted" },
  ]);
  assert.equal(read(main, "a.txt"), "main\n");
  assert.equal(read(br, "a.txt"), "branch\n");
  assert.equal(s.merge("app", "b", { prefer: "theirs" }).status, "merged");
  assert.equal(read(main, "a.txt"), "branch\n");
  assert.equal(read(main, "src/lib.js"), undefined);
});

test("find resolves a directory inside a tree", (t) => {
  const s = fresh(t);
  const main = s.register("app", project(t));
  const br = s.branch("app", "x");
  assert.deepEqual(
    (({ ws, tree }) => ({ ws, tree }))(Store.find(path.join(main, "src"))),
    { ws: "app", tree: "main" },
  );
  assert.equal(Store.find(br).tree, "x");
  assert.equal(Store.find(s.root), null);
  assert.equal(Store.find(tmp()), null);
});

test("prune drops old snapshots but keeps branch bases", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  for (let i = 0; i < 6; i++) {
    write(main, "f", String(i));
    s.checkpoint("app", "main");
  }
  s.branch("app", "b");
  const n = s.prune("app", 2);
  assert.ok(n >= 4);
  const live = s.log({ ws: "app", tree: "main", n: 100 }).filter((c) => !c.pruned);
  assert.ok(live.length >= 2);
  assert.throws(() => s.diff(s.log({ ws: "app", tree: "main", n: 100 }).at(-1).id), { code: "pruned" });
});

test("the lock serializes processes and recovers from a dead holder", (t) => {
  const s = fresh(t);
  s.register("app");
  fs.mkdirSync(path.join(s.root, ".lock"));
  fs.writeFileSync(path.join(s.root, ".lock", "pid"), "999999");
  write(s.treePath("app"), "f", "x");
  assert.ok(s.checkpoint("app", "main"));
  assert.equal(fs.existsSync(path.join(s.root, ".lock")), false);
});
