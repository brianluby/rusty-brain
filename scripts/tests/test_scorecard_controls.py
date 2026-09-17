"""Exercise the real bash runner with a deterministic, offline process transport."""
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
ARMS = {"memory-on", "realistic-baseline", "steelman-baseline", "memory-off", "length-matched-placebo"}


def texts(payload):
    return [payload.get("hookSpecificOutput", {}).get("additionalContext", "")]


def estimate(text):
    return (len(text.encode("utf-8")) + 3) // 4


class ControlsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="scorecard-controls-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        fixture = SCRIPTS / "tests/fake-scorecard-transport.py"
        # Wrapper executables preserve argv[0] for the dispatch fixture.
        for name in ("rusty-brain", "rusty-brain-hooks", "rusty-brain-install", "claude"):
            p = self.bin / name
            p.write_text(fixture.read_text())
            p.chmod(0o755)
        self.scenarios = self.root / "scenarios.json"
        self.scenarios.write_text(json.dumps({"config": {"runs_per_scenario": 1, "min_runs": 1}, "scenarios": [{
            "id": "control", "dimension": "freshness", "plant_mode": "explicit",
            "plant": [{"content": "use CURRENT_CHOICE"}], "work": "Choose the stored convention.",
            "expect": "CURRENT_CHOICE", "stale_token": "OLD_CHOICE",
            "realistic_claude_md": "use OLD_CHOICE", "steelman_claude_md": "use CURRENT_CHOICE"}]}))
        self.trace = self.root / "trace.jsonl"
        self.out = self.root / "results.tsv"
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                        ANTHROPIC_API_KEY="fake-offline-test-not-a-key", FAKE_TRACE=str(self.trace))

    def run_scorecard(self, **env):
        return subprocess.run(["bash", str(SCRIPTS / "memory-scorecard.sh"), "--bin-dir", str(self.bin),
                               "--scenarios-file", str(self.scenarios), "--out", str(self.out),
                               "--log-dir", str(self.root / "logs")],
                              env=dict(self.env, **env), text=True, capture_output=True, timeout=45)

    def test_placebo_matches_actual_emitted_channels_and_off_is_hook_free(self):
        result = self.run_scorecard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = [line.split("\t") for line in self.out.read_text().splitlines() if not line.startswith("#")]
        self.assertEqual({r[2] for r in rows}, ARMS)
        trace = [json.loads(line) for line in self.trace.read_text().splitlines()]
        on = next(t for t in trace if "/on/" in t["project"])
        placebo = next(t for t in trace if "/placebo/" in t["project"])
        off = next(t for t in trace if "/off/" in t["project"])
        self.assertEqual(off["hooks"], {})
        self.assertFalse(off["claude_md"])
        for event in ("SessionStart", "UserPromptSubmit"):
            a, b = on["injections"][event], placebo["injections"][event]
            self.assertEqual(len(a), 1)
            self.assertEqual(len(b), 1)
            self.assertEqual(b[0].get("continue"), a[0].get("continue"))
            self.assertEqual(b[0].get("suppressOutput"), a[0].get("suppressOutput"))
            self.assertEqual([estimate(s) for s in texts(a[0])], [estimate(s) for s in texts(b[0])])
            self.assertNotIn("CURRENT_CHOICE", json.dumps(b))
            self.assertNotIn("OLD_CHOICE", json.dumps(b))
            self.assertGreater(sum(map(estimate, texts(b[0]))), 0)

    def test_user_facing_system_message_is_not_counted_as_model_context(self):
        result = self.run_scorecard(FAKE_UI_MESSAGE="1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = [line.split("\t") for line in self.out.read_text().splitlines()]
        row = next(r for r in rows if r[2] == "memory-on")
        trace = [json.loads(line) for line in self.trace.read_text().splitlines()]
        on = next(t for t in trace if "/on/" in t["project"])
        measured = [sum(estimate(s) for p in on["injections"][event] for s in texts(p))
                    for event in ("SessionStart", "UserPromptSubmit")]
        self.assertEqual(list(map(int, row[17:19])), measured)

    def test_control_receipts_survive_cleanup_at_log_dir(self):
        result = self.run_scorecard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        log = self.root / "logs/control-r1"
        self.assertTrue((log / "placebo/work.jsonl").is_file(), "placebo session log must be retained")
        for arm in ("on", "placebo"):
            for channel in ("session-start", "prompt-time"):
                self.assertTrue((log / arm / "injections" / (channel + ".jsonl")).is_file())
        receipt = json.loads((log / "placebo/injections/pair-check.json").read_text())
        self.assertTrue(receipt["matched"])
        self.assertFalse(receipt["exact_model_tokens"])
        self.assertFalse((log / "on/injections/commands.json").exists())

    def test_empty_prompt_channel_stays_empty_not_fabricated_padding(self):
        result = self.run_scorecard(FAKE_EMPTY_CHANNEL="UserPromptSubmit")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = [line.split("\t") for line in self.out.read_text().splitlines()]
        for row in rows:
            if row[2] in ("memory-on", "length-matched-placebo"):
                self.assertGreater(int(row[17]), 0)
                self.assertEqual(int(row[18]), 0)

    def test_legacy_stale_substring_gate_still_fails_complete_run(self):
        result = self.run_scorecard(FAKE_STALE_OUTPUT="1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("memory-induced errors: 1", result.stdout)
        self.assertIn("result: UNSAFE", result.stdout)

    def test_reach_pairs_actual_identity_b_hook_output(self):
        data = json.loads(self.scenarios.read_text())
        data["scenarios"][0]["dimension"] = "reach"
        self.scenarios.write_text(json.dumps(data))
        result = self.run_scorecard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        trace = [json.loads(line) for line in self.trace.read_text().splitlines()]
        on = next(t for t in trace if "/on/" in t["project"])
        self.assertTrue(on["project"].endswith("/on/pb"))
        self.assertTrue((self.root / "logs/control-r1/placebo/injections/pair-check.json").exists())

    def test_failed_extra_placebo_hook_cannot_be_hidden_by_agent_fail_open(self):
        result = self.run_scorecard(FAKE_EXTRA_PLACEBO_HOOK="1")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("control hook error", result.stderr)
        self.assertNotIn("result: SAFE", result.stdout)
        self.assertTrue((self.root / "logs/control-r1/placebo/injections/error.json").exists())

    def test_missing_placebo_channel_fails_closed_instead_of_claiming_matched(self):
        result = self.run_scorecard(FAKE_SKIP_PLACEBO_PROMPT="1")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("control channel mismatch", result.stderr)
        self.assertNotIn("result: SAFE", result.stdout)

    def test_every_reported_rate_carries_arm_injection_sizes_and_proxy_label(self):
        data = json.loads(self.scenarios.read_text())
        data["config"]["runs_per_scenario"] = 2
        scenario = data["scenarios"][0]
        data["scenarios"] += [dict(scenario, id="scale", dimension="retrieval_scale"),
                              dict(scenario, id="capture", dimension="capture", plant_mode="auto-capture",
                                   plant=[], plant_session="Record CURRENT_CHOICE")]
        self.scenarios.write_text(json.dumps(data))
        result = self.run_scorecard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = [line.split("\t") for line in self.out.read_text().splitlines() if not line.startswith("#")]
        self.assertTrue(all(len(row) == 22 for row in rows), "TSV needs per-channel and total estimates + estimator")
        for row in rows:
            start, prompt, file_tokens, total = map(int, row[17:21])
            self.assertEqual(total, start + prompt + file_tokens)
            self.assertEqual(row[21], "utf8-bytes-div4-ceil-v1")
            if row[2] == "memory-off":
                self.assertEqual(total, 0)
            if row[2] in ("memory-on", "length-matched-placebo"):
                self.assertGreater(start, prompt)
                self.assertGreater(prompt, 0)
                self.assertEqual(file_tokens, 0)
        rate_lines = [line for line in result.stdout.splitlines() if "% [" in line or "%=" in line or "cache%" not in line and re.search(r"\d%\s+\d", line)]
        self.assertGreater(len(rate_lines), 15)
        for line in rate_lines:
            self.assertIn("injected_est[", line, line)
        self.assertIn("length-matched-placebo", result.stdout)
        self.assertIn("legacy-substring-proxy", result.stdout)
        metadata = json.loads(Path(str(self.out) + ".metadata.json").read_text())
        self.assertEqual(metadata["scoring"], "legacy-substring-proxy")
        self.assertEqual(metadata["assertion_fixture"]["uses_model_judge"], False)
        self.assertEqual(metadata["assertion_fixture"]["executed_by_this_run"], False)
        self.assertEqual(metadata["assertion_fixture"]["coverage_equivalent"], False)
        self.assertEqual(metadata["injected_tokens"]["exact_model_tokens"], False)
        for row in rows:
            if row[2] == "memory-on":
                paired = next(r for r in rows if r[1] == row[1] and r[3] == row[3] and r[2] == "length-matched-placebo")
                self.assertEqual(row[17:22], paired[17:22])
        for arm in ARMS:
            selected = [r for r in rows if r[0] == "freshness" and r[2] == arm]
            means = [sum(int(r[i]) for r in selected) / len(selected) for i in range(17, 21)]
            label = "injected_est[start=%.1f,prompt=%.1f,file=%.1f,total=%.1f]" % tuple(means)
            line = next(line for line in result.stdout.splitlines() if line.startswith("freshness ") and arm in line)
            self.assertIn(label, line)


if __name__ == "__main__":
    unittest.main()
