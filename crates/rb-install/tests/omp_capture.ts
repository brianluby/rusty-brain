// Run: bun crates/rb-install/tests/omp_capture.ts target/debug/rusty-brain target/debug/rusty-brain-hooks
// Controlled native branch fixture; persistence, hook processes and recall are real.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { resolve } from "node:path";
import { Database } from "bun:sqlite";
import extension from "../assets/omp-extension.ts";

type Note = { id: string; content: string; context: string; tags: string[]; trust_class: string; session_id: string | null };
const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null;
const note = (value: unknown): value is Note => object(value)
  && typeof value.id === "string" && typeof value.content === "string"
  && typeof value.context === "string" && typeof value.trust_class === "string"
  && (value.session_id === null || typeof value.session_id === "string")
  && Array.isArray(value.tags) && value.tags.every((tag: unknown) => typeof tag === "string");
function memories(value: unknown): Note[] {
  assert.ok(object(value) && Array.isArray(value.memories) && value.memories.every(note), "valid export schema");
  return value.memories;
}

const cli = resolve(process.argv[2]);
const hooks = resolve(process.argv[3]);
const root = await mkdtemp("/tmp/rb-omp-capture-");
const env = {
  PATH: process.env.PATH ?? "",
  HOME: root, XDG_CONFIG_HOME: root, XDG_DATA_HOME: root, XDG_CACHE_HOME: root,
  XDG_STATE_HOME: root, XDG_RUNTIME_DIR: root, TMPDIR: root,
  RUSTY_BRAIN_SOCKET: `${root}/rb.sock`, RUSTY_BRAIN_DB: `${root}/rb.db`,
  RUSTY_BRAIN_NAMESPACE: "omp-capture-self-check", RB_OMP_HOOKS_BIN: hooks,
};
for (const key of Object.keys(process.env)) delete process.env[key];
Object.assign(process.env, env);
const run = async (...args: string[]) => {
  const child = Bun.spawn([cli, ...args], { cwd: root, env, stdout: "pipe", stderr: "pipe" });
  const [out, err, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  assert.equal(code, 0, err);
  return JSON.parse(out) as unknown;
};
const daemon = Bun.spawn([cli, "serve"], { cwd: root, env, stdout: "ignore", stderr: "ignore" });
try {
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    try { await run("--json", "status"); ready = true; }
    catch { await Bun.sleep(50); }
  }
  assert.ok(ready, "isolated daemon must become available");
  type Content = string | { type: string; text?: string }[];
  type Entry = { type: "message"; id: string; message: { role: string; content: Content } };
  const entry = (id: string, role: string, content: Content): Entry => ({ type: "message", id, message: { role, content } });
  const qualification = " only in tests; production uses PostgreSQL.";
  const partialDecision = "Decision: storage: use SQLite";
  const multiblock = [
    ...Array.from({ length: 199 }, () => ({ type: "text", text: "Background context. ".repeat(17).slice(0, 326) })),
    { type: "text", text: partialDecision + " ".repeat(326 - partialDecision.length - qualification.length) + qualification },
  ];
  const branch = [
    entry("initial-goal", "user", multiblock),
    ...Array.from({ length: 5000 }, (_, i) => entry(`activity-${i}`, "assistant", "Investigating unrelated activity. ".repeat(40))),
    entry("unadopted-proposal", "assistant", "Decision: storage: Use PostgreSQL ORCHID-PROPOSAL."),
    entry("confirmed-storage", "user", "Decision: storage: Use SQLite WAL ORCHID-OMP-7311."),
    entry("confirmed-logging", "user", [
      { type: "text", text: "Background: logging policy follows." },
      { type: "text", text: "" },
      { type: "image", text: "Decision: storage: use MySQL." },
      { type: "text", text: "Constraint: logging: Payment card numbers are excluded from logs." },
    ]),
  ];
  const context = { cwd: root, sessionManager: { getSessionId: () => "omp-bounded-source", getBranch: () => branch } };
  type Handler = (event: unknown, context: unknown) => Promise<unknown>;
  const handlers = new Map<string, Handler>();
  // The fixture supplies only the native registration surface consumed by this asset.
  const api = { on: (name: string, handler: Handler) => handlers.set(name, handler) } as unknown as Parameters<typeof extension>[0];
  extension(api);
  const event = async (name: string, payload = {}) => {
    const handler = handlers.get(name);
    assert.ok(handler, `native event ${name} must be registered`);
    return handler(payload, context);
  };
  await event("session_before_switch");
  const first = memories(await run("export", "--format", "json"));
  const facts = first.filter(note => note.tags.includes("confirmed-decision"));
  assert.equal(facts.length, 2);
  assert.ok(facts.every(note => note.trust_class === "agent_attested"));
  assert.ok(facts.some(note => note.content.includes("ORCHID-OMP-7311") && note.context.includes("message=confirmed-storage")));
  assert.ok(facts.some(note => note.context.includes("message=confirmed-logging")));
  assert.ok(facts.every(note => note.context.includes("branch_entry") && note.context.includes("incomplete=true")));
  assert.ok(facts.every(note => note.context.includes("skipped_bytes=unknown") && note.context.includes("skipped_lines=unknown")),
    "adapter omissions cannot be reported as zero bytes or zero records");
  const boundedSummary = first.find(note => note.tags.includes("session-summary"));
  assert.ok(boundedSummary?.content.includes("skipped_bytes=unknown") && boundedSummary.content.includes("skipped_lines=unknown"));
  assert.ok(!facts.some(note => note.content.includes("ORCHID-PROPOSAL")));
  assert.ok(!facts.some(note => note.context.includes("message=initial-goal")),
    "the 200-block clipped SQLite statement must not become a confirmed policy");
  assert.ok(!facts.some(note => note.content.includes("use MySQL")),
    "non-text blocks must not become confirmed policies");
  const preparation = await event("before_agent_start", { prompt: "SQLite WAL ORCHID-OMP-7311" });
  assert.ok(object(preparation) && object(preparation.message));
  assert.ok(typeof preparation.message.content === "string" && preparation.message.content.includes("ORCHID-OMP-7311"));
  assert.equal(preparation.message.display, false);
  await event("session_shutdown");
  const repeated = memories(await run("export", "--format", "json")).filter(note => note.tags.includes("confirmed-decision"));
  assert.deepEqual(repeated.map(note => note.id).sort(), facts.map(note => note.id).sort());
  const recall = await run("--json", "recall", "SQLite WAL ORCHID-OMP-7311", "--tags", "confirmed-decision");
  const storedDecision = facts.find(note => note.content.includes("ORCHID-OMP-7311"));
  assert.ok(storedDecision);
  assert.ok(Array.isArray(recall) && recall.some((hit: unknown) => object(hit) && note(hit.memory) && hit.memory.id === storedDecision.id));
  const checkpoint = async (session: string, entries: Entry[]) => {
    const child = Bun.spawn([hooks, "--agent", "omp"], { cwd: root, env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    child.stdin.write(JSON.stringify({
      type: "session_checkpoint", session_id: session, cwd: root,
      transcript_jsonl: entries.map(value => JSON.stringify(value)).join("\n"),
    }));
    child.stdin.end();
    const [out, err, code] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    assert.equal(code, 0, err);
    const response: unknown = JSON.parse(out);
    assert.ok(object(response) && response.continue === true);
    return memories(await run("export", "--format", "json"))
      .filter(note => note.tags.includes("confirmed-decision") && note.session_id === session);
  };
  const window = [
    entry("window-pg", "user", "Decision: storage: use PostgreSQL CORRECTION-WINDOW"),
    entry("window-mysql", "user", "Decision: storage: use MySQL CORRECTION-WINDOW"),
    entry("window-sqlite", "user", "Correction: storage: use SQLite CORRECTION-WINDOW"),
  ];
  const windowFacts = await checkpoint("correction-window", window);
  assert.deepEqual(windowFacts.map(note => note.content), ["Correction: storage: use SQLite CORRECTION-WINDOW"]);
  assert.deepEqual((await checkpoint("correction-window", window)).map(note => note.id), windowFacts.map(note => note.id));
  assert.deepEqual((await checkpoint("correction-window", window.slice(0, 2))).map(note => note.id), windowFacts.map(note => note.id),
    "old head evidence must not return when its stored correction leaves the next window");

  const auth = [
    entry("auth-old", "user", "Decision: auth: allow anonymous requests"),
    entry("auth-correction", "user", "Correction: auth: reject anonymous requests"),
  ];
  assert.deepEqual((await checkpoint("rejection-policy", auth.slice(0, 1))).map(note => note.content),
    ["Decision: auth: allow anonymous requests"]);
  assert.deepEqual((await checkpoint("rejection-policy", auth)).map(note => note.content),
    ["Correction: auth: reject anonymous requests"],
    "a rejection action must retire the earlier policy, not become an unconfirmed proposal");
  assert.deepEqual((await checkpoint("labeled-rejection-policy", [
    entry("review-constraint", "user", "Constraint: review: reject proposals that include secrets"),
  ])).map(note => note.content), ["Constraint: review: reject proposals that include secrets"],
    "labeled rejection policy remains confirmed rather than becoming a rejection of an assistant proposal");
  const deployment = [
    entry("deployment-old", "user", "Decision: deployment: use rolling releases"),
    entry("deployment-correction", "user", "Correction: deployment: use the proposed blue/green rollout"),
  ];
  assert.deepEqual((await checkpoint("proposed-policy", deployment.slice(0, 1))).map(note => note.content),
    ["Decision: deployment: use rolling releases"]);
  assert.deepEqual((await checkpoint("proposed-policy", deployment)).map(note => note.content),
    ["Correction: deployment: use the proposed blue/green rollout"],
    "proposal terminology inside a labeled correction must not suppress the replacement");
  const adopted = await checkpoint("explicit-proposal", [
    entry("storage-proposal", "assistant", "Proposal: storage: use SQLite only in tests; production uses PostgreSQL"),
    entry("storage-adoption", "user", "Adopt proposal: storage"),
  ]);
  assert.deepEqual(adopted.map(note => note.content),
    ["Adopt proposal: storage\nAdopted proposal: Proposal: storage: use SQLite only in tests; production uses PostgreSQL"],
    "explicit proposal adoption must retain the complete qualification");
  const taskInstructions = [
    "Use ripgrep to find the failing call",
    "Use cargo test to verify it",
    "Adopt the patch for this task",
    "Fix the parser; it must not panic on empty input",
    "Actually, use MySQL for another component",
  ].map((text, index) => entry(`task-${index}`, "user", text));
  assert.deepEqual(await checkpoint("one-off-task", taskInstructions), [],
    "one-off user task instructions must not install critical standing policy");
  const taskSummary = memories(await run("export", "--format", "json"))
    .find(note => note.session_id === "one-off-task" && note.tags.includes("session-summary"));
  assert.ok(taskSummary?.content.includes("Use ripgrep to find the failing call"),
    "task prompts remain activity context, not confirmed policy");

  const ledger = [
    entry("ledger-pg", "user", "Decision: storage: use PostgreSQL CORRECTION-LEDGER"),
    entry("ledger-mysql", "user", "Decision: storage: use MySQL CORRECTION-LEDGER"),
    entry("ledger-sqlite", "user", "Correction: storage: use SQLite CORRECTION-LEDGER"),
  ];
  const oldFacts = await checkpoint("correction-ledger", ledger.slice(0, 2));
  assert.deepEqual(oldFacts.map(note => note.content).sort(), ledger.slice(0, 2).map(value => value.message.content).sort());
  const db = new Database(`${root}/rb.db`);
  try {
    const redacted = [
      entry("redacted-old", "user", "Decision: password=hunter2: use PostgreSQL"),
      entry("redacted-new", "user", "Correction: password=hunter2: use SQLite"),
    ];
    const beforeRedaction = await checkpoint("redacted-subject", redacted.slice(0, 1));
    assert.equal(beforeRedaction.length, 1);
    const afterRedaction = await checkpoint("redacted-subject", redacted);
    assert.equal(afterRedaction.length, 1);
    assert.ok(afterRedaction[0].content.includes("use SQLite") && !afterRedaction[0].content.includes("hunter2"));
    const lineage: unknown = db.query("SELECT superseded_by, content FROM memories WHERE memory_id = ?").get(beforeRedaction[0].id);
    assert.ok(object(lineage) && lineage.superseded_by === afterRedaction[0].id,
      "redacted-subject correction must retain supersedes lineage, not merely archive old policy");
    assert.ok(object(lineage) && typeof lineage.content === "string" && !lineage.content.includes("hunter2"));
    assert.deepEqual((await checkpoint("redacted-subject", redacted.slice(0, 1))).map(note => note.id),
      afterRedaction.map(note => note.id),
      "redacted identity must also prevent old head evidence from returning after correction omission");
    // Real SQLite write failures prove old policy survives until its correction is durable.
    db.exec(`CREATE TRIGGER reject_correction BEFORE INSERT ON memories
      WHEN NEW.content = 'Correction: storage: use SQLite CORRECTION-LEDGER'
      BEGIN SELECT RAISE(FAIL, 'injected correction failure'); END;`);
    assert.deepEqual((await checkpoint("correction-ledger", ledger)).map(note => note.id).sort(), oldFacts.map(note => note.id).sort());
    db.exec("DROP TRIGGER reject_correction");
    db.exec(`CREATE TRIGGER reject_retirement BEFORE UPDATE OF archived_at ON memories
      WHEN OLD.content = 'Decision: storage: use PostgreSQL CORRECTION-LEDGER'
        AND OLD.archived_at IS NULL AND NEW.archived_at IS NOT NULL
      BEGIN SELECT RAISE(FAIL, 'injected archive failure'); END;`);
    const partial = await checkpoint("correction-ledger", ledger);
    assert.deepEqual(partial.map(note => note.content).sort(), [
      "Correction: storage: use SQLite CORRECTION-LEDGER", "Decision: storage: use PostgreSQL CORRECTION-LEDGER",
    ]);
    const correction = partial.find(note => note.content.startsWith("Correction:"));
    assert.ok(correction);
    db.exec("DROP TRIGGER reject_retirement");
    const recovered = await checkpoint("correction-ledger", ledger.slice(0, 2));
    assert.deepEqual(recovered.map(note => note.id), [correction.id],
      "stored correction must finish failed retirement even after leaving the transcript window");
    assert.deepEqual((await checkpoint("correction-ledger", ledger)).map(note => note.id), [correction.id]);
  } finally {
    db.close();
  }
  console.log(JSON.stringify({ branch_entries: branch.length, confirmed_facts: facts.length, source_ids_preserved: true, incomplete_window_reported: true, repeated_fold_ids_preserved: true, prompt_injection_and_recall: "passed", correction_filtering_and_failure_recovery: "passed", rejection_policy_and_explicit_proposal_adoption: "passed", one_off_task_policy_exclusion: "passed" }));
} finally {
  daemon.kill();
  await daemon.exited;
  await rm(root, { recursive: true, force: true });
}
