import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { tmp, write } from "./helpers.js";

const CLI = path.join(import.meta.dirname, "..", "src", "cli.js");

function cli(env, ...args) {
  const r = spawnSync(process.execPath, [CLI, ...args], { encoding: "utf8", env: { ...process.env, ...env } });
  return { code: r.status, out: r.stdout.trim(), err: r.stderr.trim() };
}

test("the tracefs-lite command covers the workflow", (t) => {
  const root = tmp();
  const src = tmp();
  t.after(() => {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(src, { recursive: true, force: true });
  });
  write(src, "a.txt", "v0\n");
  const env = { TRACEFS_LITE_ROOT: root };
  assert.match(cli(env, "init").out, /^dir /);
  const main = cli(env, "register", "app", src).out;
  assert.equal(fs.readFileSync(path.join(main, "a.txt"), "utf8"), "v0\n");
  assert.equal(cli(env, "path", "app").out, main);
  const br = cli(env, "branch", "app", "b1").out;
  write(br, "a.txt", "v1\n");
  const c = cli(env, "checkpoint", br, "-m", "edit").out;
  assert.match(c, /^c\d+$/);
  assert.equal(cli(env, "checkpoint", br).out, "unchanged");
  assert.match(cli(env, "log", "app", "--branch", "b1").out, new RegExp(`^${c} app/b1 tracefs-lite edit`));
  assert.equal(cli(env, "diff", c).out, "no changes");
  assert.match(cli(env, "merge", "app", "b1").out, /^merged c\d+/);
  assert.equal(fs.readFileSync(path.join(main, "a.txt"), "utf8"), "v1\n");
  write(main, "a.txt", "m\n");
  write(br, "a.txt", "b\n");
  const conflict = cli(env, "merge", "app", "b1");
  assert.equal(conflict.code, 2);
  assert.match(conflict.out, /conflict: a.txt \(both modified\)/);
  const status = cli(env, "status", "--json");
  assert.deepEqual(JSON.parse(status.out).workspaces.map((w) => [w.name, w.trees]), [["app", ["main", "b1"]]]);
  assert.equal(cli(env, "frobnicate").code, 64);
  const bad = cli(env, "register", "app");
  assert.equal(bad.code, 1);
  assert.match(bad.err, /workspace app exists/);
  assert.equal(cli(env, "drop", "app", "b1").code, 0);
  assert.equal(cli(env, "unregister", "app").code, 0);
  assert.equal(JSON.parse(cli(env, "status", "--json").out).workspaces.length, 0);
});
