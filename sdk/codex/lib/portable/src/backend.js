import { DaemonBackend } from "./daemon.js";
import { readState } from "./image.js";
import { Store } from "./store.js";

export class PortableBackend {
  kind = "portable";

  constructor(store, ws, tree) {
    this.store = store;
    this.ws = ws;
    this.tree = tree;
  }

  describe() {
    const s = readState();
    const how = s && s.root === this.store.root ? s.kind : "dir";
    return `acfs-lite ${this.ws}/${this.tree} (${how})`;
  }

  get path() {
    return this.store.treePath(this.ws, this.tree);
  }

  async checkpoint(message, { toolCall = null, since = null, author = "agent" } = {}) {
    return this.store.checkpoint(this.ws, this.tree, { author, message, toolCall, since }) ?? undefined;
  }

  async diff(a, b) {
    const d = this.store.diff(a, b ?? null);
    return d.changes.map((c) => ({ ws: d.ws, kind: c.kind, path: c.path }));
  }

  result(r) {
    return { conflict: r.conflict, conflicts: (r.conflicts ?? []).map((c) => `${this.ws}: ${c.path} (${c.reason})`), changeset: r.changeset ?? null };
  }

  async undo(changeset) {
    return this.result(this.store.revert({ changeset }));
  }

  async revert(from, to) {
    return this.result(this.store.revert({ from, to }));
  }

  async reset(target) {
    return this.result(this.store.reset(target, { tree: this.tree }));
  }

  async log(n = 20) {
    return this.store.log({ ws: this.ws, tree: this.tree, n }).map((c) => ({ id: c.id, author: c.author, message: c.message, created_at: c.created_at, tool_call: c.tool_call }));
  }

  async settle({ author = "agent" } = {}) {
    if (this.tree === "main") return { state: "main", conflicts: [] };
    const r = this.store.merge(this.ws, this.tree, { author });
    return { state: r.status, conflicts: r.conflicts.map((c) => `${this.ws}: ${c.path} (${c.reason})`) };
  }

  async status() {
    const head = this.store.head(this.ws, this.tree);
    return `${this.describe()}: ${this.path}, head ${head ?? "none"}`;
  }
}

export function open({ cwd = process.cwd(), env = process.env } = {}) {
  const socket = DaemonBackend.available(env);
  if (socket) return new DaemonBackend(env.ACFS_PROC, socket);
  let found = null;
  try {
    found = Store.find(cwd);
  } catch {}
  if (found) return new PortableBackend(found.store, found.ws, found.tree);
  return null;
}
