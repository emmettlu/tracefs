import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { describe } from "../hooks/acfs.mjs";
import { Store } from "../lib/portable/index.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = path.resolve(ROOT, "..", "..");

function setup(t) {
  const tmp = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "acfs-codex-")));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const store = Store.init(path.join(tmp, "store"));
  const main = store.register("app");
  fs.writeFileSync(path.join(main, "a.txt"), "v0\n");
  store.checkpoint("app", "main");
  const br = store.branch("app", "codex");
  const env = { ...process.env, PLUGIN_ROOT: ROOT, PLUGIN_DATA: path.join(tmp, "data"), ACFS_PROC: "" };
  return { tmp, store, main, br, env };
}

function hook(env, input) {
  const r = spawnSync(process.execPath, [path.join(ROOT, "hooks", "hook.mjs")], { input: JSON.stringify(input), env, encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.stderr, "");
  return r.stdout.trim() ? JSON.parse(r.stdout) : null;
}

function skill(env, cwd, ...args) {
  const r = spawnSync(process.execPath, [path.join(ROOT, "skills", "acfs", "scripts", "acfs.mjs"), ...args], { cwd, env, encoding: "utf8" });
  return { code: r.status, out: r.stdout.trim(), err: r.stderr.trim() };
}

const base = (cwd, extra) => ({ session_id: "s1", turn_id: "t1", cwd, model: "m", permission_mode: "default", transcript_path: null, ...extra });
const put = (dir, f, text) => fs.writeFileSync(path.join(dir, f), text);
const get = (dir, f) => (fs.existsSync(path.join(dir, f)) ? fs.readFileSync(path.join(dir, f), "utf8") : undefined);

async function toolCall(env, cwd, id, tool, input, body) {
  hook(env, base(cwd, { hook_event_name: "PreToolUse", tool_name: tool, tool_input: input, tool_use_id: id }));
  body();
  hook(env, base(cwd, { hook_event_name: "PostToolUse", tool_name: tool, tool_input: input, tool_response: {}, tool_use_id: id }));
}

test("the plugin manifest, hooks and marketplace entry fit together", () => {
  const plugin = JSON.parse(fs.readFileSync(path.join(ROOT, ".codex-plugin", "plugin.json"), "utf8"));
  assert.equal(plugin.name, "acfs");
  assert.ok(fs.existsSync(path.join(ROOT, plugin.hooks)));
  assert.ok(fs.existsSync(path.join(ROOT, plugin.skills, "acfs", "SKILL.md")));
  const hooks = JSON.parse(fs.readFileSync(path.join(ROOT, plugin.hooks), "utf8")).hooks;
  assert.deepEqual(Object.keys(hooks).sort(), ["PostToolUse", "PreToolUse", "SessionStart", "Stop"]);
  for (const groups of Object.values(hooks))
    for (const g of groups)
      for (const h of g.hooks) {
        assert.equal(h.type, "command");
        assert.match(h.command, /PLUGIN_ROOT/);
        assert.match(h.commandWindows, /process\.env\.PLUGIN_ROOT/);
      }
  assert.equal(hooks.PreToolUse[0].matcher, "Bash|apply_patch|Edit|Write");
  const market = JSON.parse(fs.readFileSync(path.join(REPO, ".agents", "plugins", "marketplace.json"), "utf8"));
  const entry = market.plugins.find((p) => p.name === plugin.name);
  assert.equal(path.resolve(REPO, entry.source.path), ROOT);
  const skillMd = fs.readFileSync(path.join(ROOT, "skills", "acfs", "SKILL.md"), "utf8");
  assert.match(skillMd, /^---\nname: acfs\ndescription: .+\n---\n/);
});

test("lib/portable is the same as sdk/portable (run sdk/sync-portable.sh)", () => {
  const src = path.join(REPO, "sdk", "portable");
  const dst = path.join(ROOT, "lib", "portable");
  const files = ["index.js", "index.d.ts", "package.json", ...fs.readdirSync(path.join(src, "src")).map((f) => `src/${f}`)];
  for (const f of files) assert.equal(fs.readFileSync(path.join(dst, f), "utf8"), fs.readFileSync(path.join(src, f), "utf8"), f);
  assert.deepEqual(fs.readdirSync(path.join(dst, "src")).sort(), fs.readdirSync(path.join(src, "src")).sort());
});

test("describe names the command or the patched files", () => {
  assert.equal(describe("Bash", { command: "npm test\nmore" }), "Bash: npm test");
  assert.equal(describe("Bash", { command: ["bash", "-lc", "ls"] }), "Bash: bash -lc ls");
  assert.equal(describe("apply_patch", { command: "*** Begin Patch\n*** Update File: a.txt\n@@\n*** Add File: b/c.txt\n+x\n*** End Patch" }), "apply_patch: a.txt, b/c.txt");
  assert.equal(describe("apply_patch", "*** Delete File: z\n"), "apply_patch: z");
  assert.equal(describe("Write", { file_path: "q.txt" }), "Write: q.txt");
});

test("session start tells the model acfs is on", (t) => {
  const { br, env } = setup(t);
  const out = hook(env, base(br, { hook_event_name: "SessionStart", source: "startup" }));
  assert.equal(out.hookSpecificOutput.hookEventName, "SessionStart");
  assert.match(out.hookSpecificOutput.additionalContext, /acfs is recording your file changes \(acfs-lite app\/codex/);
});

test("each writing tool call becomes a changeset; stop merges into main", async (t) => {
  const { store, main, br, env } = setup(t);
  await toolCall(env, br, "call_1", "apply_patch", { command: "*** Begin Patch\n*** Update File: a.txt\n*** End Patch" }, () => put(br, "a.txt", "v1\n"));
  await toolCall(env, br, "call_2", "Bash", { command: "echo b > b.txt" }, () => put(br, "b.txt", "b\n"));
  hook(env, base(br, { hook_event_name: "PreToolUse", tool_name: "view_image", tool_input: {}, tool_use_id: "call_3" }));
  const mine = store.log({ ws: "app", tree: "codex", n: 20 }).filter((c) => c.author === "codex" && c.tool_call);
  assert.deepEqual(
    mine.map((c) => [c.tool_call, c.message]),
    [
      ["call_2", "Bash: echo b > b.txt"],
      ["call_1", "apply_patch: a.txt"],
    ],
  );
  assert.equal(get(main, "a.txt"), "v0\n");
  assert.equal(hook(env, base(br, { hook_event_name: "Stop", stop_hook_active: false, last_assistant_message: "done" })), null);
  assert.deepEqual([get(main, "a.txt"), get(main, "b.txt")], ["v1\n", "b\n"]);
});

test("a tool call that changes nothing records nothing", async (t) => {
  const { store, br, env } = setup(t);
  await toolCall(env, br, "call_ls", "Bash", { command: "ls" }, () => {});
  assert.equal(store.log({ ws: "app", tree: "codex", n: 20 }).filter((c) => c.tool_call).length, 0);
});

test("a merge conflict at stop becomes a system message and leaves main alone", async (t) => {
  const { main, br, env } = setup(t);
  put(main, "a.txt", "main\n");
  await toolCall(env, br, "call_1", "Bash", { command: "echo branch > a.txt" }, () => put(br, "a.txt", "branch\n"));
  const out = hook(env, base(br, { hook_event_name: "Stop", stop_hook_active: false, last_assistant_message: null }));
  assert.deepEqual(out, { systemMessage: "acfs: not merged into main yet, conflicts in app: a.txt (both modified)" });
  assert.equal(get(main, "a.txt"), "main\n");
  assert.equal(hook(env, base(br, { hook_event_name: "Stop", stop_hook_active: true, last_assistant_message: null })), null);
});

test("ACFS_CODEX_MERGE=0 keeps work on the branch", async (t) => {
  const { main, br, env } = setup(t);
  await toolCall(env, br, "c", "Bash", { command: "x" }, () => put(br, "x", "x"));
  hook({ ...env, ACFS_CODEX_MERGE: "0" }, base(br, { hook_event_name: "Stop", stop_hook_active: false, last_assistant_message: null }));
  assert.equal(get(main, "x"), undefined);
});

test("outside a store every hook is silent", (t) => {
  const dir = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "acfs-codex-none-")));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const env = { ...process.env, PLUGIN_ROOT: ROOT, PLUGIN_DATA: path.join(dir, "data"), ACFS_PROC: "" };
  for (const ev of ["SessionStart", "PreToolUse", "PostToolUse", "Stop"]) {
    assert.equal(hook(env, base(dir, { hook_event_name: ev, tool_name: "Bash", tool_input: { command: "x" }, tool_use_id: "c" })), null);
  }
  assert.equal(fs.existsSync(path.join(dir, "data")), false);
  const s = skill(env, dir, "status");
  assert.equal(s.code, 0);
  assert.match(s.out, /acfs is not active/);
});

