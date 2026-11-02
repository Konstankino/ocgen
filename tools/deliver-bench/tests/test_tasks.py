"""`validate` over every task, and the rules each task folder has to keep."""

import fnmatch
import json
import re
import unittest

from bench_helpers import (
    CALC_PARTIAL,
    CALC_SOLUTION,
    REAL_TASKS,
    TINY,
    Sandbox,
    combined,
    make_task,
)

REQUIRED_FIELDS = (
    "slug",
    "kind",
    "goal",
    "allowed_paths",
    "trap",
    "base_hidden",
    "visible_base",
    "reference_report",
)
CHECK_NAMES = "base_hidden|visible_base|solution_hidden|visible_solution|traps|repo_rule"
FORBIDDEN_IN_REPO = {".git", ".claude", "claude.md", "claude.local.md"}
FIXTURE_LINE_LIMIT = 120

# Words a goal must not use, because they would hand the agent the trap.
TRAP_WORDS = {
    "misleading-bug": ["round", "refund", "money"],
    "far-caller-contract": ["scheduler", "optional"],
    "reuse-existing-helper": ["timeutil", "parse_duration", "uppercase"],
    "ambiguous-requirement": ["case", "updated_at", "recent", "plus"],
    "scope-trap": ["legacy", "todo"],
    "windows-path": ["ntpath", "os.path"],
    "dependent-tasks": [
        "depend", "keyerror", "older", "old file", "written before", "existing", "migrat", "keep working",
        "load_tasks",
    ],
    "ci-only-failure": ["__all__", "check_style", "style check", "workflow"],
    "red-base": ["failing", "leap", "calendar"],
    "test-guards-invariant": ["invariant", "never"],
    "false-premise": ["reproduce", "doesn't"],
    "idempotent-retry": ["4xx", "idempotency", "client error"],
}

ONE_PASSING_HIDDEN_TEST = """\
import unittest

import calc


class Hidden(unittest.TestCase):
    def test_add(self):
        self.assertEqual(calc.add(1, 2), 3)
"""

FAILS_TO_COLLECT_ON_BASE = """\
import unittest

from calc import mul


class Hidden(unittest.TestCase):
    def test_mul(self):
        self.assertEqual(mul(3, 2), 6)
"""


def real_tasks():
    if not REAL_TASKS.is_dir():
        return []
    return sorted(path for path in REAL_TASKS.iterdir() if path.is_dir())


def load_task(root):
    return json.loads((root / "task.json").read_text(encoding="utf-8"))


class RealTasks(unittest.TestCase):
    def test_the_specified_tasks_are_all_there(self):
        self.assertLessEqual(set(TRAP_WORDS), {root.name for root in real_tasks()})

    def test_every_task_has_the_full_shape(self):
        self.assertTrue(real_tasks(), "no tasks found")
        for root in real_tasks():
            with self.subTest(task=root.name):
                task = load_task(root)
                for field in REQUIRED_FIELDS:
                    self.assertTrue(task.get(field), field)
                self.assertEqual(task["slug"], root.name)
                self.assertNotIn("\n", task["trap"])
                self.assertIn(task["base_hidden"], ("fail", "pass"))
                self.assertIn(task["visible_base"], ("fail", "pass"))
                self.assertIn(task.get("visible_solution", "pass"), ("fail", "pass"))
                self.assertTrue((root / "spec.md").is_file())
                self.assertTrue((root / "repo" / "tests").is_dir())
                self.assertTrue((root / "hidden" / "test_hidden.py").is_file())
                self.assertTrue((root / "hidden" / "traps.py").is_file())

    def test_repo_rule_holds_for_every_task(self):
        for root in real_tasks():
            for path in (root / "repo").rglob("*"):
                self.assertNotIn(path.name.lower(), FORBIDDEN_IN_REPO, str(path))

    def test_the_reference_solution_stays_inside_allowed_paths(self):
        for root in real_tasks():
            solution = root / "solution"
            files = [
                path.relative_to(solution).as_posix()
                for path in solution.rglob("*")
                if path.is_file() and "__pycache__" not in path.parts
            ]
            self.assertTrue(files, root.name)
            allowed = load_task(root)["allowed_paths"]
            for name in files:
                matched = any(fnmatch.fnmatchcase(name, glob) for glob in allowed)
                self.assertTrue(matched, f"{root.name}: {name}")

    def test_goals_do_not_name_the_trap(self):
        for root in real_tasks():
            goal = load_task(root)["goal"].lower()
            for word in TRAP_WORDS.get(root.name, []):
                self.assertNotIn(word, goal, root.name)

    def test_fixtures_stay_small(self):
        for root in real_tasks():
            lines = sum(
                len(path.read_text(encoding="utf-8").splitlines())
                for path in (root / "repo").rglob("*.py")
            )
            self.assertLessEqual(lines, FIXTURE_LINE_LIMIT, root.name)

    def test_validate_passes_on_every_real_task(self):
        sandbox = Sandbox(self)
        done = sandbox.bench("validate")
        out = combined(done)
        self.assertEqual(done.returncode, 0, out)
        self.assertTrue(real_tasks(), "no tasks found")
        for root in real_tasks():
            self.assertIn(root.name, out)


