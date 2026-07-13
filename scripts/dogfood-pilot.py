#!/usr/bin/env python3
"""Fail-closed aggregation for the bounded Phase 5 dogfood pilot.

The tool accepts only sanitized, aggregate observations. It never reads a
rusty-brain database, prompt, transcript, model response, or memory body.
"""

from __future__ import annotations

import argparse
import copy
import json
import math
import re
import statistics
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
CANDIDATE_REPOSITORIES = {"rusty-brain", "threatmitigator", "vikunja-rust-mcp"}
SURFACES = {"claude_native", "http", "mcp"}
ARM_ORDERS = {"baseline_first", "treatment_first"}
THRESHOLDS: dict[str, int | float] = {
    "memory_induced_errors_max": 0,
    "contested_regression_max_pp": 1.0,
    "multi_answer_regression_max_pp": 1.0,
    "provenance_label_coverage_min": 1.0,
    "contested_label_coverage_min": 1.0,
    "time_to_first_useful_recall_hours_max": 72.0,
    "useful_recall_repository_coverage_min": 1.0,
    "helpful_ratio_min": 0.80,
    "wrong_ratio_max": 0.10,
    "stale_ratio_max": 0.10,
    "task_success_regression_max_pp": 1.0,
    "correction_incidence_regression_max_pp": 1.0,
    "median_turns_saved_min": 0.0,
    "median_active_seconds_saved_min": 0.0,
    "exact_recoveries_min": 1,
    "semantic_helpful_recalls_min": 1,
    "review_backlog_growth_max": 0,
    "backup_attempts_min": 1,
    "backup_failures_max": 0,
    "retention_actions_min": 1,
    "retention_failures_max": 0,
}

MANIFEST_KEYS = {
    "schema_version",
    "pilot_id",
    "status",
    "period",
    "paired_runs_per_repository",
    "admission",
    "thresholds",
    "repositories",
}
PERIOD_KEYS = {"duration_days", "start_utc", "end_utc"}
ADMISSION_KEYS = {"frozen_at_utc", "task_56", "task_57"}
TASK_56_KEYS = {
    "state",
    "evidence",
    "overall_pilot_go",
    "blocker",
    "qualified_treatment_arm",
    "unqualified_behavior_allowed",
}
TASK_57_KEYS = {"state", "complete", "evidence", "max_active_memories"}
REPOSITORY_KEYS = {
    "name",
    "commit_sha",
    "baseline_store_id",
    "treatment_store_id",
    "baseline_namespace",
    "treatment_namespace",
    "pair_plan",
}
PLAN_KEYS = {"pair_id", "scenario_id", "arm_order", "snapshot_sha256"}
PAIR_KEYS = {
    "schema_version",
    "pair_id",
    "scenario_id",
    "repository",
    "commit_sha",
    "observed_at_utc",
    "arm_order",
    "snapshot_sha256",
    "baseline",
    "treatment",
    "qualitative_examples",
}
COMMON_ARM_KEYS = {
    "environment_id",
    "store_id",
    "namespace",
    "agent",
    "task_success",
    "turns",
    "active_seconds",
    "corrections",
    "contested_opportunities",
    "contested_correct",
    "multi_answer_opportunities",
    "multi_answer_correct",
}
TREATMENT_KEYS = COMMON_ARM_KEYS | {
    "arm_id",
    "delivery_surface",
    "qualified_behavior_only",
    "injections_total",
    "helpful_injections",
    "wrong_injections",
    "stale_injections",
    "ignored_injections",
    "stale_wrong_exact_injections",
    "memory_induced_errors",
    "exact_recoveries",
    "semantic_helpful_recalls",
    "injected_tokens",
    "injected_cost_usd",
    "provenance_labeled_injections",
    "contested_injections",
    "contested_labeled_injections",
    "corpus_rows_before",
    "corpus_rows_after",
    "corpus_bytes_before",
    "corpus_bytes_after",
    "review_backlog_before",
    "review_backlog_after",
    "retention_actions",
    "retention_failures",
    "retention_active_seconds",
    "backup_attempts",
    "backup_failures",
    "backup_active_seconds",
}
EXAMPLE_KEYS = {
    "category",
    "sanitized_summary",
    "provenance_label",
    "contested",
    "sanitization_attested",
}
EXAMPLE_CATEGORIES = {"helpful", "wrong", "stale", "correction", "friction"}
PROVENANCE_LABELS = {"hook", "mcp", "cli", "job", "http", "none"}
IDENTIFIER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")
IMMUTABLE_EVIDENCE = re.compile(
    r"^(?:commit:[0-9a-f]{40}|sha256:[0-9a-f]{64}|github-run:[1-9][0-9]*)$"
)
SUSPECT_SECRET = re.compile(
    r"(?i)(Bearer\s+[A-Za-z0-9._-]{8,}|sk-[A-Za-z0-9-]{8,}|"
    r"AKIA[A-Z0-9]{12,}|-----BEGIN .*PRIVATE KEY-----|"
    r"gh[pousr]_[A-Za-z0-9]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|"
    r"AIza[A-Za-z0-9_-]{20,}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|"
    r"[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}|://[^/\s:@]+:[^@\s]+@|"
    r"(?:key|token|secret|password)\s*[=:]\s*\S+|/Users/|/home/)"
)


class PilotError(ValueError):
    """Input is incomplete, unsafe, or inconsistent with the preregistration."""


def fail(message: str) -> None:
    raise PilotError(message)


