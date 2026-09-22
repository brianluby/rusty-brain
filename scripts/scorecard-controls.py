#!/usr/bin/env python3
"""OMP scorecard control accounting and exact assertion validation.

The native OMP extension records each before_agent_start custom-message
preparation attempt (not a provider delivery receipt). This module measures
those receipts, rejects unmatched placebos, and checks the internal consistency
of the fixed no-judge exact-ID report produced locally by the current job.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys

ESTIMATOR = "utf8-bytes-div4-ceil-v1"
CHANNEL = "prompt-time"
REQUIRED_ASSERTION_CASES = {
    "supersede-chain",
    "five-slot-budget",
    "namespace-selection",
}


def estimate(text):
    """Estimate tokens from UTF-8 bytes without claiming provider token counts."""
    return (len(text.encode("utf-8")) + 3) // 4


def records(directory):
    """Read validated message objects, ignoring blank lines and absent receipts."""
    path = Path(directory) / f"{CHANNEL}.jsonl"
    try:
        content = path.read_text()
    except FileNotFoundError:
        return []
    except (OSError, UnicodeError) as error:
        raise ValueError(f"cannot read prompt receipt {path}: {error}") from error
    rows = []
    for number, line in enumerate(content.splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(
                f"invalid prompt receipt {path}, line {number}: malformed JSON"
            ) from error
        if not isinstance(row, dict) or not isinstance(row.get("message"), str):
            raise ValueError(
                f"invalid prompt receipt {path}, line {number}: "
                "expected an object with a string message"
            )
        rows.append(row)
    return rows


def messages(directory):
    """Return message strings only after the entire receipt has been validated."""
    return [record["message"] for record in records(directory)]


def stale_evidence(directory, stale_marker):
    """Return whether a stale-record-only marker appears in a prompt receipt."""
    if not stale_marker:
        return "na", "no_stale_marker"
    directory = Path(directory)
    if (directory / "control-error.json").exists():
        return "unknown", "control_error"
    receipt = directory / f"{CHANNEL}.jsonl"
    if not receipt.exists():
        return "unknown", "missing_prompt_receipt"
    try:
        rows = records(directory)
    except ValueError:
        return "unknown", "malformed_prompt_receipt"
    if not rows:
        return "unknown", "empty_prompt_receipt"
    needle = stale_marker.casefold()
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
    # A stale-bearing pair needs a clean control, including successful arms:
    # unknown is not evidence of absence, and contamination defeats the control.
    if off[24] == "1":
        return 0, "unassessable", "contaminated_control_evidence"
    if off[24] != "0":
        return 0, "unassessable", "missing_control_evidence"
    if on[4] == "1":
        return 0, "not_memory_induced", "memory_on_succeeded"
    if off[4] == "0":
        return 0, "not_memory_induced", "both_arms_failed"
    if evidence == "0":
        return 0, "not_memory_induced", "stale_not_injected"
    return 1, "memory_induced", "memory_on_only_failure_with_stale_injection"


def is_agent_skip(fields):
    """Recognize the runner's seven-field unsupported-agent skip records."""
    return (
        len(fields) == 7
        and fields[0] in {"agent=codex", "agent=opencode", "agent=gemini", "agent=hermes"}
        and fields[1:3] == ["dimension=all", "scenario=all"]
        and fields[3] in {"phase=capture", "phase=config", "phase=scoring"}
        and fields[4] == "status=skip"
        and fields[5].startswith("reason=")
        and fields[6].startswith("detail=")
    )


def retain_attribution(outcome, off_rows, mie, attribution, reason):
    """Update optional diagnostics without letting a damaged sidecar lose TSV rows."""
    try:
        data = json.loads(outcome.read_text())
        if not isinstance(data, dict):
            raise ValueError("expected a JSON object")
        data.update({
            "paired_memory_off_success":
                off_rows[0][4] == "1" if len(off_rows) == 1 else None,
            "memory_induced_error": bool(mie),
            "causal_attribution": attribution,
            "causal_reason": reason,
            "causal_rule":
                "memory-on failed AND paired memory-off succeeded "
                "AND stale evidence appeared in the prompt-time receipt "
                "AND the memory-off control has assessable clean evidence",
        })
    except FileNotFoundError:
        return
    except (OSError, ValueError, UnicodeError) as error:
        print(f"warning: skipping outcome sidecar {outcome}: {error}", file=sys.stderr)
        return
    try:
        outcome.write_text(json.dumps(data, indent=2) + "\n")
    except (OSError, UnicodeError) as error:
        print(f"warning: cannot write outcome sidecar {outcome}: {error}", file=sys.stderr)


