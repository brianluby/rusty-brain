#!/usr/bin/env python3
"""OMP scorecard prompt-time receipt accounting.

The native OMP extension records the custom message returned by each
before_agent_start preparation attempt (not a provider delivery receipt).
This module measures those receipts and rejects unmatched placebos.
"""
import hashlib
import json
from pathlib import Path
import sys

ESTIMATOR = "utf8-bytes-div4-ceil-v1"
CHANNEL = "prompt-time"


def estimate(text):
    return (len(text.encode("utf-8")) + 3) // 4


def records(directory):
    path = Path(directory) / f"{CHANNEL}.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def messages(directory):
    return [record["message"] for record in records(directory)
            if isinstance(record.get("message"), str)]


def main():
    if len(sys.argv) < 2:
        print("usage: scorecard-controls.py "
              "{file-tokens|metrics|validate-pair|metadata} ...", file=sys.stderr)
        raise SystemExit(2)
    action, *args = sys.argv[1:]
    if action == "file-tokens":
        path = Path(args[0])
        print(estimate(path.read_text()) if path.exists() else 0)
    elif action == "metrics":
        directory, file_tokens = args
        prompt_tokens = sum(estimate(message) for message in messages(directory))
        print(f"0\t{prompt_tokens}\t{int(file_tokens)}\t{prompt_tokens + int(file_tokens)}\t{ESTIMATOR}")
    elif action == "validate-pair":
        source, placebo = args
        source_error = Path(source) / "control-error.json"
        placebo_error = Path(placebo) / "control-error.json"
        source_sizes = [estimate(message) for message in messages(source)]
        placebo_sizes = [estimate(message) for message in messages(placebo)]
        if (
            source_error.exists()
            or placebo_error.exists()
            or not source_sizes
            or source_sizes != placebo_sizes
        ):
            raise SystemExit(
                f"control channel mismatch: OMP before_agent_start: "
                f"memory-on={source_sizes}, placebo={placebo_sizes}, "
                f"source_error={source_error.exists()}, placebo_error={placebo_error.exists()}"
            )
        (Path(placebo) / "pair-check.json").write_text(json.dumps({
            "matched": True,
            "channel": "before_agent_start",
            "estimator": ESTIMATOR,
            "exact_model_tokens": False,
            "per_invocation_field_sizes": source_sizes,
        }, indent=2) + "\n")
    elif action == "metadata":
        destination, model, version, extension_path = args
        metadata = {
            "schema_version": 3,
            "agent": "omp",
            "model_selector": model,
            "omp_version": version,
            "extension_sha256": hashlib.sha256(Path(extension_path).read_bytes()).hexdigest(),
            "resolved_model_source": "assistant provider/model fields in each retained work.jsonl",
            "scoring": "legacy-substring-proxy",
            "hard_gate": "legacy zero memory-induced-error proxy (unchanged)",
            "assertion_fixture": {
                "uses_model_judge": False,
                "executed_by_this_run": False,
                "coverage_equivalent": False,
                "documentation": "docs/eval/assertion-precision.md",
            },
            "injected_tokens": {
                "estimator": ESTIMATOR,
                "exact_model_tokens": False,
                "scope": "OMP before_agent_start custom messages and seeded AGENTS.md",
                "excludes": "provider framing, prompts, tool traffic, and auto-capture plant sessions",
                "aggregation": "per-arm arithmetic mean of per-run sizes; unknown for legacy TSV",
            },
            "arms": ["memory-on", "realistic-baseline", "steelman-baseline", "length-matched-placebo", "memory-off"],
            "tsv_appended_columns": [
                "injected_session_start_est",
                "injected_prompt_time_est",
                "injected_agents_md_est",
                "injected_total_est",
                "injected_estimator",
            ],
        }
        Path(destination).write_text(json.dumps(metadata, indent=2) + "\n")
    else:
        raise ValueError("unknown action: " + action)


if __name__ == "__main__":
    main()
