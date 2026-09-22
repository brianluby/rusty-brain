import { afterEach, beforeEach, expect, test } from "bun:test";
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import rustyBrainOmpExtension from "../scorecard-omp-extension.ts";

type JsonObject = Record<string, unknown>;
type Handler = (event: JsonObject, context: ExtensionContext) => unknown;
type FixtureRecord = { phase: string; envelope?: JsonObject; session_id?: string };
const environmentKeys = [
  "RB_OMP_HOOKS_BIN", "RB_SCORECARD_INJECTION_DIR", "RB_SCORECARD_PLACEBO_SOURCE",
  "RB_SCORECARD_FORBIDDEN", "RB_OMP_FIXTURE_LOG",
];
let directory: string;
let receipts: string;
let log: string;
let savedEnvironment: Record<string, string | undefined>;

beforeEach(async () => {
  savedEnvironment = Object.fromEntries(environmentKeys.map((key) => [key, process.env[key]]));
  for (const key of environmentKeys) delete process.env[key];
  directory = await mkdtemp(join(tmpdir(), "rb-omp-runtime-"));
  receipts = join(directory, "receipts");
  log = join(directory, "hook-events.jsonl");
  const fixture = await readFile(new URL("./fixtures/omp-hook.ts", import.meta.url), "utf8");
  const executable = join(directory, "fixture-hook.ts");
  await writeFile(executable, `#!${process.execPath}\n${fixture}`, { mode: 0o700 });
  process.env.RB_OMP_HOOKS_BIN = executable;
  process.env.RB_SCORECARD_INJECTION_DIR = receipts;
  process.env.RB_OMP_FIXTURE_LOG = log;
});

