//! A Claude Code worktree is a fresh checkout of committed files, and its
//! `.worktreeinclude` copies only git-ignored ones. These tests pin that a project
//! whose `.claude/` isn't in git still gets its config into every worktree: ocgen
//! keeps the config out of git in this clone (`.git/info/exclude`), so the copy
//! picks it up — and steps aside once `.claude/` is committed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ocgen::agent;
use ocgen::gitcheck::{self, worktree_gaps, WORKTREE_COPIES};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::verify::{verify, Options, Status};
use tempfile::tempdir;

fn claude_project(name: &str) -> Project {
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn ignored(dir: &Path, rel: &str) -> bool {
    git(dir, &["check-ignore", "-q", "--", rel])
        .status
        .success()
}

/// The repository's local exclude file, shared by all its worktrees.
fn exclude_file(dir: &Path) -> PathBuf {
    PathBuf::from(git_ok(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    ))
}

fn exclude(dir: &Path) -> String {
    fs::read_to_string(exclude_file(dir)).unwrap_or_default()
}

fn blocks(text: &str) -> usize {
    text.lines()
        .filter(|l| l.starts_with("# ocgen:exclude "))
        .count()
}

fn first_file(dir: &Path, sub: &str) -> String {
    let name = fs::read_dir(dir.join(sub))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| !n.starts_with('.'))
        .unwrap_or_else(|| panic!("nothing in {sub}"));
    format!("{sub}/{name}")
}

#[test]
fn uncommitted_claude_config_is_kept_out_of_git_so_worktrees_copy_it() {
    let dir = tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]);
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    claude_project("local").scaffold(dir.path(), false).unwrap();

    let text = exclude(dir.path());
    assert_eq!(blocks(&text), 1, "{text}");

    // What a worktree must find: ignored, so `.worktreeinclude` copies it.
    let hook = first_file(dir.path(), ".claude/hooks");
    for rel in [
        ".claude/settings.json",
        ".claude/rules/ocgen-workflow.md",
        hook.as_str(),
        ".claude/.ocgen-state.json",
        "CLAUDE.md",
    ] {
        assert!(ignored(dir.path(), rel), "{rel} not ignored:\n{text}");
    }
    // Agents and skills don't need copying (Claude Code reads them from the main
    // checkout), but they're config too: kept out of git with the rest.
    assert!(ignored(dir.path(), ".claude/agents/implementer.md"));
    assert!(ignored(dir.path(), ".claude/skills/fanout/SKILL.md"));
    assert!(ignored(
        dir.path(),
        ".claude/agent-memory/reviewer/MEMORY.md"
    ));
    assert!(ignored(dir.path(), ".claude/worktrees/w1/x"));
    // The copy list itself and the user's code stay visible.
    assert!(!ignored(dir.path(), ".worktreeinclude"));
    assert!(!ignored(dir.path(), "src/main.rs"));

    let status = git_ok(
        dir.path(),
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert!(
        !status
            .lines()
            .any(|l| l.contains(".claude/") || l.contains("CLAUDE.md")),
        "{status}"
    );

    // Nothing a worktree needs is left uncopyable.
    let gaps = worktree_gaps(dir.path()).unwrap();
    assert!(gaps.missing.is_empty(), "{:?}", gaps.missing);
    assert!(!gaps.committed);
    assert!(gitcheck::excluded_locally(dir.path()));
}

#[test]
fn rewriting_keeps_one_block_and_the_users_lines() {
    let dir = tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]);
    let file = exclude_file(dir.path());
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "# mine\n*.log\n").unwrap();

    let p = claude_project("again");
    p.scaffold(dir.path(), false).unwrap();
    let once = exclude(dir.path());
    p.scaffold(dir.path(), true).unwrap();
    let twice = exclude(dir.path());

    assert_eq!(once, twice, "a rewrite changes nothing");
    assert_eq!(blocks(&twice), 1);
    assert!(twice.starts_with("# mine\n*.log\n"), "{twice}");
    assert!(ignored(dir.path(), "x.log"));
}

#[test]
fn committing_claude_config_drops_the_block() {
    let dir = tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]);
    let p = claude_project("shared");
    p.scaffold(dir.path(), false).unwrap();
    assert_eq!(blocks(&exclude(dir.path())), 1);

    // The user chose to commit `.claude/`: anything under it tracked means so.
    git_ok(dir.path(), &["add", "-f", ".claude/settings.json"]);
    git_ok(dir.path(), &["commit", "-q", "-m", "config"]);
    p.scaffold(dir.path(), true).unwrap();

    let text = exclude(dir.path());
    assert_eq!(blocks(&text), 0, "{text}");
    assert!(!ignored(dir.path(), ".claude/rules/ocgen-workflow.md"));

    // The rest is now neither committed nor ignored: a worktree misses it.
    let gaps = worktree_gaps(dir.path()).unwrap();
    assert!(gaps.committed);
    assert!(gaps
        .missing
        .iter()
        .any(|f| f == ".claude/rules/ocgen-workflow.md"));
    assert!(!gaps.missing.iter().any(|f| f == ".claude/settings.json"));
    assert!(gitcheck::gaps_warning(&gaps).contains("Commit"));
}

