import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

import { Client, type PathName, type ProcInfo } from "../typescript/src/index.ts";

const READ_ONLY = new Set(["read", "grep", "find", "ls"]);
const CHECKPOINT = "acfs-checkpoint";

interface CheckpointData {
  proc: string;
  target: string;
  from?: string;
  tool_call?: string;
  undid?: string;
}

export default function (pi: ExtensionAPI) {
  const acfs = new Client();
  const proc = process.env.ACFS_PROC;
  let before: string | undefined;
  let asked = false;

  const branch = (ctx: ExtensionContext, leaf?: string | null) => (leaf === null ? [] : ctx.sessionManager.getBranch(leaf ?? undefined));

  function checkpoints(entries: ReturnType<typeof branch>): CheckpointData[] {
    return entries.flatMap((e) =>
      e.type === "custom" && e.customType === CHECKPOINT && (e.data as CheckpointData)?.proc === proc ? [e.data as CheckpointData] : [],
    );
  }

  const stateAt = (ctx: ExtensionContext, leaf?: string | null) => checkpoints(branch(ctx, leaf)).at(-1)?.target;

  async function guard(ctx: ExtensionContext, f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      const msg = `acfs: ${e instanceof Error ? e.message : e}`;
      if (ctx.hasUI) ctx.ui.notify(msg, "error");
      else console.error(msg);
    }
  }

  const changeset = async (message: string, tool_call?: string, since?: string) =>
    (await acfs.call("proc.changeset", { proc_id: proc!, author: "pi", message, ...(tool_call ? { tool_call } : {}), ...(since ? { since } : {}) }))
      .changeset ?? undefined;

  async function restore(ctx: ExtensionContext, from: string | null, to: string | null) {
    const target = stateAt(ctx, to);
    if (!target || target === stateAt(ctx, from)) return;
    const mode = process.env.ACFS_PI_RESTORE ?? "ask";
    if (mode === "never" || (mode === "ask" && !(ctx.hasUI && (await ctx.ui.confirm("acfs", "Bring the files back to this point?"))))) return;
    const r = await acfs.call("reset", { target });
    if (r.conflict) throw new Error(`reset to ${target} conflicts`);
    pi.appendEntry<CheckpointData>(CHECKPOINT, { proc: proc!, target: (await changeset("after reset", undefined, stateAt(ctx, from))) ?? target });
    ctx.ui.notify(`acfs: files back to ${target}`, "info");
  }

  if (!proc) return;

  pi.on("session_start", async (e, ctx) => {
    ctx.ui.setStatus("acfs", `acfs ${proc}`);
    if (e.reason === "fork") await guard(ctx, () => restore(ctx, null, ctx.sessionManager.getLeafId()));
  });

  pi.on("turn_start", () => {
    before = undefined;
    asked = false;
  });

  pi.on("tool_call", async (e, ctx) => {
    if (asked || READ_ONLY.has(e.toolName)) return;
    asked = true;
    await guard(ctx, async () => {
      const last = stateAt(ctx);
      before = (await changeset("before pi tool calls", undefined, last)) ?? last;
    });
  });

  pi.on("turn_end", async (e, ctx) => {
    const calls = e.message.role === "assistant" ? e.message.content.filter((c) => c.type === "toolCall") : [];
    const writes = e.toolResults.filter((r) => !READ_ONLY.has(r.toolName));
    if (!writes.length) return;
    await guard(ctx, async () => {
      const message = writes.map((r) => describe(r.toolName, calls.find((c) => c.id === r.toolCallId)?.arguments ?? {})).join("; ");
      const tool_call = writes[0].toolCallId;
      const target = await changeset(clip(message), tool_call, before);
      if (!target) {
        before = undefined;
        return;
      }
      const cp: CheckpointData = { proc: proc!, target, tool_call };
      if (before) cp.from = before;
      before = undefined;
      pi.appendEntry(CHECKPOINT, cp);
    });
  });

  pi.on("agent_settled", async (_e, ctx) => {
    await guard(ctx, async () => {
      const r = await acfs.call("proc.merge", { proc_id: proc! });
      const left = r.results.flatMap((m) => (m.conflicts ?? []).map((c) => `${m.ws}: ${pathName(c.path)} (${c.reason})`));
      if (left.length) ctx.ui.notify(`acfs: not merged yet, conflicts in ${left.join(", ")}`, "warning");
    });
  });

  pi.on("session_tree", async (e, ctx) => {
    await guard(ctx, () => restore(ctx, e.oldLeafId, e.newLeafId));
  });

  pi.registerCommand("acfs", {
    description: "acfs: status | log | diff | undo | merge",
    handler: async (args, ctx) => {
      await guard(ctx, async () => {
        const [cmd = "status"] = args.trim().split(/\s+/).filter(Boolean);
        if (cmd === "status") {
          const p: ProcInfo = await acfs.call("proc.show", { proc_id: proc! });
          const brs = p.branches.map((b) => `${b.ws} (${b.state})`).join(", ");
          ctx.ui.notify(`acfs: process ${p.id}, ${p.state}; branches ${brs || "none"}`, "info");
        } else if (cmd === "log") {
          const log = await acfs.call("log", { proc_id: proc!, n: 20 });
          ctx.ui.notify(log.map((c) => `${c.id} ${c.author} ${c.message}`).join("\n") || "no changesets", "info");
        } else if (cmd === "diff") {
          const first = checkpoints(branch(ctx))[0];
          const start = first?.from ?? first?.target;
          if (!start) throw new Error("no changes recorded in this conversation");
          const d = await acfs.call("diff", { a: start, b: (await changeset("diff")) ?? stateAt(ctx) ?? start });
          const lines = d.flatMap((r) => r.changes.map((c) => `${r.ws}: ${c.kind} ${pathName(c.path)}`));
          ctx.ui.notify(lines.join("\n") || "no changes since the conversation started", "info");
        } else if (cmd === "undo") {
          const cps = checkpoints(branch(ctx));
          const undone = new Set(cps.flatMap((c) => (c.undid ? [c.undid] : [])));
          const last = cps.findLast((c) => c.tool_call && !undone.has(c.target));
          if (!last) throw new Error("nothing to undo");
          const r = await acfs.call("revert", last.from ? { from: last.from, to: last.target } : { changeset: last.target });
          if (r.conflict) throw new Error(`undo of ${last.target} conflicts with later changes`);
          const now = (await changeset("after undo", undefined, last.target)) ?? last.target;
          pi.appendEntry<CheckpointData>(CHECKPOINT, { proc: proc!, target: now, undid: last.target });
          ctx.ui.notify(`acfs: undid ${last.target}`, "info");
        } else if (cmd === "merge") {
          const r = await acfs.call("proc.merge", { proc_id: proc! });
          ctx.ui.notify(`acfs: ${r.state}${r.conflict ? ", conflicts left" : ""}`, r.conflict ? "warning" : "info");
        } else {
          throw new Error(`unknown subcommand ${cmd}`);
        }
      });
    },
  });
}

function describe(tool: string, input: Record<string, unknown>): string {
  const arg = input.command ?? input.path ?? input.file_path;
  return typeof arg === "string" ? `${tool}: ${arg}` : tool;
}

function clip(s: string): string {
  return s.length > 200 ? `${s.slice(0, 197)}...` : s;
}

function pathName(p: PathName): string {
  return typeof p === "string" ? p : Buffer.from(p.base64, "base64").toString();
}
