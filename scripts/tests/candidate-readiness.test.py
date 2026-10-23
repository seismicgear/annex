"""Native qualification must wait for, never waive, successful source gates."""
import contextlib
import io
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native-acceptance"))
from candidate_readiness import wait_for_native_candidate

SOURCE = "a" * 40
JOB = "Build (Windows x86_64, required)"


class ReadinessTests(unittest.TestCase):
    def exercise(self, states, *, source=SOURCE, run_status="in_progress",
                 workflow=".github/workflows/release-desktop.yml", timeout=60):
        elapsed = [0]
        polls = [0]

        def api(route):
            if not route.endswith("/jobs?per_page=100"):
                return dict(id=12, head_sha=source, path=workflow,
                            event="workflow_dispatch", status=run_status)
            state = states[min(polls[0], len(states) - 1)]
            polls[0] += 1
            rows = [dict(name="Pre-flight gates", head_sha=SOURCE,
                         status="completed", conclusion="success")]
            if state != "absent":
                rows.append(dict(name=JOB, head_sha=SOURCE,
                                 status="in_progress" if state == "pending" else "completed",
                                 conclusion=None if state == "pending" else state))
            return dict(total_count=len(rows), jobs=rows)

        with contextlib.redirect_stdout(io.StringIO()):
            wait_for_native_candidate(api, "12", SOURCE, JOB, timeout,
                clock=lambda: elapsed[0], sleep=lambda seconds: elapsed.__setitem__(0, elapsed[0] + seconds))
        return polls[0]

    def test_pending_then_success(self):
        self.assertEqual(self.exercise(["pending", "success"]), 2)

    def test_job_not_created_yet(self):
        self.assertEqual(self.exercise(["absent", "success"]), 2)

    def test_failed_gate_is_terminal(self):
        for state in ("failure", "cancelled", "skipped"):
            with self.subTest(state=state), self.assertRaises(RuntimeError):
                self.exercise([state, "success"])

    def test_deadline_does_not_grant_acceptance(self):
        with self.assertRaises(TimeoutError):
            self.exercise(["pending"], timeout=30)

    def test_completed_run_missing_job_is_refused(self):
        with self.assertRaises(RuntimeError):
            self.exercise(["absent"], run_status="completed")

    def test_wrong_source_or_workflow_is_refused(self):
        for override in (dict(source="b" * 40), dict(workflow=".github/workflows/other.yml")):
            with self.subTest(override=override), self.assertRaises(AssertionError):
                self.exercise(["success"], **override)


if __name__ == "__main__":
    unittest.main()