test("bad hook input does not fail the tool call", () => {
  const r = spawnSync(process.execPath, [path.join(ROOT, "hooks", "hook.mjs")], { input: "not json", encoding: "utf8" });
  assert.equal(r.status, 0);
  assert.match(r.stderr, /not JSON/);
});

test("the skill script: status, log, diff, undo, merge", async (t) => {
  const { main, br, env } = setup(t);
  await toolCall(env, br, "call_1", "Bash", { command: "echo 1 > a.txt" }, () => put(br, "a.txt", "1\n"));
  await toolCall(env, br, "call_2", "Bash", { command: "echo n > n.txt" }, () => put(br, "n.txt", "n\n"));
  assert.match(skill(env, br, "status").out, /acfs-lite app\/codex/);
  const log = skill(env, br, "log").out.split("\n").map((l) => l.split("\t"));
  const [c2, c1] = log.filter((l) => l[2] !== "-");
  assert.deepEqual([c2[1], c2[2], c2[3]], ["codex", "call_2", "Bash: echo n > n.txt"]);
  assert.equal(skill(env, br, "diff", c1[0], c2[0]).out, "add\tn.txt");
  assert.equal(skill(env, br, "undo", c2[0]).out, `undid ${c2[0]}`);
  assert.equal(get(br, "n.txt"), undefined);
  put(br, "a.txt", "later\n");
  const u = skill(env, br, "undo", c1[0]);
  assert.equal(u.code, 2);
  assert.match(u.out, /app: a.txt \(changed since\)/);
  assert.equal(skill(env, br, "merge").out, "merged");
  assert.equal(get(main, "a.txt"), "later\n");
  assert.equal(skill(env, br, "frob").code, 1);
});
