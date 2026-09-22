"""Tests for OMP prompt-time receipt accounting; no model process is started."""
from contextlib import redirect_stderr
import io
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest import mock

SCRIPTS = Path(__file__).resolve().parents[1]
CONTROLS = SCRIPTS / "scorecard-controls.py"


class ControlsTest(unittest.TestCase):
    def setUp(self):
        """Create isolated receipt and output directories for each regression."""
        self.tmp = tempfile.TemporaryDirectory(prefix="scorecard-controls-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.on = self.root / "on"
        self.placebo = self.root / "placebo"
        self.on.mkdir()
        self.placebo.mkdir()

    def invoke(self, *args):
        """Run the controls CLI and retain both output channels for assertions."""
        return subprocess.run(
            ["python3", str(CONTROLS), *args],
            text=True,
            capture_output=True,
            timeout=10,
        )

    def receipt(self, directory, message):
        """Write one valid prompt preparation receipt."""
        (directory / "prompt-time.jsonl").write_text(
            json.dumps({"message": message}) + "\n"
        )

    def score_row(self, arm, success, stale_evidence, evidence_reason, run="1",
                  is_error="false"):
        """Build a complete pre-attribution row with controllable causal evidence."""
        fields = [
            "freshness", "scenario", arm, run, str(success), "2", "0",
            "0", "0", "0", "0", "0", is_error,
            "na", "na", "0", "0",
            "0", "0", "0", "0", "utf8-bytes-div4-ceil-v1",
            "na", "na", stale_evidence, evidence_reason,
        ]
        return "\t".join(fields)

    def attribute(self, *rows):
        """Attribute rows through the CLI and return the consumer-visible fields."""
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join(rows) + "\n")
        result = self.invoke("attribute-results", str(source), str(destination))
        self.assertEqual(result.returncode, 0, result.stderr)
        return [line.split("\t") for line in destination.read_text().splitlines()]

    def assertion_report(self):
        """Create a consistent local report with passing required cases."""
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
        """A missing command must produce usage rather than a Python traceback."""
        result = self.invoke()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn("usage:", result.stderr.lower())
        self.assertNotIn("Traceback", result.stderr)

    def test_matched_omp_receipts_emit_a_pair_receipt(self):
        """Equal estimated receipt sizes produce a retained successful pair check."""
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
        """A placebo with different preparation size cannot pass the pair check."""
        self.receipt(self.on, "stored decision")
        self.receipt(self.placebo, ".")
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("control channel mismatch", result.stderr)

    def test_empty_receipts_fail_closed(self):
        """Missing receipts are not evidence of a matched control."""
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("control channel mismatch", result.stderr)

    def test_control_error_file_fails_closed(self):
        """A control error invalidates otherwise matching receipt sizes."""
        self.receipt(self.on, "stored decision")
        self.receipt(self.placebo, "." * len("stored decision"))
        (self.placebo / "control-error.json").write_text('{"reason":"missing source"}\n')
        result = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("placebo_error=True", result.stderr)

    def test_metrics_count_only_omp_prompt_receipts_and_agents_file(self):
        """Token accounting combines prompt preparation with the seeded file."""
        self.receipt(self.on, "four")
        result = self.invoke("metrics", str(self.on), "7")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip().split("\t"), [
            "0", "1", "7", "8", "utf8-bytes-div4-ceil-v1"
        ])

    def test_stale_evidence_requires_a_unique_stale_record_marker(self):
        """Only the predecessor-specific marker establishes stale injection."""
        marker = "for outbound HTTP this project uses the `reqwest` crate"
        result = self.invoke("stale-evidence", str(self.on), marker)
        self.assertEqual(result.stdout.strip(), "unknown\tmissing_prompt_receipt")

        self.receipt(
            self.on,
            "Update: we migrated off reqwest. All HTTP now uses ureq.",
        )
        result = self.invoke("stale-evidence", str(self.on), marker)
        self.assertEqual(result.stdout.strip(), "0\tstale_not_injected")

        self.receipt(self.on, f"Decision: {marker}.")
        result = self.invoke("stale-evidence", str(self.on), marker.upper())
        self.assertEqual(result.stdout.strip(), "1\tstale_injected")

    def test_stale_evidence_is_not_required_without_a_stale_candidate(self):
        """Scenarios without stale candidates do not require injection receipts."""
        result = self.invoke("stale-evidence", str(self.on), "")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "na\tno_stale_marker")

    def test_paired_failure_with_injected_stale_content_is_attributed(self):
        """A failed on arm and clean successful control establish the differential."""
        rows = self.attribute(
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )
        self.assertEqual(rows[0][6], "1")
        self.assertEqual(rows[0][26:], [
            "memory_induced", "memory_on_only_failure_with_stale_injection"
        ])

    def test_both_arms_failure_is_not_memory_induced(self):
        """Failure shared by clean paired arms is not a memory-only differential."""
        rows = self.attribute(
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 0, "0", "extension_disabled"),
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "not_memory_induced", "both_arms_failed"
        ])

    def test_failure_without_injected_stale_content_is_not_attributed(self):
        """A failed on arm without stale injection cannot establish attribution."""
        rows = self.attribute(
            self.score_row("memory-on", 0, "0", "stale_not_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "not_memory_induced", "stale_not_injected"
        ])

    def test_missing_pair_or_evidence_is_unassessable(self):
        """Missing paired outcomes and missing on receipts stay unassessable."""
        rows = self.attribute(
            self.score_row("memory-on", 0, "unknown", "missing_prompt_receipt")
        )
        self.assertEqual(rows[0][6], "0")
        self.assertEqual(rows[0][26:], [
            "unassessable", "missing_memory_off_pair"
        ])

        rows = self.attribute(
            self.score_row("memory-on", 0, "unknown", "missing_prompt_receipt"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )
        self.assertEqual(rows[0][26:], [
            "unassessable", "missing_prompt_receipt"
        ])

    def test_attribution_is_retained_in_memory_on_diagnostics(self):
        """Retained diagnostics expose the same causal result as the TSV."""
        diagnostics = self.root / "scenario-r1" / "on" / "diagnostics"
        diagnostics.mkdir(parents=True)
        outcome = diagnostics / "outcome.json"
        outcome.write_text(json.dumps({"success": 0}) + "\n")
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join([
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
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

    def test_receipt_readers_ignore_blank_lines_consistently(self):
        """Blank separators must not break metrics, pair checks, or stale detection."""
        content = '\n  \n{"message": "four"}\n\n'
        (self.on / "prompt-time.jsonl").write_text(content)
        (self.placebo / "prompt-time.jsonl").write_text(content)
        metrics = self.invoke("metrics", str(self.on), "0")
        self.assertEqual(metrics.returncode, 0, metrics.stderr)
        self.assertEqual(metrics.stdout.split("\t")[1], "1")
        pair = self.invoke("validate-pair", str(self.on), str(self.placebo))
        self.assertEqual(pair.returncode, 0, pair.stderr)
        evidence = self.invoke("stale-evidence", str(self.on), "four")
        self.assertEqual(evidence.stdout.strip(), "1\tstale_injected")

    def test_malformed_receipts_fail_closed_without_tracebacks(self):
        """Broken JSON, invalid UTF-8, and invalid message shapes stay unassessable."""
        self.receipt(self.placebo, "four")
        for malformed in (b"{", b"\xff", b"[]", b'"text"', b"{}", b'{"message": 1}'):
            with self.subTest(receipt=malformed):
                # A valid prefix must not hide a malformed trailing record.
                (self.on / "prompt-time.jsonl").write_bytes(
                    b'{"message": "four"}\n\n' + malformed + b"\n"
                )
                for args in (
                    ("metrics", str(self.on), "0"),
                    ("validate-pair", str(self.on), str(self.placebo)),
                ):
                    result = self.invoke(*args)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, "")
                    self.assertIn("prompt receipt", result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                self.assertFalse((self.placebo / "pair-check.json").exists())
                evidence = self.invoke("stale-evidence", str(self.on), "four")
                self.assertEqual(evidence.returncode, 0, evidence.stderr)
                self.assertEqual(
                    evidence.stdout.strip(), "unknown\tmalformed_prompt_receipt"
                )

    def test_unreadable_receipt_remains_unknown(self):
        """A receipt read error is reported, never interpreted as clean evidence."""
        (self.on / "prompt-time.jsonl").mkdir()
        metrics = self.invoke("metrics", str(self.on), "0")
        self.assertNotEqual(metrics.returncode, 0)
        self.assertNotIn("Traceback", metrics.stderr)
        evidence = self.invoke("stale-evidence", str(self.on), "four")
        self.assertEqual(evidence.stdout.strip(), "unknown\tmalformed_prompt_receipt")

    def test_stale_pairs_require_clean_assessable_controls(self):
        """Missing or contaminated controls invalidate even an otherwise safe pair."""
        for control, reason in (
            ("1", "contaminated_control_evidence"),
            ("unknown", "missing_control_evidence"),
            ("na", "missing_control_evidence"),
        ):
            for success in (0, 1):
                with self.subTest(control=control, success=success):
                    rows = self.attribute(
                        self.score_row("memory-on", success, "1", "stale_injected"),
                        self.score_row("memory-off", 1, control, "control_state"),
                    )
                    self.assertEqual(rows[0][26:], ["unassessable", reason])
                    self.assertEqual(rows[0][6], "0")

    def test_no_stale_candidate_does_not_require_control_evidence(self):
        """Non-stale scenarios retain their not-memory-induced classification."""
        rows = self.attribute(
            self.score_row("memory-on", 0, "na", "no_stale_marker"),
            self.score_row("memory-off", 1, "na", "no_stale_marker"),
        )
        self.assertEqual(rows[0][26:], ["not_memory_induced", "no_stale_candidate"])

    def test_attribution_never_clears_existing_failure(self):
        """Both unassessable and noncausal outcomes preserve a runner MIE flag."""
        for evidence, reason in (
            ("unknown", "missing_prompt_receipt"),
            ("0", "stale_not_injected"),
        ):
            with self.subTest(evidence=evidence):
                on = self.score_row("memory-on", 0, evidence, reason).split("\t")
                on[6] = "1"
                rows = self.attribute(
                    "\t".join(on),
                    self.score_row("memory-off", 1, "0", "extension_disabled"),
                )
                self.assertEqual(rows[0][6], "1")
                self.assertNotEqual(rows[0][26], "memory_induced")

    def test_schema_rejection_precedes_all_mutations(self):
        """A malformed later row leaves the destination and prior diagnostics intact."""
        outcome = self.root / "scenario-r1" / "on" / "diagnostics" / "outcome.json"
        outcome.parent.mkdir(parents=True)
        original = '{"success": 0}\n'
        outcome.write_text(original)
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        destination.write_text("existing results\n")
        on = self.score_row("memory-on", 0, "1", "stale_injected")
        off = self.score_row("memory-off", 1, "0", "extension_disabled")
        for bad in (on.rsplit("\t", 1)[0], on + "\textra", "agent=unknown"):
            with self.subTest(row=bad):
                source.write_text("\n".join((on, off, bad)) + "\n")
                result = self.invoke(
                    "attribute-results", str(source), str(destination), str(self.root)
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("26 pre-attribution fields", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(outcome.read_text(), original)
                self.assertEqual(destination.read_text(), "existing results\n")

    def test_known_agent_skip_rows_survive_attribution(self):
        """Unsupported-agent records stay unchanged alongside exact 28-field results."""
        skip = (
            "agent=codex\tdimension=all\tscenario=all\tphase=capture\tstatus=skip"
            "\treason=scorecard_unsupported_codex_fixture_gated\tdetail=fixture gated"
        )
        rows = self.attribute(
            skip,
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )
        self.assertEqual("\t".join(rows[0]), skip)
        self.assertEqual([len(row) for row in rows[1:]], [28, 28])
        self.assertEqual(rows[1][26], "memory_induced")

    def test_tsv_path_components_cannot_escape_diagnostics(self):
        """Absolute scenarios and traversal in either component cannot modify files."""
        root = self.root / "diagnostics"
        root.mkdir()
        outcome = root / "scenario-r1" / "on" / "diagnostics" / "outcome.json"
        outcome.parent.mkdir(parents=True)
        original = '{"success": 0}\n'
        outcome.write_text(original)
        external = self.root / "outside-r1" / "on" / "diagnostics" / "outcome.json"
        external.parent.mkdir(parents=True)
        external.write_text(original)
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        on = self.score_row("memory-on", 0, "1", "stale_injected")
        off = self.score_row("memory-off", 1, "0", "extension_disabled")
        for index, value in (
            (1, str(self.root / "outside")),
            (1, "../outside"),
            (3, "../../../outside"),
            (3, "/absolute"),
        ):
            with self.subTest(component=index, value=value):
                bad = on.split("\t")
                bad[index] = value
                source.write_text("\n".join((on, off, "\t".join(bad))) + "\n")
                result = self.invoke(
                    "attribute-results", str(source), str(destination), str(root)
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("path component", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(outcome.read_text(), original)
                self.assertEqual(external.read_text(), original)
                self.assertFalse(destination.exists())

    def test_resolved_sidecar_symlinks_cannot_escape_diagnostics(self):
        """Both directory and leaf symlinks must remain within the diagnostics root."""
        root = self.root / "diagnostics"
        root.mkdir()
        external = self.root / "outside"
        external.mkdir()
        original = '{"success": 0}\n'
        outside_outcome = external / "outcome.json"
        outside_outcome.write_text(original)
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join((
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )) + "\n")
        diagnostics = root / "scenario-r1" / "on" / "diagnostics"
        diagnostics.parent.mkdir(parents=True)
        for leaf_only in (False, True):
            with self.subTest(leaf_only=leaf_only):
                if leaf_only:
                    diagnostics.mkdir()
                    (diagnostics / "outcome.json").symlink_to(outside_outcome)
                else:
                    diagnostics.symlink_to(external, target_is_directory=True)
                result = self.invoke(
                    "attribute-results", str(source), str(destination), str(root)
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("escapes diagnostics root", result.stderr)
                self.assertEqual(outside_outcome.read_text(), original)
                self.assertFalse(destination.exists())
                if not leaf_only:
                    diagnostics.unlink()

    def test_bad_sidecars_do_not_discard_valid_tsv_or_later_diagnostics(self):
        """Corrupt, non-object, invalid-UTF-8, and unreadable sidecars are optional."""
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        rows = []
        for run in range(1, 6):
            rows.extend((
                self.score_row("memory-on", 0, "1", "stale_injected", run=str(run)),
                self.score_row("memory-off", 1, "0", "extension_disabled", run=str(run)),
            ))
        source.write_text("\n".join(rows) + "\n")
        outcomes = []
        for run, content in enumerate((b"{", b"[]", b"\xff", None, b'{"success": 0}'), 1):
            outcome = self.root / f"scenario-r{run}" / "on" / "diagnostics" / "outcome.json"
            outcome.parent.mkdir(parents=True)
            if content is None:
                outcome.mkdir()
            else:
                outcome.write_bytes(content)
            outcomes.append(outcome)
        result = self.invoke(
            "attribute-results", str(source), str(destination), str(self.root)
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr.count("warning:"), 4)
        self.assertNotIn("Traceback", result.stderr)
        scored = [line.split("\t") for line in destination.read_text().splitlines()]
        self.assertEqual(len(scored), 10)
        self.assertTrue(all(row[26] == "memory_induced" for row in scored[::2]))
        retained = json.loads(outcomes[-1].read_text())
        self.assertTrue(retained["memory_induced_error"])
        self.assertEqual(outcomes[0].read_bytes(), b"{")

    def test_sidecar_write_error_does_not_discard_tsv(self):
        """A failed diagnostics write must leave the attributed TSV available."""
        outcome = self.root / "scenario-r1" / "on" / "diagnostics" / "outcome.json"
        outcome.parent.mkdir(parents=True)
        outcome.write_text('{"success": 0}\n')
        source = self.root / "raw.tsv"
        destination = self.root / "results.tsv"
        source.write_text("\n".join((
            self.score_row("memory-on", 0, "1", "stale_injected"),
            self.score_row("memory-off", 1, "0", "extension_disabled"),
        )) + "\n")
        controls = runpy.run_path(str(CONTROLS))
        write_text = Path.write_text

        def fail_sidecar_write(path, *args, **kwargs):
            """Inject a disk failure only for the optional outcome sidecar."""
            if path == outcome.resolve():
                raise OSError("simulated disk write failure")
            return write_text(path, *args, **kwargs)

        warnings = io.StringIO()
        with mock.patch.object(Path, "write_text", fail_sidecar_write):
            with redirect_stderr(warnings):
                controls["attribute_results"](source, destination, self.root)
        self.assertIn("warning:", warnings.getvalue())
        self.assertIn("simulated disk write failure", warnings.getvalue())
        scored = destination.read_text().splitlines()[0].split("\t")
        self.assertEqual(scored[6], "1")
        self.assertEqual(scored[26], "memory_induced")

    def test_assertion_gate_recomputes_exact_ids_and_required_scope(self):
        """Only required cases determine the gate; optional failures remain visible."""
        result = self.invoke("assertion-gate", str(self.assertion_report()))
        self.assertEqual(result.returncode, 0, result.stderr)
        gate = json.loads(result.stdout)
        self.assertTrue(gate["gate_passed"])
        self.assertEqual(gate["passed_required_cases"], 3)
        self.assertEqual(gate["required_cases"], 3)
        self.assertFalse(gate["all_cases_passed"])

    def test_assertion_gate_rejects_duplicate_ids_even_if_report_claims_pass(self):
        """Duplicate returned IDs invalidate a report's claimed passing verdict."""
        path = self.assertion_report()
        report = json.loads(path.read_text())
        query = report["cases"][0]["queries"][0]
        returned_id = query["expected_ids"][0]
        query["returned_evidence"] = [{"id": returned_id}, {"id": returned_id}]
        query["duplicate_ids"] = [returned_id]
        query["returned_ids"] = [returned_id, returned_id]
        path.write_text(json.dumps(report) + "\n")
        result = self.invoke("assertion-gate", str(path))
        self.assertNotEqual(result.returncode, 0)

    def test_metadata_records_integrated_no_judge_and_causal_gates(self):
        """Metadata identifies the supplied report and exposes its evaluated gate."""
        metadata_path = self.root / "metadata.json"
        assertion_report = self.assertion_report()
        result = self.invoke("metadata", str(metadata_path), "openai/test-model",
                             "test-version", str(CONTROLS), str(assertion_report))
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata = json.loads(metadata_path.read_text())
        self.assertFalse(metadata["assertion_fixture"]["uses_model_judge"])
        self.assertTrue(metadata["assertion_fixture"]["executed_by_this_run"])
        self.assertTrue(metadata["assertion_fixture"]["gate_passed"])
        self.assertEqual(metadata["assertion_fixture"]["required_cases"], 3)
        report_path = Path(metadata["sidecars"]["assertion_report"])
        self.assertEqual(report_path, assertion_report)
        self.assertTrue(report_path.is_file())
        self.assertEqual(metadata["tsv_appended_columns"][-6:], [
            "session_start_answer_present", "prompt_recall_answer_present",
            "injected_stale_evidence", "injection_evidence_reason",
            "causal_attribution", "causal_reason",
        ])


if __name__ == "__main__":
    unittest.main()
