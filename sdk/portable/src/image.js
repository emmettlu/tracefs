import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { AcfsLiteError, Store } from "./store.js";
import { writeAtomic } from "./tree.js";

export const DEFAULT_SIZE = "64g";

export function home(env = process.env) {
  if (env.ACFS_LITE_HOME) return path.resolve(env.ACFS_LITE_HOME);
  if (process.platform === "win32") return path.join(env.LOCALAPPDATA ?? path.join(os.homedir(), "AppData", "Local"), "acfs-lite");
  return path.join(os.homedir(), ".acfs-lite");
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.error) throw new AcfsLiteError("unavailable", `${cmd}: ${r.error.message}`);
  if (r.status !== 0) throw new AcfsLiteError("failed", `${cmd} ${args.join(" ")}: ${(r.stderr || r.stdout || "").trim()}`);
  return r.stdout;
}

function has(cmd) {
  const r = spawnSync(cmd, ["--version"], { encoding: "utf8" });
  return !r.error;
}

function bytes(size) {
  const m = /^(\d+)([kmgt]?)$/i.exec(String(size));
  if (!m) throw new AcfsLiteError("invalid", `bad size ${size}`);
  return Number(m[1]) * 1024 ** "_kmgt".indexOf((m[2] || "_").toLowerCase());
}

function mountedAt(dir) {
  try {
    return fs.statSync(dir).dev !== fs.statSync(path.dirname(dir)).dev;
  } catch {
    return false;
  }
}

export function fsType(dir) {
  if (process.platform !== "linux") return null;
  try {
    return run("stat", ["-f", "-c", "%T", dir]).trim();
  } catch {
    return null;
  }
}

function stateFile(h) {
  return path.join(h, "image.json");
}

export function readState(h = home()) {
  try {
    return JSON.parse(fs.readFileSync(stateFile(h), "utf8"));
  } catch {
    return null;
  }
}

function saveState(h, s) {
  writeAtomic(stateFile(h), JSON.stringify(s, null, 1));
  return s;
}

function darwin(h, size) {
  const image = path.join(h, "acfs.sparseimage");
  const mount = path.join(h, "mnt");
  fs.mkdirSync(mount, { recursive: true });
  if (!mountedAt(mount)) {
    if (!fs.existsSync(image)) run("hdiutil", ["create", "-quiet", "-size", size, "-type", "SPARSE", "-fs", "APFS", "-volname", "acfs-lite", image]);
    run("hdiutil", ["attach", "-quiet", "-nobrowse", "-noautoopen", "-owners", "on", "-mountpoint", mount, image]);
  }
  return saveState(h, { kind: "apfs-image", image, mount, root: mount });
}