afterEach(async () => {
  for (const key of environmentKeys) {
    const value = savedEnvironment[key];
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  await rm(directory, { recursive: true, force: true });
});

function host(initialId: unknown = "session-one", initialBranch: unknown[] = []) {
  const handlers = new Map<string, Handler>();
  let id = initialId;
  let branch = initialBranch;
  const context = {
    cwd: directory,
    sessionManager: { getSessionId: () => id, getBranch: () => branch },
  } as unknown as ExtensionContext;
  rustyBrainOmpExtension({
    on(name: string, handler: Handler) { handlers.set(name, handler); },
  } as unknown as ExtensionAPI);
  return {
    context,
    switchTo(nextId: unknown, nextBranch: unknown[] = []) { id = nextId; branch = nextBranch; },
    async emit(type: string, fields: JsonObject = {}) {
      const handler = handlers.get(type);
      if (!handler) throw new Error(`Missing OMP event handler: ${type}`);
      return await handler({ type, ...fields }, context);
    },
  };
}

function returnedMessage(result: unknown): string | undefined {
  if (typeof result !== "object" || result === null || !("message" in result)) return undefined;
  const message = result.message;
  if (typeof message !== "object" || message === null || !("content" in message)) return undefined;
  return typeof message.content === "string" ? message.content : undefined;
}

async function fixtureRecords(): Promise<FixtureRecord[]> {
  if (!await Bun.file(log).exists()) return [];
  return (await readFile(log, "utf8")).trim().split("\n").map((line) => JSON.parse(line) as FixtureRecord);
}

async function receiptMessages(path = receipts): Promise<string[]> {
  const file = join(path, "prompt-time.jsonl");
  if (!await Bun.file(file).exists()) return [];
  return (await readFile(file, "utf8")).trim().split("\n")
    .map((line) => {
      const receipt: { message: string } = JSON.parse(line);
      return receipt.message;
    });
}

async function placeboSource(messages: string[]): Promise<string> {
  const source = join(directory, "source");
  await mkdir(source, { recursive: true });
  await writeFile(join(source, "prompt-time.jsonl"), messages.map((message) => `${JSON.stringify({ message })}\n`).join(""));
  return source;
}

const successfulTool = {
  toolName: "write", input: { path: "src/result.ts", content: "export const result = 1;" },
  content: [{ type: "text", text: "written" }], details: {}, isError: false,
};

test("returned prompt messages and appended receipts follow preparation order", async () => {
  const instance = host();
  await instance.emit("session_start");
  const returned: unknown[] = [];
  await Promise.all([
    instance.emit("before_agent_start", { prompt: "alpha" }).then((value) => returned.push(value)),
    instance.emit("before_agent_start", { prompt: "beta" }).then((value) => returned.push(value)),
  ]);
  const messages = returned.map(returnedMessage);
  expect(messages).toEqual(["Stored alpha decision", "Stored β decision 𐐀"]);
  expect(returned[0]).toEqual({ message: {
    customType: "dev.rusty-brain.recall", content: messages[0], display: false, attribution: "agent",
  } });
  expect(await receiptMessages()).toEqual(messages);
  expect((await fixtureRecords()).map((record) => record.envelope?.type)).toEqual(["session_start", "prompt", "prompt"]);
});

test("placebo matches UTF-8 token estimates and never starts hooks across lifecycle", async () => {
  const sourceMessages = ["é𐐀a", "two independent source records"];
  process.env.RB_SCORECARD_PLACEBO_SOURCE = await placeboSource(sourceMessages);
  const instance = host();
  await instance.emit("session_start");
  const messages = [];
  for (const prompt of ["alpha", "beta"]) {
    messages.push(returnedMessage(await instance.emit("before_agent_start", { prompt })));
    await instance.emit("tool_result", successfulTool);
  }
  expect(messages).toEqual(sourceMessages.map((message) => ".".repeat(Math.ceil(Buffer.byteLength(message) / 4) * 4)));
  expect(await receiptMessages()).toEqual(messages);
  for (const event of ["session_before_switch", "session_switch", "session_before_branch", "session_branch", "session_tree"]) {
    await instance.emit(event);
  }
  await instance.emit("session_shutdown");
  expect(await fixtureRecords()).toEqual([]);
});

test("empty, missing, malformed, oversized, and prohibited placebo sources cannot fall back to memory", async () => {
  const source = await placeboSource(["example"]);
  const cases = ["", join(directory, "missing"), source];
  for (const path of cases) {
    process.env.RB_SCORECARD_PLACEBO_SOURCE = path;
    process.env.RB_SCORECARD_FORBIDDEN = path === source ? "." : "";
    const instance = host();
    await instance.emit("session_start");
    expect(await instance.emit("before_agent_start", { prompt: "alpha" })).toBeUndefined();
    await instance.emit("tool_result", successfulTool);
    await instance.emit("session_shutdown");
  }
  process.env.RB_SCORECARD_FORBIDDEN = "";
  for (const invalid of ["{broken\n", `${JSON.stringify({ message: 12 })}\n`, " ".repeat(4 * 1024 * 1024 + 1)]) {
    await writeFile(join(source, "prompt-time.jsonl"), invalid);
    const instance = host();
    expect(await instance.emit("before_agent_start", { prompt: "alpha" })).toBeUndefined();
  }
  expect(await receiptMessages()).toEqual([]);
  expect(await fixtureRecords()).toEqual([]);
  expect(JSON.parse(await readFile(join(receipts, "control-error.json"), "utf8"))).toEqual({
    reason: "placebo receipt unavailable or prohibited",
  });
});

test("placebo exhaustion, factories, session switching, and branch counters stay independent", async () => {
  process.env.RB_SCORECARD_PLACEBO_SOURCE = await placeboSource(["one", "second"]);
  const first = host("session-a");
  const second = host("session-a");
  const firstMessage = await first.emit("before_agent_start", { prompt: "alpha" });
  expect(returnedMessage(firstMessage)).toBe("....");
  expect(returnedMessage(await first.emit("before_agent_start", { prompt: "beta" }))).toBe("........");
  expect(await first.emit("before_agent_start", { prompt: "exhausted" })).toBeUndefined();
  expect(returnedMessage(await second.emit("before_agent_start", { prompt: "alpha" }))).toBe("....");
  first.switchTo("session-b");
  expect(returnedMessage(await first.emit("before_agent_start", { prompt: "alpha" }))).toBe("....");
  first.switchTo("session-c");
  await first.emit("session_branch");
  expect(returnedMessage(await first.emit("before_agent_start", { prompt: "alpha" }))).toBe("....");
  await first.emit("session_tree");
  expect(returnedMessage(await first.emit("before_agent_start", { prompt: "beta" }))).toBe("........");
  first.switchTo("session-a");
  await first.emit("session_switch");
  expect(await first.emit("before_agent_start", { prompt: "still exhausted" })).toBeUndefined();
  expect(await receiptMessages()).toEqual(["....", "........", "....", "....", "....", "........"]);
  expect(await fixtureRecords()).toEqual([]);
});

test("factory receipt destinations do not leak through module-global configuration", async () => {
  const first = host();
  const secondDirectory = join(directory, "second-receipts");
  process.env.RB_SCORECARD_INJECTION_DIR = secondDirectory;
  const second = host();
  await first.emit("before_agent_start", { prompt: "alpha" });
  await second.emit("before_agent_start", { prompt: "beta" });
  expect(await receiptMessages()).toEqual(["Stored alpha decision"]);
  expect(await receiptMessages(secondDirectory)).toEqual(["Stored β decision 𐐀"]);
});

test("nonzero exit, malformed response, missing message, and missing executable fail open without receipts", async () => {
  const instance = host();
  for (const mode of ["nonzero", "malformed", "no-message"]) {
    expect(await instance.emit("before_agent_start", { prompt: `fixture/${mode}` })).toBeUndefined();
  }
  process.env.RB_OMP_HOOKS_BIN = join(directory, "not-installed");
  expect(await host().emit("before_agent_start", { prompt: "alpha" })).toBeUndefined();
  expect(await receiptMessages()).toEqual([]);
  expect(returnedMessage(await instance.emit("before_agent_start", { prompt: "alpha" }))).toBe("Stored alpha decision");
  expect(await receiptMessages()).toEqual(["Stored alpha decision"]);
});

test("oversized hook output and stdin fail open and the next request still succeeds", async () => {
  const instance = host();
  expect(await instance.emit("before_agent_start", { prompt: "fixture/oversize" })).toBeUndefined();
  expect(await instance.emit("before_agent_start", { prompt: "x".repeat(1024 * 1024) })).toBeUndefined();
  expect(returnedMessage(await instance.emit("before_agent_start", { prompt: "alpha" }))).toBe("Stored alpha decision");
  expect((await fixtureRecords()).map((record) => record.envelope?.prompt)).toEqual(["fixture/oversize", "alpha"]);
  expect(await receiptMessages()).toEqual(["Stored alpha decision"]);
});

test("a hook ignoring SIGTERM is killed by the deadline without blocking subsequent work", async () => {
  const instance = host();
  const started = performance.now();
  expect(await instance.emit("before_agent_start", { prompt: "fixture/timeout" })).toBeUndefined();
  expect(performance.now() - started).toBeLessThan(10_000);
  expect(returnedMessage(await instance.emit("before_agent_start", { prompt: "alpha" }))).toBe("Stored alpha decision");
  expect(await receiptMessages()).toEqual(["Stored alpha decision"]);
}, 15_000);

test("receipt I/O failure is distinguishable from absent recall without blocking injection", async () => {
  await mkdir(join(receipts, "prompt-time.jsonl"), { recursive: true });
  const instance = host();
  expect(returnedMessage(await instance.emit("before_agent_start", { prompt: "alpha" }))).toBe("Stored alpha decision");
  expect(JSON.parse(await readFile(join(receipts, "control-error.json"), "utf8"))).toEqual({ reason: "prompt receipt write failed" });
  await rm(join(receipts, "prompt-time.jsonl"), { recursive: true });
  expect(returnedMessage(await instance.emit("before_agent_start", { prompt: "beta" }))).toBe("Stored β decision 𐐀");
  expect(await receiptMessages()).toEqual(["Stored β decision 𐐀"]);
});

test("shutdown waits for actual capture, preserves hashline input and sends text-only inline transcript", async () => {
  const branch = [
    { type: "message", message: { role: "user", content: "Choose option B" } },
    { type: "message", message: { role: "assistant", content: [
      { type: "thinking", thinking: "private reasoning" }, { type: "text", text: "Option B selected" },
      { type: "toolCall", name: "write", arguments: { content: "tool secret" } },
      { type: "image", data: "image secret" },
    ] } },
    { type: "message", message: { role: "toolResult", content: "tool output secret" } },
    { type: "message", message: { role: "developer", content: "developer secret" } },
    { type: "custom_message", content: "injected memory secret" },
    { type: "message", message: { role: "custom", content: "custom secret" } },
  ];
  const instance = host("session-capture", branch);
  const patch = "*** Begin Patch\n[src/result.ts#ABCD]\nPUT 1.=1:\n+changed\n*** End Patch\n";
  const capture = instance.emit("tool_result", {
    toolName: "edit", input: { input: patch }, content: [{ type: "text", text: "edited" }], isError: false,
  });
  const shutdown = instance.emit("session_shutdown");
  await Promise.all([capture, shutdown]);
  const records = await fixtureRecords();
  expect(records.map((record) => record.phase)).toEqual(["received", "captured", "received"]);
  expect(records[0].envelope?.tool_input).toEqual({ input: patch });
  expect(records[2].envelope).toEqual({
    type: "session_shutdown", reason: "shutdown", cwd: directory, session_id: "session-capture",
    transcript_jsonl: [
      JSON.stringify({ message: { role: "user", content: "Choose option B" } }),
      JSON.stringify({ message: { role: "assistant", content: "Option B selected" } }), "",
    ].join("\n"),
  });
  expect((await readdir(directory)).sort()).toEqual(["fixture-hook.ts", "hook-events.jsonl"]);
  await instance.emit("tool_result", successfulTool);
  await instance.emit("session_shutdown");
  expect(await fixtureRecords()).toEqual(records);
});

test("large writes retain mutation paths and error captures retain bounded error text", async () => {
  const instance = host();
  await instance.emit("tool_result", {
    ...successfulTool,
    input: { path: "src/large.ts", content: "x".repeat(2 * 1024 * 1024) },
    content: [{ type: "text", text: "y".repeat(2 * 1024 * 1024) }],
  });
  await instance.emit("tool_result", {
    toolName: "bash", input: { command: "cargo check" }, isError: true,
    content: [{ type: "text", text: "missing import" }],
  });
  const envelopes = (await fixtureRecords()).filter((record) => record.phase === "received").map((record) => record.envelope);
  expect(envelopes[0]?.tool_input).toEqual({ path: "src/large.ts" });
  expect(envelopes[1]?.tool_input).toEqual({ command: "cargo check" });
  expect(envelopes[1]?.tool_response).toEqual({ is_error: true, content: "missing import" });
});

test("transcript cap counts escaped UTF-8 bytes and retains newest conversation as valid JSONL", async () => {
  const instance = host("bounded", [
    { type: "message", message: { role: "user", content: "old".repeat(200_000) } },
    { type: "message", message: { role: "assistant", content: "é𐐀\u0000\"".repeat(80_000) } },
    { type: "message", message: { role: "user", content: "Newest question" } },
  ]);
  await instance.emit("session_shutdown");
  const inline = (await fixtureRecords())[0].envelope?.transcript_jsonl;
  expect(typeof inline).toBe("string");
  const text = inline as string;
  expect(Buffer.byteLength(text)).toBeLessThanOrEqual(256 * 1024);
  const messages = text.trim().split("\n").map((line) => {
    const record: { message: { role: string; content: string } } = JSON.parse(line);
    return record.message;
  });
  expect(messages.map((message) => message.role)).toEqual(["assistant", "user"]);
  expect(messages[0].content).not.toContain("\ufffd");
  expect(messages[1].content).toBe("Newest question");
});

test("real stdin envelopes retain session identities and unavailable IDs never collide", async () => {
  const first = host(undefined);
  const second = host(null);
  // Undefined explicitly selects the function default, so remove the native getter too.
  Object.defineProperty(first.context.sessionManager, "getSessionId", { value: () => { throw new Error("not ready"); } });
  await first.emit("session_start");
  await second.emit("session_start");
  await first.emit("before_agent_start", { prompt: "alpha" });
  const switching = host("session/a");
  await switching.emit("session_start");
  switching.switchTo("session_a");
  await switching.emit("session_switch");
  await switching.emit("before_agent_start", { prompt: "beta" });
  const ids = (await fixtureRecords()).map((record) => record.envelope?.session_id);
  expect(typeof ids[0]).toBe("string");
  expect(ids[0]).not.toBe(ids[1]);
  expect(ids[2]).toBe(ids[0]);
  expect(ids.slice(3)).toEqual(["session/a", "session_a", "session_a"]);
});

test("cancel-safe transition checkpoints use the old session and drain prior captures without closing it", async () => {
  const instance = host("old-session", [
    { type: "message", message: { role: "user", content: "Old conversation" } },
  ]);
  const capture = instance.emit("tool_result", successfulTool);
  const checkpoint = instance.emit("session_before_switch");
  await Promise.all([capture, checkpoint]);
  // The host may cancel the switch after this handler returns; old work must remain accepted.
  await instance.emit("tool_result", successfulTool);
  await instance.emit("session_before_branch");
  instance.switchTo("new-session", [
    { type: "message", message: { role: "user", content: "New conversation" } },
  ]);
  await instance.emit("session_branch");
  await instance.emit("session_shutdown");
  const records = await fixtureRecords();
  expect(records.map((record) => record.phase === "captured" ? "captured" : record.envelope?.type)).toEqual([
    "tool_result", "captured", "session_checkpoint", "tool_result", "captured", "session_checkpoint",
    "session_start", "session_shutdown",
  ]);
  expect(records[2].envelope?.session_id).toBe("old-session");
  expect(records[2].envelope?.transcript_jsonl).toBe(`${JSON.stringify({ message: { role: "user", content: "Old conversation" } })}\n`);
  expect(records[7].envelope?.session_id).toBe("new-session");
  expect(records[7].envelope?.transcript_jsonl).toBe(`${JSON.stringify({ message: { role: "user", content: "New conversation" } })}\n`);
});

test("malformed tool inputs and throwing host getters do not poison later captures", async () => {
  const instance = host();
  await instance.emit("tool_result", { ...successfulTool, input: undefined });
  await instance.emit("tool_result", {
    ...successfulTool,
    input: Object.defineProperty({}, "path", { get() { throw new Error("disposed input"); } }),
  });
  Object.defineProperty(instance.context, "cwd", {
    configurable: true, get() { throw new Error("disposed context"); },
  });
  await instance.emit("tool_result", successfulTool);
  Object.defineProperty(instance.context, "cwd", { configurable: true, value: directory });
  await instance.emit("tool_result", successfulTool);
  const records = await fixtureRecords();
  expect(records.map((record) => record.phase)).toEqual(["received", "captured"]);
  expect(records[0].envelope?.tool_input).toEqual({ path: "src/result.ts" });
});

test("shutdown drains pending captures from a previously active session without mixing identities", async () => {
  const instance = host("capturing-session");
  const capture = instance.emit("tool_result", successfulTool);
  instance.switchTo("closing-session");
  const shutdown = instance.emit("session_shutdown");
  await Promise.all([capture, shutdown]);
  const records = await fixtureRecords();
  expect(records.map((record) => record.phase)).toEqual(["received", "captured", "received"]);
  expect(records[0].envelope?.session_id).toBe("capturing-session");
  expect(records[2].envelope?.session_id).toBe("closing-session");
  expect(records[2].envelope?.type).toBe("session_shutdown");
});
