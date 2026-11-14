import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { apply, copyTree, diff, entryOf, remove, same, scan, writeAtomic } from "./tree.js";

export const META = "tracefs-lite.json";
const NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const STALE_MS = 120_000;
const sleeper = new Int32Array(new SharedArrayBuffer(4));
const depth = new Map();

export class TracefsLiteError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "TracefsLiteError";
    this.code = code;
  }
}

function checkName(kind, name) {
  if (!NAME.test(name ?? "")) throw new TracefsLiteError("invalid", `bad ${kind} name ${JSON.stringify(name)}`);
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === "EPERM";
  }
}

export class Store {
  constructor(root) {
    this.root = path.resolve(root);
  }

  static init(root) {
    fs.mkdirSync(root, { recursive: true });
    const store = new Store(root);
    if (!fs.existsSync(store.metaPath)) writeAtomic(store.metaPath, JSON.stringify({ version: 1, next: 1, workspaces: {}, changesets: [] }, null, 1));
    fs.mkdirSync(path.join(store.root, "trees"), { recursive: true });
    fs.mkdirSync(path.join(store.root, "snaps"), { recursive: true });
    return store;
  }

  static find(dir) {
    let cur = fs.realpathSync.native(path.resolve(dir));
    const parts = [];
    for (;;) {
      if (fs.existsSync(path.join(cur, META))) {
        const [top, ws, tree] = parts.slice(-3).reverse();
        if (top !== "trees" || !ws || !tree) return null;
        const store = new Store(cur);
        if (!store.read().workspaces[ws]?.trees[tree]) return null;
        return { store, ws, tree };
      }
      const up = path.dirname(cur);
      if (up === cur) return null;
      parts.push(path.basename(cur));
      cur = up;
    }
  }

  get metaPath() {
    return path.join(this.root, META);
  }

  read() {
    return JSON.parse(fs.readFileSync(this.metaPath, "utf8"));
  }

  write(meta) {
    writeAtomic(this.metaPath, JSON.stringify(meta, null, 1));
  }

  locked(f) {
    const key = this.root;
    if (depth.get(key)) {
      depth.set(key, depth.get(key) + 1);
      try {
        return f();
      } finally {
        depth.set(key, depth.get(key) - 1);
      }
    }
    const lock = path.join(this.root, ".lock");
    for (let i = 0; ; i++) {
      try {
        fs.mkdirSync(lock);
        fs.writeFileSync(path.join(lock, "pid"), String(process.pid));
        break;
      } catch (e) {
        if (e.code !== "EEXIST") throw e;
        let pid = 0;
        let age = 0;
        try {
          pid = Number(fs.readFileSync(path.join(lock, "pid"), "utf8"));
          age = Date.now() - fs.statSync(lock).mtimeMs;
        } catch {}
        if ((pid && !alive(pid)) || age > STALE_MS) remove(lock);
        else if (i > 6000) throw new TracefsLiteError("busy", `store ${this.root} is locked by pid ${pid}`);
        else Atomics.wait(sleeper, 0, 0, 10);
      }
    }
    depth.set(key, 1);
    try {
      return f();
    } finally {
      depth.set(key, 0);
      remove(lock);
    }
  }

  treePath(ws, tree = "main") {
    return path.join(this.root, "trees", ws, tree);
  }

  snapPath(id) {
    return path.join(this.root, "snaps", id);
  }

  changeset(id, meta = this.read()) {
    const c = meta.changesets.find((c) => c.id === id);
    if (!c) throw new TracefsLiteError("not_found", `no changeset ${id}`);
    if (c.pruned) throw new TracefsLiteError("pruned", `changeset ${id} was pruned`);
    return c;
  }

  head(ws, tree, meta = this.read()) {
    return meta.changesets.findLast((c) => c.ws === ws && c.tree === tree && !c.pruned)?.id;
  }

  workspaces() {
    return Object.entries(this.read().workspaces).map(([name, w]) => ({ name, trees: Object.keys(w.trees), path: this.treePath(name) }));
  }

  register(ws, from) {
    checkName("workspace", ws);
    return this.locked(() => {
      const meta = this.read();
      if (meta.workspaces[ws]) throw new TracefsLiteError("exists", `workspace ${ws} exists`);
      const dst = this.treePath(ws);
      if (from) copyTree(path.resolve(from), dst);
      else fs.mkdirSync(dst, { recursive: true });
      meta.workspaces[ws] = { trees: { main: { base: null } }, from: from ? path.resolve(from) : null };
      this.write(meta);
      this.checkpoint(ws, "main", { author: "tracefs-lite", message: "register" });
      return dst;
    });
  }

  unregister(ws) {
    return this.locked(() => {
      const meta = this.read();
      if (!meta.workspaces[ws]) throw new TracefsLiteError("not_found", `no workspace ${ws}`);
      for (const c of meta.changesets.filter((c) => c.ws === ws)) remove(this.snapPath(c.id));
      remove(path.join(this.root, "trees", ws));
      delete meta.workspaces[ws];
      meta.changesets = meta.changesets.filter((c) => c.ws !== ws);
      this.write(meta);
    });
  }

