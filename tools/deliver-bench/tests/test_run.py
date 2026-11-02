"""`run`: the flags on every call, the scripted-user loop and the approval check; and the small
subcommands around it (`approvals`, `first-probe`, `inspect-init`)."""

import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from bench_helpers import (
    BENCH,
    FAKE_SCAFFOLD,
    PINNED_MODEL,
    SPEC_SENTINEL,
    STUB_CLAUDE,
    TINY,
    VARIANT,
    Sandbox,
    combined,
    git,
    install,
    task_folder,
)
from fake_ocgen import PROTOCOL
from stub_claude import (
    BARE_NEEDS_KEY,
    ERROR_TEXT,
    FINAL_MARK,
    FINAL_REPORT,
    FIRST_SESSION,
    MESSAGE_MODEL,
    QUESTION,
    REPLY_TEXT,
    TOOL_QUESTION,
    USER_DOWN_TEXT,
)

SANDBOX_SETTINGS = {"sandbox": {"enabled": True, "autoAllowBashIfSandboxed": True}}
APPROVE_SUFFIX = "--minutes 1440"
GOAL_PROMPT = "/deliver Add the missing calculator functions to calc.py."
SUBCOMMANDS = (
    "validate",
    "prepare",
    "approvals",
    "first-probe",
    "inspect-init",
    "run",
    "score",
    "report",
    "compare",
)


def inside_git(path):
    return any((folder / ".git").exists() for folder in [path, *path.parents])


class Case(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)
        self.sb.prepare()

    def finished_run(self, *extra, **env):
        done = self.sb.run(*extra, **env)
        self.assertEqual(done.returncode, 0, combined(done))
        return self.sb.agent_calls()


class Flags(Case):
    def test_the_first_call(self):
        first = self.finished_run()[0]
        argv, flags = first["argv"], first["flags"]
        self.assertIn("-p", argv)
        self.assertIn("--verbose", argv)
        self.assertNotIn("--resume", argv)
        self.assertEqual(flags["--model"], PINNED_MODEL)
        self.assertEqual(flags["--output-format"], "stream-json")
        self.assertEqual(flags["--permission-mode"], "acceptEdits")
        self.assertEqual(json.loads(flags["--settings"]), SANDBOX_SETTINGS)
        self.assertEqual(argv[-2:], ["--", GOAL_PROMPT])
        self.assertEqual(first["prompt"], GOAL_PROMPT)
        self.assertEqual(first["cwd"], str(self.sb.repo()))

    def test_resume_repeats_every_flag_because_it_restores_none(self):
        first, second = self.finished_run()
        self.assertEqual(second["flags"]["--resume"], FIRST_SESSION)
        for flag in ("--model", "--output-format", "--permission-mode", "--settings"):
            self.assertEqual(second["flags"][flag], first["flags"][flag], flag)
        self.assertIn("-p", second["argv"])
        self.assertIn("--verbose", second["argv"])
        self.assertEqual(second["cwd"], first["cwd"])
        self.assertEqual(second["prompt"], REPLY_TEXT)

    def test_no_call_bypasses_permissions_or_uses_an_alias(self):
        self.finished_run()
        self.assertTrue(self.sb.claude_calls())
        for call in self.sb.claude_calls():
            self.assertNotIn("bypassPermissions", " ".join(call["argv"]))
            self.assertNotIn("--dangerously-skip-permissions", call["argv"])
            self.assertNotIn("opus", call["argv"])

    def test_no_sandbox_opts_out_on_every_call(self):
        calls = self.finished_run("--no-sandbox")
        self.assertEqual(len(calls), 2)
        for call in calls:
            settings = json.loads(call["flags"].get("--settings", "{}"))
            self.assertFalse(settings.get("sandbox", {}).get("enabled", False))

    def test_a_given_model_is_used_on_every_call(self):
        self.finished_run("--model", "claude-other-2")
        calls = self.sb.agent_calls() + self.sb.user_calls()
        self.assertTrue(calls)
        for call in calls:
            self.assertEqual(call["flags"]["--model"], "claude-other-2")
        reasons = self.sb.run_json()["invalid_reasons"]
        self.assertEqual([reason for reason in reasons if "model" in reason], [])

    def test_a_model_alias_is_refused(self):
        done = self.sb.run("--model", "opus")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertIn("opus", combined(done))
        self.assertEqual(self.sb.agent_calls(), [])

    def test_the_claude_option_overrides_the_binary(self):
        (self.sb.bin / "claude").unlink()
        other = self.sb.root / "other-claude"
        install(other, STUB_CLAUDE)
        self.assertEqual(len(self.finished_run("--claude", other)), 2)


