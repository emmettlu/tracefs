import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { Store } from "../lib/portable/index.js";
import * as plugin from "../index.js";

class FakeCtx {
  listeners = new Map();
  tools = new Map();
  warnings = [];
  dirs = new Map();
  workingDirectory = { get: (session) => this.dirs.get(session) };
  logger = () => ({ warn: (m) => this.warnings.push(m) });

  constructor() {
    const self = this;
    this.tools.register = (def) => self.tools.set(def.name, def);
  }

  on(name, fn) {
    if (!this.listeners.has(name)) this.listeners.set(name, []);
    this.listeners.get(name).push(fn);
  }

  async waterfall(name, payload, last) {
    const fns = this.listeners.get(name) ?? [];
    const step = (i) => (i < fns.length ? fns[i](payload, () => step(i + 1)) : last());
    return step(0);
  }

  async waterfall2(name, a, last) {
    return this.waterfall(name, a, last);
  }

  emit(name, ...args) {
    for (const fn of this.listeners.get(name) ?? []) fn(...args);
  }
}

class Harness {
  constructor(t, cwd) {
    this.ctx = new FakeCtx();
    this.session = { id: "s1" };
    this.agent = { session: this.session };
    this.ctx.dirs.set(this.session, cwd);
    this.cwd = cwd;
    this.n = 0;
    this.turn = 0;
    plugin.apply(this.ctx, { env: {} });
  }

  async call(name, args, body) {
    const exec = { callId: `call-${++this.n}`, name, arguments: args, agent: this.agent };
    await this.ctx.waterfall("tools/pre-execute", exec, async () => ({ kind: "allow" }));
    await body?.();
    this.ctx.emit("tools/result", exec, { isError: false, content: [] });
    return exec.callId;
  }

  async step() {
    await this.ctx.waterfall("agent/pre-step", { agent: this.agent, turn: this.turn, step: 0 }, async () => ({ kind: "continue" }));
  }

  async endTurn() {
    this.ctx.emit("session/event", this.session, { type: "turn/end", data: { turn: this.turn++ } });
    await this.tool("status");
  }

  async tool(action, extra = {}) {
    return this.ctx.tools.get("acfs").execute({ action, ...extra }, { agent: this.agent, callId: "x", signal: new AbortController().signal });
  }
}

function setup(t) {
  const root = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "dsh-acfs-")));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const store = Store.init(root);
  const main = store.register("app");
  fs.writeFileSync(path.join(main, "a.txt"), "v0\n");
  store.checkpoint("app", "main");
  const br = store.branch("app", "agent");
  return { store, main, br };
}

const put = (dir, f, text) => fs.writeFileSync(path.join(dir, f), text);
const get = (dir, f) => (fs.existsSync(path.join(dir, f)) ? fs.readFileSync(path.join(dir, f), "utf8") : undefined);

test("lib/portable is the same as sdk/portable (run sdk/sync-portable.sh)", () => {
  const src = new URL("../../portable/", import.meta.url);
  const dst = new URL("../lib/portable/", import.meta.url);
  const files = ["index.js", "index.d.ts", "package.json", ...fs.readdirSync(new URL("src/", src)).map((f) => `src/${f}`)];
  for (const f of files) assert.equal(fs.readFileSync(new URL(f, dst), "utf8"), fs.readFileSync(new URL(f, src), "utf8"), f);
  assert.deepEqual(fs.readdirSync(new URL("src/", dst)).sort(), fs.readdirSync(new URL("src/", src)).sort());
});

test("the package is self-contained: it imports nothing outside its directory", () => {
  const text = fs.readFileSync(new URL("../index.js", import.meta.url), "utf8");
  assert.doesNotMatch(text, /from "\.\.\//);
});

test("the bundle manifest inserts the plugin by package name", () => {
  const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url)));
  const patch = fs.readFileSync(new URL("../cordis.patch.yml", import.meta.url), "utf8");
  assert.deepEqual(pkg.dsh.bundle.patch, ["./cordis.patch.yml"]);
  assert.match(patch, new RegExp(`name: ${pkg.name}$`, "m"));
  assert.equal(plugin.name, "acfs");
  assert.deepEqual(plugin.inject, ["tools", "workingDirectory"]);
});