  checkpoint(ws, tree, { author = "tracefs-lite", message = "", toolCall = null, since = null, force = false } = {}) {
    return this.locked(() => {
      const meta = this.read();
      if (!meta.workspaces[ws]?.trees[tree]) throw new TracefsLiteError("not_found", `no tree ${ws}/${tree}`);
      const base = since ?? this.head(ws, tree, meta);
      const live = this.treePath(ws, tree);
      if (base && !force && this.diffLive(base, meta).length === 0) return null;
      const id = `c${meta.next++}`;
      const taken = Date.now();
      copyTree(live, this.snapPath(id));
      meta.changesets.push({ id, ws, tree, author, message, tool_call: toolCall, since: base ?? null, created_at: Date.now(), taken });
      this.write(meta);
      return id;
    });
  }

  diffLive(id, meta = this.read()) {
    const c = this.changeset(id, meta);
    const snap = this.snapPath(id);
    const live = this.treePath(c.ws, c.tree);
    return diff(snap, scan(snap), live, scan(live), c.taken);
  }

  diff(a, b = null) {
    const meta = this.read();
    const ca = this.changeset(a, meta);
    if (!b) return { ws: ca.ws, from: a, to: null, changes: this.diffLive(a, meta) };
    const cb = this.changeset(b, meta);
    if (ca.ws !== cb.ws) throw new TracefsLiteError("invalid", `${a} and ${b} are of different workspaces`);
    const [sa, sb] = [this.snapPath(a), this.snapPath(b)];
    return { ws: ca.ws, from: a, to: b, changes: diff(sa, scan(sa), sb, scan(sb), Math.min(ca.taken, cb.taken)) };
  }

  revert({ changeset = null, from = null, to = null, author = "tracefs-lite" } = {}) {
    return this.locked(() => {
      const meta = this.read();
      if (changeset) {
        const c = this.changeset(changeset, meta);
        if (!c.since) throw new TracefsLiteError("invalid", `${changeset} is the first changeset of ${c.ws}/${c.tree}`);
        from = c.since;
        to = changeset;
      }
      if (!from || !to) throw new TracefsLiteError("invalid", "revert needs a changeset or from and to");
      const cf = this.changeset(from, meta);
      const ct = this.changeset(to, meta);
      if (cf.ws !== ct.ws) throw new TracefsLiteError("invalid", `${from} and ${to} are of different workspaces`);
      const live = this.treePath(ct.ws, ct.tree);
      const [sf, st] = [this.snapPath(from), this.snapPath(to)];
      const ef = scan(sf);
      const et = scan(st);
      const changes = diff(sf, ef, st, et, Math.min(cf.taken, ct.taken));
      const paths = changes.map((c) => c.path);
      const conflicts = [];
      const now = scan(live);
      for (const p of paths) {
        if (!same(et.get(p), now.get(p), path.join(st, p), path.join(live, p), ct.taken)) conflicts.push({ path: p, reason: "changed since" });
      }
      const set = new Set(paths);
      for (const p of paths) {
        if (et.get(p)?.type !== "dir" || ef.get(p)?.type === "dir") continue;
        for (const q of now.keys()) if (q.startsWith(`${p}/`) && !set.has(q) && !et.has(q)) conflicts.push({ path: q, reason: "added since" });
      }
      if (conflicts.length) return { conflict: true, conflicts, changeset: null };
      this.checkpoint(ct.ws, ct.tree, { author, message: "before revert" });
      apply(sf, ef, live, paths);
      const id = this.checkpoint(ct.ws, ct.tree, { author, message: `revert ${from}..${to}` });
      return { conflict: false, conflicts: [], changeset: id };
    });
  }

  reset(target, { author = "tracefs-lite", tree = null } = {}) {
    return this.locked(() => {
      const meta = this.read();
      const c = this.changeset(target, meta);
      const into = tree ?? c.tree;
      const live = this.treePath(c.ws, into);
      const snap = this.snapPath(target);
      const es = scan(snap);
      const changes = diff(snap, es, live, scan(live), c.taken);
      if (!changes.length) return { conflict: false, changeset: null };
      this.checkpoint(c.ws, into, { author, message: "before reset" });
      apply(snap, es, live, changes.map((x) => x.path));
      return { conflict: false, changeset: this.checkpoint(c.ws, into, { author, message: `reset to ${target}` }) };
    });
  }

  log({ ws = null, tree = null, n = 50 } = {}) {
    const all = this.read().changesets.filter((c) => (!ws || c.ws === ws) && (!tree || c.tree === tree));
    return all.slice(-n).reverse();
  }

  branch(ws, name) {
    checkName("branch", name);
    return this.locked(() => {
      const meta = this.read();
      if (!meta.workspaces[ws]) throw new TracefsLiteError("not_found", `no workspace ${ws}`);
      if (meta.workspaces[ws].trees[name]) throw new TracefsLiteError("exists", `branch ${ws}/${name} exists`);
      const base = this.checkpoint(ws, "main", { message: `branch ${name}` }) ?? this.head(ws, "main");
      copyTree(this.snapPath(base), this.treePath(ws, name));
      const m = this.read();
      m.workspaces[ws].trees[name] = { base };
      this.write(m);
      this.checkpoint(ws, name, { message: `branch from ${base}`, force: true });
      return this.treePath(ws, name);
    });
  }