#[test]
fn a_project_in_a_subfolder_gets_its_own_prefixed_block() {
    let top = tempdir().unwrap();
    git_ok(top.path(), &["init", "-q"]);
    let svc = top.path().join("svc/api");
    fs::create_dir_all(&svc).unwrap();
    claude_project("svc").scaffold(&svc, false).unwrap();

    let text = exclude(top.path());
    assert_eq!(blocks(&text), 1);
    assert!(text.contains("# ocgen:exclude svc/api/"), "{text}");
    assert!(ignored(top.path(), "svc/api/.claude/settings.json"));
    assert!(ignored(top.path(), "svc/api/CLAUDE.md"));
    // Only that project's files: another folder's config isn't swept up.
    assert!(!ignored(top.path(), "CLAUDE.md"));
    assert!(!ignored(top.path(), "web/.claude/settings.json"));

    // A second project in the same repository adds its own block.
    let web = top.path().join("web");
    fs::create_dir_all(&web).unwrap();
    claude_project("web").scaffold(&web, false).unwrap();
    let text = exclude(top.path());
    assert_eq!(blocks(&text), 2, "{text}");
    assert!(ignored(top.path(), "web/.claude/settings.json"));
    assert!(ignored(top.path(), "svc/api/.claude/settings.json"));
}

#[test]
fn linked_worktrees_share_the_exclusion() {
    let top = tempdir().unwrap();
    let main = top.path().join("main");
    fs::create_dir_all(&main).unwrap();
    git_ok(&main, &["init", "-q"]);
    fs::write(main.join("README"), "x\n").unwrap();
    git_ok(&main, &["add", "README"]);
    git_ok(&main, &["commit", "-q", "-m", "init"]);
    claude_project("shared-wt").scaffold(&main, false).unwrap();

    git_ok(&main, &["worktree", "add", "-q", "../wt"]);
    let wt = top.path().join("wt");
    // The worktree's own `.claude/` (a copy) is ignored there too.
    assert!(ignored(&wt, ".claude/settings.json"));
    assert!(ignored(&wt, "CLAUDE.md"));
}

#[test]
fn no_git_means_nothing_to_do() {
    let dir = tempdir().unwrap();
    claude_project("nogit").scaffold(dir.path(), false).unwrap();
    assert!(worktree_gaps(dir.path()).is_none());
    assert!(!gitcheck::excluded_locally(dir.path()));
}

#[test]
fn gaps_list_only_config_a_worktree_cannot_get() {
    let dir = tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]);
    fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
    fs::write(dir.path().join(".claude/settings.json"), "{}\n").unwrap();
    fs::write(dir.path().join(".claude/agents/a.md"), "x\n").unwrap();
    fs::write(dir.path().join("CLAUDE.md"), "x\n").unwrap();

    let gaps = worktree_gaps(dir.path()).unwrap();
    let mut missing = gaps.missing.clone();
    missing.sort();
    // Agents load from the main checkout, so they aren't a gap.
    assert_eq!(missing, [".claude/settings.json", "CLAUDE.md"]);
    assert!(!gaps.committed);
    let warning = gitcheck::gaps_warning(&gaps);
    assert!(warning.contains("ocgen doctor"), "{warning}");

    // Ignored by the user's own .gitignore: copied by `.worktreeinclude`.
    fs::write(dir.path().join(".gitignore"), ".claude/\nCLAUDE.md\n").unwrap();
    assert!(worktree_gaps(dir.path()).unwrap().missing.is_empty());
}

#[test]
fn every_copied_pattern_is_in_the_worktreeinclude() {
    let dir = tempdir().unwrap();
    let mut p = claude_project("wti");
    // Not tied to /fanout or /deliver: `claude --worktree` exists either way.
    p.claude.workflow.fanout = false;
    p.claude.workflow.deliver = false;
    p.scaffold(dir.path(), false).unwrap();
    let wti = fs::read_to_string(dir.path().join(".worktreeinclude")).unwrap();
    for want in WORKTREE_COPIES {
        assert!(
            wti.lines().any(|l| l.trim() == want),
            "missing {want}:\n{wti}"
        );
    }
}

/// verify in-process, with no user settings.
fn worktree_check(dir: &Path) -> (Status, String) {
    let user = tempdir().unwrap();
    let p = Project::load_state(dir).unwrap();
    let checks = verify(
        &p,
        dir,
        &Options {
            run_claude: false,
            run_check: false,
            user_settings: Some(user.path().join("settings.json")),
        },
    );
    let c = checks
        .iter()
        .find(|c| c.name.contains("worktree"))
        .unwrap_or_else(|| panic!("no worktree check: {checks:#?}"));
    (c.status, c.detail.clone())
}

#[test]
fn verify_warns_when_a_worktree_would_miss_config() {
    let dir = tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]);
    claude_project("check").scaffold(dir.path(), false).unwrap();
    let (status, detail) = worktree_check(dir.path());
    assert_eq!(status, Status::Pass, "{detail}");

    // Without ocgen's block the config is merely untracked: nothing copies it.
    fs::write(exclude_file(dir.path()), "").unwrap();
    let (status, detail) = worktree_check(dir.path());
    assert_eq!(status, Status::Warn);
    assert!(
        detail.contains("`.claude/hooks/") && detail.contains("ocgen doctor"),
        "{detail}"
    );
}