def exact_keys(value: Any, expected: set[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{where} must be an object")
    actual = set(value)
    if actual != expected:
        fail(f"{where} keys differ: missing={sorted(expected - actual)} unknown={sorted(actual - expected)}")
    return value


def integer(value: Any, where: str, minimum: int = 0) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        fail(f"{where} must be an integer >= {minimum}")
    return value


def number(value: Any, where: str, minimum: float = 0.0) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        fail(f"{where} must be numeric")
    result = float(value)
    if not math.isfinite(result) or result < minimum:
        fail(f"{where} must be finite and >= {minimum}")
    return result


def identifier(value: Any, where: str) -> str:
    if (
        not isinstance(value, str)
        or not IDENTIFIER.fullmatch(value)
        or SUSPECT_SECRET.search(value)
    ):
        fail(f"{where} must be a short opaque identifier")
    return value


def timestamp(value: Any, where: str) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{where} must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise PilotError(f"{where} is not a valid timestamp") from error
    if parsed.tzinfo != timezone.utc:
        fail(f"{where} must be UTC")
    return parsed


def nonempty_string(value: Any, where: str) -> str:
    if not isinstance(value, str) or not value.strip():
        fail(f"{where} must be a non-empty string")
    return value


def evidence_reference(value: Any, where: str) -> str:
    value = nonempty_string(value, where)
    if not IMMUTABLE_EVIDENCE.fullmatch(value) or SUSPECT_SECRET.search(value):
        fail(
            f"{where} must be commit:<40-hex>, sha256:<64-hex>, or github-run:<id>"
        )
    return value


def validate_manifest(manifest: Any) -> dict[str, Any]:
    manifest = exact_keys(manifest, MANIFEST_KEYS, "manifest")
    if integer(manifest["schema_version"], "manifest.schema_version", 1) != SCHEMA_VERSION:
        fail("unsupported manifest schema_version")

    admission = exact_keys(manifest["admission"], ADMISSION_KEYS, "manifest.admission")
    task_56 = exact_keys(admission["task_56"], TASK_56_KEYS, "manifest.admission.task_56")
    if task_56["state"] != "go" or task_56["overall_pilot_go"] is not True:
        fail(
            "task #56 blocks treatment: reviewed production evidence must record "
            "state='go' and overall_pilot_go=true"
        )
    task_57 = exact_keys(admission["task_57"], TASK_57_KEYS, "manifest.admission.task_57")
    if task_57["state"] != "go" or task_57["complete"] is not True:
        fail("task #57 blocks treatment: reviewed evidence must record state='go' and complete=true")

    if manifest["status"] != "admitted":
        fail("manifest.status must be admitted before any treatment observation")
    identifier(manifest["pilot_id"], "manifest.pilot_id")

    period = exact_keys(manifest["period"], PERIOD_KEYS, "manifest.period")
    if integer(period["duration_days"], "manifest.period.duration_days", 1) != 14:
        fail("manifest.period.duration_days must remain the preregistered 14 days")
    start = timestamp(period["start_utc"], "manifest.period.start_utc")
    end = timestamp(period["end_utc"], "manifest.period.end_utc")
    if (end - start).total_seconds() != 14 * 86_400:
        fail("pilot start/end must span exactly 14 days")

    runs = integer(manifest["paired_runs_per_repository"], "paired_runs_per_repository", 5)
    frozen = timestamp(admission["frozen_at_utc"], "manifest.admission.frozen_at_utc")
    if frozen >= start:
        fail("admission must be frozen before the pilot starts")

    evidence_reference(task_56["evidence"], "task_56.evidence")
    if task_56["blocker"] is not None:
        fail("task #56 blocker must be null after admission")
    identifier(task_56["qualified_treatment_arm"], "task_56.qualified_treatment_arm")
    if task_56["unqualified_behavior_allowed"] is not False:
        fail("unqualified experimental behavior must remain disallowed")

    evidence_reference(task_57["evidence"], "task_57.evidence")
    integer(task_57["max_active_memories"], "task_57.max_active_memories", 1)

    frozen_thresholds = json.dumps(THRESHOLDS, sort_keys=True, separators=(",", ":"))
    supplied_thresholds = json.dumps(manifest["thresholds"], sort_keys=True, separators=(",", ":"))
    if supplied_thresholds != frozen_thresholds:
        fail("pilot thresholds differ from the preregistered constants")
    repositories = manifest["repositories"]
    if not isinstance(repositories, list) or not repositories:
        fail("at least one repository must be confirmed before admission")

    seen_repositories: set[str] = set()
    seen_pairs: set[str] = set()
    seen_storage: set[tuple[str, str]] = set()
    for index, repository in enumerate(repositories):
        where = f"manifest.repositories[{index}]"
        repository = exact_keys(repository, REPOSITORY_KEYS, where)
        name = repository["name"]
        if name not in CANDIDATE_REPOSITORIES or name in seen_repositories:
            fail(f"{where}.name is not a unique confirmed candidate")
        seen_repositories.add(name)
        if not isinstance(repository["commit_sha"], str) or not COMMIT_SHA.fullmatch(repository["commit_sha"]):
            fail(f"{where}.commit_sha must be a full lowercase commit SHA")
        for field in ("baseline_store_id", "treatment_store_id", "baseline_namespace", "treatment_namespace"):
            identifier(repository[field], f"{where}.{field}")
        if repository["baseline_store_id"] == repository["treatment_store_id"]:
            fail(f"{where} baseline and treatment stores must be isolated")
        if repository["baseline_namespace"] == repository["treatment_namespace"]:
            fail(f"{where} baseline and treatment namespaces must be isolated")
        for arm in ("baseline", "treatment"):
            storage = (
                repository[f"{arm}_store_id"],
                repository[f"{arm}_namespace"],
            )
            if storage in seen_storage:
                fail(f"{where} reuses a store/namespace tuple from another pilot arm")
            seen_storage.add(storage)
        plan = repository["pair_plan"]
        if not isinstance(plan, list) or len(plan) != runs:
            fail(f"{where}.pair_plan must contain exactly {runs} pairs")
        for pair_index, pair in enumerate(plan):
            pair_where = f"{where}.pair_plan[{pair_index}]"
            pair = exact_keys(pair, PLAN_KEYS, pair_where)
            pair_id = identifier(pair["pair_id"], f"{pair_where}.pair_id")
            identifier(pair["scenario_id"], f"{pair_where}.scenario_id")
            if not isinstance(pair["snapshot_sha256"], str) or not SHA256.fullmatch(
                pair["snapshot_sha256"]
            ):
                fail(f"{pair_where}.snapshot_sha256 must be a lowercase SHA-256")
            if pair["arm_order"] not in ARM_ORDERS:
                fail(f"{pair_where}.arm_order is invalid")
            if pair_id in seen_pairs:
                fail(f"duplicate pair_id {pair_id}")
            seen_pairs.add(pair_id)
        baseline_first = sum(pair["arm_order"] == "baseline_first" for pair in plan)
        treatment_first = len(plan) - baseline_first
        if abs(baseline_first - treatment_first) > 1:
            fail(f"{where}.pair_plan must balance baseline-first and treatment-first order")
    return manifest


def validate_common_arm(arm: Any, where: str) -> dict[str, Any]:
    arm = exact_keys(arm, COMMON_ARM_KEYS, where)
    for field in ("environment_id", "store_id", "namespace", "agent"):
        identifier(arm[field], f"{where}.{field}")
    if not isinstance(arm["task_success"], bool):
        fail(f"{where}.task_success must be boolean")
    for field in ("turns", "corrections", "contested_opportunities", "contested_correct", "multi_answer_opportunities", "multi_answer_correct"):
        integer(arm[field], f"{where}.{field}")
    number(arm["active_seconds"], f"{where}.active_seconds")
    if arm["contested_correct"] > arm["contested_opportunities"]:
        fail(f"{where}.contested_correct exceeds opportunities")
    if arm["multi_answer_correct"] > arm["multi_answer_opportunities"]:
        fail(f"{where}.multi_answer_correct exceeds opportunities")
    return arm


def validate_treatment(arm: Any, where: str) -> dict[str, Any]:
    arm = exact_keys(arm, TREATMENT_KEYS, where)
    validate_common_arm({key: arm[key] for key in COMMON_ARM_KEYS}, where)
    identifier(arm["arm_id"], f"{where}.arm_id")
    if arm["delivery_surface"] not in SURFACES:
        fail(f"{where}.delivery_surface must attribute Claude-native, HTTP, or MCP")
    if arm["delivery_surface"] == "claude_native" and arm["agent"] != "claude-code":
        fail(f"{where}.delivery_surface=claude_native requires agent=claude-code")
    if arm["qualified_behavior_only"] is not True:
        fail(f"{where} attempted unqualified experimental behavior")
    integer_fields = TREATMENT_KEYS - COMMON_ARM_KEYS - {
        "arm_id",
        "delivery_surface",
        "qualified_behavior_only",
        "injected_cost_usd",
        "retention_active_seconds",
        "backup_active_seconds",
    }
    for field in integer_fields:
        integer(arm[field], f"{where}.{field}")
    for field in ("retention_active_seconds", "backup_active_seconds"):
        number(arm[field], f"{where}.{field}")
    if arm["injected_cost_usd"] is not None:
        number(arm["injected_cost_usd"], f"{where}.injected_cost_usd")
    classified = arm["helpful_injections"] + arm["wrong_injections"] + arm["stale_injections"] + arm["ignored_injections"]
    if classified != arm["injections_total"]:
        fail(f"{where} injection outcomes must partition injections_total")
    if arm["stale_wrong_exact_injections"] > arm["wrong_injections"] + arm["stale_injections"]:
        fail(f"{where}.stale_wrong_exact_injections exceeds wrong+stale")
    if arm["exact_recoveries"] > arm["helpful_injections"]:
        fail(f"{where}.exact_recoveries exceeds helpful injections")
    if arm["semantic_helpful_recalls"] > arm["helpful_injections"]:
        fail(f"{where}.semantic_helpful_recalls exceeds helpful injections")
    if arm["exact_recoveries"] + arm["semantic_helpful_recalls"] > arm["helpful_injections"]:
        fail(f"{where} exact and semantic helpful classifications must be disjoint")
    if arm["provenance_labeled_injections"] > arm["injections_total"]:
        fail(f"{where}.provenance_labeled_injections exceeds total")
    if arm["contested_injections"] > arm["injections_total"]:
        fail(f"{where}.contested_injections exceeds total")
    if arm["contested_labeled_injections"] > arm["contested_injections"]:
        fail(f"{where}.contested_labeled_injections exceeds contested injections")
    if arm["retention_failures"] > arm["retention_actions"]:
        fail(f"{where}.retention_failures exceeds actions")
    if arm["backup_failures"] > arm["backup_attempts"]:
        fail(f"{where}.backup_failures exceeds attempts")
    return arm


def validate_example(example: Any, where: str) -> dict[str, Any]:
    example = exact_keys(example, EXAMPLE_KEYS, where)
    if example["category"] not in EXAMPLE_CATEGORIES:
        fail(f"{where}.category is invalid")
    summary = example["sanitized_summary"]
    if not isinstance(summary, str) or not summary or len(summary) > 280 or "\n" in summary:
        fail(f"{where}.sanitized_summary must be a single sanitized line <=280 chars")
    if SUSPECT_SECRET.search(summary):
        fail(f"{where}.sanitized_summary contains a secret/path-like pattern")
    if example["provenance_label"] not in PROVENANCE_LABELS:
        fail(f"{where}.provenance_label is invalid")
    if not isinstance(example["contested"], bool):
        fail(f"{where}.contested must be boolean")
    if example["sanitization_attested"] is not True:
        fail(f"{where} must carry an explicit sanitization attestation")
    return example


def validate_pair(pair: Any, manifest: dict[str, Any]) -> dict[str, Any]:
    pair = exact_keys(pair, PAIR_KEYS, "pair")
    if integer(pair["schema_version"], "pair.schema_version", 1) != SCHEMA_VERSION:
        fail("pair schema_version does not match")
    pair_id = identifier(pair["pair_id"], "pair.pair_id")
    identifier(pair["scenario_id"], "pair.scenario_id")
    observed = timestamp(pair["observed_at_utc"], "pair.observed_at_utc")
    start = timestamp(manifest["period"]["start_utc"], "manifest.period.start_utc")
    end = timestamp(manifest["period"]["end_utc"], "manifest.period.end_utc")
    if not start <= observed < end:
        fail(f"pair {pair_id} falls outside the frozen pilot period")
    if not isinstance(pair["snapshot_sha256"], str) or not SHA256.fullmatch(pair["snapshot_sha256"]):
        fail(f"pair {pair_id} snapshot_sha256 is invalid")

    repository = next((item for item in manifest["repositories"] if item["name"] == pair["repository"]), None)
    if repository is None:
        fail(f"pair {pair_id} uses an unconfirmed repository")
    if pair["commit_sha"] != repository["commit_sha"]:
        fail(f"pair {pair_id} commit differs from the frozen repository commit")
    planned = next((item for item in repository["pair_plan"] if item["pair_id"] == pair_id), None)
    if planned is None or planned["scenario_id"] != pair["scenario_id"]:
        fail(f"pair {pair_id} is absent from the frozen pair plan")
    if pair["arm_order"] != planned["arm_order"]:
        fail(f"pair {pair_id} arm order differs from the frozen pair plan")
    if pair["snapshot_sha256"] != planned["snapshot_sha256"]:
        fail(f"pair {pair_id} snapshot differs from the frozen pair plan")

    baseline = validate_common_arm(pair["baseline"], f"pair {pair_id}.baseline")
    treatment = validate_treatment(pair["treatment"], f"pair {pair_id}.treatment")
    if baseline["environment_id"] == treatment["environment_id"]:
        fail(f"pair {pair_id} baseline and treatment environments are not isolated")
    expected = {
        "baseline_store": repository["baseline_store_id"],
        "treatment_store": repository["treatment_store_id"],
        "baseline_namespace": repository["baseline_namespace"],
        "treatment_namespace": repository["treatment_namespace"],
    }
    actual = {
        "baseline_store": baseline["store_id"],
        "treatment_store": treatment["store_id"],
        "baseline_namespace": baseline["namespace"],
        "treatment_namespace": treatment["namespace"],
    }
    if actual != expected:
        fail(f"pair {pair_id} store/namespace isolation differs from the manifest")
    if baseline["agent"] != treatment["agent"]:
        fail(f"pair {pair_id} must use the same agent in both arms")
    for prefix in ("contested", "multi_answer"):
        if baseline[f"{prefix}_opportunities"] != treatment[f"{prefix}_opportunities"]:
            fail(f"pair {pair_id} must use identical {prefix} opportunities in both arms")
    if treatment["arm_id"] != manifest["admission"]["task_56"]["qualified_treatment_arm"]:
        fail(f"pair {pair_id} treatment arm was not qualified by task #56")
    ceiling = manifest["admission"]["task_57"]["max_active_memories"]
    if treatment["corpus_rows_before"] > ceiling or treatment["corpus_rows_after"] > ceiling:
        fail(f"pair {pair_id} exceeds the task #57 active-memory ceiling")

    examples = pair["qualitative_examples"]
    if not isinstance(examples, list):
        fail(f"pair {pair_id}.qualitative_examples must be an array")
    for index, example in enumerate(examples):
        validate_example(example, f"pair {pair_id}.qualitative_examples[{index}]")
    return pair


def ratio(numerator: int, denominator: int) -> float | None:
    return numerator / denominator if denominator else None


def median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def interquartile_range(values: list[float]) -> tuple[float | None, float | None]:
    if not values:
        return None, None
    if len(values) == 1:
        return values[0], values[0]
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive")
    return q1, q3


def correctness_rate(pairs: list[dict[str, Any]], arm: str, prefix: str) -> float | None:
    opportunities = sum(pair[arm][f"{prefix}_opportunities"] for pair in pairs)
    correct = sum(pair[arm][f"{prefix}_correct"] for pair in pairs)
    return ratio(correct, opportunities)


def aggregate(manifest: dict[str, Any], pairs: list[dict[str, Any]], final: bool) -> dict[str, Any]:
    manifest = validate_manifest(manifest)
    validated: list[dict[str, Any]] = []
    seen: set[str] = set()
    for pair in pairs:
        pair = validate_pair(pair, manifest)
        if pair["pair_id"] in seen:
            fail(f"duplicate observation for pair {pair['pair_id']}")
        seen.add(pair["pair_id"])
        validated.append(pair)

    planned = {
        item["pair_id"]
        for repository in manifest["repositories"]
        for item in repository["pair_plan"]
    }
    if not seen:
        fail("no pilot observations supplied")
    if final and seen != planned:
        fail(f"final aggregation requires every planned pair: missing={sorted(planned - seen)}")
    if not seen <= planned:
        fail("observations include an unplanned pair")

    validated.sort(key=lambda item: timestamp(item["observed_at_utc"], "observed_at_utc"))
    treatments = [pair["treatment"] for pair in validated]
    injections = sum(item["injections_total"] for item in treatments)
    helpful = sum(item["helpful_injections"] for item in treatments)
    wrong = sum(item["wrong_injections"] for item in treatments)
    stale = sum(item["stale_injections"] for item in treatments)
    ignored = sum(item["ignored_injections"] for item in treatments)
    rated = helpful + wrong + stale
    contested_base = correctness_rate(validated, "baseline", "contested")
    contested_treatment = correctness_rate(validated, "treatment", "contested")
    multi_base = correctness_rate(validated, "baseline", "multi_answer")
    multi_treatment = correctness_rate(validated, "treatment", "multi_answer")

    def regression_pp(baseline: float | None, treatment: float | None) -> float | None:
        if baseline is None or treatment is None:
            return None
        return (baseline - treatment) * 100.0

    success_base = sum(pair["baseline"]["task_success"] for pair in validated) / len(validated)
    success_treatment = sum(pair["treatment"]["task_success"] for pair in validated) / len(validated)
    correction_base = sum(pair["baseline"]["corrections"] > 0 for pair in validated) / len(validated)
    correction_treatment = sum(pair["treatment"]["corrections"] > 0 for pair in validated) / len(validated)
    turns_saved = [float(pair["baseline"]["turns"] - pair["treatment"]["turns"]) for pair in validated]
    seconds_saved = [pair["baseline"]["active_seconds"] - pair["treatment"]["active_seconds"] for pair in validated]
    turns_saved_q1, turns_saved_q3 = interquartile_range(turns_saved)
    seconds_saved_q1, seconds_saved_q3 = interquartile_range(seconds_saved)

    start = timestamp(manifest["period"]["start_utc"], "manifest.period.start_utc")
    by_repository: dict[str, Any] = {}
    activated_repositories = 0
    for repository in manifest["repositories"]:
        repository_pairs = [pair for pair in validated if pair["repository"] == repository["name"]]
        useful = [pair for pair in repository_pairs if pair["treatment"]["helpful_injections"] > 0]
        first_useful_hours = None
        first_useful_pair_id = None
        if useful:
            first_pair = min(
                useful,
                key=lambda item: timestamp(item["observed_at_utc"], "observed_at_utc"),
            )
            first = timestamp(first_pair["observed_at_utc"], "observed_at_utc")
            first_useful_hours = (first - start).total_seconds() / 3600.0
            first_useful_pair_id = first_pair["pair_id"]
            activated_repositories += 1
        if repository_pairs:
            ordered = sorted(
                repository_pairs,
                key=lambda item: timestamp(item["observed_at_utc"], "observed_at_utc"),
            )
            first_treatment = ordered[0]["treatment"]
            last_treatment = ordered[-1]["treatment"]
            row_growth = last_treatment["corpus_rows_after"] - first_treatment["corpus_rows_before"]
            byte_growth = last_treatment["corpus_bytes_after"] - first_treatment["corpus_bytes_before"]
            backlog_growth = last_treatment["review_backlog_after"] - first_treatment["review_backlog_before"]
        else:
            row_growth = byte_growth = backlog_growth = None
        by_repository[repository["name"]] = {
            "observed_pairs": len(repository_pairs),
            "time_to_first_useful_recall_hours": first_useful_hours,
            "first_useful_pair_id": first_useful_pair_id,
            "corpus_row_growth": row_growth,
            "corpus_byte_growth": byte_growth,
            "review_backlog_growth": backlog_growth,
        }

    by_surface: dict[str, Any] = {}
    for surface in sorted(SURFACES):
        surface_pairs = [pair for pair in validated if pair["treatment"]["delivery_surface"] == surface]
        surface_injections = sum(pair["treatment"]["injections_total"] for pair in surface_pairs)
        surface_helpful = sum(pair["treatment"]["helpful_injections"] for pair in surface_pairs)
        surface_costs = [pair["treatment"]["injected_cost_usd"] for pair in surface_pairs]
        by_surface[surface] = {
            "pairs": len(surface_pairs),
            "injections": surface_injections,
            "helpful_injections": surface_helpful,
            "helpful_ratio": ratio(surface_helpful, sum(
                pair["treatment"]["helpful_injections"]
                + pair["treatment"]["wrong_injections"]
                + pair["treatment"]["stale_injections"]
                for pair in surface_pairs
            )),
            "injected_tokens": sum(
                pair["treatment"]["injected_tokens"] for pair in surface_pairs
            ),
            "injected_cost_usd": (
                sum(float(value) for value in surface_costs)
                if surface_costs and all(value is not None for value in surface_costs)
                else None
            ),
        }

    by_agent_surface: dict[str, Any] = {}
    attributions = {
        (pair["treatment"]["agent"], pair["treatment"]["delivery_surface"])
        for pair in validated
    }
    for agent, surface in sorted(attributions):
        attributed = [
            pair
            for pair in validated
            if pair["treatment"]["agent"] == agent
            and pair["treatment"]["delivery_surface"] == surface
        ]
        by_agent_surface[f"{agent}/{surface}"] = {
            "pairs": len(attributed),
            "injections": sum(pair["treatment"]["injections_total"] for pair in attributed),
            "helpful_injections": sum(
                pair["treatment"]["helpful_injections"] for pair in attributed
            ),
        }

    injected_cost_values = [item["injected_cost_usd"] for item in treatments]
    all_costs_measured = all(value is not None for value in injected_cost_values)
    qualitative_examples_by_category = {
        category: sum(
            example["category"] == category
            for pair in validated
            for example in pair["qualitative_examples"]
        )
        for category in sorted(EXAMPLE_CATEGORIES)
    }
    metrics = {
        "planned_pairs": len(planned),
        "observed_pairs": len(validated),
        "repository_activation_coverage": activated_repositories / len(manifest["repositories"]),
        "helpful_injections": helpful,
        "wrong_injections": wrong,
        "stale_injections": stale,
        "ignored_injections": ignored,
        "helpful_ratio": ratio(helpful, rated),
        "wrong_ratio": ratio(wrong, rated),
        "stale_ratio": ratio(stale, rated),
        "corrections_baseline": sum(pair["baseline"]["corrections"] for pair in validated),
        "corrections_treatment": sum(pair["treatment"]["corrections"] for pair in validated),
        "correction_incidence_regression_pp": (correction_treatment - correction_base) * 100.0,
        "task_success_baseline": success_base,
        "task_success_treatment": success_treatment,
        "task_success_regression_pp": (success_base - success_treatment) * 100.0,
        "median_turns_saved": median(turns_saved),
        "turns_saved_q1": turns_saved_q1,
        "turns_saved_q3": turns_saved_q3,
        "median_active_seconds_saved": median(seconds_saved),
        "active_seconds_saved_q1": seconds_saved_q1,
        "active_seconds_saved_q3": seconds_saved_q3,
        "exact_recoveries": sum(item["exact_recoveries"] for item in treatments),
        "semantic_helpful_recalls": sum(item["semantic_helpful_recalls"] for item in treatments),
        "stale_wrong_exact_injections": sum(item["stale_wrong_exact_injections"] for item in treatments),
        "memory_induced_errors": sum(item["memory_induced_errors"] for item in treatments),
        "contested_regression_pp": regression_pp(contested_base, contested_treatment),
        "multi_answer_regression_pp": regression_pp(multi_base, multi_treatment),
        "provenance_label_coverage": ratio(sum(item["provenance_labeled_injections"] for item in treatments), injections),
        "contested_label_coverage": ratio(sum(item["contested_labeled_injections"] for item in treatments), sum(item["contested_injections"] for item in treatments)),
        "injected_tokens": sum(item["injected_tokens"] for item in treatments),
        "injected_tokens_per_pair": sum(item["injected_tokens"] for item in treatments) / len(validated),
        "injected_cost_usd": sum(float(value) for value in injected_cost_values) if all_costs_measured else None,
        "retention_actions": sum(item["retention_actions"] for item in treatments),
        "retention_failures": sum(item["retention_failures"] for item in treatments),
        "retention_active_seconds": sum(item["retention_active_seconds"] for item in treatments),
        "backup_attempts": sum(item["backup_attempts"] for item in treatments),
        "backup_failures": sum(item["backup_failures"] for item in treatments),
        "backup_active_seconds": sum(item["backup_active_seconds"] for item in treatments),
        "qualitative_examples_by_category": qualitative_examples_by_category,
    }

    stop_reasons: list[str] = []
    if metrics["memory_induced_errors"] > THRESHOLDS["memory_induced_errors_max"]:
        stop_reasons.append("memory-induced error observed")
    regression_thresholds = {
        "contested_regression_pp": "contested_regression_max_pp",
        "multi_answer_regression_pp": "multi_answer_regression_max_pp",
    }
    for name, threshold_name in regression_thresholds.items():
        value = metrics[name]
        if value is None and final:
            stop_reasons.append(f"{name} is unmeasured")
        elif value is not None and value > THRESHOLDS[threshold_name]:
            stop_reasons.append(f"{name} exceeded 1.0 pp")
    for name in ("provenance_label_coverage", "contested_label_coverage"):
        value = metrics[name]
        if (value is None and final) or (
            value is not None and value < THRESHOLDS[f"{name}_min"]
        ):
            stop_reasons.append(f"{name} fell below 100%")

    no_go_reasons: list[str] = []
    if final and not stop_reasons:
        if metrics["repository_activation_coverage"] < THRESHOLDS["useful_recall_repository_coverage_min"]:
            no_go_reasons.append("not every repository reached a useful recall")
        for name, repository_metrics in by_repository.items():
            first_useful = repository_metrics["time_to_first_useful_recall_hours"]
            if first_useful is None or first_useful > THRESHOLDS["time_to_first_useful_recall_hours_max"]:
                no_go_reasons.append(f"{name} missed the 72-hour activation threshold")
            backlog_growth = repository_metrics["review_backlog_growth"]
            if backlog_growth is None or backlog_growth > THRESHOLDS["review_backlog_growth_max"]:
                no_go_reasons.append(f"{name} review backlog grew")
        for name in ("helpful_ratio", "wrong_ratio", "stale_ratio"):
            value = metrics[name]
            if value is None:
                no_go_reasons.append(f"{name} is unmeasured")
        if metrics["helpful_ratio"] is not None and metrics["helpful_ratio"] < THRESHOLDS["helpful_ratio_min"]:
            no_go_reasons.append("helpful ratio below 80%")
        if metrics["wrong_ratio"] is not None and metrics["wrong_ratio"] > THRESHOLDS["wrong_ratio_max"]:
            no_go_reasons.append("wrong ratio above 10%")
        if metrics["stale_ratio"] is not None and metrics["stale_ratio"] > THRESHOLDS["stale_ratio_max"]:
            no_go_reasons.append("stale ratio above 10%")
        value_regression_thresholds = {
            "task_success_regression_pp": "task_success_regression_max_pp",
            "correction_incidence_regression_pp": "correction_incidence_regression_max_pp",
        }
        for name, threshold_name in value_regression_thresholds.items():
            if metrics[name] > THRESHOLDS[threshold_name]:
                no_go_reasons.append(f"{name} exceeded 1.0 pp")
        if metrics["median_turns_saved"] < THRESHOLDS["median_turns_saved_min"]:
            no_go_reasons.append("median turns regressed")
        if metrics["median_active_seconds_saved"] < THRESHOLDS["median_active_seconds_saved_min"]:
            no_go_reasons.append("median active time regressed")
        if metrics["median_turns_saved"] == 0 and metrics["median_active_seconds_saved"] == 0:
            no_go_reasons.append("neither paired efficiency metric improved")
        for name in ("exact_recoveries", "semantic_helpful_recalls"):
            if metrics[name] < THRESHOLDS[f"{name}_min"]:
                no_go_reasons.append(f"{name} below preregistered minimum")
        for name in ("backup_attempts", "retention_actions"):
            if metrics[name] < THRESHOLDS[f"{name}_min"]:
                no_go_reasons.append(f"{name} below preregistered minimum")
        for name in ("backup_failures", "retention_failures"):
            if metrics[name] > THRESHOLDS[f"{name}_max"]:
                no_go_reasons.append(f"{name} exceeded zero")

    verdict = "STOP" if stop_reasons else ("NO-GO" if final and no_go_reasons else ("GO" if final else "CONTINUE"))
    unmeasured = []
    if not all_costs_measured:
        unmeasured.append("injected_cost_usd")
    if any(value is None for value in (contested_base, contested_treatment)):
        unmeasured.append("contested_correctness_regression")
    if any(value is None for value in (multi_base, multi_treatment)):
        unmeasured.append("multi_answer_correctness_regression")
    return {
        "schema_version": SCHEMA_VERSION,
        "pilot_id": manifest["pilot_id"],
        "mode": "final" if final else "interim",
        "verdict": verdict,
        "stop_reasons": stop_reasons,
        "no_go_reasons": no_go_reasons,
        "metrics": metrics,
        "by_repository": by_repository,
        "by_surface": by_surface,
        "by_agent_surface": by_agent_surface,
        "unmeasured": unmeasured,
    }


def read_json(path: Path) -> Any:
    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        raise PilotError(f"cannot read JSON {path}: {error}") from error


def read_jsonl(path: Path) -> list[Any]:
    rows = []
    try:
        with path.open("r", encoding="utf-8") as handle:
            for line_number, line in enumerate(handle, 1):
                if not line.strip():
                    fail(f"{path}:{line_number}: blank lines are not allowed")
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError as error:
                    raise PilotError(f"{path}:{line_number}: invalid JSON") from error
    except OSError as error:
        raise PilotError(f"cannot read JSONL {path}: {error}") from error
    return rows


def write_json(path: Path, value: Any) -> None:
    try:
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    except OSError as error:
        raise PilotError(f"cannot write report {path}: {error}") from error


def self_test() -> None:
    def check(condition: bool, message: str) -> None:
        if not condition:
            raise AssertionError(message)

    start = "2026-08-01T00:00:00Z"
    end = "2026-08-15T00:00:00Z"
    manifest = {
        "schema_version": 1,
        "pilot_id": "self-test",
        "status": "admitted",
        "period": {"duration_days": 14, "start_utc": start, "end_utc": end},
        "paired_runs_per_repository": 5,
        "admission": {
            "frozen_at_utc": "2026-07-31T00:00:00Z",
            "task_56": {
                "state": "go",
                "evidence": f"commit:{'c' * 40}",
                "overall_pilot_go": True,
                "blocker": None,
                "qualified_treatment_arm": "linear-qualified",
                "unqualified_behavior_allowed": False,
            },
            "task_57": {
                "state": "go",
                "complete": True,
                "evidence": f"sha256:{'d' * 64}",
                "max_active_memories": 1000,
            },
        },
        "thresholds": copy.deepcopy(THRESHOLDS),
        "repositories": [{
            "name": "rusty-brain",
            "commit_sha": "a" * 40,
            "baseline_store_id": "baseline-store",
            "treatment_store_id": "treatment-store",
            "baseline_namespace": "baseline-ns",
            "treatment_namespace": "treatment-ns",
            "pair_plan": [
                {
                    "pair_id": f"pair-{index}",
                    "scenario_id": f"scenario-{index}",
                    "arm_order": "baseline_first" if index % 2 else "treatment_first",
                    "snapshot_sha256": "b" * 64,
                }
                for index in range(1, 6)
            ],
        }],
    }

    def make_pair(index: int) -> dict[str, Any]:
        common = {
            "store_id": "baseline-store",
            "namespace": "baseline-ns",
            "agent": "claude-code",
            "task_success": True,
            "turns": 5,
            "active_seconds": 300.0,
            "corrections": 0,
            "contested_opportunities": 1,
            "contested_correct": 1,
            "multi_answer_opportunities": 1,
            "multi_answer_correct": 1,
        }
        baseline = {"environment_id": f"baseline-{index}", **common}
        treatment = {
            "environment_id": f"treatment-{index}",
            **{**common, "store_id": "treatment-store", "namespace": "treatment-ns", "turns": 4, "active_seconds": 240.0},
            "arm_id": "linear-qualified",
            "delivery_surface": ("claude_native", "http", "mcp")[index % 3],
            "qualified_behavior_only": True,
            "injections_total": 1,
            "helpful_injections": 1,
            "wrong_injections": 0,
            "stale_injections": 0,
            "ignored_injections": 0,
            "stale_wrong_exact_injections": 0,
            "memory_induced_errors": 0,
            "exact_recoveries": 1 if index == 1 else 0,
            "semantic_helpful_recalls": 1 if index == 2 else 0,
            "injected_tokens": 50,
            "injected_cost_usd": None,
            "provenance_labeled_injections": 1,
            "contested_injections": 1,
            "contested_labeled_injections": 1,
            "corpus_rows_before": 10 + index,
            "corpus_rows_after": 11 + index,
            "corpus_bytes_before": 1000 + index,
            "corpus_bytes_after": 1100 + index,
            "review_backlog_before": 0,
            "review_backlog_after": 0,
            "retention_actions": 1 if index == 1 else 0,
            "retention_failures": 0,
            "retention_active_seconds": 0.0,
            "backup_attempts": 1,
            "backup_failures": 0,
            "backup_active_seconds": 2.0,
        }
        return {
            "schema_version": 1,
            "pair_id": f"pair-{index}",
            "scenario_id": f"scenario-{index}",
            "repository": "rusty-brain",
            "commit_sha": "a" * 40,
            "observed_at_utc": f"2026-08-0{index}T01:00:00Z",
            "arm_order": "baseline_first" if index % 2 else "treatment_first",
            "snapshot_sha256": "b" * 64,
            "baseline": baseline,
            "treatment": treatment,
            "qualitative_examples": [{
                "category": "helpful",
                "sanitized_summary": "Recovered the chosen test command without exposing stored text.",
                "provenance_label": "hook",
                "contested": False,
                "sanitization_attested": True,
            }],
        }

    pairs = [make_pair(index) for index in range(1, 6)]
    report = aggregate(manifest, pairs, final=True)
    check(report["verdict"] == "GO", f"expected GO, got {report}")
    check(report["metrics"]["injected_tokens"] == 250, "token aggregation drifted")
    check(report["unmeasured"] == ["injected_cost_usd"], "unmeasured fields drifted")

    mie_pairs = copy.deepcopy(pairs)
    mie_pairs[0]["treatment"]["memory_induced_errors"] = 1
    check(
        aggregate(manifest, mie_pairs, final=False)["verdict"] == "STOP",
        "MIE did not stop the pilot",
    )

    regression_pairs = copy.deepcopy(pairs)
    regression_pairs[0]["treatment"]["contested_correct"] = 0
    check(
        aggregate(manifest, regression_pairs, final=False)["verdict"] == "STOP",
        "contested regression did not stop the pilot",
    )

    no_value_pairs = copy.deepcopy(pairs)
    for pair in no_value_pairs:
        pair["treatment"]["helpful_injections"] = 0
        pair["treatment"]["ignored_injections"] = 1
        pair["treatment"]["exact_recoveries"] = 0
        pair["treatment"]["semantic_helpful_recalls"] = 0
    check(
        aggregate(manifest, no_value_pairs, final=True)["verdict"] == "NO-GO",
        "safe run without measured value did not return NO-GO",
    )

    unsafe_pairs = copy.deepcopy(pairs)
    unsafe_pairs[0]["qualitative_examples"][0]["sanitized_summary"] = "token=supersecret"
    try:
        aggregate(manifest, unsafe_pairs, final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("secret-like qualitative evidence was accepted")

    unqualified_pairs = copy.deepcopy(pairs)
    unqualified_pairs[0]["treatment"]["arm_id"] = "unqualified-arm"
    try:
        aggregate(manifest, unqualified_pairs, final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("unqualified treatment arm was accepted")

    try:
        aggregate(manifest, pairs[:-1], final=True)
    except PilotError:
        pass
    else:
        raise AssertionError("incomplete final run was accepted")

    check(
        aggregate(manifest, pairs[:1], final=False)["verdict"] == "CONTINUE",
        "safe interim observation did not continue",
    )
    blocked_manifest = copy.deepcopy(manifest)
    blocked_manifest["admission"]["task_56"]["state"] = "no-go"
    blocked_manifest["admission"]["task_56"]["overall_pilot_go"] = False
    blocked_manifest["admission"]["task_56"]["blocker"] = "poison-exposure"
    try:
        aggregate(blocked_manifest, pairs[:1], final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("task #56 NO-GO admitted treatment")
    incomplete_manifest = copy.deepcopy(manifest)
    incomplete_manifest["admission"]["task_57"]["state"] = "not-complete"
    incomplete_manifest["admission"]["task_57"]["complete"] = False
    try:
        aggregate(incomplete_manifest, pairs[:1], final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("incomplete task #57 evidence admitted treatment")
    boolean_threshold_manifest = copy.deepcopy(manifest)
    boolean_threshold_manifest["thresholds"]["memory_induced_errors_max"] = False
    try:
        aggregate(boolean_threshold_manifest, pairs[:1], final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("boolean threshold bypassed the frozen numeric value")
    placeholder_evidence = copy.deepcopy(manifest)
    placeholder_evidence["admission"]["task_56"]["evidence"] = "TBD"
    try:
        aggregate(placeholder_evidence, pairs[:1], final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("placeholder gate evidence admitted treatment")
    changed_snapshot = copy.deepcopy(pairs)
    changed_snapshot[0]["snapshot_sha256"] = "e" * 64
    try:
        aggregate(manifest, changed_snapshot, final=True)
    except PilotError:
        pass
    else:
        raise AssertionError("an observation changed the frozen fixture hash")
    double_counted_value = copy.deepcopy(pairs)
    double_counted_value[0]["treatment"]["semantic_helpful_recalls"] = 1
    try:
        aggregate(manifest, double_counted_value, final=True)
    except PilotError:
        pass
    else:
        raise AssertionError("one helpful injection counted as exact and semantic")
    secret_identifier = copy.deepcopy(manifest)
    secret_identifier["pilot_id"] = "ghp_" + "a" * 36
    try:
        aggregate(secret_identifier, pairs[:1], final=False)
    except PilotError:
        pass
    else:
        raise AssertionError("secret-shaped pilot identifier passed sanitization")
    cross_repository_reuse = copy.deepcopy(manifest)
    second = copy.deepcopy(cross_repository_reuse["repositories"][0])
    second["name"] = "threatmitigator"
    second["commit_sha"] = "f" * 40
    second["baseline_store_id"] = "treatment-store"
    second["baseline_namespace"] = "treatment-ns"
    second["treatment_store_id"] = "second-treatment-store"
    second["treatment_namespace"] = "second-treatment-ns"
    for index, plan in enumerate(second["pair_plan"], 1):
        plan["pair_id"] = f"second-pair-{index}"
        plan["scenario_id"] = f"second-scenario-{index}"
    cross_repository_reuse["repositories"].append(second)
    try:
        validate_manifest(cross_repository_reuse)
    except PilotError:
        pass
    else:
        raise AssertionError("cross-repository store/namespace reuse passed isolation")
    unmeasured_pairs = copy.deepcopy(pairs[:1])
    for arm in ("baseline", "treatment"):
        unmeasured_pairs[0][arm]["contested_opportunities"] = 0
        unmeasured_pairs[0][arm]["contested_correct"] = 0
        unmeasured_pairs[0][arm]["multi_answer_opportunities"] = 0
        unmeasured_pairs[0][arm]["multi_answer_correct"] = 0
    unmeasured_pairs[0]["treatment"]["contested_injections"] = 0
    unmeasured_pairs[0]["treatment"]["contested_labeled_injections"] = 0
    check(
        aggregate(manifest, unmeasured_pairs, final=False)["verdict"] == "CONTINUE",
        "unmeasured predeclared safety strata stopped an interim run too early",
    )
    with tempfile.TemporaryDirectory() as directory:
        events = Path(directory) / "events.jsonl"
        output = Path(directory) / "report.json"
        events.write_text(
            "".join(json.dumps(pair, sort_keys=True) + "\n" for pair in pairs),
            encoding="utf-8",
        )
        check(len(read_jsonl(events)) == 5, "JSONL round-trip failed")
        write_json(output, report)
        check(read_json(output)["verdict"] == "GO", "report round-trip failed")
    print("dogfood-pilot self-test PASS")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run API-free aggregation tests")
    subparsers = parser.add_subparsers(dest="command")
    validate_parser = subparsers.add_parser("validate", help="validate a frozen manifest")
    validate_parser.add_argument("--manifest", required=True, type=Path)
    aggregate_parser = subparsers.add_parser("aggregate", help="aggregate sanitized JSONL observations")
    aggregate_parser.add_argument("--manifest", required=True, type=Path)
    aggregate_parser.add_argument("--events", required=True, type=Path)
    aggregate_parser.add_argument("--output", required=True, type=Path)
    aggregate_parser.add_argument("--interim", action="store_true", help="evaluate stop rules before all pairs finish")
    args = parser.parse_args(argv)

    if args.self_test:
        if args.command is not None:
            parser.error("--self-test cannot be combined with a subcommand")
        self_test()
        return 0
    if args.command == "validate":
        validate_manifest(read_json(args.manifest))
        print("dogfood-pilot manifest valid and admitted")
        return 0
    if args.command == "aggregate":
        report = aggregate(read_json(args.manifest), read_jsonl(args.events), final=not args.interim)
        write_json(args.output, report)
        print(f"dogfood-pilot verdict: {report['verdict']}")
        if report["verdict"] == "STOP":
            return 3
        if report["verdict"] == "NO-GO":
            return 4
        return 0
    parser.error("choose --self-test, validate, or aggregate")
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except PilotError as error:
        print(f"dogfood-pilot error: {error}", file=sys.stderr)
        sys.exit(2)
