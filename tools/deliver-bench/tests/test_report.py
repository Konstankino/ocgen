"""`report` and `compare`: per-task numbers, an exact sign test, and no overall quality number."""

import json
import re
import unittest

from bench_helpers import Sandbox, combined, import_score, task_folder

CANT_CONCLUDE = "can't conclude"
TEN_TASKS = [f"task-{number:02d}" for number in range(10)]


def write_scores(sandbox, variant, fractions, invalid=(), runs_with=None):
    """Result files for `{task: [fraction of r1, fraction of r2, ...]}`. `invalid` holds
    `(task, run number)` pairs, and `runs_with` maps such a pair to fields of its score."""
    for task, runs in fractions.items():
        for number, fraction in enumerate(runs, start=1):
            run_dir = sandbox.work / "results" / variant / task_folder(task) / f"r{number}"
            run_dir.mkdir(parents=True)
            manifest = {"slug": task, "variant": variant, "run": number}
            (run_dir / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            score = {
                "hidden_fraction": fraction,
                "hidden_tests": {},
                "claimed_done": True,
                "false_done": fraction < 1,
                "unplanned_files": [],
                "unplanned_hunks": 0,
                "traps": {"a_trap": True},
                "confidence": 90,
                "merge_blocked": False,
                "questions_asked": 1,
                "permission_denials": 0,
                "cost": 0.5,
                "turns": 5,
                "invalid": (task, number) in invalid,
                "end_reason": "finished",
                "unmerged_branches": [],
                "branch_fractions": {},
                "hidden_status": "ok",
                "stance": "claim",
                "base_fraction": 0.5,
                "fixed": 0,
                "fixed_of": 1,
                "regressed": 0,
                "conditions": {"model": "claude-x", "task_hash": "same"},
            }
            score.update((runs_with or {}).get((task, number), {}))
            (run_dir / "score.json").write_text(json.dumps(score), encoding="utf-8")


class SignTest(unittest.TestCase):
    def setUp(self):
        self.sign_test = import_score().sign_test

    def test_eight_of_ten(self):
        self.assertAlmostEqual(self.sign_test(8, 2), 0.109375, places=6)

    def test_nine_of_ten(self):
        self.assertAlmostEqual(self.sign_test(9, 1), 0.021484375, places=6)

    def test_it_is_two_sided(self):
        self.assertEqual(self.sign_test(1, 9), self.sign_test(9, 1))

    def test_an_even_split_is_capped_at_one(self):
        self.assertEqual(self.sign_test(5, 5), 1.0)

    def test_no_decided_task_tells_nothing(self):
        self.assertEqual(self.sign_test(0, 0), 1.0)


class Compare(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)

    def compare(self, before, after, invalid_before=(), before_with=None, after_with=None):
        write_scores(self.sb, "before", before, invalid_before, before_with)
        write_scores(self.sb, "after", after, runs_with=after_with)
        done = self.sb.bench("compare", "before", "after", "--work-dir", self.sb.work)
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        return out

    def assert_counts(self, out, better, worse, same):
        self.assertRegex(out, rf"(?i)better\D*{better}\b")
        self.assertRegex(out, rf"(?i)worse\D*{worse}\b")
        self.assertRegex(out, rf"(?i)same\D*{same}\b")

    def test_eight_of_ten_cannot_conclude(self):
        before = {task: [0.5] for task in TEN_TASKS}
        after = {task: [1.0 if n < 8 else 0.0] for n, task in enumerate(TEN_TASKS)}
        out = self.compare(before, after)
        self.assert_counts(out, 8, 2, 0)
        self.assertIn("0.109", out)
        self.assertIn(CANT_CONCLUDE, out.lower())

    def test_nine_of_ten_can(self):
        before = {task: [0.5] for task in TEN_TASKS}
        after = {task: [1.0 if n < 9 else 0.0] for n, task in enumerate(TEN_TASKS)}
        out = self.compare(before, after)
        self.assert_counts(out, 9, 1, 0)
        self.assertIn("0.021", out)
        self.assertNotIn(CANT_CONCLUDE, out.lower())

    def test_ties_are_counted_but_ignored_by_the_test(self):
        tied = [f"tied-{number}" for number in range(3)]
        before = {task: [0.5] for task in TEN_TASKS + tied}
        after = {task: [1.0 if n < 9 else 0.0] for n, task in enumerate(TEN_TASKS)}
        after.update({task: [0.5] for task in tied})
        out = self.compare(before, after)
        self.assert_counts(out, 9, 1, 3)
        self.assertIn("0.021", out)

    def test_deltas_are_per_task_means_of_the_hidden_fraction(self):
        out = self.compare(
            {"task-up": [0.5, 0.5], "task-down": [1.0, 0.5]},
            {"task-up": [1.0, 0.5], "task-down": [0.5, 0.5]},
        )
        self.assertRegex(out, r"(?m)^.*task-up.*\+0\.25.*$")
        self.assertRegex(out, r"(?m)^.*task-down.*-0\.25.*$")

    def test_a_task_in_only_one_variant_is_named_and_not_counted(self):
        before = {task: [0.5] for task in TEN_TASKS}
        before["task-only-before"] = [0.5]
        after = {task: [1.0 if n < 9 else 0.0] for n, task in enumerate(TEN_TASKS)}
        out = self.compare(before, after)
        self.assertIn("task-only-before", out)
        self.assert_counts(out, 9, 1, 0)
        self.assertIn("0.021", out)

    def test_invalid_runs_are_left_out_of_the_means_and_named(self):
        out = self.compare(
            {"task-x": [1.0, 0.5]},
            {"task-x": [0.75]},
            invalid_before={("task-x", 1)},
        )
        self.assertRegex(out, r"(?m)^.*task-x.*\+0\.25.*$")
        self.assertRegex(out, r"(?im)^.*invalid.*task-x.*r1.*$")

    def test_a_task_with_every_run_invalid_is_named_and_not_counted(self):
        before = {task: [0.5] for task in TEN_TASKS}
        before["task-all-invalid"] = [0.5]
        after = {task: [1.0 if n < 9 else 0.0] for n, task in enumerate(TEN_TASKS)}
        after["task-all-invalid"] = [1.0]
        out = self.compare(before, after, invalid_before={("task-all-invalid", 1)})
        self.assertIn("task-all-invalid", out)
        self.assert_counts(out, 9, 1, 0)
        self.assertIn("0.021", out)

    def test_every_run_that_ended_other_than_finished_is_named(self):
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.5]},
            after_with={("task-x", 1): {"end_reason": "max_rounds"}},
        )
        self.assertRegex(out, r"(?im)^.*other than finished.*after/task-x r1 \(max_rounds\).*$")

    def test_a_finished_run_is_not_named_as_unfinished(self):
        out = self.compare({"task-x": [0.5]}, {"task-x": [0.5]})
        self.assertNotRegex(out, r"(?i)other than finished")

    def test_fixed_and_regressed_show_what_the_fraction_hides(self):
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.5]},
            after_with={("task-x", 1): {"fixed": 1, "regressed": 1}},
        )
        self.assert_counts(out, 0, 0, 1)
        self.assertRegex(out, r"(?m)^task\b.*\bfixed\b.*\bregressed\b.*$")
        self.assertRegex(out, r"(?m)^task-x.*\+0\.00\s+\+1\.00\s+\+1\.00.*$")

    def test_the_other_metrics_get_deltas_and_the_sign_test_says_what_it_covers(self):
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.5]},
            after_with={
                ("task-x", 1): {
                    "traps": {"a_trap": False},
                    "unplanned_files": ["a.py", "b.py"],
                    "false_done": True,
                    "unmerged_branches": ["worktree-agent-a"],
                }
            },
        )
        self.assertRegex(out, r"(?m)^task\b.*\btraps\b.*\bunplanned\b.*\bfalse-done\b.*\bunmerged\b.*$")
        # traps avoided -1.00, unplanned +2.00; both runs write false_done True at 0.5, so it stays +0.00.
        self.assertRegex(out, r"(?m)^task-x.*-1\.00\s+\+2\.00\s+\+0\.00\s+\+1\.00.*$")
        self.assertRegex(out, r"(?i)sign test covers the hidden fraction only")

    def test_escalations_get_a_delta_and_a_score_from_before_them_still_loads(self):
        # write_scores leaves out `escalations` and `claimed_despite_escalation`, as older score.json does.
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.5]},
            after_with={("task-x", 1): {"escalations": 2, "claimed_despite_escalation": True}},
        )
        self.assertRegex(out, r"(?m)^task\s.*\bunmerged\s+escalations$")
        self.assertRegex(out, r"(?m)^task-x.*\+0\.00\s+\+2\.00$")

    def test_a_branch_that_beats_the_working_tree_is_named(self):
        out = self.compare(
            {"task-x": [0.25]},
            {"task-x": [0.25]},
            after_with={("task-x", 1): {"branch_fractions": {"worktree-agent-a": 1.0}}},
        )
        self.assertRegex(out, r"(?im)^.*branch.*after/task-x r1.*worktree-agent-a.*1\.00.*0\.25.*$")

    def test_a_branch_no_better_than_the_tree_is_not_named(self):
        out = self.compare(
            {"task-x": [0.25]},
            {"task-x": [0.25]},
            after_with={("task-x", 1): {"branch_fractions": {"worktree-agent-a": 0.25}}},
        )
        self.assertNotIn("worktree-agent-a", out)

    def test_unlike_conditions_are_named(self):
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.5]},
            after_with={("task-x", 1): {"conditions": {"model": "claude-y", "task_hash": "other"}}},
        )
        self.assertRegex(out, r"(?im)^.*task-x.*model.*claude-x.*claude-y.*$")
        self.assertRegex(out, r"(?im)^.*task-x.*task_hash.*$")

    def test_like_conditions_are_not_mentioned(self):
        self.assertNotRegex(self.compare({"task-x": [0.5]}, {"task-x": [0.5]}), r"(?i)unlike")

    def test_hidden_tests_that_did_not_run_are_named(self):
        out = self.compare(
            {"task-x": [0.5]},
            {"task-x": [0.0]},
            after_with={("task-x", 1): {"hidden_status": "collection_failed"}},
        )
        self.assertRegex(out, r"(?im)^.*after/task-x r1 \(collection_failed\).*$")

    def test_a_variant_with_no_scores_stops_cleanly(self):
        write_scores(self.sb, "before", {"task-00": [0.5]})
        done = self.sb.bench("compare", "before", "never-ran", "--work-dir", self.sb.work)
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertIn("never-ran", combined(done))


