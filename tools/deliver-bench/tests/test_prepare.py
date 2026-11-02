"""`prepare`: a fixture is its own repository before anything scaffolds it, and is measured honestly."""

import hashlib
import json
import os
import shlex
import shutil
import sys
import unittest

from bench_helpers import (
    FAKE_SCAFFOLD,
    PINNED_MODEL,
    TINY,
    TOOL_DIR,
    VARIANT,
    Sandbox,
    combined,
    git,
    install,
    make_task,
    task_folder,
)
from fake_ocgen import PROTOCOL

SCAFFOLDED_NAMES = ("CLAUDE.md", ".mcp.json", ".gitattributes", ".worktreeinclude")


class Case(unittest.TestCase):
    def setUp(self):
        self.sb = Sandbox(self)

    def marker(self, repo=None):
        repo = repo or self.sb.repo()
        return json.loads((repo / ".claude" / "scaffold-marker.json").read_text(encoding="utf-8"))

    def tracked(self, commit):
        return git(self.sb.repo(), "ls-tree", "-r", "--name-only", commit).splitlines()


class FixtureLayout(Case):
    def test_the_fixture_is_a_copy_of_repo_with_nothing_hidden(self):
        self.sb.prepare()
        repo = self.sb.work / "fixtures" / VARIANT / task_folder(TINY) / "r1" / "repo"
        self.assertTrue((repo / "calc.py").is_file())
        self.assertTrue((repo / "tests" / "test_calc.py").is_file())
        for name in ("spec.md", "hidden", "solution", "task.json"):
            self.assertFalse((repo / name).exists(), name)

    def test_the_manifest_is_in_the_results_tree_and_nothing_sits_beside_the_fixture(self):
        self.sb.prepare()
        results = self.sb.work / "results" / VARIANT / task_folder(TINY) / "r1"
        self.assertTrue((results / "manifest.json").is_file())
        repo = self.sb.repo()
        self.assertEqual(list(repo.parent.iterdir()), [repo])
        fixtures = self.sb.work / "fixtures"
        around = [p for p in fixtures.rglob("*") if p.is_file() and repo not in p.parents]
        self.assertEqual(around, [])

    def test_nothing_the_agent_sees_names_the_task(self):
        slug = "sneaky-slug"
        make_task(self.sb.tasks, slug)
        self.sb.prepare(task=slug)
        repo = self.sb.repo(task=slug)
        self.assertNotIn(slug, str(repo))
        names = [str(path.relative_to(repo)) for path in repo.rglob("*")]
        self.assertEqual([name for name in names if slug in name], [])
        self.assertNotIn(slug, git(repo, "log", "--format=%s%n%b%n%an%n%ae"))
        self.assertNotIn(slug, (repo / ".git" / "config").read_text(encoding="utf-8"))
        self.assertEqual(self.sb.manifest(task=slug)["slug"], slug)

    def test_each_run_is_its_own_repository(self):
        self.sb.prepare(runs=2)
        for run in (1, 2):
            repo = self.sb.repo(run=run)
            self.assertEqual(git(repo, "rev-parse", "--show-toplevel"), str(repo))

    def test_an_unknown_task_is_refused(self):
        done = self.sb.prepare(task="no-such-task", check=False)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("no-such-task", combined(done))
        self.assertNotIn("Traceback", combined(done))

    def test_preparing_twice_keeps_what_is_there(self):
        # Approval is keyed per path, and a prepared folder may hold a finished run.
        self.sb.prepare()
        repo = self.sb.repo()
        head = git(repo, "rev-parse", "HEAD")
        (repo / "work-in-progress.txt").write_text("keep me\n", encoding="utf-8")
        done = self.sb.prepare(check=False)
        self.assertNotIn("Traceback", combined(done))
        self.assertEqual(git(repo, "rev-parse", "HEAD"), head)
        self.assertTrue((repo / "work-in-progress.txt").is_file())

    def assert_stale(self, done, word):
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(word, out)

    def test_a_kept_fixture_from_another_commit_stops_prepare(self):
        self.sb.prepare()
        (self.sb.checkout / "Cargo.toml").write_text('[package]\nname = "changed"\n', encoding="utf-8")
        git(self.sb.checkout, "commit", "-q", "-am", "a later commit")
        self.assert_stale(self.sb.prepare(check=False), "ocgen_commit")

    def test_a_kept_fixture_with_another_model_stops_prepare(self):
        self.sb.prepare()
        self.assert_stale(self.sb.prepare("--model", "claude-other-2", check=False), "model")

    def test_a_kept_fixture_from_another_checkout_stops_prepare(self):
        self.sb.prepare()
        other = self.sb.root / "other-checkout"
        shutil.copytree(self.sb.checkout, other)
        done = self.sb.bench(
            "prepare",
            "--work-dir", self.sb.work,
            "--tasks-dir", self.sb.tasks,
            "--variant", VARIANT,
            "--ocgen-dir", other,
            "--task", TINY,
            "--scaffold-cmd", self.sb.scaffold_cmd,
            *(["--allow-nested-work-dir"] if self.sb.nested else []),
        )
        self.assert_stale(done, "ocgen_dir")

    def test_a_scaffold_that_fails_stops_and_its_folder_stays_refused(self):
        failing = shlex.join([sys.executable, "-c", "import sys; sys.exit(3)"])
        done = self.sb.prepare("--scaffold-cmd", failing, fake_scaffold=False, check=False)
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(str(self.sb.repo()), out)
        manifest = self.sb.run_dir() / "manifest.json"
        self.assertFalse(manifest.exists())
        again = self.sb.prepare(check=False)
        self.assertNotEqual(again.returncode, 0, combined(again))
        self.assertFalse(manifest.exists())


