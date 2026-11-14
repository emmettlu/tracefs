import fs from "node:fs";
import { connect } from "node:net";

export const DEFAULT_SOCKET = "/run/acfs/acfs-daemon.sock";

export class AcfsError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "AcfsError";
    this.code = code;
  }
}

export function call(socket, op, args) {
  const req = args === undefined ? { op } : { op, args };
  return new Promise((resolve, reject) => {
    const chunks = [];
    const s = connect(socket, () => s.end(JSON.stringify(req) + "\n"));
    s.on("data", (c) => chunks.push(c));
    s.on("error", reject);
    s.on("end", () => {
      let r;
      try {
        r = JSON.parse(Buffer.concat(chunks).toString("utf8").split("\n")[0]);
      } catch (e) {
        reject(new AcfsError("invalid", `bad response: ${e}`));
        return;
      }
      if (r.ok) resolve(r.result);
      else reject(new AcfsError(r.error?.code ?? "internal", r.error?.message ?? "unknown error"));
    });
  });
}

export function pathName(p) {
  return typeof p === "string" ? p : Buffer.from(p.base64, "base64").toString();
}

export class DaemonBackend {
  kind = "acfsd";

  constructor(proc, socket = DEFAULT_SOCKET) {
    this.proc = proc;
    this.socket = socket;
  }

  static available(env = process.env) {
    const socket = env.ACFS_SOCKET ?? DEFAULT_SOCKET;
    return Boolean(env.ACFS_PROC) && process.platform === "linux" && fs.existsSync(socket) ? socket : null;
  }

  call(op, args) {
    return call(this.socket, op, args);
  }

  describe() {
    return `acfs ${this.proc}`;
  }

  async checkpoint(message, { toolCall = null, since = null, author = "agent" } = {}) {
    const r = await this.call("proc.changeset", { proc_id: this.proc, author, message, ...(toolCall ? { tool_call: toolCall } : {}), ...(since ? { since } : {}) });
    return r.changeset ?? undefined;
  }

  async diff(a, b) {
    const d = await this.call("diff", { a, ...(b ? { b } : {}) });
    return d.flatMap((r) => r.changes.map((c) => ({ ws: r.ws, kind: c.kind, path: pathName(c.path) })));
  }

  async undo(changeset) {
    return this.applied(await this.call("revert", { changeset }));
  }

  async revert(from, to) {
    return this.applied(await this.call("revert", { from, to }));
  }

  async reset(target) {
    return this.applied(await this.call("reset", { target }));
  }

  applied(r) {
    const conflicts = (r.results ?? []).flatMap((m) => (m.conflicts ?? []).map((c) => `${m.ws}: ${pathName(c.path)} (${c.reason})`));
    return { conflict: Boolean(r.conflict), conflicts };
  }

  async log(n = 20) {
    return (await this.call("log", { proc_id: this.proc, n })).map((c) => ({ id: c.id, author: c.author, message: c.message, created_at: c.created_at, tool_call: c.tool_call ?? null }));
  }

  async settle() {
    const r = await this.call("proc.merge", { proc_id: this.proc });
    const conflicts = r.results.flatMap((m) => (m.conflicts ?? []).map((c) => `${m.ws}: ${pathName(c.path)} (${c.reason})`));
    return { state: r.state, conflicts };
  }

  async status() {
    const p = await this.call("proc.show", { proc_id: this.proc });
    const brs = p.branches.map((b) => `${b.ws} (${b.state})`).join(", ");
    return `acfs: process ${p.id}, ${p.state}; branches ${brs || "none"}`;
  }
}