test("each step's writing tool calls become one changeset; reads record nothing", async (t) => {
  const { store, br } = setup(t);
  const h = new Harness(t, br);
  const c1 = await h.call("write", { path: "a.txt" }, () => put(br, "a.txt", "v1\n"));
  const c2 = await h.call("bash", { command: "echo b > b.txt" }, () => put(br, "b.txt", "b\n"));
  await h.call("read", { path: "a.txt" });
  await h.step();
  const log = store.log({ ws: "app", tree: "agent", n: 20 });
  const mine = log.filter((c) => c.author === "dsh" && c.tool_call);
  assert.deepEqual(
    mine.map((c) => [c.message, c.tool_call]),
    [["write: a.txt; bash: echo b > b.txt", c1]],
  );
  assert.ok(!log.some((c) => c.tool_call === c2));
});

test("turn end merges the branch into main", async (t) => {
  const { main, br } = setup(t);
  const h = new Harness(t, br);
  await h.call("write", { path: "a.txt" }, () => put(br, "a.txt", "v1\n"));
  await h.endTurn();
  assert.equal(get(main, "a.txt"), "v1\n");
});

test("the acfs tool: log, diff, undo, merge, status", async (t) => {
  const { main, br } = setup(t);
  const h = new Harness(t, br);
  await h.call("write", { path: "a.txt" }, () => put(br, "a.txt", "v1\n"));
  await h.step();
  await h.call("write", { path: "c.txt" }, () => put(br, "c.txt", "c\n"));
  await h.step();
  assert.match(await h.tool("log"), /dsh write: c.txt\n.*dsh write: a.txt/);
  assert.equal(await h.tool("diff"), "app: modify a.txt\napp: add c.txt");
  assert.match(await h.tool("undo"), /^undid c\d+$/);
  assert.equal(get(br, "c.txt"), undefined);
  assert.match(await h.tool("undo"), /^undid c\d+$/);
  assert.equal(get(br, "a.txt"), "v0\n");
  assert.equal(await h.tool("undo"), "nothing to undo");
  put(br, "z.txt", "z\n");
  assert.equal(await h.tool("merge"), "merged");
  assert.equal(get(main, "z.txt"), "z\n");
  assert.match(await h.tool("status"), /acfs-lite app\/agent/);
});

test("undo reports a conflict and keeps the later change", async (t) => {
  const { br } = setup(t);
  const h = new Harness(t, br);
  await h.call("write", { path: "a.txt" }, () => put(br, "a.txt", "v1\n"));
  await h.step();
  put(br, "a.txt", "user edit\n");
  assert.match(await h.tool("undo"), /conflicts with later changes: app: a.txt \(changed since\)/);
  assert.equal(get(br, "a.txt"), "user edit\n");
});

test("merge conflicts at turn end are warned, not thrown", async (t) => {
  const { main, br } = setup(t);
  const h = new Harness(t, br);
  put(main, "a.txt", "main\n");
  await h.call("write", { path: "a.txt" }, () => put(br, "a.txt", "branch\n"));
  await h.endTurn();
  assert.deepEqual(h.ctx.warnings, ["acfs: not merged yet, conflicts in app: a.txt (both modified)"]);
  assert.equal(get(main, "a.txt"), "main\n");
});

test("outside any store the plugin stays out of the way", async (t) => {
  const dir = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "dsh-acfs-none-")));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const h = new Harness(t, dir);
  await h.call("write", { path: "a.txt" }, () => put(dir, "a.txt", "x"));
  await h.step();
  await h.endTurn();
  assert.match(await h.tool("status"), /acfs is not active/);
  assert.deepEqual(h.ctx.warnings, []);
});

test("writeTools and merge are configurable", async (t) => {
  const { store, main, br } = setup(t);
  const ctx = new FakeCtx();
  const session = {};
  ctx.dirs.set(session, br);
  plugin.apply(ctx, { env: {}, writeTools: ["custom_writer"], merge: false, author: "me" });
  const agent = { session };
  const exec = { callId: "k1", name: "custom_writer", arguments: { path: "q" }, agent };
  await ctx.waterfall("tools/pre-execute", exec, async () => ({ kind: "allow" }));
  put(br, "q", "q");
  ctx.emit("tools/result", exec, {});
  ctx.emit("session/event", session, { type: "turn/end" });
  await ctx.tools.get("acfs").execute({ action: "status" }, { agent });
  assert.equal(store.log({ ws: "app", tree: "agent", n: 5 }).find((c) => c.tool_call === "k1")?.author, "me");
  assert.equal(get(main, "q"), undefined);
});
