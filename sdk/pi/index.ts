import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

import { type Backend, open } from "../portable/index.js";

const READ_ONLY = new Set(["read", "grep", "find", "ls"]);
const CHECKPOINT = "acfs-checkpoint";
const AUTHOR = "pi";

interface CheckpointData {
  proc: string;
  target: string;
  from?: string;
  tool_call?: string;
  undid?: string;
}

export default function (pi: ExtensionAPI) {
  const acfs = open({ cwd: process.cwd() });
  let before: string | undefined;
  let asked = false;

  pi.registerCommand("acfs", {
    description: "acfs: status | log | diff | undo | merge",
    handler: async (args, ctx) => {
      if (!acfs) {
        ctx.ui.notify(
          "acfs: not active here. Start pi under `acfs run` (Linux, acfsd) or in a tree of an acfs-lite store (`acfs-lite register <ws> <dir>`, then cd to the printed path).",
          "warning",
        );
        return;
      }
      await guard(ctx, () => command(acfs, args.trim().split(/\s+/).filter(Boolean)[0] ?? "status", ctx));
    },
  });

  if (!acfs) return;
  const backend: Backend = acfs;
  const key = acfs.kind === "acfsd" ? acfs.proc : `${acfs.ws}/${acfs.tree}`;

  const branch = (ctx: ExtensionContext, leaf?: string | null) => (leaf === null ? [] : ctx.sessionManager.getBranch(leaf ?? undefined));

  function checkpoints(entries: ReturnType<typeof branch>): CheckpointData[] {
    return entries.flatMap((e) =>
      e.type === "custom" && e.customType === CHECKPOINT && (e.data as CheckpointData)?.proc === key ? [e.data as CheckpointData] : [],
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

  const changeset = (message: string, toolCall?: string, since?: string) => backend.checkpoint(message, { author: AUTHOR, toolCall, since });

  async function restore(ctx: ExtensionContext, from: string | null, to: string | null) {
    const target = stateAt(ctx, to);
    if (!target || target === stateAt(ctx, from)) return;
    const mode = process.env.ACFS_PI_RESTORE ?? "ask";
    if (mode === "never" || (mode === "ask" && !(ctx.hasUI && (await ctx.ui.confirm("acfs", "Bring the files back to this point?"))))) return;
    const r = await backend.reset(target);
    if (r.conflict) throw new Error(`reset to ${target} conflicts`);
    pi.appendEntry<CheckpointData>(CHECKPOINT, { proc: key, target: (await changeset("after reset", undefined, stateAt(ctx, from))) ?? target });
    ctx.ui.notify(`acfs: files back to ${target}`, "info");
  }

  pi.on("session_start", async (e, ctx) => {
    ctx.ui.setStatus("acfs", backend.describe());
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
      const cp: CheckpointData = { proc: key, target, tool_call };
      if (before) cp.from = before;
      before = undefined;
      pi.appendEntry(CHECKPOINT, cp);
    });
  });

  pi.on("agent_settled", async (_e, ctx) => {
    await guard(ctx, async () => {
      const r = await backend.settle({ author: AUTHOR });
      if (r.conflicts.length) ctx.ui.notify(`acfs: not merged yet, conflicts in ${r.conflicts.join(", ")}`, "warning");
    });
  });

  pi.on("session_tree", async (e, ctx) => {
    await guard(ctx, () => restore(ctx, e.oldLeafId, e.newLeafId));
  });

  async function command(b: Backend, cmd: string, ctx: ExtensionContext) {
    if (cmd === "status") {
      ctx.ui.notify(await b.status(), "info");
    } else if (cmd === "log") {
      const log = await b.log(20);
      ctx.ui.notify(log.map((c) => `${c.id} ${c.author} ${c.message}`).join("\n") || "no changesets", "info");
    } else if (cmd === "diff") {
      const first = checkpoints(branch(ctx))[0];
      const start = first?.from ?? first?.target;
      if (!start) throw new Error("no changes recorded in this conversation");
      const d = await b.diff(start, (await changeset("diff")) ?? stateAt(ctx) ?? start);
      ctx.ui.notify(d.map((c) => `${c.ws}: ${c.kind} ${c.path}`).join("\n") || "no changes since the conversation started", "info");
    } else if (cmd === "undo") {
      const cps = checkpoints(branch(ctx));
      const undone = new Set(cps.flatMap((c) => (c.undid ? [c.undid] : [])));
      const last = cps.findLast((c) => c.tool_call && !undone.has(c.target));
      if (!last) throw new Error("nothing to undo");
      const r = last.from ? await b.revert(last.from, last.target) : await b.undo(last.target);
      if (r.conflict) throw new Error(`undo of ${last.target} conflicts with later changes`);
      const now = (await changeset("after undo", undefined, last.target)) ?? last.target;
      pi.appendEntry<CheckpointData>(CHECKPOINT, { proc: key, target: now, undid: last.target });
      ctx.ui.notify(`acfs: undid ${last.target}`, "info");
    } else if (cmd === "merge") {
      const r = await b.settle({ author: AUTHOR });
      ctx.ui.notify(`acfs: ${r.state}${r.conflicts.length ? `, conflicts left: ${r.conflicts.join(", ")}` : ""}`, r.conflicts.length ? "warning" : "info");
    } else {
      throw new Error(`unknown subcommand ${cmd}`);
    }
  }
}

function describe(tool: string, input: Record<string, unknown>): string {
  const arg = input.command ?? input.path ?? input.file_path;
  return typeof arg === "string" ? `${tool}: ${arg}` : tool;
}

function clip(s: string): string {
  return s.length > 200 ? `${s.slice(0, 197)}...` : s;
}
