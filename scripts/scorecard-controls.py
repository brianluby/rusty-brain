#!/usr/bin/env python3
"""OMP scorecard control accounting and exact assertion validation.

The native OMP extension records each before_agent_start custom-message
preparation attempt (not a provider delivery receipt). This module measures
those receipts, rejects unmatched placebos, and independently verifies the
fixed no-judge exact-ID report used by the hard gate.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

ESTIMATOR = "utf8-bytes-div4-ceil-v1"
CHANNEL = "prompt-time"
REQUIRED_ASSERTION_CASES = {
    "supersede-chain",
    "five-slot-budget",
    "namespace-selection",
}


def estimate(text):
    return (len(text.encode("utf-8")) + 3) // 4


def records(directory):
    path = Path(directory) / f"{CHANNEL}.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def messages(directory):
    return [record["message"] for record in records(directory)
            if isinstance(record.get("message"), str)]


def stale_evidence(directory, stale):
    """Return whether a stale marker appears in an actual prompt-time receipt."""
    if not stale:
        return "na", "no_stale_token"
    directory = Path(directory)
    if (directory / "control-error.json").exists():
        return "unknown", "control_error"
    receipt = directory / f"{CHANNEL}.jsonl"
    if not receipt.exists():
        return "unknown", "missing_prompt_receipt"
    try:
        rows = [
            json.loads(line)
            for line in receipt.read_text().splitlines()
            if line.strip()
        ]
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        return "unknown", "malformed_prompt_receipt"
    if not rows:
        return "unknown", "empty_prompt_receipt"
    if any(
        not isinstance(row, dict) or not isinstance(row.get("message"), str)
        for row in rows
    ):
        return "unknown", "malformed_prompt_receipt"
    needle = stale.casefold()
    present = any(needle in row["message"].casefold() for row in rows)
    return ("1", "stale_injected") if present else ("0", "stale_not_injected")


def causal_attribution(on_rows, off_rows):
    """Attribute one scenario/run pair without treating correlation as proof."""
    if len(on_rows) != 1:
        return 0, "unassessable", (
            "missing_memory_on_pair" if not on_rows else "duplicate_memory_on_pair"
        )
    if len(off_rows) != 1:
        return 0, "unassessable", (
            "missing_memory_off_pair" if not off_rows else "duplicate_memory_off_pair"
        )
    on, off = on_rows[0], off_rows[0]
    if on[12] == "true":
        return 0, "unassessable", "memory_on_session_error"
    if off[12] == "true":
        return 0, "unassessable", "memory_off_session_error"
    if on[4] not in {"0", "1"} or off[4] not in {"0", "1"}:
        return 0, "unassessable", "invalid_outcome"
    evidence, evidence_reason = on[24], on[25]
    if evidence == "na":
        return 0, "not_memory_induced", "no_stale_candidate"
    if evidence not in {"0", "1"}:
        return 0, "unassessable", evidence_reason or "missing_injection_evidence"
    if on[4] == "1":
        return 0, "not_memory_induced", "memory_on_succeeded"
    if off[4] == "0":
        return 0, "not_memory_induced", "both_arms_failed"
    if evidence == "0":
        return 0, "not_memory_induced", "stale_not_injected"
    return 1, "memory_induced", "memory_on_only_failure_with_stale_injection"


def attribute_results(source, destination, diagnostics_root=None):
    """Append causal fields, replace column 7, and retain pair diagnostics."""
    lines = Path(source).read_text().splitlines()
    rows = []
    pairs = {}
    for line in lines:
        fields = line.split("\t")
        rows.append(fields)
        if line.startswith("agent=") or len(fields) < 26:
            continue
        key = (fields[0], fields[1], fields[3])
        pairs.setdefault(key, {}).setdefault(fields[2], []).append(fields)

    attributed = {}
    for key, arms in pairs.items():
        attributed[key] = causal_attribution(
            arms.get("memory-on", []), arms.get("memory-off", [])
        )

    output = []
    for fields in rows:
        if fields[0].startswith("agent=") or len(fields) < 26:
            output.append("\t".join(fields))
            continue
        key = (fields[0], fields[1], fields[3])
        mie, attribution, reason = attributed[key]
        if fields[2] == "memory-on":
            fields[6] = str(mie)
            fields.extend([attribution, reason])
            if diagnostics_root:
                outcome = (
                    Path(diagnostics_root)
                    / f"{fields[1]}-r{fields[3]}"
                    / "on"
                    / "diagnostics"
                    / "outcome.json"
                )
                if outcome.exists():
                    data = json.loads(outcome.read_text())
                    data.update({
                        "paired_memory_off_success":
                            pairs[key].get("memory-off", [[None] * 5])[0][4]
                            == "1"
                            if len(pairs[key].get("memory-off", [])) == 1
                            else None,
                        "memory_induced_error": bool(mie),
                        "causal_attribution": attribution,
                        "causal_reason": reason,
                        "causal_rule":
                            "memory-on failed AND paired memory-off succeeded "
                            "AND stale evidence appeared in the prompt-time receipt",
                    })
                    outcome.write_text(json.dumps(data, indent=2) + "\n")
        else:
            fields.extend(["na", "not_memory_on_arm"])
        output.append("\t".join(fields))
    Path(destination).write_text("\n".join(output) + ("\n" if output else ""))

def exact_id_report(path):
    data = json.loads(Path(path).read_text())
    if data.get("schema_version") != 2:
        raise ValueError("assertion report schema_version must be 2")
    if data.get("judge_used") is not False:
        raise ValueError("assertion report must declare judge_used=false")
    if not isinstance(data.get("no_judge_statement"), str) or not data["no_judge_statement"]:
        raise ValueError("assertion report needs a no_judge_statement")
    cases = data.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("assertion report needs at least one case")

    passed_cases = 0
    required_cases = 0
    passed_required_cases = 0
    case_ids = set()
    required_ids = set()
    for case in cases:
        case_id = case.get("id")
        if not isinstance(case_id, str) or not case_id or case_id in case_ids:
            raise ValueError("assertion report case IDs must be unique non-empty strings")
        case_ids.add(case_id)
        if not isinstance(case.get("gate_required"), bool):
            raise ValueError(f"{case_id}: gate_required must be boolean")
        queries = case.get("queries")
        if not isinstance(queries, list) or not queries:
            raise ValueError(f"{case.get('id', '<missing>')}: case needs queries")
        query_passes = []
        for query in queries:
            expected = query.get("expected_ids")
            returned = query.get("returned_ids")
            if (
                not isinstance(expected, list)
                or not isinstance(returned, list)
                or not all(isinstance(value, str) for value in expected + returned)
            ):
                raise ValueError("assertion IDs must be string arrays")
            expected_set = set(expected)
            returned_set = set(returned)
            if len(expected_set) != len(expected):
                raise ValueError("assertion expected_ids must be unique")
            evidence = query.get("returned_evidence")
            if (
                not isinstance(evidence, list)
                or [row.get("id") for row in evidence if isinstance(row, dict)] != returned
                or any(not isinstance(row, dict) for row in evidence)
            ):
                raise ValueError("returned_evidence IDs must match returned_ids in order")
            duplicates = sorted([
                value for value, count in Counter(returned).items() if count > 1
            ])
            missing = sorted(expected_set - returned_set)
            extra = sorted(returned_set - expected_set)
            passed = not missing and not extra and not duplicates
            if query.get("missing_ids") != missing:
                raise ValueError("assertion report missing_ids do not match exact sets")
            if query.get("extra_ids") != extra:
                raise ValueError("assertion report extra_ids do not match exact sets")
            if query.get("duplicate_ids") != duplicates:
                raise ValueError("assertion report duplicate_ids do not match exact output")
            if query.get("passed") is not passed:
                raise ValueError("assertion report query verdict does not match exact IDs")
            query_passes.append(passed)
        case_passed = all(query_passes)
        if case.get("passed") is not case_passed:
            raise ValueError("assertion report case verdict is not atomic")
        passed_cases += int(case_passed)
        if case.get("gate_required") is True:
            required_cases += 1
            passed_required_cases += int(case_passed)
            required_ids.add(case_id)

    if required_ids != REQUIRED_ASSERTION_CASES:
        raise ValueError(
            "required assertion cases changed: "
            f"expected {sorted(REQUIRED_ASSERTION_CASES)}, got {sorted(required_ids)}"
        )
    gate_passed = required_cases > 0 and passed_required_cases == required_cases
    expected_totals = {
        "passed_cases": passed_cases,
        "total_cases": len(cases),
        "passed_required_cases": passed_required_cases,
        "required_cases": required_cases,
        "all_cases_passed": passed_cases == len(cases),
        "precision_gate_passed": gate_passed,
    }
    for field, value in expected_totals.items():
        if data.get(field) != value:
            raise ValueError(f"assertion report {field} does not match case verdicts")
    return {
        "gate_passed": gate_passed,
        "passed_required_cases": passed_required_cases,
        "required_cases": required_cases,
        "passed_cases": passed_cases,
        "total_cases": len(cases),
        "all_cases_passed": passed_cases == len(cases),
        "uses_model_judge": False,
    }

def main():
    if len(sys.argv) < 2:
        print("usage: scorecard-controls.py "
              "{file-tokens|metrics|stale-evidence|attribute-results|validate-pair|assertion-gate|metadata} ...",
              file=sys.stderr)
        raise SystemExit(2)
    action, *args = sys.argv[1:]
    if action == "file-tokens":
        path = Path(args[0])
        print(estimate(path.read_text()) if path.exists() else 0)
    elif action == "metrics":
        directory, file_tokens = args
        prompt_tokens = sum(estimate(message) for message in messages(directory))
        print(f"0\t{prompt_tokens}\t{int(file_tokens)}\t{prompt_tokens + int(file_tokens)}\t{ESTIMATOR}")
    elif action == "stale-evidence":
        evidence, reason = stale_evidence(*args)
        print(f"{evidence}\t{reason}")
    elif action == "attribute-results":
        attribute_results(*args)
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
    elif action == "assertion-gate":
        print(json.dumps(exact_id_report(args[0]), separators=(",", ":")))
    elif action == "metadata":
        destination, model, version, extension_path, assertion_report = args
        assertion = exact_id_report(assertion_report)
        metadata = {
            "schema_version": 4,
            "agent": "omp",
            "model_selector": model,
            "omp_version": version,
            "extension_sha256": hashlib.sha256(Path(extension_path).read_bytes()).hexdigest(),
            "resolved_model_source": "assistant provider/model fields in each retained work.jsonl",
            "scoring": {
                "assertion_hard_gate": "fixed local exact evidence-ID assertions",
                "causal_safety_gate": "paired stale-injection differential for complete live runs",
                "model_task_proxy": "workspace or exact-artifact outcome; report-only outside declared artifact assertions",
                "unsupported": "agent-task correctness has no general exact-ID outcome contract",
            },
            "outcome_scoring": {
                "freshness": "exact delivered-artifact equality; assistant prose excluded",
                "other_dimensions": "workspace-only compatibility substring report",
            },
            "hard_gate": {
                "name": "exact-id-assertions-plus-paired-causal-safety",
                "rule": "required exact-ID cases pass; for complete live runs, memory-on failed AND paired memory-off succeeded AND stale marker appears in the prompt-time receipt is unsafe",
                "safe": "the required assertion scope passes and every required live pair/receipt is assessable with zero causal attributions",
                "unsafe": "required assertion failure, at least one causal attribution, or an unassessable required live pair",
                "limitations": [
                    "before_agent_start receipts record prepared custom messages, not provider delivery or attention",
                    "paired model runs are nondeterministic controls, not randomized causal proof",
                ],
            },
            "assertion_fixture": {
                **assertion,
                "executed_by_this_run": True,
                "report": str(Path(assertion_report)),
                "report_sha256": hashlib.sha256(Path(assertion_report).read_bytes()).hexdigest(),
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
                "session_start_answer_present",
                "prompt_recall_answer_present",
                "injected_stale_evidence",
                "injection_evidence_reason",
                "causal_attribution",
                "causal_reason",
            ],
            "sidecars": {
                "assertion_report": "RESULTS.assertions.json",
                "schema": "rb-eval assertion-precision schema_version 2",
            },
        }
        Path(destination).write_text(json.dumps(metadata, indent=2) + "\n")
    else:
        raise ValueError("unknown action: " + action)


if __name__ == "__main__":
    main()