  dropBranch(ws, name) {
    if (name === "main") throw new TracefsLiteError("invalid", "main is not a branch");
    return this.locked(() => {
      const meta = this.read();
      if (!meta.workspaces[ws]?.trees[name]) throw new TracefsLiteError("not_found", `no branch ${ws}/${name}`);
      remove(this.treePath(ws, name));
      delete meta.workspaces[ws].trees[name];
      this.write(meta);
    });
  }

  merge(ws, name, { author = "tracefs-lite", prefer = null } = {}) {
    if (name === "main") throw new TracefsLiteError("invalid", "main is not a branch");
    return this.locked(() => {
      const meta = this.read();
      const info = meta.workspaces[ws]?.trees[name];
      if (!info) throw new TracefsLiteError("not_found", `no branch ${ws}/${name}`);
      const base = this.changeset(info.base, meta);
      const sb = this.snapPath(info.base);
      const ours = this.treePath(ws, "main");
      const theirs = this.treePath(ws, name);
      const [eb, eo, et] = [scan(sb), scan(ours), scan(theirs)];
      const changedTheirs = diff(sb, eb, theirs, et, base.taken).map((c) => c.path);
      const changedOurs = diff(sb, eb, ours, eo, base.taken).map((c) => c.path);
      if (!changedTheirs.length && !changedOurs.length) return { status: "up-to-date", conflicts: [], changeset: null };
      const oursSet = new Set(changedOurs);
      const take = [];
      const merged = new Map();
      const conflicts = [];
      for (const p of changedTheirs) {
        if (!oursSet.has(p)) {
          take.push(p);
          continue;
        }
        oursSet.delete(p);
        const [b, o, t] = [eb.get(p), eo.get(p), et.get(p)];
        if (same(o, t, path.join(ours, p), path.join(theirs, p))) continue;
        if (o?.type === "dir" && t?.type === "dir") continue;
        const text = o?.type === "file" && t?.type === "file" ? mergeText(b?.type === "file" ? path.join(sb, p) : null, path.join(ours, p), path.join(theirs, p)) : null;
        if (text !== null) merged.set(p, text);
        else if (prefer === "theirs") take.push(p);
        else if (prefer === "ours") oursSet.add(p);
        else conflicts.push({ path: p, reason: !o || !t ? "modified/deleted" : !b ? "both added" : "both modified" });
      }
      if (conflicts.length) return { status: "conflict", conflicts, changeset: null };
      this.checkpoint(ws, "main", { author, message: `before merging ${name}` });
      this.checkpoint(ws, name, { author, message: "before merge into main" });
      apply(theirs, et, ours, take);
      for (const [p, text] of merged) {
        fs.writeFileSync(path.join(ours, p), text);
      }
      const toTheirs = [...oursSet, ...merged.keys()];
      apply(ours, scan(ours), theirs, toTheirs);
      const id = this.checkpoint(ws, "main", { author, message: `merge ${name}` }) ?? this.head(ws, "main");
      this.checkpoint(ws, name, { author, message: "updated from main" });
      const m = this.read();
      m.workspaces[ws].trees[name].base = id;
      this.write(m);
      return { status: "merged", conflicts: [], changeset: id, actions: take.length + merged.size };
    });
  }

  prune(ws, keep = 200) {
    return this.locked(() => {
      const meta = this.read();
      const pinned = new Set(Object.values(meta.workspaces[ws]?.trees ?? {}).map((t) => t.base));
      const byTree = new Map();
      for (const c of meta.changesets.filter((c) => c.ws === ws && !c.pruned)) {
        if (!byTree.has(c.tree)) byTree.set(c.tree, []);
        byTree.get(c.tree).push(c);
      }
      let n = 0;
      for (const list of byTree.values()) {
        for (const c of list.slice(0, Math.max(0, list.length - keep))) {
          if (pinned.has(c.id)) continue;
          remove(this.snapPath(c.id));
          c.pruned = true;
          n++;
        }
      }
      this.write(meta);
      return n;
    });
  }
}

function mergeText(base, ours, theirs) {
  const big = 8 << 20;
  for (const f of [base, ours, theirs]) if (f && fs.statSync(f).size > big) return null;
  const read = (f) => (f ? fs.readFileSync(f) : Buffer.alloc(0));
  const bufs = [read(base), read(ours), read(theirs)];
  if (bufs.some((b) => b.includes(0))) return null;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "tracefs-lite-merge-"));
  try {
    const files = ["base", "ours", "theirs"].map((n, i) => {
      const f = path.join(tmp, n);
      fs.writeFileSync(f, bufs[i]);
      return f;
    });
    const r = spawnSync("git", ["merge-file", "-p", files[1], files[0], files[2]], { maxBuffer: 64 << 20 });
    if (r.error || r.status !== 0) return null;
    return r.stdout;
  } finally {
    remove(tmp);
  }
}

export { entryOf };