class CommitsAroundTheScaffold(Case):
    def test_the_fixture_is_a_clean_repository_when_scaffolding_starts(self):
        self.sb.prepare()
        marker = self.marker()
        self.assertEqual(marker["head"], self.sb.manifest()["fixture_commit"])
        self.assertEqual(marker["toplevel"], str(self.sb.repo()))
        self.assertEqual(marker["status"], "")

    def test_the_first_commit_holds_only_the_fixture(self):
        self.sb.prepare()
        tracked = self.tracked(self.sb.manifest()["fixture_commit"])
        self.assertEqual(sorted(tracked), ["calc.py", "notes.txt", "tests/test_calc.py"])

    def test_the_second_commit_holds_what_the_scaffold_left_and_the_tree_is_clean(self):
        self.sb.prepare()
        repo = self.sb.repo()
        manifest = self.sb.manifest()
        self.assertEqual(git(repo, "rev-parse", "HEAD"), manifest["base_commit"])
        self.assertEqual(git(repo, "rev-parse", "HEAD^"), manifest["fixture_commit"])
        self.assertEqual(git(repo, "status", "--porcelain"), "")
        tracked = self.tracked(manifest["base_commit"])
        for name in SCAFFOLDED_NAMES:
            self.assertIn(name, tracked)

    def test_claude_is_in_no_commit(self):
        self.sb.prepare()
        manifest = self.sb.manifest()
        self.assertTrue((self.sb.repo() / ".claude" / "settings.json").is_file())
        for commit in (manifest["fixture_commit"], manifest["base_commit"]):
            self.assertEqual([n for n in self.tracked(commit) if n.startswith(".claude")], [])

    def test_backups_are_excluded(self):
        self.sb.prepare()
        exclude = self.sb.repo() / ".git" / "info" / "exclude"
        self.assertIn(".ocgen-backup/", exclude.read_text(encoding="utf-8").splitlines())

    def test_git_settings_are_the_fixtures_own(self):
        self.sb.prepare()
        repo = self.sb.repo()
        self.assertEqual(git(repo, "config", "--local", "core.autocrlf"), "false")
        self.assertTrue(git(repo, "config", "--local", "user.name"))
        self.assertTrue(git(repo, "config", "--local", "user.email"))

    def test_the_default_scaffold_goes_through_the_demo_example(self):
        install(self.sb.bin / "cargo", FAKE_SCAFFOLD)
        self.sb.prepare(fake_scaffold=False)
        self.assertEqual(
            self.marker()["argv"],
            [
                "run", "--quiet", "--example", "demo",
                "--manifest-path", str(self.sb.checkout / "Cargo.toml"),
                "--", str(self.sb.repo()), "claude-team", "English",
            ],
        )


