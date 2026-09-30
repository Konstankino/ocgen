//! The hardened execution-approval line: what counts as high-impact, where an
//! approval lives, and that it expires.

use std::fs;

use ocgen::approval;
use ocgen::risk::high_impact;

#[test]
fn detection_survives_flags_quotes_and_indirection() {
    for cmd in [
        "git push origin main",
        "git -C . push origin main",
        "git -c user.name=x push",
        "git --git-dir=.git push",
        "/usr/bin/git push",
        "git \"push\" origin",
        "git 'push'",
        "cd repo && git push",
        "sh -c 'git push'",
        "terraform apply -auto-approve",
        "terraform -chdir=infra apply",
        "kubectl -n prod delete pod x",
        "helm --kube-context prod upgrade app ./chart",
        "gh pr merge 42 --merge",
        "aws s3 rm s3://b/x --recursive",
        "ssh host uptime",
        "npm publish",
        "cargo publish",
        "sh ./deploy.sh",
        "./scripts/publish.sh --prod",
        "make deploy",
        "make -C infra deploy",
        "npm run deploy",
        "curl -X POST https://api.example.com/x",
    ] {
        assert!(high_impact(cmd).is_some(), "should be high-impact: {cmd}");
    }
    for cmd in [
        "git status",
        "git log --grep='push'",
        "git diff",
        "cargo test",
        "terraform plan",
        "terraform -chdir=infra validate",
        "kubectl get pods",
        "helm template ./chart",
        "grep -r deploy docs/",
        "cat scripts/deploy.sh",
        "npm test",
        "make test",
        "echo push",
    ] {
        assert!(
            high_impact(cmd).is_none(),
            "should NOT be high-impact: {cmd}"
        );
    }
}

#[test]
fn approval_is_per_project_outside_it_and_expires() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    assert!(
        approval::remaining(home.path(), proj.path()).is_none(),
        "locked by default"
    );

    let marker = approval::grant(home.path(), proj.path(), 30).unwrap();
    assert!(marker.starts_with(home.path().join(".claude/ocgen/approvals")));
    assert!(!marker.starts_with(proj.path()), "never inside the project");
    let left = approval::remaining(home.path(), proj.path()).unwrap();
    assert!((29 * 60..=30 * 60).contains(&left), "{left}");
    assert!(
        approval::remaining(home.path(), other.path()).is_none(),
        "per project"
    );

    // An expired approval is locked again, with no cleanup needed.
    fs::write(&marker, "1\n").unwrap();
    assert!(approval::remaining(home.path(), proj.path()).is_none());

    approval::grant(home.path(), proj.path(), 5).unwrap();
    assert!(approval::revoke(home.path(), proj.path()).unwrap());
    assert!(approval::remaining(home.path(), proj.path()).is_none());
    assert!(!approval::revoke(home.path(), proj.path()).unwrap());
}

#[test]
fn a_worktree_shares_its_repositorys_approval() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str], dir: &std::path::Path| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
    };
    git(&["init", "-q"], repo.path());
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "i",
        ],
        repo.path(),
    );
    let wt = repo.path().join(".claude/worktrees/w");
    git(
        &["worktree", "add", "-q", wt.to_str().unwrap()],
        repo.path(),
    );
    assert_eq!(
        approval::project_key(repo.path()),
        approval::project_key(&wt)
    );
    approval::grant(home.path(), repo.path(), 10).unwrap();
    assert!(approval::remaining(home.path(), &wt).is_some());
}

// ------------------------------------------------------------ pre-push hook --

use ocgen::agent;
use ocgen::claude::Team;
use ocgen::manifest::Manifest;
use ocgen::render::{PrePush, Project};
use ocgen::target::Target;
use std::path::Path;
use std::process::Command;

fn gated_project() -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "pp".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: false,
        confidence_threshold: 0,
        risk_rounds: false,
        approval_gate: true,
    };
    p
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn run_pre_push(repo: &Path, home: &Path, under_claude: bool) -> i32 {
    let mut cmd = Command::new("sh");
    cmd.arg(repo.join(".git/hooks/pre-push"))
        .current_dir(repo)
        .env("HOME", home)
        .env_remove("CLAUDECODE");
    if under_claude {
        cmd.env("CLAUDECODE", "1");
    }
    cmd.output().unwrap().status.code().unwrap()
}

#[test]
fn pre_push_hook_blocks_agent_pushes_but_never_a_human() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    let p = gated_project();
    p.scaffold(repo.path(), false).unwrap();
    let hook = repo.path().join(".git/hooks/pre-push");
    assert!(fs::read_to_string(&hook)
        .unwrap()
        .contains("ocgen:pre-push"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            fs::metadata(&hook).unwrap().permissions().mode() & 0o111 != 0,
            "executable"
        );
    }

    assert_eq!(
        run_pre_push(repo.path(), home.path(), false),
        0,
        "a human's push is never touched"
    );
    assert_eq!(
        run_pre_push(repo.path(), home.path(), true),
        1,
        "an agent's push needs approval"
    );
    approval::grant(home.path(), repo.path(), 10).unwrap();
    assert_eq!(run_pre_push(repo.path(), home.path(), true), 0, "approved");
    approval::revoke(home.path(), repo.path()).unwrap();
    assert_eq!(run_pre_push(repo.path(), home.path(), true), 1, "re-locked");
}

#[test]
fn pre_push_install_respects_existing_hooks_and_non_git_dirs() {
    // Someone else's hook is never overwritten.
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    fs::write(
        repo.path().join(".git/hooks/pre-push"),
        "#!/bin/sh\necho mine\n",
    )
    .unwrap();
    let p = gated_project();
    p.scaffold(repo.path(), false).unwrap();
    assert!(matches!(
        p.install_pre_push(repo.path()).unwrap(),
        PrePush::Foreign(_)
    ));
    assert_eq!(
        fs::read_to_string(repo.path().join(".git/hooks/pre-push")).unwrap(),
        "#!/bin/sh\necho mine\n"
    );
    // The gate script is still shipped so the owner can chain it.
    assert!(repo.path().join(".claude/hooks/git-pre-push.sh").exists());

    // Not a git repository, or the gate is off: nothing to install.
    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(
        p.install_pre_push(plain.path()).unwrap(),
        PrePush::NotApplicable
    ));
    let mut off = gated_project();
    off.claude.team.approval_gate = false;
    let repo2 = tempfile::tempdir().unwrap();
    git(repo2.path(), &["init", "-q"]);
    off.scaffold(repo2.path(), false).unwrap();
    assert!(!repo2.path().join(".git/hooks/pre-push").exists());
}
