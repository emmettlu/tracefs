import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";

import { DaemonBackend, PortableBackend, open } from "../index.js";
import { WIN, fresh, read, tmp, write } from "./helpers.js";

test("open picks the portable backend inside a tree and nothing outside", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  const b = open({ cwd: path.join(main), env: {} });
  assert.ok(b instanceof PortableBackend);
  assert.deepEqual([b.ws, b.tree], ["app", "main"]);
  assert.equal(open({ cwd: tmp(), env: {} }), null);
  assert.equal(open({ cwd: "/definitely/not/here", env: {} }), null);
});

test("open ignores ACFS_PROC without a daemon socket", (t) => {
  const s = fresh(t);
  const main = s.register("app");
  assert.ok(open({ cwd: main, env: { ACFS_PROC: "p1", ACFS_SOCKET: path.join(tmp(), "none.sock") } }) instanceof PortableBackend);
});

test("portable backend: checkpoint, undo, diff, log and settle", async (t) => {
  const s = fresh(t);
  s.register("app");
  const br = s.branch("app", "agent");
  const b = open({ cwd: br, env: {} });
  write(br, "a.txt", "1\n");
  const c1 = await b.checkpoint("write a", { toolCall: "t1", author: "pi" });
  write(br, "b.txt", "2\n");
  const c2 = await b.checkpoint("write b", { toolCall: "t2", since: c1, author: "pi" });
  assert.equal(await b.checkpoint("nothing", { since: c2 }), undefined);
  assert.deepEqual(await b.diff(c1, c2), [{ ws: "app", kind: "add", path: "b.txt" }]);
  const log = await b.log(10);
  assert.deepEqual(log.slice(0, 2).map((c) => [c.id, c.tool_call]), [[c2, "t2"], [c1, "t1"]]);
  assert.equal((await b.undo(c2)).conflict, false);
  assert.equal(read(br, "b.txt"), undefined);
  const settled = await b.settle({ author: "pi" });
  assert.deepEqual(settled, { state: "merged", conflicts: [] });
  assert.equal(read(s.treePath("app"), "a.txt"), "1\n");
  assert.match(await b.status(), /app\/agent/);
  assert.deepEqual(await open({ cwd: s.treePath("app"), env: {} }).settle(), { state: "main", conflicts: [] });
});

test("portable backend reports conflicts as text", async (t) => {
  const s = fresh(t);
  const main = s.register("app");
  write(main, "f", "base");
  s.checkpoint("app", "main");
  const br = s.branch("app", "a");
  write(main, "f", "main");
  write(br, "f", "branch");
  const r = await open({ cwd: br, env: {} }).settle();
  assert.deepEqual(r, { state: "conflict", conflicts: ["app: f (both modified)"] });
});

function fakeDaemon(t, handler) {
  const dir = tmp("acfsd-");
  const sock = path.join(dir, "d.sock");
  const seen = [];
  const server = net.createServer((c) => {
    let buf = "";
    c.on("data", (d) => {
      buf += d;
      if (!buf.includes("\n")) return;
      const req = JSON.parse(buf.split("\n")[0]);
      seen.push(req);
      let reply;
      try {
        reply = { ok: true, result: handler(req) };
      } catch (e) {
        reply = { ok: false, error: { code: e.code ?? "internal", message: e.message } };
      }
      c.end(JSON.stringify(reply) + "\n");
    });
  });
  return new Promise((resolve) =>
    server.listen(sock, () => {
      t.after(() => {
        server.close();
        fs.rmSync(dir, { recursive: true, force: true });
      });
      resolve({ sock, seen });
    }),
  );
}

test("daemon backend speaks acfsd's protocol", { skip: WIN && "unix sockets" }, async (t) => {
  const { sock, seen } = await fakeDaemon(t, (req) => {
    switch (req.op) {
      case "proc.changeset":
        return { changeset: req.args.message === "same" ? null : "cs9" };
      case "diff":
        return [{ ws: "w", to: "b", changes: [{ kind: "add", path: { base64: Buffer.from("é.txt").toString("base64") } }] }];
      case "revert":
        return { conflict: true, results: [{ ws: "w", status: "conflict", conflicts: [{ path: "a", reason: "changed" }] }] };
      case "log":
        return [{ id: "cs9", author: "pi", message: "m", created_at: 1, tool_call: "t", owner: "p", owner_kind: "process", snaps: [] }];
      case "proc.merge":
        return { id: "p", state: "merged", conflict: false, results: [], updated: [] };
      case "proc.show":
        return { id: "p", state: "running", branches: [{ ws: "w", state: "dirty" }] };
      default:
        throw Object.assign(new Error(`no op ${req.op}`), { code: "invalid" });
    }
  });
  const env = { ACFS_PROC: "p", ACFS_SOCKET: sock };
  const b = process.platform === "linux" ? open({ cwd: tmp(), env }) : new DaemonBackend("p", sock);
  assert.ok(b instanceof DaemonBackend);
  assert.equal(await b.checkpoint("m", { toolCall: "t", since: "cs1", author: "pi" }), "cs9");
  assert.deepEqual(seen.at(-1), { op: "proc.changeset", args: { proc_id: "p", author: "pi", message: "m", tool_call: "t", since: "cs1" } });
  assert.equal(await b.checkpoint("same"), undefined);
  assert.deepEqual(await b.diff("a", "b"), [{ ws: "w", kind: "add", path: "é.txt" }]);
  assert.deepEqual(await b.undo("cs9"), { conflict: true, conflicts: ["w: a (changed)"] });
  assert.deepEqual(seen.at(-1), { op: "revert", args: { changeset: "cs9" } });
  assert.deepEqual((await b.log(5))[0], { id: "cs9", author: "pi", message: "m", created_at: 1, tool_call: "t" });
  assert.deepEqual(await b.settle(), { state: "merged", conflicts: [] });
  assert.equal(await b.status(), "acfs: process p, running; branches w (dirty)");
  await assert.rejects(b.call("nope"), { code: "invalid" });
});
