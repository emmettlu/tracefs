import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const { open } = await import(new URL("../../../lib/portable/index.js", import.meta.url).href);

const [cmd = "status", ...args] = process.argv.slice(2);
const cwd = process.env.ACFS_CWD ?? process.cwd();
const b = open({ cwd });
if (!b) {
  console.log(`acfs is not active in ${cwd}. Work under \`acfs run\` (Linux, acfsd) or in a tree of an acfs-lite store: node ${path.join(here, "..", "..", "..", "lib", "portable", "src", "cli.js")} register <ws> <dir>, then work in the printed path.`);
  process.exit(cmd === "status" ? 0 : 1);
}
try {
  if (cmd === "status") console.log(await b.status());
  else if (cmd === "log") {
    const log = await b.log(Number(args[0] ?? 20));
    console.log(log.map((c) => `${c.id}\t${c.author}\t${c.tool_call ?? "-"}\t${c.message}`).join("\n") || "no changesets");
  } else if (cmd === "diff") {
    if (!args[0]) throw new Error("usage: diff <from> [to]");
    const d = await b.diff(args[0], args[1] ?? (await b.checkpoint("diff", { author: "codex" })) ?? (await b.log(1))[0]?.id);
    console.log(d.map((c) => `${c.kind}\t${c.path}`).join("\n") || "no changes");
  } else if (cmd === "undo") {
    if (!args[0]) throw new Error("usage: undo <changeset>");
    const r = await b.undo(args[0]);
    if (r.conflict) {
      console.log(`undo of ${args[0]} conflicts with later changes:\n${r.conflicts.join("\n")}`);
      process.exitCode = 2;
    } else console.log(`undid ${args[0]}`);
  } else if (cmd === "merge") {
    const r = await b.settle({ author: "codex" });
    console.log(`${r.state}${r.conflicts.length ? `\nconflicts:\n${r.conflicts.join("\n")}` : ""}`);
    if (r.conflicts.length) process.exitCode = 2;
  } else throw new Error(`unknown command ${cmd} (status, log, diff, undo, merge)`);
} catch (e) {
  console.error(`acfs: ${e instanceof Error ? e.message : e}`);
  process.exitCode = 1;
}
