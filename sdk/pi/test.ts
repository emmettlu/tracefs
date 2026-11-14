import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

import { Client } from "../typescript/src/index.ts";

process.env.TRACEFS_PI_RESTORE = "always";
const tracefs = new Client();
const mainText = async (ws: string, path: string) => {
  try {
    const f = await tracefs.call("fs.read", { ws, path });
    return f.content == null ? undefined : Buffer.from(f.content, "base64").toString();
  } catch {
    return undefined;
  }
};

if (!process.env.TRACEFS_PROC) {
  const dir = mkdtempSync(join(process.env.HOME!, "tracefs-pi-test-"));
  const name = `pitest${Date.now()}`;
  writeFileSync(join(dir, "a.txt"), "v0\n");
  await tracefs.call("ws.register", { name, from: dir });
  const r = spawnSync(process.env.TRACEFS_BIN ?? "tracefs", ["run", "--ws", name, "--", process.execPath, ...process.execArgv, import.meta.filename], {
    cwd: dir,
    stdio: "inherit",
    env: { ...process.env, TRACEFS_PI_TEST_WS: name },
  });
  console.log(`ok ${(await mainText(name, "a.txt")) === "v4\n" ? "" : "NOT "}- main has the last merged state after pi exits`);
  await tracefs.call("ws.detach", { ws: name });
  rmSync(dir, { recursive: true, force: true });
  process.exit(r.status ?? 1);
}

const name = process.env.TRACEFS_PI_TEST_WS!;
const dir = process.cwd();

const tmp = mkdtempSync(join(tmpdir(), "tracefs-pi-"));
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

const read = (f: string) => (existsSync(join(dir, f)) ? readFileSync(join(dir, f), "utf8") : undefined);
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

try {
  await turn("write v1", fauxToolCall("write", { path: "a.txt", content: "v1\n" }));
  ok("write tool changes the process's view", () => assert.equal(read("a.txt"), "v1\n"));
  await turn("write v2 and b", fauxToolCall("write", { path: "a.txt", content: "v2\n" }), fauxToolCall("bash", { command: "echo b >b.txt" }));
  ok("bash tool changes the process's view", () => assert.equal(read("b.txt"), "b\n"));

  const log = await tracefs.call("log", { proc_id: process.env.TRACEFS_PROC!, n: 100 });
  const calls = entries().flatMap((e) => (e.type === "message" && e.message.role === "assistant" ? e.message.content.filter((c) => c.type === "toolCall") : []));
  ok("each assistant message has one changeset, tagged with its first call", () =>
    assert.deepEqual(
      calls.map((c) => log.find((l) => l.tool_call === c.id)?.message),
      ["write: a.txt", "write: a.txt; bash: echo b >b.txt", undefined],
    ),
  );

  const [done1, done2] = assistantTexts();
  await session.navigateTree(done1.id);
  ok("/tree back to turn 1 restores its files", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await session.navigateTree(done2.id);
  ok("/tree forward to turn 2 restores its files", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v2\n", "b\n"]));

  await turn("slow write", fauxToolCall("bash", { command: "echo slow >c.txt; sleep 4" }));
  await session.prompt("/tracefs undo");
  ok("/tracefs undo reverts a slow tool call whose writes an auto snapshot took", () =>
    assert.deepEqual([read("a.txt"), read("b.txt"), read("c.txt")], ["v2\n", "b\n", undefined]),
  );
  await session.prompt("/tracefs undo");
  ok("/tracefs undo reverts the last batch of tool calls", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await session.prompt("/tracefs undo");
  ok("a second /tracefs undo reverts the one before", () => assert.equal(read("a.txt"), "v0\n"));

  await session.prompt("/tracefs merge");
  assert.equal(await mainText(name, "a.txt"), "v0\n");
  ok("/tracefs merge brings the undone state into main", () => undefined);

  await turn("write v3", fauxToolCall("write", { path: "a.txt", content: "v3\n" }));
  assert.equal(await mainText(name, "a.txt"), "v3\n");
  ok("a settled prompt merges its work into main", () => undefined);

  await runtime.fork(userMessages()[1].id, { position: "before" });
  session = runtime.session;
  await session.bindExtensions({});
  ok("/fork before turn 2 restores the files of turn 1", () => assert.deepEqual([read("a.txt"), read("b.txt")], ["v1\n", undefined]));
  await turn("write v4", fauxToolCall("write", { path: "a.txt", content: "v4\n" }));
  assert.equal(await mainText(name, "a.txt"), "v4\n");
  ok("the fork's work merges into main", () => undefined);
} finally {
  await runtime.dispose();
  rmSync(tmp, { recursive: true, force: true });
}
