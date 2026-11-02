"""Shared fixtures for the deliver-bench tests: a throwaway work area, tiny tasks and stand-ins.

Nothing here needs the network, `claude` or cargo. The stand-ins reach the tool through PATH, the
way the real binaries do. A missing bench.py or score.py fails each test by name, not the file.
"""

import hashlib
import importlib.util
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TESTS_DIR = Path(__file__).resolve().parent
TOOL_DIR = TESTS_DIR.parent
REPO_ROOT = TOOL_DIR.parents[1]
BENCH = TOOL_DIR / "bench.py"
SCORE = TOOL_DIR / "score.py"
REAL_TASKS = TOOL_DIR / "tasks"
STUB_CLAUDE = TESTS_DIR / "stub_claude.py"
FAKE_OCGEN = TESTS_DIR / "fake_ocgen.py"
FAKE_SCAFFOLD = TESTS_DIR / "fake_scaffold.py"

PINNED_MODEL = "claude-opus-5-5"
VARIANT = "v1"
TINY = "tiny"
SPEC_SENTINEL = "SPEC-SENTINEL-7f3a"
BENCH_TIMEOUT = 300

CALC_BASE = "def add(a, b):\n    return a + b\n"
CALC_PARTIAL = CALC_BASE + "\n\ndef sub(a, b):\n    return a - b\n"
CALC_SOLUTION = CALC_PARTIAL + "\n\ndef mul(a, b):\n    return a * b\n\n\ndef neg(a):\n    return -a\n"
NOTES = "".join(f"note line {number}\n" for number in range(1, 21))

VISIBLE_TEST = """\
import unittest

import calc


class Visible(unittest.TestCase):
    def test_add(self):
        self.assertEqual(calc.add(2, 2), 4)
"""

# On base one of the four passes, so a fraction is 0.25, 0.5 or 1.0 by how much of calc.py exists.
HIDDEN_TEST = """\
import unittest

import calc


class Hidden(unittest.TestCase):
    def test_add(self):
        self.assertEqual(calc.add(1, 2), 3)

    def test_sub(self):
        self.assertEqual(calc.sub(3, 2), 1)

    def test_mul(self):
        self.assertEqual(calc.mul(3, 2), 6)

    def test_neg(self):
        self.assertEqual(calc.neg(3), -3)
"""

# `transcript_given` pins that `validate` hands the reference report over as the transcript too.
TRAPS = """\
def check(repo, final_report, transcript):
    return {
        "calc_present": (repo / "calc.py").exists(),
        "report_says_done": "done" in final_report.lower(),
        "transcript_given": "done" in transcript.lower(),
    }
"""


def import_score():
    """score.py, imported per test so that a missing one fails that test by name."""
    if str(TOOL_DIR) not in sys.path:
        sys.path.insert(0, str(TOOL_DIR))
    import score

    return score


def task_folder(slug):
    """A task's opaque folder: the agent's cwd must not name the trap."""
    return "t-" + hashlib.sha256(slug.encode("utf-8")).hexdigest()[:8]


def combined(done):
    return done.stdout + done.stderr


def git(repo, *args):
    """`git -C repo args`, with no user or system config; its output, stripped. Raises on failure."""
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_AUTHOR_NAME="test",
        GIT_AUTHOR_EMAIL="test@example.invalid",
        GIT_COMMITTER_NAME="test",
        GIT_COMMITTER_EMAIL="test@example.invalid",
    )
    done = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, encoding="utf-8", env=env
    )
    if done.returncode != 0:
        raise AssertionError(f"git {' '.join(args)} failed: {done.stderr}")
    return done.stdout.strip()


def install(path, script):
    """An executable wrapper around a Python script, which may not carry an exec bit itself."""
    path.write_text(
        f'#!/bin/sh\nexec {shlex.quote(sys.executable)} {shlex.quote(str(script))} "$@"\n',
        encoding="utf-8",
    )
    path.chmod(0o755)


def make_task(tasks_dir, slug=TINY, files=None, task=None, drop=()):
    """A tiny valid task: `calc.py` has one of the four functions the hidden tests want.

    `files` adds or replaces files by their path in the task folder, `task` overrides fields in
    task.json, `drop` removes files. `visible_solution` is left out to pin its default.
    """
    root = Path(tasks_dir) / slug
    contents = {
        "spec.md": f"# Spec\n\nEvery function is plain arithmetic. {SPEC_SENTINEL}\n",
        "repo/calc.py": CALC_BASE,
        "repo/notes.txt": NOTES,
        "repo/tests/test_calc.py": VISIBLE_TEST,
        "hidden/test_hidden.py": HIDDEN_TEST,
        "hidden/traps.py": TRAPS,
        "solution/calc.py": CALC_SOLUTION,
    }
    contents.update(files or {})
    for name in drop:
        del contents[name]
    for name, text in contents.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    fields = {
        "slug": slug,
        "kind": "tiny",
        "goal": "Add the missing calculator functions to calc.py.",
        "allowed_paths": ["calc.py", "tests/*"],
        "trap": "None; this task only exercises the tool.",
        "base_hidden": "fail",
        "visible_base": "pass",
        "reference_report": "Done: added the missing functions.\nConfidence: 90%",
    }
    fields.update(task or {})
    (root / "task.json").write_text(json.dumps(fields, indent=2) + "\n", encoding="utf-8")
    return root