class Loop(Case):
    def test_it_ends_when_the_scripted_user_says_finished(self):
        self.assertEqual(len(self.finished_run()), 2)
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "finished")
        self.assertEqual(run["rounds"], 2)
        self.assertEqual(run["questions"], [QUESTION])
        self.assertIn(FINAL_MARK, run["final_report"])
        self.assertEqual(len(run["permission_denials"]), 1)
        self.assertTrue((self.sb.repo() / "calc.py").read_text(encoding="utf-8").count("def neg"))

    def test_every_agent_message_is_kept(self):
        self.finished_run()
        self.assertEqual(self.sb.run_json()["messages"], [QUESTION, FINAL_REPORT])

    def test_the_model_each_message_names_is_recorded(self):
        self.finished_run()
        run = self.sb.run_json()
        self.assertEqual(run["assistant_models"], [PINNED_MODEL])
        self.assertIs(run["model_mismatch"], False)

    def test_another_model_on_the_messages_is_a_recorded_fact_not_an_invalid_run(self):
        # /deliver's frontmatter names an alias, so its messages may name another id than the pinned one.
        self.assertEqual(len(self.finished_run(BENCH_STUB_MODE="other_message_model")), 2)
        run = self.sb.run_json()
        self.assertEqual(run["assistant_models"], [MESSAGE_MODEL])
        self.assertIs(run["model_mismatch"], True)
        self.assertEqual(run["end_reason"], "finished")
        self.assertEqual([reason for reason in run["invalid_reasons"] if "model" in reason], [])

    def test_the_claude_version_at_run_time_is_recorded(self):
        self.finished_run()
        self.assertIn("stub", self.sb.run_json()["claude_version"])

    def test_each_calls_raw_numbers_are_kept_and_their_sum_reported(self):
        self.finished_run()
        run = self.sb.run_json()
        self.assertEqual([call["total_cost_usd"] for call in run["calls"]], [0.01, 0.01])
        self.assertEqual([call["num_turns"] for call in run["calls"]], [2, 2])
        self.assertAlmostEqual(run["total_cost_usd"], 0.02)
        self.assertEqual(run["num_turns"], 4)

    def test_the_transcript_keeps_every_line_of_every_call(self):
        self.finished_run()
        text = (self.sb.run_dir() / "transcript.jsonl").read_text(encoding="utf-8")
        self.assertIn("stub: warming up", text)
        self.assertEqual(text.count('"subtype": "init"'), 2)
        self.assertIn(QUESTION, text)
        self.assertIn(FINAL_MARK, text)

    def test_max_rounds_ends_a_run_that_never_finishes(self):
        self.assertEqual(len(self.finished_run("--max-rounds", 3, BENCH_STUB_MODE="never_finishes")), 3)
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "max_rounds")
        self.assertEqual(run["rounds"], 3)
        self.assertGreaterEqual(len(run["questions"]), 2)

    def test_max_rounds_defaults_to_twelve(self):
        self.assertEqual(len(self.finished_run(BENCH_STUB_MODE="never_finishes")), 12)
        self.assertEqual(self.sb.run_json()["end_reason"], "max_rounds")

    def test_each_resume_uses_the_session_id_of_the_last_result(self):
        calls = self.finished_run("--max-rounds", 3, BENCH_STUB_MODE="never_finishes")
        self.assertEqual(
            [call["flags"]["--resume"] for call in calls[1:]],
            ["stub-session-1", "stub-session-2"],
        )

    def test_an_error_result_is_recorded_is_no_crash_and_counts_as_no_measurement(self):
        done = self.sb.run(BENCH_STUB_MODE="error_max_turns")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertIn(TINY, combined(done))
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "error")
        self.assertEqual(run["subtype"], "error_max_turns")
        self.assertEqual(run["errors"], [ERROR_TEXT])
        self.assertFalse(run.get("final_report"))
        self.assertIs(run["invalid"], True)
        self.assertTrue(any("error" in reason for reason in run["invalid_reasons"]), run)
        self.assertEqual(len(self.sb.agent_calls()), 1)
        self.assertEqual(self.sb.user_calls(), [])

    def test_a_call_that_times_out_is_recorded_and_keeps_its_lines(self):
        done = self.sb.run("--call-timeout", 2, BENCH_STUB_MODE="hang")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "timeout")
        self.assertIs(run["invalid"], True)
        text = (self.sb.run_dir() / "transcript.jsonl").read_text(encoding="utf-8")
        self.assertIn('"subtype": "init"', text)

    def test_a_scripted_user_that_cannot_answer_stops_the_bench_before_the_first_fixture(self):
        # Otherwise every fixture pays for a first agent round and then ends in user_error.
        done = self.sb.run(BENCH_STUB_MODE="user_down")
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(USER_DOWN_TEXT, out)
        self.assertEqual(self.sb.agent_calls(), [])
        self.assertFalse((self.sb.run_dir() / "transcript.jsonl").exists())
        self.assertFalse((self.sb.run_dir() / "run.json").exists())

    def test_a_scripted_user_that_goes_down_later_counts_as_no_measurement(self):
        done = self.sb.run(BENCH_STUB_MODE="user_down_later")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "user_error")
        self.assertIs(run["invalid"], True)
        self.assertTrue(any("user_error" in reason for reason in run["invalid_reasons"]), run)
        self.assertEqual(len(self.sb.user_calls()), 2)

    def test_a_scripted_user_call_that_fails_once_is_retried(self):
        marker = self.sb.root / "failed-once"
        self.finished_run(BENCH_STUB_BARE_FAIL_ONCE=str(marker))
        self.assertTrue(marker.exists())
        self.assertEqual(self.sb.run_json()["end_reason"], "finished")
        self.assertEqual(len(self.sb.user_calls()), 3)

    def test_a_question_through_the_tool_reaches_the_scripted_user(self):
        self.finished_run(BENCH_STUB_MODE="ask_tool")
        self.assertIn(TOOL_QUESTION, self.sb.user_calls()[0]["prompt"])
        asked = self.sb.run_json()["tool_questions"]
        self.assertEqual(len(asked), 1)
        self.assertIs(asked[0]["answered"], True)
        self.assertIn(TOOL_QUESTION, json.dumps(asked[0]["input"]))

    def test_a_question_through_the_tool_with_no_reply_counts_as_unanswered(self):
        self.sb.run(BENCH_STUB_MODE="ask_tool", BENCH_STUB_BARE_FAIL_ONCE=str(self.sb.root / "m"), BENCH_STUB_REPLY="not an answer")
        asked = self.sb.run_json()["tool_questions"]
        self.assertIs(asked[0]["answered"], False)

    def test_nothing_a_call_started_outlives_it(self):
        pids = self.sb.root / "survivors"
        self.addCleanup(self.kill_survivors, pids)
        self.finished_run(BENCH_STUB_MODE="survivor", BENCH_STUB_SURVIVOR_PID=str(pids))
        started = [int(word) for word in pids.read_text(encoding="utf-8").split()]
        self.assertTrue(started)
        deadline = time.monotonic() + 10
        alive = started
        while alive and time.monotonic() < deadline:
            alive = [pid for pid in alive if self.is_alive(pid)]
            time.sleep(0.1)
        self.assertEqual(alive, [])

    @staticmethod
    def is_alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True

    def kill_survivors(self, pids):
        if not pids.exists():
            return
        for word in pids.read_text(encoding="utf-8").split():
            try:
                os.kill(int(word), signal.SIGKILL)
            except ProcessLookupError:
                pass

    def test_a_reply_that_starts_with_a_dash_reaches_the_agent_as_text(self):
        # A scripted user may answer with a Markdown bullet: it must not be read as an option.
        calls = self.finished_run(BENCH_STUB_REPLY="REPLY: - yes, keep going")
        self.assertEqual(calls[1]["argv"][-2:], ["--", "- yes, keep going"])
        self.assertEqual(calls[1]["prompt"], "- yes, keep going")
        self.assertEqual(self.sb.run_json()["end_reason"], "finished")