def attribute_results(source, destination, diagnostics_root=None):
    """Validate all input, append causal fields, and only upgrade column 7 failures."""
    rows = []
    pairs = {}
    outcomes = {}
    root = Path(diagnostics_root).resolve() if diagnostics_root else None
    for number, line in enumerate(Path(source).read_text().splitlines(), 1):
        fields = line.split("\t")
        rows.append(fields)
        if is_agent_skip(fields):
            continue
        if len(fields) != 26:
            raise ValueError(
                f"TSV line {number}: expected 26 pre-attribution fields, got {len(fields)}"
            )
        # Scenario and run are the only TSV-derived path components. Reject
        # separators before joining; resolving also catches existing symlink escapes.
        for index, name in ((1, "scenario"), (3, "run")):
            if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", fields[index]):
                raise ValueError(f"TSV line {number}: invalid {name} path component")
        if not fields[6].isascii() or not fields[6].isdigit():
            raise ValueError(f"TSV line {number}: column 7 must be a nonnegative integer")
        key = (fields[0], fields[1], fields[3])
        pairs.setdefault(key, {}).setdefault(fields[2], []).append(fields)
        if root is not None and fields[2] == "memory-on":
            outcome = root / f"{fields[1]}-r{fields[3]}" / "on" / "diagnostics" / "outcome.json"
            try:
                resolved = outcome.resolve()
            except (OSError, RuntimeError) as error:
                raise ValueError(f"TSV line {number}: cannot resolve diagnostics path") from error
            if not resolved.is_relative_to(root):
                raise ValueError(f"TSV line {number}: diagnostics path escapes diagnostics root")
            outcomes[key] = resolved

    # Finish validation before touching any sidecar or destination. A malformed
    # later row must not leave earlier diagnostics partially attributed.
    attributed = {
        key: causal_attribution(arms.get("memory-on", []), arms.get("memory-off", []))
        for key, arms in pairs.items()
    }
    output = []
    for fields in rows:
        if is_agent_skip(fields):
            output.append("\t".join(fields))
            continue
        key = (fields[0], fields[1], fields[3])
        mie, attribution, reason = attributed[key]
        if fields[2] == "memory-on":
            # Causal attribution can upgrade a prior failure, never erase it.
            fields[6] = str(max(int(fields[6]), mie))
            fields.extend([attribution, reason])
            if key in outcomes:
                retain_attribution(
                    outcomes[key], pairs[key].get("memory-off", []),
                    int(fields[6]), attribution, reason,
                )
        else:
            fields.extend(["na", "not_memory_on_arm"])
        output.append("\t".join(fields))
    Path(destination).write_text("\n".join(output) + ("\n" if output else ""))

def exact_id_report(path):
    """Check a trusted in-job report's consistency, not external authenticity.

    The caller must supply the current job's assertion-precision output. Neither
    this validation nor a recorded hash authenticates a producer or fixture.
    """
    data = json.loads(Path(path).read_text())
    if not isinstance(data, dict):
        raise ValueError("assertion report must be a JSON object")
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
        if not isinstance(case, dict):
            raise ValueError("assertion report cases must be JSON objects")
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
            if not isinstance(query, dict):
                raise ValueError("assertion report queries must be JSON objects")
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
    """Dispatch receipt accounting, paired attribution, and local report checks."""
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
        # The runner supplies its freshly generated local artifact; this check
        # does not attest external provenance or compare against a trusted hash.
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
                "rule": "required exact-ID cases pass; for complete live runs, memory-on failed AND paired memory-off succeeded AND stale marker appears in the prompt-time receipt AND the control has assessable clean evidence is unsafe",
                "safe": "the required assertion scope passes and every required live pair/receipt is assessable with zero causal attributions",
                "unsafe": "required assertion failure, at least one causal attribution, or an unassessable required live pair",
                "limitations": [
                    "before_agent_start receipts record prepared custom messages, not provider delivery or attention",
                    "paired model runs are nondeterministic controls, not randomized causal proof",
                    "memory-off extension_disabled evidence records a harness control, not a delivery receipt",
                ],
            },
            "assertion_fixture": {
                **assertion,
                "executed_by_this_run": True,
                "report": str(Path(assertion_report)),
                "report_sha256": hashlib.sha256(Path(assertion_report).read_bytes()).hexdigest(),
                "trust_boundary": "local in-job assertion-precision artifact; internal consistency only, not authenticated external attestation",
                "report_sha256_purpose": "artifact identification only; not compared with an independently trusted digest",
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
                "assertion_report": str(Path(assertion_report)),
                "schema": "rb-eval assertion-precision schema_version 2",
            },
        }
        Path(destination).write_text(json.dumps(metadata, indent=2) + "\n")
    else:
        raise ValueError("unknown action: " + action)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, UnicodeError) as error:
        print(f"scorecard-controls: {error}", file=sys.stderr)
        raise SystemExit(1)
