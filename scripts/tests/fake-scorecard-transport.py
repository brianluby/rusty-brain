#!/usr/bin/env python3
"""Offline test double ONLY: never claims to run a model or a real memory daemon."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys

name = Path(sys.argv[0]).name
args = sys.argv[1:]
if name == "rusty-brain":
    if "serve" in args:
        sock = socket.socket(socket.AF_UNIX)
        sock.bind(os.environ["RUSTY_BRAIN_SOCKET"])
        signal.pause()
    elif "remember" in args:
        print(json.dumps({"id": "00000000-0000-0000-0000-000000000001"}))
    elif "list" in args:
        print(json.dumps([{"content": "use CURRENT_CHOICE", "tags": ["session-summary"],
                           "origin_source": "hook", "archived_at": None}]))
    elif "recall" in args:
        print("[]")
    else:
        print("{}")
elif name == "rusty-brain-install":
    p = Path(".claude/settings.json")
    p.parent.mkdir(exist_ok=True)
    p.write_text(json.dumps({"hooks": {event: [{"hooks": [{"type": "command", "command":
        "rusty-brain-hooks --agent claude-code"}]}] for event in ("SessionStart", "UserPromptSubmit")}}))
elif name == "rusty-brain-hooks":
    event = json.load(sys.stdin)
    # Different lengths/channels, unicode, and different diagnostics vs actual
    # prove that measuring the preflight probe is NOT enough.
    live = event["session_id"] != "scorecard-diagnostic"
    text = ("Current decision CURRENT_CHOICE; superseded OLD_CHOICE. λ\n" if live else "probe only")
    if "-r2/" in event.get("cwd", ""):
        text *= 2
    if os.environ.get("FAKE_EMPTY_CHANNEL") == event["hook_event_name"]:
        print("{}")
    else:
        output = {"continue": True, "suppressOutput": True, "hookSpecificOutput": {
            "hookEventName": event["hook_event_name"],
            "additionalContext": text * (2 if event["hook_event_name"] == "SessionStart" else 1)}}
        if os.environ.get("FAKE_UI_MESSAGE"):
            output["systemMessage"] = "user-facing only, NOT model context " * 10
        print(json.dumps(output))
elif name == "claude":
    settings = Path(".claude/settings.json")
    config = json.loads(settings.read_text()) if settings.exists() else {}
    injections = {}
    for event in ("SessionStart", "UserPromptSubmit"):
        outputs = []
        if os.environ.get("FAKE_SKIP_PLACEBO_PROMPT") and "/placebo/" in str(Path.cwd()) and event == "UserPromptSubmit":
            continue
        for group in config.get("hooks", {}).get(event, []):
            for hook in group["hooks"]:
                payload = {"hook_event_name": event, "session_id": "fake-work", "cwd": str(Path.cwd()), "prompt": args[1]}
                result = subprocess.run(hook["command"], shell=True, input=json.dumps(payload), text=True, capture_output=True)
                if result.returncode:
                    print(result.stderr, file=sys.stderr)
                    sys.exit(result.returncode)
                outputs.append(json.loads(result.stdout) if result.stdout.strip() else {})
                if os.environ.get("FAKE_EXTRA_PLACEBO_HOOK") and "/placebo/" in str(Path.cwd()):
                    # Real agents can fail open on a hook error. Instrumentation
                    # must notice even if the agent reports session success.
                    subprocess.run(hook["command"], shell=True, input=json.dumps(payload), text=True, capture_output=True)
        injections[event] = outputs
    md = Path("CLAUDE.md")
    context = json.dumps(injections) + (md.read_text() if md.exists() else "")
    with open(os.environ["FAKE_TRACE"], "a") as f:
        f.write(json.dumps({"project": str(Path.cwd()), "hooks": config.get("hooks", {}),
                            "injections": injections, "claude_md": md.exists()}) + "\n")
    answer = "CURRENT_CHOICE" if "CURRENT_CHOICE" in context else "no evidence"
    if os.environ.get("FAKE_STALE_OUTPUT") and "/on/" in str(Path.cwd()):
        answer = "OLD_CHOICE"
    print(json.dumps({"type": "result", "is_error": False, "num_turns": 1,
                      "total_cost_usd": 0.01, "usage": {}, "result": answer}))
else:
    raise SystemExit("unknown test transport name: " + name)
