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

    def test_metadata_names_omp_and_separate_assertion_fixture(self):
        metadata_path = self.root / "metadata.json"
        result = self.invoke("metadata", str(metadata_path), "openai/test-model",
                             "test-version", str(CONTROLS))
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata = json.loads(metadata_path.read_text())
        self.assertEqual(metadata["agent"], "omp")
        self.assertEqual(metadata["schema_version"], 3)
        self.assertFalse(metadata["assertion_fixture"]["uses_model_judge"])
        self.assertFalse(metadata["assertion_fixture"]["executed_by_this_run"])
        self.assertFalse(metadata["assertion_fixture"]["coverage_equivalent"])
        self.assertIn("before_agent_start", metadata["injected_tokens"]["scope"])


if __name__ == "__main__":
    unittest.main()