class ResultsTree(Case):
    def test_a_run_leaves_its_files_in_the_results_tree(self):
        self.finished_run()
        results = self.sb.work / "results" / VARIANT / task_folder(TINY) / "r1"
        for name in ("manifest.json", "run.json", "transcript.jsonl", "user.jsonl"):
            self.assertTrue((results / name).is_file(), name)

    def test_nothing_around_the_fixture_holds_results_the_slug_or_the_spec(self):
        self.finished_run()
        repo = self.sb.repo()
        self.assertEqual(list(repo.parent.iterdir()), [repo])
        fixtures = self.sb.work / "fixtures"
        self.assertEqual(
            [p for p in fixtures.rglob("*") if p.is_file() and repo not in p.parents], []
        )
        self.assertNotIn(TINY, [part for part in repo.relative_to(self.sb.work).parts])
        for path in repo.rglob("*"):
            if path.is_file():
                self.assertNotIn(SPEC_SENTINEL.encode("utf-8"), path.read_bytes(), str(path))


class Rerun(Case):
    def test_a_fixture_that_already_ran_is_skipped_by_name(self):
        self.finished_run()
        calls = len(self.sb.agent_calls())
        done = self.sb.run()
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(TINY, out)
        self.assertIn("skipped", out)
        self.assertIn("--rerun-finished", out)
        self.assertEqual(len(self.sb.agent_calls()), calls)

    def test_force_alone_leaves_a_finished_fixture_alone(self):
        self.finished_run()
        results = results_of(self.sb.run_dir())
        calls = len(self.sb.agent_calls())
        done = self.sb.run("--force")
        self.assertEqual(done.returncode, 0, combined(done))
        self.assertEqual(len(self.sb.agent_calls()), calls)
        self.assertEqual(results_of(self.sb.run_dir()), results)

    def test_rerun_finished_runs_it_again_keeps_the_old_results_and_marks_the_reused_tree(self):
        self.finished_run()
        old_run = (self.sb.run_dir() / "run.json").read_bytes()
        old_transcript = (self.sb.run_dir() / "transcript.jsonl").read_bytes()
        self.assertEqual(len(self.finished_run("--rerun-finished")), 4)
        text = (self.sb.run_dir() / "transcript.jsonl").read_text(encoding="utf-8")
        self.assertEqual(text.count('"subtype": "init"'), 2)
        run = self.sb.run_json()
        self.assertEqual(run["end_reason"], "finished")
        self.assertIs(run["invalid"], True)
        self.assertTrue(any("reused tree" in reason for reason in run["invalid_reasons"]), run)
        previous = self.sb.run_dir() / "previous" / "1"
        self.assertEqual((previous / "run.json").read_bytes(), old_run)
        self.assertEqual((previous / "transcript.jsonl").read_bytes(), old_transcript)

    def test_each_rerun_keeps_its_own_old_results(self):
        self.finished_run()
        self.finished_run("--rerun-finished")
        self.finished_run("--rerun-finished")
        for number in ("1", "2"):
            self.assertTrue((self.sb.run_dir() / "previous" / number / "run.json").is_file(), number)