def attempt_copy(slug, folder, files=None, restore=()):
    """A real task's reference solution with an attempt on top: `restore` puts files back from `repo/`
    and `files` writes others. Returns the copy."""
    root = REAL_TASKS / slug
    repo = Path(folder) / "repo"
    shutil.copytree(root / "repo", repo)
    shutil.copytree(root / "solution", repo, dirs_exist_ok=True)
    for name in restore:
        shutil.copyfile(root / "repo" / name, repo / name)
    for name, text in (files or {}).items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text(text, encoding="utf-8")
    return repo


def trap_verdicts(slug, files=None, restore=(), report=None, transcript=None):
    """A real task's traps on an attempt. The report and transcript default to its reference report."""
    root = REAL_TASKS / slug
    reference = json.loads((root / "task.json").read_text(encoding="utf-8"))["reference_report"]
    with tempfile.TemporaryDirectory(prefix="deliver-bench-trap-") as folder:
        repo = attempt_copy(slug, folder, files, restore)
        spec = importlib.util.spec_from_file_location("traps_under_test", root / "hidden" / "traps.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.check(
            repo,
            reference if report is None else report,
            reference if transcript is None else transcript,
        )


def hidden_passes(slug, files=None, restore=()):
    """Whether every hidden test of a real task passes on an attempt."""
    hidden = REAL_TASKS / slug / "hidden"
    with tempfile.TemporaryDirectory(prefix="deliver-bench-mutation-") as folder:
        repo = attempt_copy(slug, folder, files, restore)
        done = subprocess.run(
            [sys.executable, "-B", "-m", "unittest", "discover", "-s", str(hidden), "-p", "test_hidden.py"],
            cwd=repo,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            timeout=BENCH_TIMEOUT,
        )
    return done.returncode == 0


def record_run(run_dir, final_report="Done. All tests pass.\nConfidence: 90%", transcript=(), **fields):
    """Write the files a finished `run` leaves, so `score` can be tested without `run`."""
    run = {
        "session_id": "stub-session-2",
        "end_reason": "finished",
        "rounds": 2,
        "final_report": final_report,
        "questions": ["Which rule?"],
        "permission_denials": [],
        "total_cost_usd": 0.25,
        "num_turns": 7,
        "invalid": False,
        "invalid_reasons": [],
        "merge_block_allowed": False,
    }
    run.update(fields)
    (run_dir / "run.json").write_text(json.dumps(run), encoding="utf-8")
    lines = list(transcript) or [json.dumps({"type": "result", "result": final_report or ""})]
    (run_dir / "transcript.jsonl").write_text("\n".join(lines) + "\n", encoding="utf-8")