def sha256_of(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class TheCheckoutsOwnOcgen(Case):
    def setUp(self):
        super().setUp()
        install(self.sb.bin / "cargo", FAKE_SCAFFOLD)
        self.built = self.sb.checkout / "target" / "debug" / "ocgen"

    def copy_for(self, variant=VARIANT):
        return self.sb.work / "bin" / variant / "ocgen"

    def test_a_copy_per_variant_is_built_and_recorded(self):
        self.sb.prepare(fake_scaffold=False)
        manifest = self.sb.manifest()
        self.assertEqual(manifest["ocgen_path"], str(self.copy_for()))
        self.assertEqual(manifest["ocgen_sha256"], sha256_of(self.copy_for()))
        self.assertIn("built", manifest["ocgen_version"])
        self.assertEqual(manifest["ocgen_protocol"], PROTOCOL)

    def test_a_later_build_leaves_an_earlier_variants_copy_alone(self):
        self.sb.prepare(fake_scaffold=False, variant="first", BENCH_FAKE_BUILD_MARK="one")
        self.sb.prepare(fake_scaffold=False, variant="second", BENCH_FAKE_BUILD_MARK="two")
        first = self.sb.manifest(variant="first")
        second = self.sb.manifest(variant="second")
        self.assertNotEqual(first["ocgen_sha256"], second["ocgen_sha256"])
        self.assertEqual(sha256_of(self.copy_for("first")), first["ocgen_sha256"])
        self.assertEqual(sha256_of(self.copy_for("second")), second["ocgen_sha256"])

    def test_a_changed_build_under_a_prepared_variant_stops_prepare(self):
        self.sb.prepare(fake_scaffold=False, BENCH_FAKE_BUILD_MARK="one")
        done = self.sb.prepare(fake_scaffold=False, runs=2, check=False, BENCH_FAKE_BUILD_MARK="two")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertIn("sha256", combined(done))
        self.assertFalse(self.sb.repo(run=2).exists())

    def test_the_build_lands_in_the_checkout_whatever_the_shell_says(self):
        # A target folder shared between checkouts would give two variants one binary.
        elsewhere = self.sb.root / "elsewhere"
        self.sb.prepare(fake_scaffold=False, CARGO_TARGET_DIR=str(elsewhere))
        self.assertTrue(self.built.is_file())
        self.assertFalse(elsewhere.exists())

    def test_a_build_that_fails_stops_before_any_fixture(self):
        done = self.sb.prepare(fake_scaffold=False, check=False, BENCH_FAKE_CARGO_FAIL="1")
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertFalse(self.sb.repo().exists())

    def test_a_missing_cargo_says_so(self):
        path = self.sb.path_with_only_git()
        done = self.sb.prepare(fake_scaffold=False, check=False, PATH=path)
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertNotIn("Traceback", combined(done))
        self.assertIn("cargo", combined(done))

    def test_a_scaffold_override_builds_nothing_and_records_the_ocgen_on_path(self):
        self.sb.prepare()
        manifest = self.sb.manifest()
        self.assertFalse(self.built.exists())
        self.assertEqual(manifest["ocgen_path"], str(self.sb.bin / "ocgen"))
        self.assertNotIn("built", manifest["ocgen_version"])


class NestedWorkDir(Case):
    def assert_refused(self, done, work):
        out = combined(done)
        self.assertNotEqual(done.returncode, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(str(work), out)

    def test_refused_inside_the_given_checkout(self):
        work = self.sb.checkout / "runs"
        done = self.sb.prepare(work=work, allow_nested=False, check=False)
        self.assert_refused(done, work)
        self.assertFalse(work.exists())

    def test_refused_through_a_symlink_into_the_checkout(self):
        link = self.sb.root / "link"
        os.symlink(self.sb.checkout, link)
        done = self.sb.prepare(work=link / "runs", allow_nested=False, check=False)
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertIn("runs", combined(done))
        self.assertFalse((self.sb.checkout / "runs").exists())

    def test_refused_inside_the_checkout_the_tool_lives_in(self):
        # `--ocgen-dir` may name another checkout: a session here would still inherit this one's
        # CLAUDE.md.
        work = TOOL_DIR / "runs" / "refusal-probe"
        self.addCleanup(shutil.rmtree, work, ignore_errors=True)
        done = self.sb.prepare(work=work, allow_nested=False, check=False)
        self.assert_refused(done, work)
        self.assertFalse(work.exists())

    def test_the_override_works_and_says_the_results_are_not_valid(self):
        work = self.sb.checkout / "runs"
        done = self.sb.prepare(work=work, allow_nested=True)
        self.assertRegex(combined(done), r"(?i)not valid|aren't valid|invalid")
        repo = work / "fixtures" / VARIANT / task_folder(TINY) / "r1" / "repo"
        self.assertTrue((repo / "calc.py").is_file())


class Manifest(Case):
    def test_it_records_what_is_being_measured(self):
        self.sb.prepare()
        manifest = self.sb.manifest()
        self.assertEqual(manifest["slug"], TINY)
        self.assertEqual(manifest["scaffold_cmd"], self.sb.scaffold_cmd)
        self.assertIs(manifest["allow_nested_work_dir"], self.sb.nested)
        self.assertRegex(manifest["task_hash"], r"^[0-9a-f]{64}$")
        self.assertEqual(manifest["base_fraction"], 0.25)
        self.assertEqual(
            sorted(name.rsplit(".", 1)[-1] for name in manifest["failing_on_base"]),
            ["test_mul", "test_neg", "test_sub"],
        )
        self.assertEqual(
            [name.rsplit(".", 1)[-1] for name in manifest["passing_on_base"]], ["test_add"]
        )
        self.assertEqual(manifest["ocgen_commit"], git(self.sb.checkout, "rev-parse", "HEAD"))
        self.assertEqual(manifest["base_commit"], git(self.sb.repo(), "rev-parse", "HEAD"))
        self.assertEqual(manifest["model"], PINNED_MODEL)
        self.assertIn("fake", manifest["ocgen_version"])
        self.assertEqual(manifest["ocgen_protocol"], PROTOCOL)
        self.assertIn("stub", manifest["claude_version"])
        self.assertEqual(manifest["template_overrides"], [])
        self.assertIs(manifest["global_claude_md"], False)
        self.assertIsInstance(manifest["inherited_claude_md"], list)

    def test_a_given_model_is_recorded(self):
        self.sb.prepare("--model", "claude-other-2")
        self.assertEqual(self.sb.manifest()["model"], "claude-other-2")

    def test_a_model_alias_is_refused(self):
        done = self.sb.prepare("--model", "opus", check=False)
        self.assertNotEqual(done.returncode, 0, combined(done))
        self.assertIn("opus", combined(done))
        self.assertFalse(self.sb.repo().exists())

    def test_inherited_claude_md_files_are_listed(self):
        inherited = self.sb.root / "CLAUDE.md"
        inherited.write_text("rules from a parent folder\n", encoding="utf-8")
        self.sb.prepare()
        listed = self.sb.manifest()["inherited_claude_md"]
        self.assertIn(str(inherited), listed)
        self.assertNotIn(str(self.sb.repo() / "CLAUDE.md"), listed)

    def test_a_global_claude_md_is_noted(self):
        folder = self.sb.home / ".claude"
        folder.mkdir()
        (folder / "CLAUDE.md").write_text("personal rules\n", encoding="utf-8")
        self.sb.prepare()
        self.assertIs(self.sb.manifest()["global_claude_md"], True)

    def test_a_template_override_warns_loudly_and_is_listed(self):
        override = self.sb.home / ".config" / "ocgen" / "templates" / "claude" / "rule.md"
        override.parent.mkdir(parents=True)
        override.write_text("changes what is measured\n", encoding="utf-8")
        done = self.sb.prepare()
        self.assertIn("WARNING", combined(done))
        self.assertIn(str(override), combined(done))
        self.assertEqual(self.sb.manifest()["template_overrides"], [str(override)])

    def test_no_override_means_no_override_warning(self):
        done = self.sb.prepare()
        self.assertNotRegex(combined(done), r"(?i)warning.*template")

    def test_missing_binaries_are_recorded_not_a_crash(self):
        self.sb.prepare(PATH=self.sb.path_with_only_git())
        manifest = self.sb.manifest()
        self.assertIsNone(manifest["ocgen_path"])
        self.assertIsNone(manifest["ocgen_version"])
        self.assertIsNone(manifest["ocgen_protocol"])
        self.assertIsNone(manifest["claude_version"])


if __name__ == "__main__":
    unittest.main()
