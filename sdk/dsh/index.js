import { open } from "./lib/portable/index.js";

export const name = "acfs";
export const inject = ["tools", "workingDirectory"];

export const WRITE_TOOLS = ["bash", "pwsh", "write", "edit", "apply_patch", "str_replace_editor", "run_code", "terminal_send", "office_convert", "office_recalculate"];
const AUTHOR = "dsh";

function describe(name, args) {
  const a = args && typeof args === "object" ? (args.command ?? args.path ?? args.file_path ?? args.code) : undefined;
  return typeof a === "string" ? `${name}: ${a.split("\n")[0]}` : name;
}

function clip(s) {
  return s.length > 200 ? `${s.slice(0, 197)}...` : s;
}

export function apply(ctx, config = {}) {
  const writes = new Set(config.writeTools ?? WRITE_TOOLS);
  const author = config.author ?? AUTHOR;
  const merge = config.merge ?? true;
  const env = config.env ?? process.env;
  const log = typeof ctx.logger === "function" ? ctx.logger("acfs") : console;
  const sessions = new WeakMap();

  function state(session) {
    let s = sessions.get(session);
    if (s) return s;
    const cwd = ctx.workingDirectory.get(session);
    s = { cwd, backend: open({ cwd, env }), last: undefined, before: undefined, pending: [], history: [], queue: Promise.resolve() };
    sessions.set(session, s);
    return s;
  }

  function run(s, f) {
    const p = s.queue.then(f).catch((e) => {
      log.warn(`acfs: ${e instanceof Error ? e.message : e}`);
    });
    s.queue = p;
    return p;
  }

  function exclusive(s, f) {
    const p = s.queue.then(f);
    s.queue = p.catch(() => {});
    return p;
  }

  function flush(s) {
    return run(s, async () => {
      if (!s.pending.length) return;
      const calls = s.pending.splice(0);
      const to = await s.backend.checkpoint(clip(calls.map((c) => describe(c.name, c.args)).join("; ")), { author, toolCall: calls[0].callId, since: s.before });
      if (to) {
        s.history.push({ from: s.before, to, toolCall: calls[0].callId, undone: false });
        s.last = to;
      }
      s.before = undefined;
    });
  }

  ctx.on("tools/pre-execute", async (exec, next) => {
    const session = exec.agent?.session;
    if (!session || !writes.has(exec.name)) return next();
    const s = state(session);
    if (!s.backend) return next();
    await run(s, async () => {
      if (s.before) return;
      s.before = (await s.backend.checkpoint("before dsh tool calls", { author, since: s.last })) ?? s.last ?? (await s.backend.log(1))[0]?.id;
    });
    return next();
  });

  ctx.on("tools/result", (exec) => {
    const session = exec.agent?.session;
    if (!session || !writes.has(exec.name)) return;
    const s = state(session);
    if (s.backend) s.pending.push({ callId: exec.callId, name: exec.name, args: exec.arguments });
  });

  ctx.on("agent/pre-step", async (payload, next) => {
    const s = payload.agent?.session && sessions.get(payload.agent.session);
    if (s?.backend) await flush(s);
    return next();
  });

  ctx.on("session/event", (session, event) => {
    if (event?.type !== "turn/end") return;
    const s = sessions.get(session);
    if (!s?.backend) return;
    flush(s);
    if (!merge) return;
    run(s, async () => {
      const r = await s.backend.settle({ author });
      if (r.conflicts.length) log.warn(`acfs: not merged yet, conflicts in ${r.conflicts.join(", ")}`);
    });
  });

  ctx.tools.register({
    name: "acfs",
    description:
      "Version history of the files you work on (acfs). Every tool call that changes files is recorded as a changeset. " +
      "action=status: where the history lives; log: recent changesets; diff: files changed since this conversation began, or between changesets a and b; " +
      "undo: revert the last recorded batch of your tool calls that is not undone yet; merge: merge this branch into main now.",
    parameters: {
      type: "object",
      properties: {
        action: { type: "string", enum: ["status", "log", "diff", "undo", "merge"] },
        a: { type: "string", description: "diff: first changeset" },
        b: { type: "string", description: "diff: second changeset (default: the files as they are)" },
      },
      required: ["action"],
      additionalProperties: false,
    },
    output: {
      schema: { type: "string" },
      render: (_args, value) => [{ type: "text", text: String(value) }],
    },
    async execute(args, exec) {
      const session = exec.agent?.session;
      if (!session) throw new Error("acfs needs a calling agent");
      const s = state(session);
      if (!s.backend) {
        return `acfs is not active in ${s.cwd}: start dsh under \`acfs run\` (Linux, acfsd) or work in a tree of an acfs-lite store (\`acfs-lite register <ws> <dir>\`).`;
      }
      await flush(s);
      let out = "";
      await exclusive(s, async () => {
        const b = s.backend;
        if (args.action === "status") out = await b.status();
        else if (args.action === "log") out = (await b.log(20)).map((c) => `${c.id} ${c.author} ${c.message}`).join("\n") || "no changesets";
        else if (args.action === "diff") {
          const start = args.a ?? s.history[0]?.from ?? s.history[0]?.to;
          if (!start) {
            out = "no changes recorded in this conversation";
            return;
          }
          const end = args.b ?? (await b.checkpoint("diff", { author })) ?? s.last ?? start;
          out = (await b.diff(start, end)).map((c) => `${c.ws}: ${c.kind} ${c.path}`).join("\n") || "no changes";
        } else if (args.action === "undo") {
          const last = s.history.findLast((h) => !h.undone);
          if (!last) {
            out = "nothing to undo";
            return;
          }
          const r = last.from ? await b.revert(last.from, last.to) : await b.undo(last.to);
          if (r.conflict) {
            out = `undo of ${last.to} conflicts with later changes: ${r.conflicts.join(", ")}`;
            return;
          }
          last.undone = true;
          s.last = (await b.checkpoint("after undo", { author, since: last.to })) ?? s.last;
          out = `undid ${last.to}`;
        } else if (args.action === "merge") {
          const r = await b.settle({ author });
          out = `${r.state}${r.conflicts.length ? `; conflicts: ${r.conflicts.join(", ")}` : ""}`;
        } else throw new Error(`unknown action ${args.action}`);
      });
      return out;
    },
  });
}
