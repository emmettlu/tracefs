import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const WIN = process.platform === "win32";
const CHUNK = 1 << 20;

export function scan(root) {
  const out = new Map();
  const walk = (rel) => {
    const abs = rel ? path.join(root, rel) : root;
    let names;
    try {
      names = fs.readdirSync(abs);
    } catch (e) {
      if (e.code === "ENOENT") return;
      throw e;
    }
    names.sort();
    for (const name of names) {
      const r = rel ? `${rel}/${name}` : name;
      const e = entryOf(path.join(abs, name));
      if (!e) continue;
      out.set(r, e);
      if (e.type === "dir") walk(r);
    }
  };
  walk("");
  return out;
}

export function entryOf(abs) {
  let st;
  try {
    st = fs.lstatSync(abs, { bigint: true });
  } catch (e) {
    if (e.code === "ENOENT") return null;
    throw e;
  }
  if (st.isSymbolicLink()) return { type: "link", target: fs.readlinkSync(abs) };
  if (st.isDirectory()) return { type: "dir" };
  if (st.isFile()) return { type: "file", size: Number(st.size), mtime: Number(st.mtimeNs / 1000000n), mode: WIN ? 0 : Number(st.mode & 0o111n) };
  return { type: "other" };
}

export function sameContent(a, b) {
  let fa, fb;
  try {
    fa = fs.openSync(a, "r");
    fb = fs.openSync(b, "r");
    const ba = Buffer.allocUnsafe(CHUNK);
    const bb = Buffer.allocUnsafe(CHUNK);
    for (;;) {
      const na = fs.readSync(fa, ba, 0, CHUNK, null);
      const nb = fs.readSync(fb, bb, 0, CHUNK, null);
      if (na !== nb) return false;
      if (na === 0) return true;
      if (!ba.subarray(0, na).equals(bb.subarray(0, nb))) return false;
    }
  } finally {
    if (fa !== undefined) fs.closeSync(fa);
    if (fb !== undefined) fs.closeSync(fb);
  }
}

export function same(a, b, absA, absB, racy = 0) {
  if (!a || !b) return !a && !b;
  if (a.type !== b.type) return false;
  if (a.type === "link") return a.target === b.target;
  if (a.type !== "file") return true;
  if (a.size !== b.size || a.mode !== b.mode) return false;
  if (a.mtime === b.mtime && a.mtime < racy) return true;
  return sameContent(absA, absB);
}

export function diff(rootA, a, rootB, b, racy = 0) {
  const changes = [];
  const paths = new Set([...a.keys(), ...b.keys()]);
  for (const p of [...paths].sort()) {
    const x = a.get(p);
    const y = b.get(p);
    if (!x) changes.push({ kind: "add", path: p });
    else if (!y) changes.push({ kind: "delete", path: p });
    else if (!same(x, y, path.join(rootA, p), path.join(rootB, p), racy)) changes.push({ kind: "modify", path: p });
  }
  return changes;
}

function copyFile(src, dst, keepTime) {
  try {
    fs.unlinkSync(dst);
  } catch (e) {
    if (e.code !== "ENOENT") throw e;
  }
  fs.copyFileSync(src, dst, fs.constants.COPYFILE_FICLONE);
  const st = fs.statSync(src);
  if (!WIN) fs.chmodSync(dst, st.mode & 0o7777);
  if (keepTime) fs.utimesSync(dst, st.atime, st.mtime);
}

function link(target, dst, srcAbs) {
  let type = "file";
  if (WIN) {
    try {
      if (fs.statSync(path.resolve(path.dirname(srcAbs), target)).isDirectory()) type = "dir";
    } catch {}
  }
  fs.symlinkSync(target, dst, type);
}

function cloneTree(src, dst) {
  if (process.platform !== "darwin" || process.env.ACFS_LITE_NO_CLONE === "1" || fs.existsSync(dst)) return false;
  fs.mkdirSync(path.dirname(dst), { recursive: true });
  const r = spawnSync("/bin/cp", ["-c", "-R", "-p", src, dst], { stdio: "ignore" });
  if (r.status === 0) return true;
  remove(dst);
  return false;
}

export function copyTree(src, dst) {
  if (cloneTree(src, dst)) return;
  fs.mkdirSync(dst, { recursive: true });
  for (const [p, e] of scan(src)) {
    const s = path.join(src, p);
    const d = path.join(dst, p);
    if (e.type === "dir") fs.mkdirSync(d, { recursive: true });
    else if (e.type === "file") copyFile(s, d, true);
    else if (e.type === "link") link(e.target, d, s);
  }
}

export function remove(abs) {
  fs.rmSync(abs, { recursive: true, force: true, maxRetries: WIN ? 5 : 0 });
}

export function apply(srcRoot, src, dstRoot, paths) {
  const dst = new Map();
  for (const p of paths) dst.set(p, entryOf(path.join(dstRoot, p)));
  const depth = (p) => p.split("/").length;
  const removals = paths.filter((p) => {
    const d = dst.get(p);
    const s = src.get(p);
    return d && (!s || s.type !== d.type || s.type !== "dir");
  });
  for (const p of removals.sort((x, y) => depth(y) - depth(x))) remove(path.join(dstRoot, p));
  for (const p of paths.filter((p) => src.has(p)).sort((x, y) => depth(x) - depth(y) || (x < y ? -1 : 1))) {
    const e = src.get(p);
    const s = path.join(srcRoot, p);
    const d = path.join(dstRoot, p);
    fs.mkdirSync(path.dirname(d), { recursive: true });
    if (e.type === "dir") fs.mkdirSync(d, { recursive: true });
    else if (e.type === "file") copyFile(s, d, false);
    else if (e.type === "link") link(e.target, d, s);
  }
}

export function writeAtomic(file, data) {
  const tmp = `${file}.${process.pid}.${Date.now()}.tmp`;
  fs.writeFileSync(tmp, data);
  fs.renameSync(tmp, file);
}
