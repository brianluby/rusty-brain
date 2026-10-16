"""Offline regression for the nightly smoke's disposable Claude profile."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ClaudeProfileTest(unittest.TestCase):
    def test_generated_workspace_is_trusted_without_changing_other_profiles(self):
        script = (Path(__file__).resolve().parents[1] / "nightly-claude-smoke.sh").read_text()
        setup = script.split('python3 - "$PROJ/.claude/settings.json" <<\'PY\'\n', 1)[1].split('\nPY', 1)[0]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home, project = root / "home", root / "project"
            home.mkdir()
            (project / ".claude").mkdir(parents=True)
            settings = project / ".claude/settings.json"
            settings.write_text(json.dumps({"hooks": {"SessionStart": []},
                                            "permissions": {"allow": ["Bash"]}}))
            subprocess.run([sys.executable, "-", str(settings)], input=setup,
                           env=dict(os.environ, HOME=str(home)), cwd=project,
                           text=True, capture_output=True, check=True)
            self.assertEqual(json.loads((home / ".claude.json").read_text()), {
                "hasCompletedOnboarding": True,
                "projects": {str(project.resolve()): {"hasTrustDialogAccepted": True}},
            })
            actual = json.loads(settings.read_text())
            self.assertEqual(actual["hooks"], {"SessionStart": []})
            self.assertEqual(actual["permissions"]["allow"], ["Bash", "mcp__rusty-brain"])
            self.assertTrue(actual["enableAllProjectMcpServers"])
            self.assertFalse((root / ".claude.json").exists())

    def test_deepseek_uses_supported_body_cache_bypass(self):
        script = (Path(__file__).resolve().parents[1] / "nightly-claude-smoke.sh").read_text()
        setup = script.split("# LiteLLM caches must not replay", 1)[1].split("# Planted FAKE", 1)[0]
        setup = setup[setup.index('if [ "$MODEL"'):]
        for model in ("deepseek-v4.1-flash", "haiku"):
            environment = dict(os.environ, MODEL=model)
            environment.pop("CLAUDE_CODE_EXTRA_BODY", None)
            result = subprocess.run(
                ["bash", "-s"], input=setup + '\nprintf "%s" "${CLAUDE_CODE_EXTRA_BODY:-}"\n',
                env=environment, text=True, capture_output=True, check=True,
            )
            if model == "deepseek-v4.1-flash":
                self.assertEqual(json.loads(result.stdout), {
                    "cache": {"no-cache": True, "no-store": True},
                    "thinking": {"type": "disabled"},
                })
            else:
                self.assertEqual(result.stdout, "")


class ScorecardProviderProfileTest(unittest.TestCase):
    def test_provider_profile_copies_catalog_prices_but_not_credentials(self):
        script_path = Path(__file__).resolve().parents[1] / "memory-scorecard.sh"
        prefix = script_path.read_text().split("# ---- arg parsing", 1)[0]
        model = {"id": "deepseek-v4.1-flash", "cost": {"input": 0.3, "output": 1.2}}
        with tempfile.TemporaryDirectory() as tmp:
            home, project = Path(tmp) / "home", Path(tmp) / "project"
            subprocess.run(
                ["bash", "-s", str(home), str(project)],
                input=prefix + '\nseed_home "$1" "$2"\n',
                env=dict(os.environ, LITELLM_MODEL_JSON=json.dumps(model),
                         LITELLM_BASE_URL="https://proxy.invalid/v1",
                         LITELLM_API_KEY="offline-regression-secret"),
                text=True, capture_output=True, check=True,
            )
            text = (home / ".omp/agent/models.yml").read_text()
            provider = json.loads(text)["providers"]["litellm"]
            configured_model = provider["models"][0]
            self.assertEqual({key: configured_model[key] for key in model}, model)
            self.assertEqual(configured_model["compat"], {
                "replayReasoningContent": True,
                "extraBody": {"cache": {"no-cache": True, "no-store": True},
                              "thinking": {"type": "disabled"}},
            })
            self.assertEqual(provider["apiKey"], "LITELLM_API_KEY")
            self.assertEqual(provider["baseUrl"], "https://proxy.invalid/v1")
            self.assertNotIn("offline-regression-secret", text)
            self.assertTrue(project.is_dir())


if __name__ == "__main__":
    unittest.main()
