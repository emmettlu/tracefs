import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { open } from "../lib/portable/index.js";

export const WRITE_TOOLS = new Set(["Bash", "apply_patch", "Edit", "Write"]);
const AUTHOR = "codex";

function clip(s) {
  return s.length > 200 ? `${s.slice(0, 197)}...` : s;
}

export function describe(tool, input) {
  if (tool === "apply_patch") {
    const text = typeof input === "string" ? input : (input?.command ?? input?.patch ?? input?.input ?? "");
    const files = [...String(text).matchAll(/^\*\*\* (Add|Update|Delete) File: (.+)$/gm)].map((m) => m[2].trim());
    return files.length ? `apply_patch: ${files.join(", ")}` : "apply_patch";
  }
  const arg = input?.command ?? input?.file_path ?? input?.path;
  const text = Array.isArray(arg) ? arg.join(" ") : arg;
  return typeof text === "string" ? `${tool}: ${text.split("\n")[0]}` : tool;
}

function stateDir(env) {
  return path.join(env.PLUGIN_DATA || path.join(os.tmpdir(), "acfs-codex"), "sessions");
}

function load(env, session) {
  try {
    return JSON.parse(fs.readFileSync(path.join(stateDir(env), `${session}.json`), "utf8"));
  } catch {
    return { last: null, before: null, history: [] };
  }
}

function save(env, session, state) {
  const dir = stateDir(env);
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `${session}.json`);
  fs.writeFileSync(`${file}.tmp`, JSON.stringify(state));
  fs.renameSync(`${file}.tmp`, file);
}

export async function handle(input, env = process.env) {
  const backend = open({ cwd: input.cwd, env });
  if (!backend) return null;
  const session = String(input.session_id ?? "default").replace(/[^A-Za-z0-9._-]/g, "_");
  const event = input.hook_event_name;
  const merge = env.ACFS_CODEX_MERGE !== "0";

  if (event === "SessionStart") {
    return {
      hookSpecificOutput: {
        hookEventName: "SessionStart",
        additionalContext: `acfs is recording your file changes (${backend.describe()}). Each Bash or apply_patch call becomes a changeset; use the acfs skill to list, diff or undo them.`,
      },
    };
  }

  if (event === "PreToolUse") {
    if (!WRITE_TOOLS.has(input.tool_name)) return null;
    const s = load(env, session);
    if (!s.before) {
      s.before = (await backend.checkpoint("before codex tool calls", { author: AUTHOR, since: s.last ?? undefined })) ?? s.last ?? (await backend.log(1))[0]?.id ?? null;
      save(env, session, s);
    }
    return null;
  }

  if (event === "PostToolUse") {
    if (!WRITE_TOOLS.has(input.tool_name)) return null;
    const s = load(env, session);
    const to = await backend.checkpoint(clip(describe(input.tool_name, input.tool_input)), {
      author: AUTHOR,
      toolCall: input.tool_use_id ?? null,
      since: s.before ?? undefined,
    });
    if (to) {
      s.history.push({ from: s.before, to, tool_call: input.tool_use_id ?? null, turn: input.turn_id ?? null });
      s.last = to;
    }
    s.before = null;
    save(env, session, s);
    return null;
  }

  if (event === "Stop") {
    if (!merge || input.stop_hook_active) return null;
    const r = await backend.settle({ author: AUTHOR });
    if (r.conflicts.length) return { systemMessage: `acfs: not merged into main yet, conflicts in ${r.conflicts.join(", ")}` };
    return null;
  }

  return null;
}
