import { appendFileSync, writeFileSync } from "node:fs";

import { TracefsError, type ArgsOf, Client, type Op, type Results } from "../sdk/typescript/src/index.ts";

const c = new Client();
const [log, name] = process.argv.slice(2);
writeFileSync(log, "");

async function call<O extends Op>(op: O, ...args: [ArgsOf<O>] extends [never] ? [] : [ArgsOf<O>]): Promise<Results[O]> {
  const r = await c.call(op, ...args);
  appendFileSync(log, JSON.stringify([op, r]) + "\n");
  return r;
}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

assert((await call("ping")).protocol === 2, "protocol");
await call("ws.list");
const s = (await call("session.create", { ws: ["w"] })).id;
const content = Buffer.from("sdk\n").toString("base64");
const cs = (await call("fs.edit", { ws: "w", ops: [{ op: "write", path: name, content }], author: "sdk", message: `sdk ${name}` })).changeset!;
const changes = (await call("diff", { a: cs })).flatMap((d) => d.changes);
assert(changes.some((ch) => ch.path === name && ch.kind === "add"), JSON.stringify(changes));
assert((await call("fs.read", { ws: "w", path: name })).content === content, "read");
await call("changeset", { session: s, author: "sdk", message: "nothing" });
await call("log", { session: s });
await call("proc.list");
for (const [op, args, code] of [
  ["proc.show", { proc_id: "nope" }, "not_found"],
  ["proc.run", { cmd: ["true"], cwd: "/" }, "invalid"],
] as const) {
  try {
    await c.call(op as Op, args as never);
    throw new Error(`${op}: expected an error`);
  } catch (e) {
    assert(e instanceof TracefsError && e.code === code, `${op}: ${e}`);
  }
}
assert((await call("session.merge", { session: s })).state === "merged", "merge");
await call("session.list");
console.log("ok");