function linuxImage(h, size) {
  const image = path.join(h, "acfs.img");
  const mount = path.join(h, "mnt");
  const prev = readState(h);
  if (prev?.kind === "btrfs-image" && mountedAt(prev.mount) && fs.existsSync(path.join(prev.mount, ".acfs-lite-volume"))) return prev;
  if (!fs.existsSync(image)) {
    if (!has("mkfs.btrfs")) return null;
    const seed = fs.mkdtempSync(path.join(h, "seed-"));
    fs.writeFileSync(path.join(seed, ".acfs-lite-volume"), "");
    try {
      fs.closeSync(fs.openSync(image, "w"));
      fs.truncateSync(image, bytes(size));
      run("mkfs.btrfs", ["-q", "-L", "acfs-lite", "--rootdir", seed, image]);
    } catch (e) {
      fs.rmSync(image, { force: true });
      throw e;
    } finally {
      fs.rmSync(seed, { recursive: true, force: true });
    }
  }
  if (process.getuid?.() === 0) {
    fs.mkdirSync(mount, { recursive: true });
    if (!mountedAt(mount)) run("mount", ["-o", "loop", image, mount]);
    return saveState(h, { kind: "btrfs-image", image, mount, root: mount });
  }
  if (!has("udisksctl")) return null;
  const loop = /as (\/dev\/\S+?)\.?$/m.exec(run("udisksctl", ["loop-setup", "--no-user-interaction", "-f", image]))?.[1];
  if (!loop) throw new AcfsLiteError("failed", "udisksctl loop-setup printed no device");
  let at;
  try {
    at = /at (.+?)\.?$/m.exec(run("udisksctl", ["mount", "--no-user-interaction", "-b", loop]))?.[1];
  } catch (e) {
    const m = /already mounted at [`'"]?(.+?)[`'"]?\.?$/m.exec(e.message);
    if (!m) throw e;
    at = m[1];
  }
  if (!at) throw new AcfsLiteError("failed", "udisksctl mount printed no mount point");
  return saveState(h, { kind: "btrfs-image", image, mount: at, root: at, device: loop });
}

function diskpart(script) {
  const f = path.join(os.tmpdir(), `acfs-lite-diskpart-${process.pid}.txt`);
  fs.writeFileSync(f, script.join("\r\n") + "\r\n");
  try {
    return run("diskpart", ["/s", f]);
  } finally {
    fs.rmSync(f, { force: true });
  }
}

function windows(h, size) {
  const image = path.join(h, "acfs.vhdx");
  const mount = path.join(h, "mnt");
  fs.mkdirSync(mount, { recursive: true });
  const prev = readState(h);
  if (prev?.kind === "ntfs-image" && fs.existsSync(path.join(mount, ".acfs-lite-volume"))) return prev;
  const mb = Math.ceil(bytes(size) / (1 << 20));
  if (!fs.existsSync(image)) {
    diskpart([
      `create vdisk file="${image}" maximum=${mb} type=expandable`,
      `select vdisk file="${image}"`,
      "attach vdisk",
      "create partition primary",
      "format fs=ntfs quick label=acfs-lite",
      `assign mount="${mount}"`,
    ]);
    fs.writeFileSync(path.join(mount, ".acfs-lite-volume"), "");
  } else {
    diskpart([`select vdisk file="${image}"`, "attach vdisk"]);
    if (!fs.existsSync(path.join(mount, ".acfs-lite-volume"))) diskpart([`select vdisk file="${image}"`, "select partition 1", `assign mount="${mount}"`]);
  }
  return saveState(h, { kind: "ntfs-image", image, mount, root: mount });
}

export function ensure({ env = process.env, size = env.ACFS_LITE_SIZE ?? DEFAULT_SIZE, plain = env.ACFS_LITE_PLAIN === "1" } = {}) {
  if (env.ACFS_LITE_ROOT) return { kind: "dir", root: Store.init(path.resolve(env.ACFS_LITE_ROOT)).root };
  const h = home(env);
  fs.mkdirSync(h, { recursive: true });
  let s = null;
  let warning = null;
  if (plain) s = null;
  else if (process.platform === "darwin") s = darwin(h, size);
  else if (process.platform === "win32") s = windows(h, size);
  else if (process.platform === "linux" && fsType(h) === "btrfs") s = saveState(h, { kind: "btrfs", root: path.join(h, "store") });
  else if (process.platform === "linux") {
    try {
      s = linuxImage(h, size);
    } catch (e) {
      warning = `no btrfs image (${e.message}); snapshots are copies`;
    }
  }
  if (!s) s = saveState(h, { kind: "dir", root: path.join(h, "store"), ...(warning ? { warning } : {}) });
  Store.init(s.root);
  return s;
}

export function detach({ env = process.env } = {}) {
  const h = home(env);
  const s = readState(h);
  if (!s) return false;
  if (s.kind === "apfs-image") {
    if (mountedAt(s.mount)) run("hdiutil", ["detach", "-quiet", s.mount]);
  } else if (s.kind === "btrfs-image") {
    if (s.device) {
      if (mountedAt(s.mount)) run("udisksctl", ["unmount", "--no-user-interaction", "-b", s.device]);
      run("udisksctl", ["loop-delete", "--no-user-interaction", "-b", s.device]);
    } else if (mountedAt(s.mount)) run("umount", [s.mount]);
  } else if (s.kind === "ntfs-image") {
    diskpart([`select vdisk file="${s.image}"`, "detach vdisk"]);
  } else return false;
  fs.rmSync(stateFile(h), { force: true });
  return true;
}
