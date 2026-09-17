#!/usr/bin/env python3
"""Scorecard-only hook instrumentation. No external packages or model calls.

Counts are ceil(UTF-8 bytes / 4), separately per emitted context string. This
is a rough estimator, NOT rb-tokens BPE or the agent model's tokenizer.
"""
import json
from pathlib import Path
import shlex
import subprocess
import sys

ESTIMATOR = "utf8-bytes-div4-ceil-v1"
CHANNELS = {"SessionStart": "session-start", "UserPromptSubmit": "prompt-time"}


def estimate(text):
    return (len(text.encode("utf-8")) + 3) // 4


def contexts(payload):
    # Claude Code feeds only additionalContext to the model; systemMessage is
    # UI-only (see rb-agents/src/claude_code.rs render_output).
    return [payload.get("hookSpecificOutput", {}).get("additionalContext", "")]


def tokens(payload):
    return sum(estimate(text) for text in contexts(payload))


def read_records(directory, channel):
    path = Path(directory) / (channel + ".jsonl")
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def neutral(text, forbidden):
    # Pure punctuation: no facts, service names, IDs, decisions or instructions.
    # Use exact estimator-size padding; never copy any part of the source text.
    size = estimate(text)
    if not size:
        return ""
    for char in (".", "~", "_", "-", ":"):
        padding = (char + " ") * (size * 2)
        if not any(token and token.casefold() in padding.casefold() for token in forbidden):
            return padding
    raise ValueError("cannot construct a leak-free neutral placebo for these tokens")


def placebo_payload(payload, forbidden):
    result = {key: payload[key] for key in ("continue", "suppressOutput") if key in payload}
    # Do not replay the UI-only systemMessage, which is not a model channel.
    if "hookSpecificOutput" in payload:
        h = payload["hookSpecificOutput"]
        result["hookSpecificOutput"] = {"hookEventName": h["hookEventName"]}
        if "additionalContext" in h:
            result["hookSpecificOutput"]["additionalContext"] = neutral(h["additionalContext"], forbidden)
    return result


def install(project, directory, mode, source=None, forbidden=()):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    settings = Path(project) / ".claude/settings.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    config = json.loads(settings.read_text()) if settings.exists() else {}
    commands = {}
    for event, channel in CHANNELS.items():
        if mode == "record":
            hooks = config["hooks"][event]
            if len(hooks) != 1 or len(hooks[0]["hooks"]) != 1 or hooks[0]["hooks"][0]["type"] != "command":
                raise ValueError("expected one installed command hook per injection channel")
            commands[event] = hooks[0]["hooks"][0]["command"]
        else:
            payloads = [placebo_payload(r, forbidden) for r in read_records(source, channel)]
            (directory / (channel + "-templates.json")).write_text(json.dumps(payloads))
        command = shlex.join([sys.executable, str(Path(__file__).resolve()), mode, str(directory), event])
        config.setdefault("hooks", {})[event] = [{"hooks": [{"type": "command", "command": command}]}]
    (directory / "commands.json").write_text(json.dumps(commands))
    settings.write_text(json.dumps(config, indent=2))


def emit(mode, directory, event):
    payload_in = sys.stdin.read()
    channel = CHANNELS[event]
    if mode == "record":
        command = json.loads((Path(directory) / "commands.json").read_text())[event]
        result = subprocess.run(command, shell=True, input=payload_in, text=True, capture_output=True)
        if result.returncode:
            raise ValueError("memory hook failed: " + result.stderr)
        payload = json.loads(result.stdout) if result.stdout.strip() else {}
        output = result.stdout
    else:
        index = len(read_records(directory, channel))
        templates = json.loads((Path(directory) / (channel + "-templates.json")).read_text())
        if index >= len(templates):
            raise ValueError("placebo hook invocation exceeds memory-on channel count")
        payload = templates[index]
        output = json.dumps(payload) + "\n"
    # Write a receipt for exactly the context emitted by this invocation, not
    # the diagnostic probe or total provider input/cache usage.
    with (Path(directory) / (channel + ".jsonl")).open("a") as f:
        f.write(json.dumps(payload) + "\n")
    sys.stdout.write(output)


def main():
    action, *args = sys.argv[1:]
    if action == "install-record":
        install(*args, mode="record")
    elif action == "install-placebo":
        project, directory, source, *forbidden = args
        install(project, directory, "replay", source, forbidden)
    elif action == "file-tokens":
        path = Path(args[0])
        print(estimate(path.read_text()) if path.exists() else 0)
    elif action == "metrics":
        directory, file_tokens = args
        counts = [sum(tokens(r) for r in read_records(directory, channel)) for channel in CHANNELS.values()]
        counts.append(int(file_tokens))
        print("\t".join(map(str, counts + [sum(counts)])) + "\t" + ESTIMATOR)
    elif action == "validate-pair":
        source, placebo = args
        for directory in (source, placebo):
            if (Path(directory) / "error.json").exists():
                raise SystemExit("control hook error: " + str(Path(directory) / "error.json"))
        sizes = {}
        for event, channel in CHANNELS.items():
            a = [[estimate(s) for s in contexts(r)] for r in read_records(source, channel)]
            b = [[estimate(s) for s in contexts(r)] for r in read_records(placebo, channel)]
            if not a or a != b:
                raise SystemExit(f"control channel mismatch: {event}: memory-on={a}, placebo={b}")
            sizes[event] = a
        (Path(placebo) / "pair-check.json").write_text(json.dumps({
            "matched": True, "estimator": ESTIMATOR, "exact_model_tokens": False,
            "per_invocation_field_sizes": sizes}, indent=2) + "\n")
    elif action == "metadata":
        metadata = {
            "schema_version": 2,
            "scoring": "legacy-substring-proxy",
            "hard_gate": "legacy zero memory-induced-error proxy (unchanged)",
            "assertion_fixture": {"uses_model_judge": False, "executed_by_this_run": False,
                                  "coverage_equivalent": False,
                                  "documentation": "docs/eval/assertion-precision.md"},
            "injected_tokens": {"estimator": ESTIMATOR, "exact_model_tokens": False,
                                "scope": "emitted SessionStart + UserPromptSubmit context strings and seeded CLAUDE.md",
                                "excludes": "provider framing, prompts, MCP/tool traffic, auto-capture plant sessions",
                                "aggregation": "per-arm arithmetic mean of per-run sizes; unknown for legacy TSV"},
            "arms": ["memory-on", "realistic-baseline", "steelman-baseline", "length-matched-placebo", "memory-off"],
            "tsv_appended_columns": ["injected_session_start_est", "injected_prompt_time_est",
                                     "injected_claude_md_est", "injected_total_est", "injected_estimator"],
        }
        Path(args[0]).write_text(json.dumps(metadata, indent=2) + "\n")
    elif action in ("record", "replay"):
        try:
            emit(action, *args)
        except Exception as error:
            # Claude hooks fail open. Keep an independent harness-owned marker
            # so a successful agent result cannot conceal a broken control.
            (Path(args[0]) / "error.json").write_text(json.dumps({
                "channel": args[1], "error_type": type(error).__name__}))
            raise
    else:
        raise ValueError("unknown action: " + action)


if __name__ == "__main__":
    main()
