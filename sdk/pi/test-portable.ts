import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import {
  type AgentSession,
  type CreateAgentSessionRuntimeFactory,
  createAgentSessionFromServices,
  createAgentSessionRuntime,
  createAgentSessionServices,
  ModelRuntime,
  SessionManager,
  type SessionEntry,
} from "@earendil-works/pi-coding-agent";

import { Store } from "../portable/index.js";

process.env.TRACEFS_PI_RESTORE = "always";
delete process.env.TRACEFS_PROC;

const tmp = realpathSync.native(mkdtempSync(join(tmpdir(), "tracefs-pi-portable-")));
const store = Store.init(join(tmp, "store"));
const src = join(tmp, "src");
const { writeFileSync, mkdirSync } = await import("node:fs");
mkdirSync(src);
writeFileSync(join(src, "a.txt"), "v0\n");
const main = store.register("app", src);
const dir = store.branch("app", "pi");
process.chdir(dir);

const faux = fauxProvider();
const modelRuntime = await ModelRuntime.create({ authPath: join(tmp, "auth.json"), modelsPath: null, modelsStorePath: join(tmp, "models-store.json") });
modelRuntime.registerNativeProvider(faux.provider);

const createRuntime: CreateAgentSessionRuntimeFactory = async ({ cwd, sessionManager, sessionStartEvent }) => {
  const services = await createAgentSessionServices({
    cwd,
    agentDir: tmp,
    modelRuntime,
    resourceLoaderOptions: { additionalExtensionPaths: [join(import.meta.dirname, "index.ts")] },
  });
  const created = await createAgentSessionFromServices({ services, sessionManager, sessionStartEvent, model: faux.getModel() });
  return { ...created, services, diagnostics: services.diagnostics };
};
const runtime = await createAgentSessionRuntime(createRuntime, {
  cwd: dir,
  agentDir: tmp,
  sessionManager: SessionManager.create(dir, join(tmp, "sessions")),
});
let session: AgentSession = runtime.session;
await session.bindExtensions({});

const readIn = (d: string, f: string) => (existsSync(join(d, f)) ? readFileSync(join(d, f), "utf8") : undefined);
const read = (f: string) => readIn(dir, f);
const entries = (): SessionEntry[] => session.sessionManager.getEntries();
const assistantTexts = () => entries().filter((e) => e.type === "message" && e.message.role === "assistant" && e.message.stopReason === "stop");
const userMessages = () => entries().filter((e) => e.type === "message" && e.message.role === "user");
let step = 0;
const ok = (what: string, cond: () => void) => {
  cond();
  console.log(`ok ${++step} ${what}`);
};

async function turn(prompt: string, ...calls: ReturnType<typeof fauxToolCall>[]) {
  faux.setResponses([fauxAssistantMessage(calls, { stopReason: "toolUse" }), fauxAssistantMessage("done")]);
  await session.prompt(prompt);
  assert.equal(faux.getPendingResponseCount(), 0, "the model was not called twice");
}

let failed = false;
try {
  await turn("write v1", fauxToolCall("write", { path: "a.txt", content: "v1\n" }));
  ok("write tool changes the branch", () => assert.equal(read("a.txt"), "v1\n"));
  ok("a settled prompt merges the branch into main", () => assert.equal(readIn(main, "a.txt"), "v1\n"));
  await turn("write v2 and b", fauxToolCall("write", { path: "a.txt", content: "v2\n" }), fauxToolCall("bash", { command: "echo b >b.txt" }));
  ok("bash tool changes the branch", () => assert.equal(read("b.txt")?.trim(), "b"));

  const log = store.log({ ws: "app", tree: "pi", n: 100 });
  const calls = entries().flatMap((e) => (e.type === "message" && e.message.role === "assistant" ? e.message.content.filter((c) => c.type === "toolCall") : []));
  ok("each assistant message has one changeset, tagged with its first call", () =>
    assert.deepEqual(
      calls.map((c) => log.find((l) => l.tool_call === c.id)?.message),
      ["write: a.txt", "write: a.txt; bash: echo b >b.txt", undefined],
    ),
  );
  ok("changesets are authored by pi", () => assert.ok(log.filter((l) => l.tool_call).every((l) => l.author === "pi")));

  const [done1, done2] = assistantTexts();
  await session.navigateTree(done1.id);
  ok("/tree back to turn 1 restores its files", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await session.navigateTree(done2.id);
  ok("/tree forward to turn 2 restores its files", () => assert.deepEqual([read("a.txt"), read("b.txt")?.trim()], ["v2\n", "b"]));

  await session.prompt("/tracefs undo");
  ok("/tracefs undo reverts the last batch of tool calls", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await session.prompt("/tracefs undo");
  ok("a second /tracefs undo reverts the one before", () => assert.equal(read("a.txt"), "v0\n"));

  await session.prompt("/tracefs merge");
  ok("/tracefs merge brings the undone state into main", () => assert.equal(readIn(main, "a.txt"), "v0\n"));

  writeFileSync(join(main, "other.txt"), "from main\n");
  await turn("write v3", fauxToolCall("write", { path: "a.txt", content: "v3\n" }));
  ok("settling merges into main and brings main's other work into the branch", () => {
    assert.equal(readIn(main, "a.txt"), "v3\n");
    assert.equal(read("other.txt"), "from main\n");
  });

  await runtime.fork(userMessages()[1].id, { position: "before" });
  session = runtime.session;
  await session.bindExtensions({});
  ok("/fork before turn 2 restores the files of turn 1", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await turn("write v4", fauxToolCall("write", { path: "a.txt", content: "v4\n" }));
  ok("the fork's work merges into main", () => assert.equal(readIn(main, "a.txt"), "v4\n"));
} catch (e) {
  failed = true;
  console.error(e);
} finally {
  await runtime.dispose();
  process.chdir(tmpdir());
  rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
process.exit(failed ? 1 : 0);
