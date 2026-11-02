#!/usr/bin/env python3
"""golden-task-bench: measures ocgen's /deliver skill on small golden tasks. A regression guard.

Usage (details in README.md; paths resolve from this file, so any cwd works):

    python3 bench.py validate [--tasks-dir DIR] [--task SLUG]
    python3 bench.py prepare --work-dir DIR --variant NAME [--ocgen-dir CHECKOUT] [--task SLUG] [--runs N]
    python3 bench.py approvals --work-dir DIR [--variant NAME]
    python3 bench.py first-probe --work-dir DIR [--task SLUG]
    python3 bench.py inspect-init TRANSCRIPT
    python3 bench.py run --work-dir DIR --variant NAME [--task SLUG] [--force] [--rerun-finished]
    python3 bench.py score --work-dir DIR --variant NAME
    python3 bench.py report --work-dir DIR --variant NAME
    python3 bench.py compare A B --work-dir DIR

It never grants an approval: it prints `ocgen approve` for the human and only calls the status form.
"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True  # Task folders and fixtures stay free of __pycache__.

import score as scoring  # noqa: E402

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[1]
# Trees under a work dir: an agent can look around a fixture's folders, and results hold the hidden
# spec and name the task. `bin` holds each variant's own copy of ocgen, which an agent's PATH names.
FIXTURES = "fixtures"
RESULTS = "results"
BINARIES = "bin"
DEFAULT_TASKS = HERE / "tasks"
DEFAULT_VARIANT = "base"
PROBE_VARIANT = "probe"
DEFAULT_MODEL = "claude-opus-5-5"
MODEL_ALIASES = {"opus", "sonnet", "haiku", "opusplan", "default", "best"}
SANDBOX_SETTINGS = json.dumps({"sandbox": {"enabled": True, "autoAllowBashIfSandboxed": True}})
DEFAULT_MAX_ROUNDS = 12
DEFAULT_CALL_TIMEOUT = 3 * 60 * 60
TEST_TIMEOUT = 5 * 60
APPROVAL_MINUTES = 1440
USER_ATTEMPTS = 2
USER_CHECK_TIMEOUT = 5 * 60
API_KEY = "ANTHROPIC_API_KEY"
# What keeps the scripted user's call free of project and user instructions and hooks, on the user's own
# login. `--bare` would too, but it never reads a subscription login: it needs an API key.
USER_SETTINGS = json.dumps({"disableAllHooks": True})
USER_NO_CLAUDE_MDS = "CLAUDE_CODE_DISABLE_CLAUDE_MDS"
USER_CHECK_PROMPT = "This is a check that you can answer. Answer with exactly this word, and nothing else: FINISHED"
RESULT_FILES = ("transcript.jsonl", "user.jsonl", "stderr.log", "run.json", "score.json")
PREVIOUS = "previous"

REQUIRED_FIELDS = (
    "slug", "kind", "goal", "allowed_paths", "trap", "base_hidden", "visible_base", "reference_report",
)
FORBIDDEN_IN_REPO = {".git", ".claude", "claude.md", "claude.local.md"}
NOT_COPIED = (".git", ".claude", ".ocgen-backup", "__pycache__")
REPO_POINTERS = (
    "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
)
# Neutral on purpose: an agent reads these, and they must not say it is being measured.
FIXTURE_GIT_CONFIG = (
    ("core.autocrlf", "false"),
    ("user.name", "Project Owner"),
    ("user.email", "owner@example.invalid"),
    ("commit.gpgsign", "false"),
)
ANSI = re.compile(r"\x1b\[[0-9;]*m")

# Runs with a copy of the working tree as cwd. A failed import is one failing test, so an agent that
# deleted a module scores 0, not a crash.
HIDDEN_RUNNER = """
import json, os, sys, unittest

hidden_dir, out_path = sys.argv[1], sys.argv[2]
sys.path.insert(0, os.getcwd())
outcome = {}


class Recorder(unittest.TestResult):
    def addSuccess(self, test):
        outcome[test.id()] = True

    def addFailure(self, test, err):
        outcome[test.id()] = False

    def addError(self, test, err):
        outcome[test.id()] = False

    def addSkip(self, test, reason):
        outcome[test.id()] = False

    def addExpectedFailure(self, test, err):
        outcome[test.id()] = True

    def addUnexpectedSuccess(self, test):
        outcome[test.id()] = False

    def addSubTest(self, test, subtest, err):
        if err is not None:
            outcome[test.id()] = False


try:
    suite = unittest.TestLoader().discover(hidden_dir, pattern="test_hidden.py", top_level_dir=hidden_dir)
    suite.run(Recorder())
finally:
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(outcome, handle)
"""

USER_PROMPT = """You are playing the user of a software project. A developer is working on your \
request and has just sent the message below. Decide how to answer it.

Rules:
- Answer only what the developer asked, and only from the spec below. If the spec doesn't cover it, \
say you have no preference.
- Approve a plan that is consistent with the spec. If a plan contradicts the spec, don't approve it: \
say what's wrong, from the spec.
- Merging finished work into the working branch is already approved. If the developer asks, say so.
- Volunteer nothing: no hints, and nothing the developer didn't ask about.
- If the message is the developer's final report on the work, answer FINISHED.

Answer with exactly one of these, and nothing else:
REPLY: <your answer>
FINISHED

Your request:
{goal}

Spec:
{spec}

The developer's last message:
{message}
{asked}"""

TOOL_QUESTIONS_SECTION = """
The developer also asked this through a question tool, and is waiting for your answer:
{questions}
"""


class BenchError(Exception):
    """A problem to report in one message, not a crash."""


# ------------------------------------------------------------------ helpers --


def task_folder(slug):
    """A task's folder in a work dir: opaque, because the slug would name the trap in the agent's cwd."""
    return "t-" + hashlib.sha256(slug.encode("utf-8")).hexdigest()[:8]


def is_inside(path, folder):
    return path == folder or folder in path.parents


def read_json(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except ValueError as error:
        raise BenchError(f"{path} isn't valid JSON: {error}") from error


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def sha256_of(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def task_hash(root):
    """A hash of a task folder, to tell runs of the same task from runs of an edited one."""
    digest = hashlib.sha256()
    files = sorted(path for path in root.rglob("*") if path.is_file() and "__pycache__" not in path.parts)
    for path in files:
        digest.update(path.relative_to(root).as_posix().encode("utf-8"))
        digest.update(path.read_bytes())
    return digest.hexdigest()


def clean_env():
    """This environment, minus anything that would point git at another repository."""
    return {key: value for key, value in os.environ.items() if key not in REPO_POINTERS}


def cargo_env(ocgen_dir):
    """Every build lands in the checkout's own target: a shared CARGO_TARGET_DIR would give two variants
    one binary."""
    return dict(clean_env(), CARGO_TARGET_DIR=str(ocgen_dir / "target"))


def git(repo, *args, check=True):
    """`git -C repo args`' output, stripped. A failure raises, or with check=False returns None."""
    done = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        env=clean_env(),
    )
    if done.returncode == 0:
        return done.stdout.strip()
    if check:
        raise BenchError(f"git {' '.join(args)} failed in {repo}: {done.stderr.strip()}")
    return None


