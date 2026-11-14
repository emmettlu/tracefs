#!/usr/bin/env node
import { parseArgs } from "node:util";

import { detach, ensure, readState } from "./image.js";
import { Store } from "./store.js";

const USAGE = `usage: tracefs-lite <command> [args]

  init [--size 64g]                  create and mount the store (macOS: APFS image, Linux: btrfs or a btrfs image, Windows: NTFS VHDX)
  status                             where the store is and its workspaces
  detach                             unmount the store's image
  register <ws> [dir]                copy dir (default: empty) into the store as workspace ws; prints its path
  unregister <ws>                    drop a workspace and its history
  path <ws> [branch]                 the directory of main or a branch
  branch <ws> <name>                 a copy-on-write branch of main
  merge <ws> <name> [--prefer ours|theirs]
  drop <ws> <name>                   delete a branch
  checkpoint [dir] [-m message]      record a changeset of the tree dir is in (default: cwd)
  log [ws] [--branch b] [-n 20]
  diff <a> [b]                       changes from changeset a to b (default: the live tree)
  undo <changeset>                   revert one changeset on top of the live tree
  revert <from> <to>                 revert everything between two changesets
  reset <changeset> [--branch b]     bring a tree back to a changeset
  prune <ws> [--keep 200]            delete old snapshots`;

function store() {
  return new Store(ensure().root);
}

function out(v) {
  process.stdout.write(typeof v === "string" ? `${v}\n` : `${JSON.stringify(v, null, 2)}\n`);
}

const { values: opt, positionals: pos } = parseArgs({
  allowPositionals: true,
  options: {
    size: { type: "string" },
    prefer: { type: "string" },
    branch: { type: "string" },
    message: { type: "string", short: "m" },
    n: { type: "string", short: "n" },
    keep: { type: "string" },
    json: { type: "boolean" },
    help: { type: "boolean", short: "h" },
  },
});
const [cmd, ...args] = pos;

try {
  if (!cmd || opt.help) out(USAGE);
  else if (cmd === "init") {
    const s = ensure({ size: opt.size });
    out(opt.json ? s : `${s.kind} ${s.root}${s.warning ? `\nwarning: ${s.warning}` : ""}`);
  } else if (cmd === "status") {
    const s = ensure();
    const ws = new Store(s.root).workspaces();
    out(opt.json ? { ...s, workspaces: ws } : [`${s.kind} ${s.root}`, ...ws.map((w) => `${w.name}: ${w.trees.join(", ")}  ${w.path}`)].join("\n"));
  } else if (cmd === "detach") out(detach() ? "detached" : `nothing to detach (${readState()?.kind ?? "no store"})`);
  else if (cmd === "register") out(store().register(args[0], args[1]));
  else if (cmd === "unregister") store().unregister(args[0]);
  else if (cmd === "path") out(store().treePath(args[0], args[1] ?? "main"));
  else if (cmd === "branch") out(store().branch(args[0], args[1]));
  else if (cmd === "drop") store().dropBranch(args[0], args[1]);
  else if (cmd === "merge") {
    const r = store().merge(args[0], args[1], { prefer: opt.prefer ?? null, author: "tracefs-lite" });
    out(opt.json ? r : [r.status + (r.changeset ? ` ${r.changeset}` : ""), ...r.conflicts.map((c) => `conflict: ${c.path} (${c.reason})`)].join("\n"));
    if (r.status === "conflict") process.exitCode = 2;
  } else if (cmd === "checkpoint") {
    const found = Store.find(args[0] ?? process.cwd());
    if (!found) throw new Error(`${args[0] ?? process.cwd()} is not in an tracefs-lite tree`);
    out(found.store.checkpoint(found.ws, found.tree, { author: "tracefs-lite", message: opt.message ?? "" }) ?? "unchanged");
  } else if (cmd === "log") {
    const log = store().log({ ws: args[0] ?? null, tree: opt.branch ?? null, n: Number(opt.n ?? 20) });
    out(opt.json ? log : log.map((c) => `${c.id} ${c.ws}/${c.tree} ${c.author} ${c.message}${c.pruned ? " (pruned)" : ""}`).join("\n"));
  } else if (cmd === "diff") {
    const d = store().diff(args[0], args[1]);
    out(opt.json ? d : d.changes.map((c) => `${c.kind} ${c.path}`).join("\n") || "no changes");
  } else if (cmd === "undo" || cmd === "revert" || cmd === "reset") {
    const s = store();
    const r = cmd === "undo" ? s.revert({ changeset: args[0] }) : cmd === "revert" ? s.revert({ from: args[0], to: args[1] }) : s.reset(args[0], { tree: opt.branch ?? null });
    out(opt.json ? r : r.conflict ? r.conflicts.map((c) => `conflict: ${c.path} (${c.reason})`).join("\n") : (r.changeset ?? "unchanged"));
    if (r.conflict) process.exitCode = 2;
  } else if (cmd === "prune") out(`pruned ${store().prune(args[0], Number(opt.keep ?? 200))}`);
  else {
    process.stderr.write(`${USAGE}\n`);
    process.exitCode = 64;
  }
} catch (e) {
  process.stderr.write(`tracefs-lite: ${e.message}\n`);
  process.exitCode = 1;
}