class ValidateCatchesBadTasks(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)

    def validate(self):
        return self.sb.bench("validate", "--tasks-dir", self.sb.tasks)

    def assert_caught(self, slug, check):
        """A non-zero exit, the task and the check on one line, and the good task not blamed."""
        done = self.validate()
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertRegex(out, rf"(?m)^.*{re.escape(slug)}.*{re.escape(check)}.*$")
        self.assertNotRegex(out, rf"(?m)^.*\b{TINY}\b.*\b({CHECK_NAMES})\b.*$")

    def test_a_good_task_passes(self):
        done = self.validate()
        self.assertEqual(done.returncode, 0, combined(done))
        self.assertIn(TINY, combined(done))

    def test_it_prints_each_tasks_base_fraction(self):
        self.assertRegex(combined(self.validate()), rf"(?m)^.*{TINY}.*base fraction 0\.25")

    def test_hidden_tests_that_do_not_collect_on_base(self):
        # Every test would fail as one import error, so nothing could count as fixed.
        make_task(self.sb.tasks, "bad-collect", files={"hidden/test_hidden.py": FAILS_TO_COLLECT_ON_BASE})
        self.assert_caught("bad-collect", "base_hidden")

    def test_a_hidden_test_that_already_passes_on_base(self):
        make_task(
            self.sb.tasks, "bad-base", files={"hidden/test_hidden.py": ONE_PASSING_HIDDEN_TEST}
        )
        self.assert_caught("bad-base", "base_hidden")

    def test_visible_tests_on_base_that_do_not_match(self):
        make_task(self.sb.tasks, "bad-visible", task={"visible_base": "fail"})
        self.assert_caught("bad-visible", "visible_base")

    def test_a_solution_that_leaves_a_hidden_test_failing(self):
        make_task(self.sb.tasks, "bad-solution", files={"solution/calc.py": CALC_PARTIAL})
        self.assert_caught("bad-solution", "solution_hidden")

    def test_visible_tests_on_the_solution_that_do_not_match(self):
        make_task(self.sb.tasks, "bad-after", task={"visible_solution": "fail"})
        self.assert_caught("bad-after", "visible_solution")

    def test_visible_solution_defaults_to_pass(self):
        breaks_visible = CALC_SOLUTION.replace("return a + b", "return a + b if a != b else 0")
        make_task(self.sb.tasks, "bad-default", files={"solution/calc.py": breaks_visible})
        self.assert_caught("bad-default", "visible_solution")

    def test_a_trap_that_stays_false_on_the_solution(self):
        traps = "def check(repo, final_report, transcript):\n    return {'never': False}\n"
        make_task(self.sb.tasks, "bad-trap", files={"hidden/traps.py": traps})
        self.assert_caught("bad-trap", "traps")

    def test_a_trap_that_raises(self):
        traps = "def check(repo, final_report, transcript):\n    raise RuntimeError('boom')\n"
        make_task(self.sb.tasks, "bad-raise", files={"hidden/traps.py": traps})
        self.assert_caught("bad-raise", "traps")

    def test_a_task_without_traps(self):
        make_task(self.sb.tasks, "bad-no-traps", drop=("hidden/traps.py",))
        self.assert_caught("bad-no-traps", "traps")

    def test_hidden_tests_that_test_nothing(self):
        # Otherwise `base_hidden: pass` would hold for a file with no test in it.
        make_task(
            self.sb.tasks,
            "bad-empty",
            files={"hidden/test_hidden.py": "import unittest\n"},
            task={"base_hidden": "pass"},
        )
        self.assert_caught("bad-empty", "")

    def test_a_broken_task_json(self):
        root = make_task(self.sb.tasks, "bad-json")
        (root / "task.json").write_text("{ not json", encoding="utf-8")
        self.assert_caught("bad-json", "")

    def test_every_name_the_repo_rule_forbids(self):
        names = {
            "bad-rule-git": ".git/config",
            "bad-rule-claude": ".claude/settings.json",
            "bad-rule-md": "CLAUDE.md",
            "bad-rule-local": "CLAUDE.local.md",
            "bad-rule-deep-md": "sub/CLAUDE.md",
            "bad-rule-deep-claude": "sub/.claude/settings.json",
        }
        for slug, name in names.items():
            make_task(self.sb.tasks, slug, files={f"repo/{name}": "loaded by a session\n"})
        done = self.validate()
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        for slug in names:
            self.assertRegex(out, rf"(?m)^.*{re.escape(slug)}\b.*repo_rule.*$")

    def test_no_tasks_is_a_failure_not_a_pass(self):
        empty = self.sb.root / "empty"
        empty.mkdir()
        for folder in (empty, self.sb.root / "missing"):
            with self.subTest(folder=folder.name):
                done = self.sb.bench("validate", "--tasks-dir", folder)
                self.assertNotEqual(done.returncode, 0, combined(done))
                self.assertNotIn("Traceback", combined(done))
                self.assertIn(str(folder), combined(done))

    def test_validate_works_in_a_copy(self):
        repo = self.sb.tasks / TINY / "repo"
        before = sorted(path.as_posix() for path in repo.rglob("*"))
        done = self.validate()
        self.assertEqual(done.returncode, 0, combined(done))
        self.assertEqual(sorted(path.as_posix() for path in repo.rglob("*")), before)


if __name__ == "__main__":
    unittest.main()
