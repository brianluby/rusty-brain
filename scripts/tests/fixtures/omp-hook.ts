import { appendFile, realpath } from "node:fs/promises";

const envelope = JSON.parse(await Bun.stdin.text()) as Record<string, unknown>;
if (process.argv.slice(2).join(" ") !== "--agent omp"
  || typeof envelope.cwd !== "string"
  || await realpath(envelope.cwd) !== await realpath(process.cwd())) process.exit(41);
const log = process.env.RB_OMP_FIXTURE_LOG;
if (!log) process.exit(42);
await appendFile(log, `${JSON.stringify({ phase: "received", envelope })}\n`);
const prompt = envelope.prompt;
if (prompt === "fixture/nonzero") {
  console.log(JSON.stringify({ message: "must not inject failed output" }));
  process.exit(7);
}
if (prompt === "fixture/malformed") {
  console.log("{not valid JSON");
  process.exit(0);
}
if (prompt === "fixture/no-message") {
  console.log(JSON.stringify({ message: 12 }));
  process.exit(0);
}
if (prompt === "fixture/oversize") {
  console.log(JSON.stringify({ message: "x".repeat(512 * 1024) }));
  process.exit(0);
}
if (prompt === "fixture/timeout") {
  process.on("SIGTERM", () => undefined);
  await Bun.sleep(30_000);
  console.log(JSON.stringify({ message: "must not inject late output" }));
  process.exit(0);
}
if (envelope.type === "tool_result") {
  await Bun.sleep(40);
  await appendFile(log, `${JSON.stringify({ phase: "captured", session_id: envelope.session_id })}\n`);
}
if (envelope.type === "prompt") {
  if (prompt === "alpha") await Bun.sleep(40);
  const message = prompt === "alpha" ? "Stored alpha decision" : "Stored β decision 𐐀";
  console.log(JSON.stringify({ message }));
} else {
  console.log("{}");
}