class Sandbox:
    """One test's work area. Commands run from it, so nothing depends on the caller's cwd."""

    def __init__(self, testcase):
        holder = tempfile.TemporaryDirectory(prefix="deliver-bench-")
        testcase.addCleanup(holder.cleanup)
        self.root = Path(holder.name).resolve()
        self.home = self.root / "home"
        self.work = self.root / "work"
        self.tasks = self.root / "tasks"
        self.bin = self.root / "bin"
        self.checkout = self.root / "checkout"
        for folder in (self.home, self.tasks, self.bin, self.checkout):
            folder.mkdir()
        self.claude_log = self.root / "claude.log"
        self.ocgen_log = self.root / "ocgen.log"
        self.scaffold_cmd = shlex.join([sys.executable, str(FAKE_SCAFFOLD)])
        # TMPDIR may sit inside the checkout; only then do the fixtures need the override.
        self.nested = REPO_ROOT in self.root.parents
        install(self.bin / "claude", STUB_CLAUDE)
        install(self.bin / "ocgen", FAKE_OCGEN)
        make_task(self.tasks)
        (self.checkout / "Cargo.toml").write_text('[package]\nname = "fake"\n', encoding="utf-8")
        git(self.checkout, "init", "-q")
        git(self.checkout, "add", ".")
        git(self.checkout, "commit", "-q", "-m", "fake checkout")

    def install_cargo(self):
        """A fake `cargo`, for fixtures made the real way: `--scaffold-cmd` marks a run invalid."""
        install(self.bin / "cargo", FAKE_SCAFFOLD)

    def env(self, **extra):
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("GIT_", "BENCH_"))
            and key not in ("ANTHROPIC_API_KEY", "CLAUDE_CODE_DISABLE_CLAUDE_MDS")
        }
        env.update(
            HOME=str(self.home),
            USERPROFILE=str(self.home),
            XDG_CONFIG_HOME=str(self.home / ".config"),
            PATH=str(self.bin) + os.pathsep + os.environ.get("PATH", ""),
            BENCH_STUB_LOG=str(self.claude_log),
            BENCH_FAKE_OCGEN_LOG=str(self.ocgen_log),
            BENCH_FAKE_APPROVAL="unlocked",
        )
        env.update(extra)
        return env

    def bench(self, *args, **env):
        return subprocess.run(
            [sys.executable, str(BENCH), *[str(arg) for arg in args]],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            stdin=subprocess.DEVNULL,
            cwd=self.root,
            env=self.env(**env),
            timeout=BENCH_TIMEOUT,
        )

    def run_dir(self, task=TINY, variant=VARIANT, run=1):
        """A run's results, in a tree apart from the fixtures an agent can look around in."""
        return self.work / "results" / variant / task_folder(task) / f"r{run}"

    def repo(self, task=TINY, variant=VARIANT, run=1):
        return self.work / "fixtures" / variant / task_folder(task) / f"r{run}" / "repo"

    def prepare(
        self,
        *extra,
        work=None,
        task=TINY,
        variant=VARIANT,
        runs=1,
        fake_scaffold=True,
        allow_nested=None,
        check=True,
        **env,
    ):
        args = [
            "prepare",
            "--work-dir", work or self.work,
            "--tasks-dir", self.tasks,
            "--variant", variant,
            "--ocgen-dir", self.checkout,
            "--task", task,
            "--runs", runs,
        ]
        if fake_scaffold:
            args += ["--scaffold-cmd", self.scaffold_cmd]
        if self.nested if allow_nested is None else allow_nested:
            args.append("--allow-nested-work-dir")
        done = self.bench(*args, *extra, **env)
        if check and done.returncode != 0:
            raise AssertionError(f"prepare failed: {combined(done)}")
        return done

    def run(self, *extra, task=TINY, variant=VARIANT, **env):
        env.setdefault("BENCH_STUB_OVERLAY", str(self.tasks / task / "solution"))
        return self.bench(
            "run",
            "--work-dir", self.work,
            "--tasks-dir", self.tasks,
            "--variant", variant,
            "--task", task,
            *extra,
            **env,
        )

    def score(self, task=TINY, variant=VARIANT, run=1):
        done = self.bench(
            "score",
            "--work-dir", self.work,
            "--tasks-dir", self.tasks,
            "--variant", variant,
            "--task", task,
        )
        if done.returncode != 0 or "Traceback" in combined(done):
            raise AssertionError(f"score failed: {combined(done)}")
        path = self.run_dir(task, variant, run) / "score.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def run_json(self, task=TINY, variant=VARIANT, run=1):
        path = self.run_dir(task, variant, run) / "run.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def manifest(self, task=TINY, variant=VARIANT, run=1):
        path = self.run_dir(task, variant, run) / "manifest.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def claude_calls(self):
        if not self.claude_log.exists():
            return []
        lines = self.claude_log.read_text(encoding="utf-8").splitlines()
        return [json.loads(line) for line in lines]

    def agent_calls(self):
        return [
            call
            for call in self.claude_calls()
            if call["flags"].get("--output-format") == "stream-json"
        ]

    def scripted_calls(self):
        """Every scripted-user call, the bench's check before the first fixture included."""
        return [call for call in self.claude_calls() if call["flags"].get("--output-format") == "json"]

    def user_calls(self):
        """The scripted-user calls of a fixture: the ones that carry a spec."""
        return [call for call in self.scripted_calls() if "Spec:" in call["prompt"]]

    def approve_calls(self):
        """Every logged `ocgen` call that names `approve`, as its argv."""
        if not self.ocgen_log.exists():
            return []
        calls = [json.loads(line) for line in self.ocgen_log.read_text(encoding="utf-8").splitlines()]
        return [argv for argv in calls if "approve" in argv]

    def path_with_only_git(self):
        """A PATH with git on it and neither `claude` nor `ocgen`."""
        folder = self.root / "only-git"
        folder.mkdir()
        wrapper = folder / "git"
        wrapper.write_text(
            f'#!/bin/sh\nexec {shlex.quote(shutil.which("git"))} "$@"\n', encoding="utf-8"
        )
        wrapper.chmod(0o755)
        return str(folder)