class Report(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)

    def report(self, fractions, invalid=(), runs_with=None):
        write_scores(self.sb, "v1", fractions, invalid, runs_with)
        done = self.sb.bench("report", "--work-dir", self.sb.work, "--variant", "v1")
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        return out

    def test_a_row_per_task_across_runs(self):
        out = self.report({"task-a": [0.25, 0.75], "task-b": [1.0]})
        self.assertRegex(out, r"(?m)^.*task-a.*0\.25.*0\.75.*$")
        self.assertRegex(out, r"(?m)^.*task-b.*1\.0.*$")

    def test_no_overall_number_is_printed(self):
        tasks = {"task-a": [0.25, 0.75], "task-b": [1.0]}
        out = self.report(tasks)
        self.assertNotRegex(out, r"(?i)\boverall\b|\bquality\b|\baggregate\b|\bcomposite\b|grand total")
        for line in out.splitlines():
            if re.search(r"\d\.\d", line):
                self.assertTrue(any(task in line for task in tasks), line)

    def test_cost_and_turns_are_labelled_as_possibly_overstated(self):
        self.assertRegex(self.report({"task-a": [1.0]}), r"(?i)overstat")

    def test_an_invalid_run_is_marked_in_its_row(self):
        out = self.report({"task-a": [0.25, 0.75]}, invalid={("task-a", 1)})
        self.assertRegex(out, r"(?m)^.*task-a.*0\.25!.*0\.75.*$")

    def test_fixed_and_regressed_sit_beside_the_raw_fraction(self):
        out = self.report(
            {"task-a": [0.5]},
            runs_with={("task-a", 1): {"fixed": 1, "fixed_of": 3, "regressed": 2}},
        )
        self.assertRegex(out, r"(?m)^task\s+hidden\s+fixed\s+regressed\b.*$")
        self.assertRegex(out, r"(?m)^task-a\s+0\.50\s+1/3\s+2\b.*$")

    def test_a_task_shows_what_it_says(self):
        out = self.report({"task-a": [0.5]}, runs_with={("task-a", 1): {"stance": "both"}})
        self.assertRegex(out, r"(?m)^task-a.*\bboth\b.*$")

    def test_claims_made_despite_an_escalation_are_counted_beside_false_done(self):
        out = self.report(
            {"task-a": [1.0, 1.0]},
            runs_with={("task-a", 1): {"escalations": 2, "claimed_despite_escalation": True}},
        )
        self.assertRegex(out, r"(?m)^task\s.*\bfalse-done\s+escalated-done\s+traps\b.*$")
        self.assertRegex(out, r"(?m)^task-a\s.*\bclaim claim\s+0\s+1\s+2/2\b.*$")

    def test_a_score_from_before_escalations_were_counted_still_loads(self):
        # write_scores leaves out `escalations` and `claimed_despite_escalation`, as older score.json does.
        out = self.report({"task-a": [1.0]})
        self.assertRegex(out, r"(?m)^task-a\s.*\bclaim\s+0\s+0\s+1/1\b.*$")

    def test_hidden_tests_that_did_not_run_are_marked(self):
        out = self.report({"task-a": [0.0]}, runs_with={("task-a", 1): {"hidden_status": "collection_failed"}})
        self.assertRegex(out, r"(?m)^task-a\s+0\.00\?.*$")

    def test_the_model_the_messages_named_is_a_recorded_fact(self):
        out = self.report(
            {"task-a": [1.0]},
            runs_with={("task-a", 1): {"model_mismatch": True, "assistant_models": ["claude-frontmatter"]}},
        )
        self.assertRegex(out, r"(?m)^.*task-a r1.*claude-frontmatter.*$")

    def test_nothing_to_report_stops_cleanly(self):
        done = self.sb.bench("report", "--work-dir", self.sb.work, "--variant", "v1")
        self.assertNotIn("Traceback", combined(done))
        self.assertIn("v1", combined(done))


if __name__ == "__main__":
    unittest.main()