def tool_output(argv, env=None):
    """A small command's stdout, stripped. None when it is missing, fails or hangs."""
    try:
        done = subprocess.run(
            argv,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            stdin=subprocess.DEVNULL,
            env=env,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return done.stdout.strip() if done.returncode == 0 else None


def checked(argv, cwd, env, what):
    """Run a build or scaffold step, keeping its noise unless it fails."""
    try:
        done = subprocess.run(
            argv,
            cwd=cwd,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            stdin=subprocess.DEVNULL,
            env=env,
        )
    except OSError as error:
        raise BenchError(f"{what}: can't run {argv[0]}: {error}") from error
    if done.returncode != 0:
        tail = "\n".join((done.stdout + done.stderr).strip().splitlines()[-20:])
        raise BenchError(f"{what}: {argv[0]} exited with {done.returncode}\n{tail}")


def copy_tree(source, destination):
    """A copy of a working tree, without the tool folders."""
    shutil.copytree(source, destination, symlinks=True, ignore=shutil.ignore_patterns(*NOT_COPIED))


def check_model(model):
    if model in MODEL_ALIASES:
        raise BenchError(
            f"{model!r} is an alias, which can change between runs. Pin a full id, like {DEFAULT_MODEL}."
        )


def approve_command(repo):
    """The command for the human. Printed, never run: `ocgen approve` refuses a script anyway."""
    return f"ocgen approve {shlex.quote(str(repo))} --minutes {APPROVAL_MINUTES}"


def approval_open(ocgen, repo, env):
    """Whether this fixture's approval is open. Anything but a clear yes counts as no."""
    if not ocgen:
        return False
    status = tool_output([ocgen, "approve", str(repo), "--status"], env)
    return bool(status) and ANSI.sub("", status).startswith("unlocked")


def probe_ocgen(env):
    """Which `ocgen` a process with this environment finds, and what it says about itself."""
    path = shutil.which("ocgen", path=env.get("PATH"))
    if not path:
        return {"ocgen_path": None, "ocgen_version": None, "ocgen_protocol": None, "ocgen_sha256": None}
    return {
        "ocgen_path": path,
        "ocgen_version": tool_output([path, "--version"], env),
        "ocgen_protocol": tool_output([path, "hook", "--check"], env),
        "ocgen_sha256": sha256_of(path),
    }


def task_roots(tasks_dir):
    if not tasks_dir.is_dir():
        return []
    return sorted(
        path for path in tasks_dir.iterdir() if path.is_dir() and not path.name.startswith((".", "__"))
    )


def selected_slugs(args):
    known = [root.name for root in task_roots(args.tasks_dir)]
    if not known:
        raise BenchError(f"no tasks in {args.tasks_dir}")
    for slug in args.task or []:
        if slug not in known:
            raise BenchError(f"no task named {slug} in {args.tasks_dir}")
    return args.task or known


class Fixture:
    """A prepared run: `run_dir` in the results tree holds `manifest.json` and what `run` and `score`
    leave, and `repo` is the same path in the fixtures tree."""

    def __init__(self, work, run_dir):
        self.run_dir = run_dir
        self.repo = work / FIXTURES / run_dir.relative_to(work / RESULTS) / "repo"
        self.manifest_path = run_dir / "manifest.json"
        self.manifest = read_json(self.manifest_path)
        self.slug = self.manifest["slug"]
        self.variant = self.manifest["variant"]
        self.number = self.manifest["run"]
        self.label = f"{self.variant}/{self.slug} r{self.number}"

    def agent_env(self, api_key=False):
        """The agent's environment: hooks find the ocgen its scaffold came from first, and a key exported
        for something else doesn't silently bill the agent's calls to it."""
        env = clean_env()
        if not api_key:
            env.pop(API_KEY, None)
        first = self.manifest.get("agent_path_first")
        if first:
            env["PATH"] = first + os.pathsep + env.get("PATH", "")
        return env

    def approved(self):
        env = self.agent_env()
        return approval_open(shutil.which("ocgen", path=env.get("PATH")), self.repo, env)

    def used_reasons(self):
        """Why this fixture isn't as `prepare` left it, so a run on it wouldn't be a fresh attempt."""
        reasons = []
        if self.variant == PROBE_VARIANT:
            reasons.append("it belongs to first-probe, for one call by hand")
        if (self.run_dir / "run.json").exists():
            reasons.append("it already ran")
        transcript = self.run_dir / "transcript.jsonl"
        if transcript.exists() and transcript.stat().st_size and not (self.run_dir / "run.json").exists():
            reasons.append("an interrupted run left a transcript")
        if git(self.repo, "rev-parse", "HEAD", check=False) != self.manifest.get("base_commit"):
            reasons.append("HEAD has moved from its base commit")
        if git(self.repo, "status", "--porcelain", check=False):
            reasons.append("its tree has changes")
        if worktree_branches(self.repo, check=False):
            reasons.append("it has worktree-agent branches")
        return reasons


def find_fixtures(work, variant=None, slugs=None):
    """The prepared fixtures, in a stable order."""
    manifests = (work / RESULTS).glob(f"{variant or '*'}/t-*/r*/manifest.json")
    fixtures = [Fixture(work, path.parent) for path in manifests]
    if slugs:
        fixtures = [fixture for fixture in fixtures if fixture.slug in slugs]
    return sorted(fixtures, key=lambda fixture: (fixture.variant, fixture.slug, fixture.number))


def require_fixtures(args):
    fixtures = find_fixtures(args.work_dir, args.variant, args.task)
    if not fixtures:
        raise BenchError(f"no prepared fixtures for variant {args.variant} in {args.work_dir}")
    return fixtures


def worktree_branches(repo, check=True):
    listing = git(repo, "for-each-ref", "--format=%(refname:short)", "refs/heads/worktree-agent-*", check=check)
    return sorted(listing.splitlines()) if listing else []


# ------------------------------------------------------------ hidden, traps --


def run_hidden(copy, hidden_dir):
    """{test id: passed} for the hidden tests, with `copy` as cwd. Empty when there are none."""
    with tempfile.TemporaryDirectory(prefix="deliver-bench-hidden-") as folder:
        out = Path(folder) / "outcome.json"
        argv = [sys.executable, "-B", "-c", HIDDEN_RUNNER, str(hidden_dir), str(out)]
        try:
            subprocess.run(
                argv,
                cwd=copy,
                capture_output=True,
                stdin=subprocess.DEVNULL,
                env=clean_env(),
                timeout=TEST_TIMEOUT,
            )
        except subprocess.TimeoutExpired:
            return {"hidden tests timed out": False}
        if not out.exists():
            return {"hidden tests crashed": False}
        return json.loads(out.read_text(encoding="utf-8"))


def run_visible(copy):
    """"pass" or "fail" for the fixture's own command."""
    try:
        done = subprocess.run(
            [sys.executable, "-B", "-m", "unittest", "discover", "-s", "tests"],
            cwd=copy,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            env=clean_env(),
            timeout=TEST_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return "fail"
    return "pass" if done.returncode == 0 else "fail"


def run_traps(hidden_dir, repo, report, transcript):
    """(verdicts, error). traps.py runs from its own folder, since a trap may read the task's base.
    One that can't run counts as a trap not avoided: an agent's edit shouldn't crash the score."""
    try:
        spec = importlib.util.spec_from_file_location("bench_traps", hidden_dir / "traps.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        verdicts = module.check(repo, report, transcript)
        return {str(name): bool(value) for name, value in verdicts.items()}, None
    except (Exception, SystemExit) as error:
        return {"check_raised": False}, repr(error)


def hidden_on_base(task_root):
    """What the hidden tests do on the task's untouched repo/, in a throwaway copy."""
    with tempfile.TemporaryDirectory(prefix="deliver-bench-base-") as folder:
        copy = Path(folder) / "repo"
        copy_tree(task_root / "repo", copy)
        return run_hidden(copy, task_root / "hidden")


# ----------------------------------------------------------------- validate --


def forbidden_in_repo(repo):
    if not repo.is_dir():
        return ["no repo/ folder"]
    return sorted(
        str(path.relative_to(repo)) for path in repo.rglob("*") if path.name.lower() in FORBIDDEN_IN_REPO
    )


def validate_task(root):
    """(the task's problems as (check, detail), its base fraction or None), each step in a throwaway
    copy."""
    try:
        task = read_json(root / "task.json")
    except (BenchError, OSError) as error:
        return [("task_json", str(error))], None
    missing = [field for field in REQUIRED_FIELDS if not task.get(field)]
    if missing:
        return [("task_json", "missing " + ", ".join(missing))], None
    forbidden = forbidden_in_repo(root / "repo")
    if forbidden:
        return [("repo_rule", "repo/ must not hold " + ", ".join(forbidden))], None

    hidden, solution = root / "hidden", root / "solution"
    if not solution.is_dir():
        return [("solution_hidden", "no solution/ folder")], None
    problems = []
    with tempfile.TemporaryDirectory(prefix="deliver-bench-validate-") as folder:
        made = []

        def copy_of(with_solution):
            copy = Path(folder) / str(len(made))
            made.append(copy)
            copy_tree(root / "repo", copy)
            if with_solution:
                shutil.copytree(solution, copy, dirs_exist_ok=True)
            return copy

        on_base = run_hidden(copy_of(False), hidden)
        if not on_base:
            return [("base_hidden", "no hidden tests found")], None
        base_fraction = scoring.fraction(on_base)
        status = scoring.hidden_status(on_base)
        if status != "ok":
            problems.append(("base_hidden", f"hidden tests don't run on base ({status}), so nothing could count as fixed"))
        got = "pass" if all(on_base.values()) else "fail"
        if got != task["base_hidden"]:
            problems.append(("base_hidden", f"hidden tests {got} on base; task.json says {task['base_hidden']}"))
        got = run_visible(copy_of(False))
        if got != task["visible_base"]:
            problems.append(("visible_base", f"visible tests {got} on base; task.json says {task['visible_base']}"))
        on_solution = run_hidden(copy_of(True), hidden)
        failing = sorted(name for name, passed in on_solution.items() if not passed)
        if failing or not on_solution:
            problems.append(("solution_hidden", "still failing on the solution: " + ", ".join(failing)))
        expected = task.get("visible_solution", "pass")
        got = run_visible(copy_of(True))
        if got != expected:
            problems.append(("visible_solution", f"visible tests {got} on the solution; expected {expected}"))
        # The reference report stands in for the transcript too: there is no run to read here.
        report = task["reference_report"]
        verdicts, error = run_traps(hidden, copy_of(True), report, report)
        if error:
            problems.append(("traps", f"check can't run: {error}"))
        elif not verdicts:
            problems.append(("traps", "check returned no traps"))
        else:
            for name, avoided in verdicts.items():
                if not avoided:
                    problems.append(("traps", f"{name} is False on the solution"))
    return problems, base_fraction


def cmd_validate(args):
    roots = [args.tasks_dir / slug for slug in selected_slugs(args)]
    failed = 0
    for root in roots:
        problems, base_fraction = validate_task(root)
        for check, detail in problems:
            print(f"FAIL {root.name}: {check}: {detail}")
        if problems:
            failed += 1
        base = "" if base_fraction is None else f"  base fraction {base_fraction:.2f}"
        print(f"{'bad ' if problems else 'ok  '} {root.name}{base}")
    print(f"{len(roots) - failed} of {len(roots)} tasks valid")
    return 1 if failed else 0


# ------------------------------------------------------------------ prepare --


def build_ocgen(ocgen_dir):
    """Build the checkout's own binary, so the hooks a fixture runs match its scaffold. Hooks prefer
    the `ocgen` on PATH when its protocol matches (src/render.rs:5050-5060). Returns the binary."""
    manifest_path = ocgen_dir / "Cargo.toml"
    if not manifest_path.is_file():
        raise BenchError(f"{ocgen_dir} is not an ocgen checkout: it has no Cargo.toml")
    argv = ["cargo", "build", "--quiet", "--manifest-path", str(manifest_path), "--bin", "ocgen"]
    checked(argv, ocgen_dir, cargo_env(ocgen_dir), f"building {ocgen_dir}")
    binary = ocgen_dir / "target" / "debug" / "ocgen"
    if not binary.is_file():
        raise BenchError(f"the build of {ocgen_dir} left no {binary}")
    return binary


def variant_binary(work, variant, built):
    """A copy of the build for this variant, so a later build of the same checkout can't change what an
    earlier variant's hooks run. Returns its folder."""
    folder = work / BINARIES / variant
    copy = folder / "ocgen"
    if copy.exists():
        if sha256_of(copy) != sha256_of(built):
            raise BenchError(
                f"{copy} has another sha256 than this build, so the variant's fixtures would differ. "
                "Prepare a new variant."
            )
    else:
        folder.mkdir(parents=True, exist_ok=True)
        shutil.copy2(built, copy)
    return folder


def make_fixture(task_root, repo, run_scaffold):
    """Copy, commit, scaffold, commit; returns the two commits.

    The fixture is its own repository before the scaffold runs, because the scaffold acts on whatever
    repository `git -C <target>` resolves to (src/render.rs:3704-3707, src/gitcheck.rs:219-253). The
    second commit leaves the tree clean, as a merge expects. `.claude/` goes in the exclude file as well
    as `.ocgen-backup/`: ocgen's own block lists only part of it, and one tracked file under `.claude/`
    makes ocgen drop that block.
    """
    copy_tree(task_root / "repo", repo)
    git(repo, "init", "-q", "-b", "main")
    for key, value in FIXTURE_GIT_CONFIG:
        git(repo, "config", "--local", key, value)
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "Initial commit")
    fixture_commit = git(repo, "rev-parse", "HEAD")
    run_scaffold()
    exclude = repo / ".git" / "info" / "exclude"
    exclude.parent.mkdir(exist_ok=True)
    with open(exclude, "a", encoding="utf-8") as handle:
        handle.write("\n.ocgen-backup/\n.claude/\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "--allow-empty", "-m", "Add project configuration")
    return fixture_commit, git(repo, "rev-parse", "HEAD")


def template_overrides():
    folder = Path.home() / ".config" / "ocgen" / "templates"
    return sorted(str(path) for path in folder.rglob("*") if path.is_file()) if folder.is_dir() else []


def inherited_claude_md(repo):
    found = []
    for folder in repo.parents:
        for name in ("CLAUDE.md", "CLAUDE.local.md", ".claude/CLAUDE.md"):
            if (folder / name).is_file():
                found.append(str(folder / name))
    return found


def check_kept_fixture(run_dir, label, wanted):
    """A kept fixture must come from what this prepare would make, or a variant silently mixes two."""
    manifest = read_json(run_dir / "manifest.json")
    for field, value in wanted.items():
        if manifest.get(field) != value:
            raise BenchError(
                f"{label} was prepared with {field} {manifest.get(field)}, and this prepare has {value}. "
                "Prepare a new variant."
            )


def prepare_fixtures(args, slugs, runs):
    """Prepare what's missing and keep what's there, so an approval stays valid. Returns the fixtures."""
    check_model(args.model)
    work = args.work_dir
    ocgen_dir = args.ocgen_dir
    nested = is_inside(work, ocgen_dir) or is_inside(work, REPO_ROOT)
    if nested and not args.allow_nested_work_dir:
        raise BenchError(
            f"work dir {work} is inside an ocgen checkout: a session there inherits its CLAUDE.md, and "
            "scaffolding touches its git hooks. Pick a folder outside it."
        )
    if nested:
        print(f"WARNING: {work} is inside an ocgen checkout, so results from it aren't valid.")
    overrides = template_overrides()
    for path in overrides:
        print(f"WARNING: {path} overrides an embedded template, which changes what is measured.")
    ocgen_commit = git(ocgen_dir, "rev-parse", "HEAD", check=False)
    wanted = {"ocgen_dir": str(ocgen_dir), "ocgen_commit": ocgen_commit, "model": args.model}
    for slug in slugs:
        for number in range(1, runs + 1):
            run_dir = work / RESULTS / args.variant / task_folder(slug) / f"r{number}"
            if (run_dir / "manifest.json").exists():
                check_kept_fixture(run_dir, f"{args.variant}/{slug} r{number}", wanted)
    path_first = None
    if not args.scaffold_cmd:
        path_first = str(variant_binary(work, args.variant, build_ocgen(ocgen_dir)))

    fixtures = []
    for slug in slugs:
        base = None
        for number in range(1, runs + 1):
            relative = Path(args.variant) / task_folder(slug) / f"r{number}"
            run_dir = work / RESULTS / relative
            repo = work / FIXTURES / relative / "repo"
            label = f"{args.variant}/{slug} r{number}"
            if (run_dir / "manifest.json").exists():
                print(f"{label}: already prepared, left as it is: {repo}")
                fixtures.append(Fixture(work, run_dir))
                continue
            if repo.parent.exists():
                raise BenchError(
                    f"{repo.parent} has no manifest, so its preparation never finished ({label}). "
                    "Delete that folder and prepare again."
                )
            repo.parent.mkdir(parents=True)

            def run_scaffold(repo=repo):
                if args.scaffold_cmd:
                    argv = shlex.split(args.scaffold_cmd) + [str(repo)]
                else:
                    argv = [
                        "cargo", "run", "--quiet", "--example", "demo",
                        "--manifest-path", str(ocgen_dir / "Cargo.toml"),
                        "--", str(repo), "claude-team", "English",
                    ]
                checked(argv, ocgen_dir, cargo_env(ocgen_dir), f"scaffolding {repo}")

            task_root = args.tasks_dir / slug
            fixture_commit, base_commit = make_fixture(task_root, repo, run_scaffold)
            base = base or scoring.base_outcome(hidden_on_base(task_root))
            env = clean_env()
            if path_first:
                env["PATH"] = path_first + os.pathsep + env.get("PATH", "")
            inherited = inherited_claude_md(repo)
            for path in inherited:
                print(f"WARNING: a session in this fixture inherits {path}.")
            manifest = {
                "slug": slug,
                "variant": args.variant,
                "run": number,
                "ocgen_dir": str(ocgen_dir),
                "ocgen_commit": ocgen_commit,
                "agent_path_first": path_first,
                "model": args.model,
                "claude_version": tool_output([args.claude, "--version"]),
                "template_overrides": overrides,
                "inherited_claude_md": inherited,
                "global_claude_md": (Path.home() / ".claude" / "CLAUDE.md").is_file(),
                "fixture_commit": fixture_commit,
                "base_commit": base_commit,
                "nested_work_dir": nested,
                "allow_nested_work_dir": args.allow_nested_work_dir,
                "scaffold_cmd": args.scaffold_cmd,
                "task_hash": task_hash(task_root),
            }
            manifest.update(base)
            manifest.update(probe_ocgen(env))
            run_dir.mkdir(parents=True, exist_ok=True)
            write_json(run_dir / "manifest.json", manifest)
            print(f"{label}: prepared: {repo}")
            fixtures.append(Fixture(work, run_dir))
    return fixtures


def cmd_prepare(args):
    prepare_fixtures(args, selected_slugs(args), args.runs)
    print("Next: `approvals` prints what you need to run yourself, then `run`.")
    return 0


# ---------------------------------------------------- approvals, first-probe --


def cmd_approvals(args):
    fixtures = find_fixtures(args.work_dir, args.variant)
    if not fixtures:
        raise BenchError(f"no prepared fixtures in {args.work_dir}")
    print("Run these yourself, from your own terminal. This tool never approves anything:")
    for fixture in fixtures:
        state = "unlocked" if fixture.approved() else "locked"
        print(f"\n{fixture.label} ({state})\n  {approve_command(fixture.repo)}")
    return 0


def render_user_prompt(goal, spec, message, questions=()):
    asked = ""
    if questions:
        asked = TOOL_QUESTIONS_SECTION.format(questions=json.dumps(list(questions), indent=2))
    return USER_PROMPT.format(goal=goal, spec=spec, message=message, asked=asked)


def quoted(argv):
    return " ".join(shlex.quote(part) for part in argv)


def cmd_first_probe(args):
    slugs = selected_slugs(args)[:1]
    fixture = prepare_fixtures(args, slugs, 1)[0]
    task_root = args.tasks_dir / fixture.slug
    goal = read_json(task_root / "task.json")["goal"]
    first = fixture.manifest.get("agent_path_first")
    prefix = "" if args.agent_api_key else f"env -u {API_KEY} "
    prefix += f'PATH={shlex.quote(first)}:"$PATH" ' if first else ""
    user_prefix = "" if args.scripted_user_bare else f"env -u {API_KEY} {USER_NO_CLAUDE_MDS}=1 "
    first_call = agent_argv(args.claude, args.model, None, f"/deliver {goal}", args.no_sandbox)
    resume_call = agent_argv(args.claude, args.model, "<session_id>", "<the reply>", args.no_sandbox)
    transcript = fixture.run_dir / "probe.jsonl"
    prompt_file = fixture.run_dir / "probe-user-prompt.txt"
    prompt_file.write_text(
        render_user_prompt(
            goal,
            (task_root / "spec.md").read_text(encoding="utf-8"),
            "<paste the agent's last message here>",
        ),
        encoding="utf-8",
    )
    user_call = scripted_user_argv(args, args.model) + ["--"]
    print(f"""
First probe on {fixture.slug}, in a variant of its own, so it can't count as a measured run. Nothing has
run yet; these are yours to run, from your own terminal.

1. Approve this fixture (optional for a probe; without it the gate stops merges):
   {approve_command(fixture.repo)}

2. Make the first call:
   cd {shlex.quote(str(fixture.repo))} && {prefix}{quoted(first_call)} > {shlex.quote(str(transcript))}

3. Check the init event:
   python3 {shlex.quote(str(Path(__file__).resolve()))} inspect-init {shlex.quote(str(transcript))}

4. Resume once, with the session_id from the last line of the transcript:
   cd {shlex.quote(str(fixture.repo))} && {prefix}{quoted(resume_call)} >> {shlex.quote(str(transcript))}

5. Try the scripted user, from an empty folder outside any repository, after pasting the agent's last
   message into {shlex.quote(str(prompt_file))}:
   {user_prefix}{quoted(user_call)} "$(cat {shlex.quote(str(prompt_file))})"

What to look for:
- the verdict: the model is {args.model}, and `deliver` is in slash_commands or skills;
- that the scripted-user call answers on your login, and that the init event's auth is what you meant to
  pay with;
- permissionMode is acceptEdits, and the sandbox --settings didn't produce an error;
- `claude -p` accepted the `--` before the prompt, and /deliver ran as a command, not as plain text;
- the model each assistant message names: /deliver's frontmatter can run it on another one than the pinned id;
- whether total_cost_usd and num_turns on `--resume` count that call alone or the whole session. Until you
  know, reported sums may be overstated;
- AskUserQuestion: whether the agent uses it under -p, what its tool_result says, and whether an answer in
  the next call reaches it;
- plan approval across --resume: whether a plan approved in one call still counts in the next, or goes
  stale, since /deliver's gates hold an approval for one team run;
- that the scripted user's output has its answer in `result`, and that the merge gate's text lands in the
  tool_result of the `git merge` it denied.""")
    return 0


def cmd_inspect_init(args):
    path = Path(args.transcript)
    init = scoring.init_event(scoring.events_of(path.read_text(encoding="utf-8", errors="replace")))
    if init is None:
        raise BenchError(f"no init event in {path}")
    print(scoring.init_summary(init, args.model))
    return 1 if scoring.init_problems(init, args.model) else 0


# ---------------------------------------------------------------------- run --


def wait_for(argv, cwd, stdout, stderr, timeout, env):
    """Run one call in its own process group. Its exit code, or None when it timed out. However it ends,
    the group is killed: nothing the call started keeps editing the fixture."""
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=stdout,
        stderr=stderr,
        env=env,
        start_new_session=True,
    )
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        return None
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
        process.wait()


def agent_argv(claude, model, session_id, prompt, no_sandbox):
    """One agent call. `--settings` goes on every call: `--resume` doesn't restore it. The prompt goes
    after `--`, or a reply starting with `-` would read as an option."""
    argv = [claude, "-p"]
    if session_id:
        argv += ["--resume", session_id]
    argv += ["--model", model, "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"]
    if not no_sandbox:
        argv += ["--settings", SANDBOX_SETTINGS]
    return argv + ["--", prompt]


def call_agent(argv, fixture, env, timeout):
    """One agent call, its lines appended to transcript.jsonl as they come. (exit code, its events)."""
    transcript = fixture.run_dir / "transcript.jsonl"
    start = transcript.stat().st_size
    with open(transcript, "ab") as out, open(fixture.run_dir / "stderr.log", "ab") as err:
        code = wait_for(argv, fixture.repo, out, err, timeout, env)
    with open(transcript, "rb") as handle:
        handle.seek(start)
        return code, scoring.events_of(handle.read().decode("utf-8", "replace"))


def scripted_user_argv(args, model):
    """A scripted-user call, without its prompt. By default it runs on the user's login, isolated by these
    settings and `scripted_user_env`; `--scripted-user-bare` is for someone with an API key."""
    isolation = ["--bare"] if args.scripted_user_bare else ["--settings", USER_SETTINGS]
    return [args.claude, "-p", *isolation, "--model", model, "--output-format", "json"]


def scripted_user_env(bare):
    """The scripted user's environment. The API key reaches it only in bare mode, which needs one."""
    env = clean_env()
    if not bare:
        env.pop(API_KEY, None)
        env[USER_NO_CLAUDE_MDS] = "1"
    return env


def auth_of(env):
    return API_KEY if env.get(API_KEY) else "login"


def check_scripted_user(args, model):
    """One scripted-user call before any fixture: a login it can't use would otherwise cost every fixture
    a paid first round and then end it in `user_error`."""
    with tempfile.TemporaryDirectory(prefix="deliver-bench-user-") as cwd:
        kind, _, record = ask_scripted_user(args, model, USER_CHECK_PROMPT, cwd, USER_CHECK_TIMEOUT, None, 0)
    if kind == "unclear":
        said = record.get("error") or record.get("stderr") or record.get("stdout") or "nothing"
        hint = (
            f"--scripted-user-bare needs {API_KEY} set, because bare mode never reads a subscription login."
            if args.scripted_user_bare
            else f"It runs on your own login, without {API_KEY}: check that `{args.claude} -p` works in your terminal."
        )
        raise BenchError(f"the scripted user can't answer, so not running anything. It said:\n{said.strip()}\n{hint}")


def ask_scripted_user(args, model, prompt, cwd, timeout, log_path, number):
    """A scripted-user answer, kept out of the agent's transcript. (kind, reply, the last call's record). A
    call that fails or answers nothing usable is tried once more: one bad call shouldn't cost a whole run."""
    argv = scripted_user_argv(args, model) + ["--", prompt]
    kind, reply, record = "unclear", None, {}
    for attempt in range(1, USER_ATTEMPTS + 1):
        record = {"round": number, "attempt": attempt, "prompt": prompt, "cwd": cwd}
        try:
            done = subprocess.run(
                argv,
                cwd=cwd,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                stdin=subprocess.DEVNULL,
                env=scripted_user_env(args.scripted_user_bare),
                timeout=timeout,
            )
        except (OSError, subprocess.SubprocessError) as error:
            record["error"] = str(error)
            kind, reply = "unclear", None
        else:
            record.update(exit_code=done.returncode, stdout=done.stdout, stderr=done.stderr[-2000:])
            kind, reply = scoring.parse_user_answer(done.stdout)
        record.update(answer=kind, reply=reply)
        if log_path:
            with open(log_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")
        if kind != "unclear":
            break
    return kind, reply, record


def raw_call(number, code, result):
    """What one call reported, untouched: whether these add up is what first-probe settles."""
    result = result or {}
    return {
        "round": number,
        "exit_code": code,
        "session_id": result.get("session_id"),
        "subtype": result.get("subtype"),
        "is_error": result.get("is_error"),
        "num_turns": result.get("num_turns"),
        "total_cost_usd": result.get("total_cost_usd"),
        "permission_denials": result.get("permission_denials") or [],
    }


def declared_reasons(manifest, used):
    """What makes a run invalid from the start. None of it stops the run."""
    reasons = []
    if manifest.get("allow_nested_work_dir"):
        reasons.append("prepared with --allow-nested-work-dir")
    if manifest.get("scaffold_cmd"):
        reasons.append("prepared with --scaffold-cmd, not the real scaffold")
    if used:
        reasons.append("reused tree: " + "; ".join(used))
    return reasons


def move_old_results(run_dir):
    """Set a run's earlier results aside, because they may be hours of paid work. The folder, or None
    when there was nothing to keep."""
    old = [run_dir / name for name in RESULT_FILES if (run_dir / name).exists() and (run_dir / name).stat().st_size]
    if not old:
        return None
    number = 1
    while (run_dir / PREVIOUS / str(number)).exists():
        number += 1
    folder = run_dir / PREVIOUS / str(number)
    folder.mkdir(parents=True)
    for path in old:
        path.rename(folder / path.name)
    return folder


def run_fixture(fixture, task, spec, args, merge_block_allowed, used):
    """The loop: the agent, then the scripted user, until FINISHED, a failure or --max-rounds."""
    env = fixture.agent_env(api_key=args.agent_api_key)
    model = args.model or fixture.manifest["model"]
    fixture.manifest["ocgen_used_by_run"] = probe_ocgen(env)
    fixture.manifest["auth_used_by_run"] = {
        "agent": auth_of(env),
        "scripted_user": auth_of(scripted_user_env(args.scripted_user_bare)),
    }
    write_json(fixture.manifest_path, fixture.manifest)
    users_log = fixture.run_dir / "user.jsonl"
    moved = move_old_results(fixture.run_dir)
    if moved:
        print(f"{fixture.label}: its earlier results moved to {moved}", flush=True)
    for name in ("transcript.jsonl", "user.jsonl", "stderr.log"):
        (fixture.run_dir / name).write_bytes(b"")
    for name in ("run.json", "score.json"):
        (fixture.run_dir / name).unlink(missing_ok=True)
    run = {
        "model": model,
        "sandbox": not args.no_sandbox,
        "merge_block_allowed": merge_block_allowed,
        "claude_version": tool_output([args.claude, "--version"], env),
        "rounds": 0,
        "calls": [],
        "messages": [],
        "questions": [],
        "tool_questions": [],
        "assistant_models": [],
        "invalid": False,
        "invalid_reasons": declared_reasons(fixture.manifest, used),
        "subtype": None,
        "errors": [],
        "session_id": None,
    }
    session_id, prompt, last_message = None, f"/deliver {task['goal']}", None
    end = "max_rounds"
    with tempfile.TemporaryDirectory(prefix="deliver-bench-user-") as user_cwd:
        for number in range(1, args.max_rounds + 1):
            argv = agent_argv(args.claude, model, session_id, prompt, args.no_sandbox)
            code, events = call_agent(argv, fixture, env, args.call_timeout)
            run["rounds"] = number
            result = scoring.result_event(events)
            run["calls"].append(raw_call(number, code, result))
            run["messages"] += scoring.agent_messages(events)
            run["assistant_models"] = sorted(set(run["assistant_models"]) | set(scoring.assistant_models(events)))
            asked = scoring.tool_questions(events)
            pending = [{"round": number, "input": question, "answered": False} for question in asked]
            run["tool_questions"] += pending
            init = scoring.init_event(events)
            problems = scoring.init_problems(init, model) if number == 1 or init else []
            for problem in problems:
                if problem not in run["invalid_reasons"]:
                    run["invalid_reasons"].append(problem)
            if code is None:
                end = "timeout"
                run["errors"] = [f"the call passed --call-timeout {args.call_timeout:g}"]
                break
            if result is None:
                end = "error"
                run["errors"] = [f"no result event; the call exited with {code}"]
                break
            run["subtype"] = result.get("subtype")
            session_id = result.get("session_id") or session_id
            run["session_id"] = session_id
            # An error result carries `errors` and no `result`.
            if result.get("is_error") or "result" not in result:
                end = "error"
                run["errors"] = result.get("errors") or [str(result.get("result"))]
                break
            last_message = result["result"]
            if problems:
                # Nothing further can count, and every round costs real money.
                end = "invalid"
                break
            if not session_id:
                end = "error"
                run["errors"] = ["the result has no session_id to resume"]
                break
            user_prompt = render_user_prompt(task["goal"], spec, last_message, asked)
            kind, reply, _ = ask_scripted_user(
                args, model, user_prompt, user_cwd, args.call_timeout, users_log, number
            )
            if kind == "finished":
                end = "finished"
                break
            if kind != "reply":
                end = "user_error"
                break
            for question in pending:
                question["answered"] = True
            run["questions"].append(last_message)
            prompt = reply
    if end in scoring.FAILED_ENDS:
        run["invalid_reasons"].append(f"the run ended in {end}, so it measured nothing")
    run["invalid"] = bool(run["invalid_reasons"])
    run["end_reason"] = end
    run["final_report"] = None if end in ("error", "timeout") else last_message
    run["model_mismatch"] = any(name != model for name in run["assistant_models"])
    calls = run["calls"]
    run["total_cost_usd"] = sum(call["total_cost_usd"] or 0 for call in calls)
    run["num_turns"] = sum(call["num_turns"] or 0 for call in calls)
    run["permission_denials"] = [denial for call in calls for denial in call["permission_denials"]]
    write_json(fixture.run_dir / "run.json", run)
    return run


def recovery(fixture):
    """How to start a used fixture over: an interrupted run can't continue, because its tree holds part of
    an attempt."""
    return (
        f"To redo it fresh, delete {fixture.repo.parent} and {fixture.run_dir}, run `prepare` again, and "
        "approve that one fixture (`approvals` prints the command). --force runs it on the tree as it "
        "stands instead, marked invalid."
    )


def cmd_run(args):
    if args.model:
        check_model(args.model)
    fixtures = require_fixtures(args)
    problems, refused, locked, runnable = [], [], set(), []
    if not shutil.which(args.claude):
        problems.append(f"{args.claude} not found")
    for fixture in fixtures:
        finished = (fixture.run_dir / "run.json").exists()
        if finished and not args.rerun_finished:
            # So an interrupted bench continues with what's left, and no flag can lose a finished run.
            print(
                f"{fixture.label}: skipped, it already ran. --rerun-finished runs it again, marked as a "
                f"reused tree, and moves its results to {PREVIOUS}/."
            )
            continue
        used = fixture.used_reasons()
        if used and not finished and not args.force:
            refused.append(f"{fixture.label} can't count as a fresh attempt: {'; '.join(used)}. {recovery(fixture)}")
            continue
        runnable.append((fixture, used))
        if not fixture.approved():
            locked.add(fixture.label)
            if not args.allow_merge_block:
                problems.append(
                    f"{fixture.label} isn't approved, so the gate would stop its merges. Run this "
                    f"yourself, from your own terminal:\n  {approve_command(fixture.repo)}"
                )
    for line in refused:
        print(f"refused: {line}", flush=True)
    if problems:
        raise BenchError("not running anything:\n" + "\n".join(f"- {problem}" for problem in problems))
    for label in sorted(locked):
        print(f"{label} isn't approved: the gate will likely stop merges (--allow-merge-block).")
    if runnable:
        check_scripted_user(args, args.model or runnable[0][0].manifest["model"])
    failed = []
    for fixture, used in runnable:
        # An approval can expire during a long bench, and a run after that would be blocked at the merge.
        if not args.allow_merge_block and not fixture.approved():
            raise BenchError(
                f"the approval of {fixture.label} has expired, so stopping here. Run this yourself, from your "
                f"own terminal, then run again; finished fixtures are skipped:\n  {approve_command(fixture.repo)}"
            )
        task = read_json(args.tasks_dir / fixture.slug / "task.json")
        spec = (args.tasks_dir / fixture.slug / "spec.md").read_text(encoding="utf-8")
        print(f"{fixture.label}: running in {fixture.repo}", flush=True)
        run = run_fixture(fixture, task, spec, args, fixture.label in locked, used)
        marks = " (invalid: " + "; ".join(run["invalid_reasons"]) + ")" if run["invalid"] else ""
        print(f"{fixture.label}: {run['end_reason']} after {run['rounds']} rounds{marks}", flush=True)
        if run["end_reason"] in scoring.FAILED_ENDS:
            failed.append(f"{fixture.label} ({run['end_reason']})")
    if failed:
        print("error: these runs failed and measured nothing: " + ", ".join(failed), file=sys.stderr)
    for line in refused:
        print(f"error: not run: {line}", file=sys.stderr)
    return 1 if failed or refused else 0


# -------------------------------------------------------------------- score --


def changed_files(repo, base):
    """(tracked, untracked): what differs from `base` in the working tree. Commits don't hide a change."""
    tracked = git(repo, "diff", "--name-only", "-z", "--no-renames", base)
    untracked = git(repo, "ls-files", "--others", "--exclude-standard", "-z")
    return [p for p in tracked.split("\0") if p], [p for p in untracked.split("\0") if p]


def branch_fraction(repo, branch, hidden, scratch):
    """A branch's own hidden fraction, from an export of its tree: the fixture stays untouched."""
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", branch],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        env=clean_env(),
    )
    if archive.returncode != 0:
        raise BenchError(f"git archive {branch} failed in {repo}")
    copy = Path(tempfile.mkdtemp(dir=scratch))
    with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as tar:
        safe = {"filter": "data"} if hasattr(tarfile, "data_filter") else {}
        tar.extractall(copy, **safe)
    return scoring.fraction(run_hidden(copy, hidden))


def score_fixture(fixture, task, tasks_dir):
    repo = fixture.repo
    task_root = tasks_dir / fixture.slug
    hidden = task_root / "hidden"
    run = read_json(fixture.run_dir / "run.json")
    transcript_path = fixture.run_dir / "transcript.jsonl"
    transcript = transcript_path.read_text(encoding="utf-8", errors="replace") if transcript_path.exists() else ""
    branches = worktree_branches(repo)
    with tempfile.TemporaryDirectory(prefix="deliver-bench-score-") as folder:
        for_tests, for_traps = Path(folder) / "tests", Path(folder) / "traps"
        copy_tree(repo, for_tests)
        copy_tree(repo, for_traps)
        tests = run_hidden(for_tests, hidden)
        traps, trap_error = run_traps(hidden, for_traps, scoring.report_messages(run), transcript)
        fractions = {branch: branch_fraction(repo, branch, hidden, folder) for branch in branches}

    base = fixture.manifest["base_commit"]
    tracked, untracked = changed_files(repo, base)
    files = scoring.unplanned_files(tracked + untracked, task["allowed_paths"])
    hunks = 0
    for path in files:
        if path in untracked:
            hunks += 1
        else:
            diff = git(repo, "--literal-pathspecs", "diff", "--no-ext-diff", "--no-color", base, "--", path)
            hunks += max(1, scoring.count_hunks(diff))
    unmerged = [
        branch
        for branch in branches
        if git(repo, "merge-base", "--is-ancestor", branch, "HEAD", check=False) is None
    ]
    score = scoring.score_run(run, transcript, tests, traps, trap_error, files, hunks, fractions, unmerged, fixture.manifest)
    if task_hash(task_root) != fixture.manifest.get("task_hash"):
        score["invalid"] = True
        score["invalid_reasons"].append("the task folder changed after prepare")
    score["slug"], score["run"] = fixture.slug, fixture.number
    return score


def cmd_score(args):
    for fixture in require_fixtures(args):
        if not (fixture.run_dir / "run.json").exists():
            print(f"{fixture.label}: skipped, it hasn't run")
            continue
        task = read_json(args.tasks_dir / fixture.slug / "task.json")
        score = score_fixture(fixture, task, args.tasks_dir)
        write_json(fixture.run_dir / "score.json", score)
        print(f"{fixture.label}: hidden {score['hidden_fraction']:.2f}, end {score['end_reason']}")
    return 0


# --------------------------------------------------------- report, compare --


def load_scores(work, variant):
    """{slug: [its score.json files, in run order]}. The slug comes from each manifest."""

    def run_key(run_dir):
        number = run_dir.name[1:]
        return run_dir.parent.name, int(number) if number.isdigit() else 0

    found = {}
    for run_dir in sorted((work / RESULTS / variant).glob("t-*/r*"), key=run_key):
        path = run_dir / "score.json"
        if not path.exists():
            continue
        manifest = run_dir / "manifest.json"
        slug = read_json(manifest)["slug"] if manifest.exists() else run_dir.parent.name
        score = read_json(path)
        score.setdefault("run", run_key(run_dir)[1])
        found.setdefault(slug, []).append(score)
    return found


def scores_or_stop(work, variant):
    found = load_scores(work, variant)
    if not found:
        raise BenchError(f"no scores for variant {variant} in {work}; run `score` first")
    return found


def cmd_report(args):
    print(scoring.report_table(scores_or_stop(args.work_dir, args.variant)))
    return 0


def cmd_compare(args):
    before, after = scores_or_stop(args.work_dir, args.a), scores_or_stop(args.work_dir, args.b)
    print(scoring.compare_text(args.a, before, args.b, after))
    return 0


# ---------------------------------------------------------------------- cli --


def real_path(value):
    return Path(value).expanduser().resolve()


def build_parser():
    tasks = argparse.ArgumentParser(add_help=False)
    tasks.add_argument("--tasks-dir", type=real_path, default=DEFAULT_TASKS)
    work = argparse.ArgumentParser(add_help=False)
    work.add_argument("--work-dir", type=real_path, required=True, help="a folder outside any ocgen checkout")
    variant = argparse.ArgumentParser(add_help=False)
    variant.add_argument("--variant", default=DEFAULT_VARIANT)
    selection = argparse.ArgumentParser(add_help=False)
    selection.add_argument("--task", action="append", help="a task's slug; repeat for several (default: all)")
    claude = argparse.ArgumentParser(add_help=False)
    claude.add_argument("--claude", default="claude", help="the claude binary")
    fixture = argparse.ArgumentParser(add_help=False)
    fixture.add_argument("--model", default=DEFAULT_MODEL, help="a full model id, never an alias")
    fixture.add_argument("--ocgen-dir", type=real_path, default=REPO_ROOT, help="the checkout to measure")
    fixture.add_argument("--scaffold-cmd", help="tests only: replaces the build and the scaffold; marks runs invalid")
    fixture.add_argument(
        "--allow-nested-work-dir", action="store_true", help="tests only: results from it aren't valid"
    )
    logins = argparse.ArgumentParser(add_help=False)
    logins.add_argument(
        "--scripted-user-bare",
        action="store_true",
        help=f"use --bare for the scripted user; it needs {API_KEY} and bills that call to it",
    )
    logins.add_argument(
        "--agent-api-key",
        action="store_true",
        help=f"let {API_KEY} reach the agent's calls, which bills them to it instead of your login",
    )

    parser = argparse.ArgumentParser(
        description="Measures ocgen's /deliver skill on small golden tasks. A regression guard, not a precise instrument."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    add = sub.add_parser("validate", parents=[tasks, selection], help="check every task against its own claims")
    add.set_defaults(handler=cmd_validate)

    add = sub.add_parser(
        "prepare", parents=[tasks, work, variant, selection, claude, fixture], help="make the fixtures"
    )
    add.add_argument("--runs", type=int, default=1)
    add.set_defaults(handler=cmd_prepare)

    add = sub.add_parser("approvals", parents=[work], help="print the approve command for each fixture")
    add.add_argument("--variant", help="default: every variant")
    add.set_defaults(handler=cmd_approvals)

    add = sub.add_parser(
        "first-probe",
        parents=[tasks, work, selection, claude, fixture, logins],
        help="prepare one fixture in the probe variant and print the one-off commands for you to run",
    )
    add.add_argument("--no-sandbox", action="store_true", help="leave the sandbox settings out of the commands")
    add.set_defaults(handler=cmd_first_probe, variant=PROBE_VARIANT)

    add = sub.add_parser("inspect-init", help="print a transcript's init event and a verdict")
    add.add_argument("transcript")
    add.add_argument("--model", default=DEFAULT_MODEL)
    add.set_defaults(handler=cmd_inspect_init)

    add = sub.add_parser(
        "run", parents=[tasks, work, variant, selection, claude, logins], help="run /deliver on the fixtures"
    )
    add.add_argument("--model", help="default: the one the fixture was prepared with")
    add.add_argument("--max-rounds", type=int, default=DEFAULT_MAX_ROUNDS, help="agent calls per fixture, the first included")
    add.add_argument("--call-timeout", type=float, default=DEFAULT_CALL_TIMEOUT, help="seconds per call")
    add.add_argument("--no-sandbox", action="store_true", help="don't pass the sandbox settings")
    add.add_argument("--allow-merge-block", action="store_true", help="run without an approval; records that merges will likely stop")
    add.add_argument("--force", action="store_true", help="run a used fixture that has no results; marks it invalid")
    add.add_argument(
        "--rerun-finished",
        action="store_true",
        help="run a finished fixture again; moves its results to previous/ and marks the new run invalid",
    )
    add.set_defaults(handler=cmd_run)

    add = sub.add_parser("score", parents=[tasks, work, variant, selection], help="write score.json for each run")
    add.set_defaults(handler=cmd_score)

    add = sub.add_parser("report", parents=[work, variant], help="a table per task; never one overall number")
    add.set_defaults(handler=cmd_report)

    add = sub.add_parser("compare", parents=[work], help="per-task deltas and a sign test")
    add.add_argument("a")
    add.add_argument("b")
    add.set_defaults(handler=cmd_compare)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return args.handler(args)
    except (BenchError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
