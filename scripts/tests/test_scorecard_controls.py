"""Tests for OMP prompt-time receipt accounting; no model process is started."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
CONTROLS = SCRIPTS / "scorecard-controls.py"


class ControlsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="scorecard-controls-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.on = self.root / "on"
        self.placebo = self.root / "placebo"
        self.on.mkdir()
        self.placebo.mkdir()

    def invoke(self, *args):
        return subprocess.run(
            ["python3", str(CONTROLS), *args],
            text=True,
            capture_output=True,
            timeout=10,
        )

    def receipt(self, directory, message):
        (directory / "prompt-time.jsonl").write_text(
            json.dumps({"message": message}) + "\n"
        )

    def score_row(self, arm, success, stale_evidence, evidence_reason, run="1",
                  is_error="false"):
        fields = [
            "freshness", "scenario", arm, run, str(success), "2", "0",
            "0", "0", "0", "0", "0", is_error,
            "na", "na", "0", "0",
            "0", "0", "0", "0", "utf8-bytes-div4-ceil-v1",
            "na", "na", stale_evidence, evidence_reason,
        ]
        return "\t".join(fields)

    def attribute(self, *rows):
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join(rows) + "\n")
        result = self.invoke("attribute-results", str(source), str(destination))
        self.assertEqual(result.returncode, 0, result.stderr)
        return [line.split("\t") for line in destination.read_text().splitlines()]
    def assertion_report(self):
        path = self.root / "assertions.json"
        required_cases = [
            {
                "id": case_id,
                "gate_required": True,
                "passed": True,
                "queries": [{
                    "expected_ids": [case_id],
                    "returned_ids": [case_id],
                    "returned_evidence": [{"id": case_id}],
                    "missing_ids": [],
                    "extra_ids": [],
                    "duplicate_ids": [],
                    "passed": True,
                }],
            }
            for case_id in (
                "supersede-chain",
                "five-slot-budget",
                "namespace-selection",
            )
        ]
        report = {
            "schema_version": 2,
            "judge_used": False,
            "no_judge_statement": "No model judge.",
            "passed_cases": 3,
            "total_cases": 4,
            "passed_required_cases": 3,
            "required_cases": 3,
            "all_cases_passed": False,
            "precision_gate_passed": True,
            "cases": required_cases + [{
                "id": "investigation",
                "gate_required": False,
                "passed": False,
                "queries": [{
                    "expected_ids": [],
                    "returned_ids": ["b"],
                    "returned_evidence": [{"id": "b"}],
                    "missing_ids": [],
                    "extra_ids": ["b"],
                    "duplicate_ids": [],
                    "passed": False,
                }],
            }],
        }
        path.write_text(json.dumps(report) + "\n")
        return path

    def test_missing_action_is_a_usage_error(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn("usage:", result.stderr.lower())
        self.assertNotIn("Traceback", result.stderr)

    def test_matched_omp_receipts_emit_a_pair_receipt(self):
        self.receipt(self.on, "stored decision")
        self.receipt(self.placebo, "." * len("stored decision"))
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads((self.placebo / "pair-check.json").read_text())
        self.assertTrue(receipt["matched"])
        self.assertEqual(receipt["channel"], "before_agent_start")
        self.assertFalse(receipt["exact_model_tokens"])
        self.assertEqual(receipt["per_invocation_field_sizes"], [4])

    def test_mismatched_receipts_fail_closed(self):
        self.receipt(self.on, "stored decision")
        self.receipt(self.placebo, ".")
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("control channel mismatch", result.stderr)

    def test_empty_receipts_fail_closed(self):
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("control channel mismatch", result.stderr)

    def test_control_error_file_fails_closed(self):
        self.receipt(self.on, "stored decision")
        self.receipt(self.placebo, "." * len("stored decision"))
        (self.placebo / "control-error.json").write_text('{"reason":"missing source"}\n')
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("placebo_error=True", result.stderr)

    def test_metrics_count_only_omp_prompt_receipts_and_agents_file(self):
        self.receipt(self.on, "four")
        result = self.invoke("metrics", str(self.on), "7")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip().split("\t"), [
            "0", "1", "7", "8", "utf8-bytes-div4-ceil-v1"
        ])

    def test_stale_evidence_requires_a_valid_prompt_receipt(self):
        result = self.invoke("stale-evidence", str(self.on), "reqwest")
        self.assertEqual(result.stdout.strip(), "unknown\tmissing_prompt_receipt")

        self.receipt(self.on, "Use reqwest for outbound HTTP")
        result = self.invoke("stale-evidence", str(self.on), "REQWEST")
        self.assertEqual(result.stdout.strip(), "1\tstale_injected")

        self.receipt(self.on, "Use ureq for outbound HTTP")
        result = self.invoke("stale-evidence", str(self.on), "reqwest")
        self.assertEqual(result.stdout.strip(), "0\tstale_not_injected")

    def test_stale_evidence_is_not_required_without_a_stale_candidate(self):
        result = self.invoke("stale-evidence", str(self.on), "")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "na\tno_stale_token")

    def test_paired_failure_with_injected_stale_content_is_attributed(self):
        rows = self.attribute(
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "na", "not_memory_on_arm"),
        )
        self.assertEqual(rows[0][6], "1")
        self.assertEqual(rows[0][26:], [
            "memory_induced", "memory_on_only_failure_with_stale_injection"
        ])

    def test_both_arms_failure_is_not_memory_induced(self):
        rows = self.attribute(
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 0, "na", "not_memory_on_arm"),
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "not_memory_induced", "both_arms_failed"
        ])

    def test_failure_without_injected_stale_content_is_not_attributed(self):
        rows = self.attribute(
            self.score_row("memory-on", 0, "0", "stale_not_injected"),
            self.score_row("memory-off", 1, "na", "not_memory_on_arm"),
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "not_memory_induced", "stale_not_injected"
        ])

    def test_missing_pair_or_evidence_is_unassessable(self):
        rows = self.attribute(
            self.score_row("memory-on", 0, "unknown", "missing_prompt_receipt")
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "unassessable", "missing_memory_off_pair"
        ])

        rows = self.attribute(
            self.score_row("memory-on", 0, "unknown", "missing_prompt_receipt"),
            self.score_row("memory-off", 1, "na", "not_memory_on_arm"),
        )
        self.assertEqual(rows[0][26:], [
            "unassessable", "missing_prompt_receipt"
        ])

    def test_attribution_is_retained_in_memory_on_diagnostics(self):
        diagnostics = self.root / "scenario-r1" / "on" / "diagnostics"
        diagnostics.mkdir(parents=True)
        outcome = diagnostics / "outcome.json"
        outcome.write_text(json.dumps({"success": 0}) + "\n")
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join([
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "na", "not_memory_on_arm"),
        ]) + "\n")
        result = self.invoke(
            "attribute-results", str(source), str(destination), str(self.root)
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        retained = json.loads(outcome.read_text())
        self.assertTrue(retained["paired_memory_off_success"])
        self.assertTrue(retained["memory_induced_error"])
        self.assertEqual(retained["causal_attribution"], "memory_induced")
        self.assertEqual(
            retained["causal_reason"],
            "memory_on_only_failure_with_stale_injection",
        )

    def test_assertion_gate_recomputes_exact_ids_and_required_scope(self):
        result = self.invoke("assertion-gate", str(self.assertion_report()))
        self.assertEqual(result.returncode, 0, result.stderr)
        gate = json.loads(result.stdout)
        self.assertTrue(gate["gate_passed"])
        self.assertEqual(gate["passed_required_cases"], 3)
        self.assertEqual(gate["required_cases"], 3)
        self.assertFalse(gate["all_cases_passed"])

    def test_assertion_gate_rejects_duplicate_ids_even_if_report_claims_pass(self):
        path = self.assertion_report()
        report = json.loads(path.read_text())
        query = report["cases"][0]["queries"][0]
        query["returned_evidence"] = [{"id": "a"}, {"id": "a"}]
        query["duplicate_ids"] = ["a"]
        query["returned_ids"] = ["a", "a"]
        path.write_text(json.dumps(report) + "\n")
        result = self.invoke("assertion-gate", str(path))
        self.assertNotEqual(result.returncode, 0)

    def test_metadata_records_integrated_no_judge_and_causal_gates(self):
        metadata_path = self.root / "metadata.json"
        assertion_report = self.assertion_report()
        result = self.invoke("metadata", str(metadata_path), "openai/test-model",
                             "test-version", str(CONTROLS), str(assertion_report))
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata = json.loads(metadata_path.read_text())
        self.assertEqual(metadata["agent"], "omp")
        self.assertEqual(metadata["schema_version"], 4)
        self.assertEqual(metadata["hard_gate"]["name"],
                         "exact-id-assertions-plus-paired-causal-safety")
        self.assertIn("memory-on failed", metadata["hard_gate"]["rule"])
        self.assertIn("unassessable", metadata["hard_gate"]["unsafe"])
        self.assertFalse(metadata["assertion_fixture"]["uses_model_judge"])
        self.assertTrue(metadata["assertion_fixture"]["executed_by_this_run"])
        self.assertTrue(metadata["assertion_fixture"]["gate_passed"])
        self.assertEqual(metadata["assertion_fixture"]["required_cases"], 3)
        self.assertIn("before_agent_start", metadata["injected_tokens"]["scope"])
        self.assertEqual(metadata["tsv_appended_columns"][-6:], [
            "session_start_answer_present", "prompt_recall_answer_present",
            "injected_stale_evidence", "injection_evidence_reason",
            "causal_attribution", "causal_reason",
        ])


if __name__ == "__main__":
    unittest.main()
