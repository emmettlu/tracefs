import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { Store, detach, ensure, fsType, readState } from "../index.js";
import { tmp, write } from "./helpers.js";

const BIG = 64 << 20;

function admin() {
  return process.platform === "win32" && spawnSync("net", ["session"], { stdio: "ignore" }).status === 0;
}

function linuxImage() {
  if (process.platform !== "linux") return "not Linux";
  if (spawnSync("mkfs.btrfs", ["--version"]).error) return "no mkfs.btrfs";
  if (process.getuid() !== 0 && spawnSync("udisksctl", ["help"]).error) return "neither root nor udisksctl";
  return null;
}

function cleanup(t, h, env) {
  t.after(() => {
    try {
      detach({ env });
    } catch {}
    fs.rmSync(h, { recursive: true, force: true, maxRetries: 5 });
  });
}

function cowCheck(t, env, kind) {
  const s = ensure({ env, size: "4g" });
  assert.equal(s.kind, kind);
  const store = new Store(s.root);
  const main = store.register("big");
  fs.writeFileSync(path.join(main, "blob"), Buffer.alloc(BIG, 1));
  const before = fs.statfsSync(s.root);
  const id = store.checkpoint("big", "main");
  const after = fs.statfsSync(s.root);
  const used = (before.bavail - after.bavail) * before.bsize;
  return { s, store, main, id, used };
}

test("ACFS_LITE_ROOT gives a plain directory store", (t) => {
  const root = tmp();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const s = ensure({ env: { ACFS_LITE_ROOT: root } });
  assert.deepEqual(s, { kind: "dir", root });
  assert.ok(fs.existsSync(path.join(root, "acfs-lite.json")));
});

test("ACFS_LITE_PLAIN skips the image", (t) => {
  const h = tmp();
  t.after(() => fs.rmSync(h, { recursive: true, force: true }));
  const s = ensure({ env: { ACFS_LITE_HOME: h }, plain: true });
  assert.equal(s.kind, "dir");
  assert.equal(s.root, path.join(h, "store"));
  assert.equal(readState(h).kind, "dir");
  assert.equal(detach({ env: { ACFS_LITE_HOME: h } }), false);
});

test("macOS: an APFS sparse image whose snapshots are clones", { skip: process.platform !== "darwin" && "not macOS" }, (t) => {
  const h = tmp();
  const env = { ACFS_LITE_HOME: h };
  cleanup(t, h, env);
  const { store, main, id, used } = cowCheck(t, env, "apfs-image");
  assert.ok(id);
  assert.ok(used < BIG / 4, `a snapshot of a 64 MiB file took ${used} bytes`);
  write(main, "x", "kept");
  assert.equal(detach({ env }), true);
  assert.equal(fs.existsSync(path.join(main, "x")), false);
  const again = ensure({ env });
  assert.equal(again.root, store.root);
  assert.equal(fs.readFileSync(path.join(main, "x"), "utf8"), "kept");
  assert.equal(new Store(again.root).log({ ws: "big" }).length, 2);
});

test("Linux: btrfs home is used in place", { skip: process.platform !== "linux" && "not Linux" }, (t) => {
  const h = tmp();
  cleanup(t, h, { ACFS_LITE_HOME: h });
  if (fsType(h) !== "btrfs") return t.skip(`${h} is ${fsType(h)}`);
  const { id, used } = cowCheck(t, { ACFS_LITE_HOME: h }, "btrfs");
  assert.ok(id);
  assert.ok(used < BIG / 4, `a snapshot of a 64 MiB file took ${used} bytes`);
});

test("Linux: a btrfs image off btrfs, with reflink snapshots", { skip: linuxImage() ?? false }, (t) => {
  const h = path.join(process.env.ACFS_LITE_TEST_DIR ?? process.env.HOME, `.acfs-lite-test-${process.pid}`);
  fs.mkdirSync(h, { recursive: true });
  const env = { ACFS_LITE_HOME: h };
  cleanup(t, h, env);
  if (fsType(h) === "btrfs") return t.skip("home is btrfs; set ACFS_LITE_TEST_DIR to a directory off btrfs");
  let r;
  try {
    r = cowCheck(t, env, "btrfs-image");
  } catch (e) {
    if (readState(h)?.kind === "dir") return t.skip(`could not mount the image here: ${readState(h).warning}`);
    throw e;
  }
  assert.ok(r.id);
  assert.ok(r.used < BIG / 4, `a snapshot of a 64 MiB file took ${r.used} bytes`);
  assert.equal(detach({ env }), true);
  assert.equal(ensure({ env }).kind, "btrfs-image");
  assert.equal(fs.statSync(path.join(r.main, "blob")).size, BIG);
});

test("Windows: an NTFS VHDX mounted on a folder", { skip: process.platform !== "win32" ? "not Windows" : !admin() && "needs an elevated shell (diskpart)" }, (t) => {
  const h = tmp();
  const env = { ACFS_LITE_HOME: h };
  cleanup(t, h, env);
  const { main, id } = cowCheck(t, env, "ntfs-image");
  assert.ok(id);
  write(main, "x", "kept");
  assert.equal(detach({ env }), true);
  assert.equal(ensure({ env }).kind, "ntfs-image");
  assert.equal(fs.readFileSync(path.join(main, "x"), "utf8"), "kept");
});
