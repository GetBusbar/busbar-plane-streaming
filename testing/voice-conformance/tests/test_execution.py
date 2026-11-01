#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright (C) 2026 Busbar Inc and contributors

"""Exercise child exits and cached-battery grading through the actual shell entry points."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


BATTERY = Path(__file__).resolve().parents[1]
ROOT = BATTERY.parents[1]


class Execution(unittest.TestCase):
    def leg(self, body, kind="conformance"):
        with tempfile.TemporaryDirectory() as temporary:
            legs = Path(temporary)
            (legs / "child.sh").write_text(
                f"LEG_KIND={kind}\nLEG_STATUS=ready\nLEG_SLICES=(x)\n"
                f"leg_execute() {{ {body}; }}\n",
                encoding="utf-8",
            )
            environment = dict(os.environ, VOICE_LEGS_DIR=str(legs))
            return subprocess.run(
                ["bash", str(BATTERY / "voice-conformance.sh"), "--leg", "child"],
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )

    def test_partial_pass_followed_by_an_exit_is_not_a_completed_leg(self):
        result = self.leg('echo "RESULT x PASS first assertion"; exit 7')
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_partial_pass_followed_by_a_signal_is_not_a_completed_leg(self):
        result = self.leg('echo "RESULT x PASS first assertion"; kill -TERM "$BASHPID"')
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_a_claimed_completion_must_match_the_actual_exit(self):
        result = self.leg('echo "RESULT x PASS first assertion"; echo "VOICE_COMPLETE 0"; exit 1')
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_a_completed_assertion_failure_is_the_expected_negative_control(self):
        result = self.leg('echo "RESULT x FAIL planted answer"; echo "VOICE_COMPLETE 1"; return 1')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_a_completed_pass_still_passes(self):
        result = self.leg('echo "RESULT x PASS complete assertion"; echo "VOICE_COMPLETE 0"')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_completed_governance_observations_do_not_grade_conformance(self):
        result = self.leg(
            'echo "RESULT x FAIL policy observation"; echo "VOICE_COMPLETE 0"',
            kind="governance",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def judge(self, change=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            run = root / "voice-run"
            run.mkdir()
            output = root / "openai"
            output.mkdir()
            normal = [
                "spec-per-dialect\topenai\tPASS\tfixture",
                "replay\tdefault\tPASS\topenai transcript",
                "tool-reply\t1\tPASS\topenai tool",
            ]
            for leg in (
                "provider-credential", "metering-lease", "session-scope", "provider-dial",
                "admit-refusal", "route-failover", "audit-record", "exit-terminal",
            ):
                normal.append(f"{leg}\t1\tPASS\tfixture")
            (run / "results.tsv").write_text("\n".join(normal) + "\n", encoding="utf-8")
            (run / "red.tsv").write_text(
                "\n".join(row.replace("\tPASS\t", "\tFAIL\t") for row in normal) + "\n",
                encoding="utf-8",
            )
            (run / "battery.rc").write_text("0\n", encoding="utf-8")
            (run / "red.rc").write_text("1\n", encoding="utf-8")
            (run / "battery.log").write_text("VOICE verdict: completed battery\n", encoding="utf-8")
            (run / "red.log").write_text(
                "FAIL: 1 conformance leg(s) failed. This IS a finding about busbar.\n",
                encoding="utf-8",
            )
            (run / "done").touch()
            if change:
                change(run)
            environment = dict(
                os.environ,
                BUSBAR_CHECKOUT=str(ROOT),
                CONFORMANCE_SUITE="voice-openai-realtime",
                CONFORMANCE_OUT=str(output),
            )
            result = subprocess.run(
                ["bash", str(ROOT / "testing/conformance/voice.sh"), "openai"],
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            verdict = json.loads((output / "verdict.json").read_text(encoding="utf-8"))
            return result, verdict

    def test_completed_normal_and_red_batteries_pass(self):
        result, verdict = self.judge()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(verdict["status"], "pass")

    def test_partial_normal_or_red_rows_cannot_hide_a_failed_process(self):
        for name in ("battery", "red"):
            with self.subTest(name=name):
                result, verdict = self.judge(
                    lambda run: (run / f"{name}.rc").write_text("101\n", encoding="utf-8")
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(verdict["status"], "fail")

    def test_partial_rows_require_the_actual_terminal_summary(self):
        for name in ("battery", "red"):
            with self.subTest(name=name):
                result, verdict = self.judge(
                    lambda run: (run / f"{name}.log").write_text("aborted after partial rows\n", encoding="utf-8")
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(verdict["status"], "fail")

    def test_missing_status_is_not_a_completed_battery(self):
        result, verdict = self.judge(lambda run: (run / "battery.rc").unlink())
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(verdict["status"], "fail")

    def test_a_completed_other_dialect_failure_keeps_its_own_classification(self):
        def other_dialect(run):
            with (run / "results.tsv").open("a", encoding="utf-8") as stream:
                stream.write("twilio-media\t1\tFAIL\tTwilio finding\n")
            (run / "battery.rc").write_text("1\n", encoding="utf-8")
            (run / "battery.log").write_text(
                "FAIL: 1 conformance leg(s) failed. This IS a finding about busbar.\n",
                encoding="utf-8",
            )

        result, verdict = self.judge(other_dialect)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(verdict["status"], "pass")


if __name__ == "__main__":
    unittest.main()