def results_of(run_dir):
    """Every file under a run's results folder, by its path."""
    return {
        path.relative_to(run_dir).as_posix(): path.read_bytes()
        for path in run_dir.rglob("*")
        if path.is_file()
    }


class RealInterrupt(unittest.TestCase):
    """The state a real Ctrl-C leaves: r1 finished, r2 cut off in the middle of a call, r3 untouched."""

    def setUp(self):
        self.sb = Sandbox(self)
        self.sb.prepare(runs=3)
        process = subprocess.Popen(
            [
                sys.executable, str(BENCH), "run",
                "--work-dir", str(self.sb.work),
                "--tasks-dir", str(self.sb.tasks),
                "--variant", VARIANT,
                "--task", TINY,
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            encoding="utf-8",
            errors="replace",
            cwd=self.sb.root,
            env=self.sb.env(
                BENCH_STUB_OVERLAY=str(self.sb.tasks / TINY / "solution"),
                BENCH_STUB_HANG_IN="/r2/",
            ),
        )
        self.addCleanup(process.stdout.close)
        self.addCleanup(process.kill)
        transcript = self.sb.run_dir(run=2) / "transcript.jsonl"
        deadline = time.monotonic() + 60
        while not (transcript.exists() and '"subtype": "init"' in transcript.read_text(encoding="utf-8")):
            self.assertLess(time.monotonic(), deadline, "the bench never reached r2")
            self.assertIsNone(process.poll(), "the bench ended before r2")
            time.sleep(0.05)
        process.send_signal(signal.SIGINT)
        out, _ = process.communicate(timeout=60)
        self.assertEqual(process.returncode, 130, out)
        self.assertNotIn("Traceback", out)
        self.baseline = len(self.sb.agent_calls())

    def new_agent_folders(self):
        return {call["cwd"] for call in self.sb.agent_calls()[self.baseline:]}

    def test_it_leaves_a_finished_run_a_cut_off_one_and_a_fresh_one(self):
        self.assertTrue((self.sb.run_dir(run=1) / "run.json").is_file())
        cut_off = self.sb.run_dir(run=2)
        self.assertFalse((cut_off / "run.json").exists())
        self.assertGreater((cut_off / "transcript.jsonl").stat().st_size, 0)
        self.assertFalse((self.sb.run_dir(run=3) / "transcript.jsonl").exists())

    def test_a_plain_rerun_runs_the_fresh_fixture_and_refuses_the_cut_off_one(self):
        done = self.sb.run()
        out = combined(done)
        self.assertEqual(done.returncode, 1, out)
        self.assertNotIn("Traceback", out)
        self.assertEqual(self.new_agent_folders(), {str(self.sb.repo(run=3))})
        self.assertTrue((self.sb.run_dir(run=3) / "run.json").is_file())
        self.assertFalse((self.sb.run_dir(run=2) / "run.json").exists())
        self.assertRegex(out, r"(?m)^.*r1\b.*skipped.*$")
        for piece in (str(self.sb.repo(run=2).parent), str(self.sb.run_dir(run=2)), "prepare", "approve", "--force"):
            self.assertIn(piece, out)

    def test_the_recovery_it_names_gives_a_fresh_fixture(self):
        self.assertEqual(self.sb.run().returncode, 1)
        shutil.rmtree(self.sb.repo(run=2).parent)
        shutil.rmtree(self.sb.run_dir(run=2))
        self.sb.prepare(runs=3)
        self.baseline = len(self.sb.agent_calls())
        done = self.sb.run()
        self.assertEqual(done.returncode, 0, combined(done))
        self.assertEqual(self.new_agent_folders(), {str(self.sb.repo(run=2))})
        reasons = self.sb.run_json(run=2)["invalid_reasons"]
        self.assertEqual([reason for reason in reasons if "reused tree" in reason], [])

    def test_force_reruns_only_what_has_no_results_and_leaves_finished_runs_alone(self):
        self.sb.score(run=1)
        finished = results_of(self.sb.run_dir(run=1))
        self.assertIn("score.json", finished)
        cut_off = (self.sb.run_dir(run=2) / "transcript.jsonl").read_bytes()
        done = self.sb.run("--force")
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        self.assertEqual(self.new_agent_folders(), {str(self.sb.repo(run=2)), str(self.sb.repo(run=3))})
        self.assertEqual(results_of(self.sb.run_dir(run=1)), finished)
        self.assertIn("--rerun-finished", out)
        reasons = self.sb.run_json(run=2)["invalid_reasons"]
        self.assertTrue(any("reused tree" in reason for reason in reasons), reasons)
        reasons = self.sb.run_json(run=3)["invalid_reasons"]
        self.assertEqual([reason for reason in reasons if "reused tree" in reason], [])
        kept = self.sb.run_dir(run=2) / "previous" / "1" / "transcript.jsonl"
        self.assertEqual(kept.read_bytes(), cut_off)

    def test_rerun_finished_moves_a_finished_runs_results_aside(self):
        self.sb.score(run=1)
        run_dir = self.sb.run_dir(run=1)
        old_run = (run_dir / "run.json").read_bytes()
        old_score = (run_dir / "score.json").read_bytes()
        done = self.sb.run("--rerun-finished")
        self.assertEqual(done.returncode, 1, combined(done))
        self.assertEqual(self.new_agent_folders(), {str(self.sb.repo(run=1)), str(self.sb.repo(run=3))})
        self.assertEqual((run_dir / "previous" / "1" / "run.json").read_bytes(), old_run)
        self.assertEqual((run_dir / "previous" / "1" / "score.json").read_bytes(), old_score)
        self.assertFalse((run_dir / "score.json").exists())


class UsedFixtures(Case):
    def assert_refused(self, word):
        done = self.sb.run()
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(TINY, out)
        self.assertIn(word, out)
        self.assertIn("--force", out)
        self.assertEqual(self.sb.agent_calls(), [])

    def test_a_tree_with_changes_is_refused(self):
        (self.sb.repo() / "half-done.py").write_text("x = 1\n", encoding="utf-8")
        self.assert_refused("changes")

    def test_a_moved_head_is_refused(self):
        repo = self.sb.repo()
        (repo / "half-done.py").write_text("x = 1\n", encoding="utf-8")
        git(repo, "add", "half-done.py")
        git(repo, "commit", "-q", "-m", "an earlier attempt")
        self.assert_refused("HEAD")

    def test_a_worktree_branch_is_refused(self):
        git(self.sb.repo(), "branch", "worktree-agent-abc")
        self.assert_refused("worktree-agent")

    def test_an_interrupted_run_is_refused_even_on_a_clean_tree(self):
        (self.sb.run_dir() / "transcript.jsonl").write_text('{"type": "system"}\n', encoding="utf-8")
        self.assert_refused("transcript")

    def test_a_forced_run_on_a_used_tree_is_marked(self):
        (self.sb.repo() / "half-done.py").write_text("x = 1\n", encoding="utf-8")
        self.assertEqual(len(self.finished_run("--force")), 2)
        run = self.sb.run_json()
        self.assertIs(run["invalid"], True)
        self.assertTrue(any("reused tree" in reason for reason in run["invalid_reasons"]), run)

    def test_force_on_a_fresh_tree_marks_nothing(self):
        self.finished_run("--force")
        reasons = self.sb.run_json()["invalid_reasons"]
        self.assertEqual([reason for reason in reasons if "reused tree" in reason], [])

    def test_a_used_fixture_does_not_block_a_fresh_one(self):
        sb = Sandbox(self)
        sb.prepare(runs=2)
        (sb.repo(run=1) / "half-done.py").write_text("x = 1\n", encoding="utf-8")
        done = sb.run()
        out = combined(done)
        self.assertEqual(done.returncode, 1, out)
        self.assertEqual({call["cwd"] for call in sb.agent_calls()}, {str(sb.repo(run=2))})
        self.assertRegex(out, r"(?m)^.*r1\b.*changes.*$")

    def test_the_refusal_names_the_way_to_start_over(self):
        (self.sb.repo() / "half-done.py").write_text("x = 1\n", encoding="utf-8")
        out = combined(self.sb.run())
        for piece in (str(self.sb.repo().parent), str(self.sb.run_dir()), "prepare", "approve"):
            self.assertIn(piece, out)


class Logins(Case):
    """The docs say bare mode needs ANTHROPIC_API_KEY and never reads a subscription login. So a scripted
    user that needs no key is the default, and a key meant for it never reaches the agent."""

    ISOLATED = {"disableAllHooks": True}

    def auth(self):
        return self.sb.manifest()["auth_used_by_run"]

    def test_the_scripted_user_uses_the_login_with_no_instructions_or_hooks(self):
        self.finished_run()
        calls = self.sb.scripted_calls()
        self.assertEqual(len(calls), 3)
        for call in calls:
            self.assertNotIn("--bare", call["argv"])
            self.assertEqual(json.loads(call["flags"]["--settings"]), self.ISOLATED)
            self.assertEqual(call["disable_claude_mds"], "1")
            self.assertIs(call["api_key"], False)
            self.assertEqual(call["cwd_entries"], [])
        self.assertEqual(self.auth(), {"agent": "login", "scripted_user": "login"})

    def test_an_exported_key_reaches_no_call_by_default(self):
        calls = self.finished_run(ANTHROPIC_API_KEY="sk-test")
        self.assertEqual(len(calls), 2)
        for call in calls + self.sb.scripted_calls():
            self.assertIs(call["api_key"], False)
        self.assertEqual(self.auth(), {"agent": "login", "scripted_user": "login"})

    def test_the_agent_does_not_get_the_scripted_users_isolation(self):
        for call in self.finished_run():
            self.assertIsNone(call["disable_claude_mds"])
            self.assertEqual(json.loads(call["flags"]["--settings"]), SANDBOX_SETTINGS)

    def test_agent_api_key_lets_the_key_reach_the_agent_only(self):
        calls = self.finished_run("--agent-api-key", ANTHROPIC_API_KEY="sk-test")
        for call in calls:
            self.assertIs(call["api_key"], True)
        for call in self.sb.scripted_calls():
            self.assertIs(call["api_key"], False)
        self.assertEqual(self.auth(), {"agent": "ANTHROPIC_API_KEY", "scripted_user": "login"})

    def test_scripted_user_bare_passes_the_key_to_that_call_only(self):
        calls = self.finished_run("--scripted-user-bare", ANTHROPIC_API_KEY="sk-test")
        for call in calls:
            self.assertIs(call["api_key"], False)
        scripted = self.sb.scripted_calls()
        self.assertEqual(len(scripted), 3)
        for call in scripted:
            self.assertIn("--bare", call["argv"])
            self.assertIs(call["api_key"], True)
            self.assertNotIn("--settings", call["argv"])
        self.assertEqual(self.auth(), {"agent": "login", "scripted_user": "ANTHROPIC_API_KEY"})

    def test_scripted_user_bare_without_a_key_stops_with_the_real_error(self):
        done = self.sb.run("--scripted-user-bare")
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(BARE_NEEDS_KEY, out)
        self.assertIn("ANTHROPIC_API_KEY", out)
        self.assertEqual(self.sb.agent_calls(), [])


class DeclaredInvalid(unittest.TestCase):
    def test_a_replaced_scaffold_marks_the_run(self):
        sb = Sandbox(self)
        sb.prepare()
        self.assertEqual(sb.run().returncode, 0)
        run = sb.run_json()
        self.assertIs(run["invalid"], True)
        self.assertTrue(any("--scaffold-cmd" in reason for reason in run["invalid_reasons"]), run)
        self.assertEqual(run["end_reason"], "finished")

    def test_allowing_a_nested_work_dir_marks_the_run(self):
        sb = Sandbox(self)
        sb.install_cargo()
        sb.prepare("--allow-nested-work-dir", fake_scaffold=False, allow_nested=False)
        self.assertEqual(sb.run().returncode, 0)
        reasons = sb.run_json()["invalid_reasons"]
        self.assertTrue(any("--allow-nested-work-dir" in reason for reason in reasons), reasons)

    def test_a_fixture_made_the_real_way_is_valid(self):
        sb = Sandbox(self)
        sb.install_cargo()
        sb.prepare(fake_scaffold=False)
        self.assertEqual(sb.run().returncode, 0)
        run = sb.run_json()
        reasons = [reason for reason in run["invalid_reasons"] if "--allow-nested-work-dir" not in reason]
        self.assertEqual(reasons, [])
        self.assertIs(run["invalid"], sb.nested)


class TheCheckoutsOwnOcgen(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)
        install(self.sb.bin / "cargo", FAKE_SCAFFOLD)
        self.sb.prepare(fake_scaffold=False)
        self.built = self.sb.work / "bin" / VARIANT
        # With the copy on PATH gone, only the built one can answer the status check.
        (self.sb.bin / "ocgen").unlink()
        done = self.sb.run()
        self.assertEqual(done.returncode, 0, combined(done))

    def test_the_agents_calls_find_it_first_and_the_scripted_user_does_not(self):
        calls = self.sb.agent_calls()
        self.assertEqual(len(calls), 2)
        for call in calls:
            self.assertEqual(call["path"].split(os.pathsep)[0], str(self.built))
        for call in self.sb.user_calls():
            self.assertNotEqual(call["path"].split(os.pathsep)[0], str(self.built))

    def test_the_status_check_goes_through_it(self):
        self.assertTrue(self.sb.approve_calls())
        self.assertIs(self.sb.run_json()["merge_block_allowed"], False)

    def test_the_manifest_records_the_ocgen_the_run_used(self):
        used = self.sb.manifest()["ocgen_used_by_run"]
        self.assertEqual(used["ocgen_path"], str(self.built / "ocgen"))
        self.assertIn("built", used["ocgen_version"])
        self.assertEqual(used["ocgen_protocol"], PROTOCOL)


class InvalidRuns(Case):
    def assert_invalid(self, mode, word):
        done = self.sb.run(BENCH_STUB_MODE=mode)
        self.assertNotIn("Traceback", combined(done))
        run = self.sb.run_json()
        self.assertIs(run["invalid"], True)
        self.assertTrue(any(word in reason for reason in run["invalid_reasons"]), run)

    def test_another_model_marks_the_run_invalid(self):
        self.assert_invalid("wrong_model", "model")

    def test_no_deliver_command_marks_the_run_invalid(self):
        self.assert_invalid("no_deliver", "deliver")


class ApprovalCheck(Case):
    def assert_stopped(self, done):
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(f"ocgen approve {shlex.quote(str(self.sb.repo()))} {APPROVE_SUFFIX}", out)
        self.assertEqual(self.sb.agent_calls(), [])

    def test_a_locked_fixture_stops_with_the_exact_command(self):
        self.assert_stopped(self.sb.run(BENCH_FAKE_APPROVAL="locked"))

    def test_a_status_that_fails_counts_as_locked(self):
        self.assert_stopped(self.sb.run(BENCH_FAKE_APPROVAL="error"))

    def test_a_missing_ocgen_counts_as_locked(self):
        self.assert_stopped(self.sb.run(PATH=self.sb.path_with_only_git()))

    def test_an_unlocked_fixture_runs(self):
        self.finished_run(BENCH_FAKE_APPROVAL="unlocked")
        self.assertIs(self.sb.run_json()["merge_block_allowed"], False)

    def test_allow_merge_block_runs_and_records_it(self):
        done = self.sb.run("--allow-merge-block", BENCH_FAKE_APPROVAL="locked")
        self.assertEqual(done.returncode, 0, combined(done))
        self.assertRegex(combined(done), r"(?i)merge")
        self.assertEqual(len(self.sb.agent_calls()), 2)
        self.assertIs(self.sb.run_json()["merge_block_allowed"], True)

    def test_only_the_status_form_is_ever_called_and_it_names_the_fixture(self):
        self.finished_run()
        calls = self.sb.approve_calls()
        self.assertTrue(calls)
        for argv in calls:
            self.assertIn("--status", argv)
            self.assertIn(str(self.sb.repo()), argv)

    def test_a_fixture_that_was_never_prepared_stops_cleanly(self):
        done = self.sb.run(variant="never-prepared")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertEqual(self.sb.agent_calls(), [])

    def test_an_approval_that_expires_during_the_bench_stops_it_with_the_command(self):
        sb = Sandbox(self)
        sb.prepare(runs=2)
        # Two status calls up front, and one before the first fixture; the fourth finds it expired.
        done = sb.run(BENCH_FAKE_APPROVAL_UNLOCKED_CALLS="3")
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(f"ocgen approve {shlex.quote(str(sb.repo(run=2)))} {APPROVE_SUFFIX}", out)
        self.assertEqual({call["cwd"] for call in sb.agent_calls()}, {str(sb.repo(run=1))})
        self.assertEqual(sb.run_json(run=1)["end_reason"], "finished")


class ScriptedUser(Case):
    def test_the_spec_reaches_only_the_scripted_user(self):
        calls = self.finished_run()
        for call in calls:
            self.assertNotIn(SPEC_SENTINEL, " ".join(call["argv"]))
            self.assertNotIn(SPEC_SENTINEL, call["stdin"])
        self.assertIn(SPEC_SENTINEL, self.sb.user_calls()[0]["prompt"])
        for name in self.sb.repo().rglob("*"):
            self.assertNotEqual(name.name, "spec.md")

    def test_its_call_has_the_right_flags_and_an_empty_folder(self):
        self.finished_run()
        calls = self.sb.user_calls()
        self.assertEqual(len(calls), 2)
        for call in calls:
            self.assertIn("-p", call["argv"])
            self.assertEqual(call["flags"]["--model"], PINNED_MODEL)
            self.assertEqual(call["flags"]["--output-format"], "json")
            self.assertNotIn("--resume", call["argv"])
            self.assertEqual(call["cwd_entries"], [])
            self.assertNotEqual(Path(call["cwd"]), self.sb.repo())
            if not inside_git(Path(tempfile.gettempdir()).resolve()):
                self.assertIs(call["in_git_repo"], False)

    def test_its_prompt_has_the_rules_and_the_agents_last_message(self):
        self.finished_run()
        first, last = self.sb.user_calls()
        self.assertIn("REPLY:", first["prompt"])
        self.assertIn("FINISHED", first["prompt"])
        self.assertIn(QUESTION, first["prompt"])
        self.assertIn(FINAL_MARK, last["prompt"])

    def test_its_prompt_settles_merges_and_wrong_plans(self):
        self.finished_run()
        prompt = self.sb.user_calls()[0]["prompt"]
        self.assertRegex(prompt, r"(?i)merging finished work into the working branch is already approved")
        self.assertRegex(prompt, r"(?i)say what's wrong, from the spec")

    def test_its_calls_go_to_their_own_file(self):
        self.finished_run()
        transcript = (self.sb.run_dir() / "transcript.jsonl").read_text(encoding="utf-8")
        users = (self.sb.run_dir() / "user.jsonl").read_text(encoding="utf-8")
        self.assertNotIn(REPLY_TEXT, transcript)
        self.assertNotIn(SPEC_SENTINEL, transcript)
        self.assertIn(REPLY_TEXT, users)
        self.assertIn(SPEC_SENTINEL, users)
        for line in users.splitlines():
            json.loads(line)


class Approvals(unittest.TestCase):
    def test_it_prints_the_exact_command_for_every_fixture_and_grants_nothing(self):
        sb = Sandbox(self)
        sb.prepare(runs=2)
        done = sb.bench("approvals", "--work-dir", sb.work, "--variant", VARIANT)
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        self.assertIn(TINY, out)
        for run in (1, 2):
            self.assertIn(f"ocgen approve {shlex.quote(str(sb.repo(run=run)))} {APPROVE_SUFFIX}", out)
        for argv in sb.approve_calls():
            self.assertIn("--status", argv)


class FirstProbe(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)

    def probe(self, *extra):
        args = [
            "first-probe",
            "--work-dir", self.sb.work,
            "--tasks-dir", self.sb.tasks,
            "--ocgen-dir", self.sb.checkout,
            "--task", TINY,
            "--scaffold-cmd", self.sb.scaffold_cmd,
        ]
        if self.sb.nested:
            args.append("--allow-nested-work-dir")
        done = self.sb.bench(*args, *extra)
        self.assertEqual(done.returncode, 0, combined(done))
        return combined(done)

    def test_it_prepares_one_fixture_in_its_own_variant_and_prints_the_one_off_command(self):
        out = self.probe()
        repo = self.sb.repo(variant="probe")
        self.assertTrue((repo / ".claude").is_dir())
        for piece in (
            TINY,
            f"ocgen approve {shlex.quote(str(repo))} {APPROVE_SUFFIX}",
            "claude -p",
            f"--model {PINNED_MODEL}",
            "--output-format stream-json",
            "--verbose",
            "--permission-mode acceptEdits",
            "--settings",
            "autoAllowBashIfSandboxed",
            " -- ",
            "/deliver",
            "inspect-init",
        ):
            self.assertIn(piece, out)
        self.assertNotIn("bypassPermissions", out)
        self.assertEqual(self.sb.agent_calls(), [])

    def test_it_covers_the_resume_and_the_scripted_user_on_the_users_login(self):
        out = self.probe()
        self.assertIn("--resume", out)
        self.assertIn("--output-format json", out)
        self.assertIn("disableAllHooks", out)
        self.assertIn("CLAUDE_CODE_DISABLE_CLAUDE_MDS=1", out)
        self.assertNotIn("--bare", out)

    def test_its_commands_keep_an_exported_key_away_by_default(self):
        self.assertIn("env -u ANTHROPIC_API_KEY claude -p --model", self.probe())

    def test_it_follows_the_login_options(self):
        out = self.probe("--scripted-user-bare", "--agent-api-key")
        self.assertIn("--bare", out)
        self.assertNotIn("disableAllHooks", out)
        self.assertNotIn("env -u ANTHROPIC_API_KEY", out)

    def test_it_keeps_the_options_it_was_given(self):
        other = self.sb.root / "other-claude"
        install(other, STUB_CLAUDE)
        out = self.probe("--claude", other, "--no-sandbox")
        self.assertIn(f"{shlex.quote(str(other))} -p", out)
        self.assertNotIn("autoAllowBashIfSandboxed", out)

    def test_its_checklist_names_what_only_a_real_run_can_settle(self):
        out = self.probe()
        self.assertIn("AskUserQuestion", out)
        self.assertRegex(out, r"(?i)plan approval")

    def test_running_its_fixture_is_refused(self):
        self.probe()
        done = self.sb.run(variant="probe")
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn("first-probe", out)
        self.assertEqual(self.sb.agent_calls(), [])


class InspectInit(unittest.TestCase):
    INIT = {
        "type": "system",
        "subtype": "init",
        "session_id": "s",
        "model": PINNED_MODEL,
        "slash_commands": ["compact", "deliver"],
        "skills": ["team-plan"],
        "agents": ["implementer"],
        "permissionMode": "acceptEdits",
    }

    def setUp(self):
        self.sb = Sandbox(self)

    def inspect(self, *events):
        transcript = self.sb.root / "transcript.jsonl"
        lines = ["not json", json.dumps({"type": "system", "subtype": "hook_started"})]
        lines += [json.dumps(event) for event in events]
        transcript.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return self.sb.bench("inspect-init", transcript)

    def test_it_prints_the_fields_and_a_good_verdict(self):
        done = self.inspect(self.INIT)
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        for piece in (PINNED_MODEL, "deliver", "team-plan", "implementer", "acceptEdits"):
            self.assertIn(piece, out)
        self.assertRegex(out, r"(?i)verdict\W+ok")

    def test_deliver_may_be_a_skill_rather_than_a_command(self):
        event = dict(self.INIT, slash_commands=["compact"], skills=["deliver"])
        self.assertRegex(combined(self.inspect(event)), r"(?i)verdict\W+ok")

    def test_another_model_gets_an_invalid_verdict(self):
        out = combined(self.inspect(dict(self.INIT, model="claude-other-1")))
        self.assertRegex(out, r"(?i)verdict\W+invalid")

    def test_no_deliver_gets_an_invalid_verdict(self):
        out = combined(self.inspect(dict(self.INIT, slash_commands=["compact"], skills=[])))
        self.assertRegex(out, r"(?i)verdict\W+invalid")

    def test_a_transcript_with_no_init_event_says_so(self):
        done = self.inspect({"type": "result", "result": "x"})
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertIn("init", combined(done))


class Cli(unittest.TestCase):
    def test_help_names_every_subcommand(self):
        done = Sandbox(self).bench("--help")
        self.assertEqual(done.returncode, 0, combined(done))
        for name in SUBCOMMANDS:
            self.assertIn(name, done.stdout)


if __name__ == "__main__":
    unittest.main()
