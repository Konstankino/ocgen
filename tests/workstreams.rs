//! The `/workstreams` workflow skill: one manager session plans a goal into
//! workstreams, each worked in its own worktree, collects and checks every
//! branch and merges them in order — with the `workstreams-merge-gate` hook
//! holding any merge the board hasn't marked ready.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::claude::{Team, Workflow};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/workstreams/SKILL.md";
const RULE: &str = ".claude/rules/ocgen-workflow.md";
const HOOK: &str = ".claude/hooks/workstreams-merge-gate.sh";

/// Exactly the tools /workstreams may use without asking: reads from the main
/// checkout and the board folder. Worktree, merge, seal and branch commands are
/// left out on purpose — their prompt is the user's confirmation.
const WORKSTREAMS_TOOLS: [&str; 14] = [
    "Read",
    "Grep",
    "Glob",
    "Edit(./.claude/workstreams/**)",
    "Bash(git rev-parse:*)",
    "Bash(git worktree list:*)",
    "Bash(git for-each-ref:*)",
    "Bash(git log:*)",
    "Bash(git diff:*)",
    "Bash(git status:*)",
    "Bash(git rev-list:*)",
    "Bash(git merge-base:*)",
    "Bash(git show:*)",
    "Bash(git check-ignore:*)",
];

fn claude(name: &str) -> Project {
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn team() -> Team {
    Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 95,
        risk_rounds: true,
        approval_gate: true,
    }
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn scaffolded(p: &Project) -> (tempfile::TempDir, String) {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    (dir, md)
}

fn settings(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&read(dir, ".claude/settings.json")).unwrap()
}

/// The skill's frontmatter lines (between the `---` fences).
fn frontmatter(md: &str) -> Vec<String> {
    md.strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn tools(md: &str) -> Vec<String> {
    frontmatter(md)
        .iter()
        .find_map(|l| l.strip_prefix("allowed-tools: ").map(String::from))
        .expect("allowed-tools")
        .split(", ")
        .map(String::from)
        .collect()
}

#[test]
fn workstreams_is_a_user_run_skill_with_exact_tools() {
    let (_dir, md) = scaffolded(&claude("ws"));
    let front = frontmatter(&md);
    assert_eq!(front.first().map(String::as_str), Some("name: workstreams"));
    // description first in the template: legacy-command cleanup reads it.
    assert!(front[1].starts_with("description: "), "{front:?}");
    assert!(front.iter().any(|l| l == "disable-model-invocation: true"));
    let mut keys: Vec<&str> = front
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, _)| k)
        .collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate frontmatter keys: {front:?}");
    assert_eq!(tools(&md), WORKSTREAMS_TOOLS.map(String::from).to_vec());
    assert_eq!(
        ocgen::claude::WORKSTREAMS_TOOLS.to_vec(),
        WORKSTREAMS_TOOLS.to_vec(),
        "the shipped list is the pinned one"
    );
    // Nothing that merges, seals or removes is pre-approved.
    for t in tools(&md) {
        let t = t.replace("merge-base", "");
        for word in [
            "merge",
            "worktree add",
            "worktree remove",
            "commit",
            "branch -d",
            "reset",
        ] {
            assert!(!t.contains(word), "{t} pre-approves `{word}`");
        }
    }
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    assert!(md.lines().count() < 500);
    assert!(ocgen::claude::WORKFLOW_SKILLS.contains(&"workstreams"));
    assert!(ocgen::claude::USER_RUN_WORKFLOWS.contains(&"workstreams"));
    assert!(ocgen::templates::GATE_PROTOCOL.contains(&"claude/commands/workstreams.md.j2"));
}

#[test]
fn the_skill_teaches_the_board_and_the_report_block() {
    let (_dir, md) = scaffolded(&claude("ws"));
    for must in [
        // The resume step comes first: compaction keeps a skill's head.
        "## 0. Resume, or start",
        ".claude/workstreams/board.md",
        "--git-common-dir",
        "git check-ignore -q .claude/worktrees/x",
        "Status: APPROVED session=${CLAUDE_SESSION_ID}",
        "Base: refs/heads/<branch> @ <sha>",
        "After: <slug>",
        // The board line the hook parses, and its states.
        "— Branch: refs/heads/",
        "— Tip: ",
        "— State: ",
        "— Gate: ready",
        "Gate: pending",
        "Gate: merged",
        "Status: CLOSED",
        "never give a board a second `Status:` line",
        // The worker's report block, Confidence last.
        "Workstream: <slug>\nWorktree: ",
        "Branch: refs/heads/<branch>, or unknown",
        "Tip: <full SHA of your last commit>, or uncommitted",
        "Check: pass | fail: <what failed> | not run: <why>",
        "Changed: <the files you changed>",
        "Status: done | partial: <what is missing> | blocked: <why>\nConfidence: NN%",
        ".claude/scratch/workstream-report.md",
        // Collecting, sealing, checking, integrating.
        "git worktree list --porcelain",
        "git -C <worktree> add -A -- . ':!.claude'",
        "git -C <worktree> commit -m",
        "git diff --name-only <base>...refs/heads/<branch> -- .claude .mcp.json CLAUDE.md",
        "Undo: git reset --keep <sha of HEAD>",
        "git merge --no-ff --no-edit refs/heads/<branch>",
        "never `-m`",
        "git merge --abort",
        "workstreams-merge-gate",
        "`claude --worktree <slug> --name <slug>`, followed by the brief in a fenced block",
        "print `claude --worktree <slug> --resume` for me to run",
        "isolation: \"worktree\"",
        "git worktree remove <path>",
        "git branch -d <branch>",
        ".claude/loop-guard/escalations.md",
        // The page ocgen renders beside the board, never written by hand.
        "`board.html`",
        "never edit the `.html`",
        "ocgen notes open workstreams",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    // The manager never rewrites history or forces anything: each of these
    // appears only in a prohibition (on its line, or wrapped from the one before).
    let lines: Vec<&str> = md.lines().collect();
    for never in [
        "git reset --hard",
        "branch -D",
        "remove --force",
        "git push",
        "git stash",
    ] {
        for (i, l) in lines.iter().enumerate() {
            if l.contains(never) {
                let near = format!("{} {l}", if i > 0 { lines[i - 1] } else { "" });
                assert!(
                    near.to_lowercase().contains("never"),
                    "{never:?} outside a prohibition: {l}"
                );
            }
        }
    }
}

#[test]
fn the_project_roles_and_thresholds_are_rendered_in() {
    let mut p = claude("ws");
    p.claude.team = team();
    p.claude.workflow.subagent_confidence = 97;
    p.claude.workflow.check_cmd = "cargo test".into();
    let (_dir, md) = scaffolded(&p);
    for must in [
        "`implementer`",
        "`scope-guard`",
        "`adversary`",
        "at least 97%",
        "(`cargo test`)",
        // The approval gate holds every merge: integration waits for a human.
        "ocgen approve",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    let mut p = claude("ws");
    p.claude.team.enabled = false;
    p.claude.workflow.subagent_confidence = 0;
    p.claude.workflow.check_cmd = String::new();
    let (_dir, md) = scaffolded(&p);
    assert!(
        !md.contains("ocgen approve"),
        "no approval gate, no approval step"
    );
    assert!(
        md.contains("at least 95%"),
        "a default bar without the worker gate"
    );
    assert!(md.contains("(the brief's `Check:` command)"));
}

#[test]
fn one_workstream_offers_deliver_only_when_the_project_has_it() {
    let (_dir, md) = scaffolded(&claude("ws"));
    assert!(md.contains("**One workstream only?** Say so, and offer `/deliver` instead"));
    let deliver = read(_dir.path(), ".claude/skills/deliver/SKILL.md");
    assert!(deliver.contains("let `/workstreams` manage them"));
    let mut p = claude("ws");
    p.claude.workflow.deliver = false;
    let (_dir, md) = scaffolded(&p);
    assert!(!md.contains("/deliver"), "no /deliver to hand off to");
    assert!(md.contains("**One workstream only?** Say so. Name what the board would still give"));
}

#[test]
fn the_rule_names_the_board_and_the_gate() {
    let (dir, _md) = scaffolded(&claude("ws"));
    let rule = read(dir.path(), RULE);
    for must in [
        "## Workstreams",
        "`/workstreams <goal>`",
        ".claude/workstreams/board.md",
        "workstreams-merge-gate",
        "`Gate: ready`",
        "`/workstreams` is that coordinator",
    ] {
        assert!(rule.contains(must), "missing {must:?}");
    }
}

#[test]
fn the_gate_is_a_fail_closed_hook_behind_the_approval_gate() {
    let mut p = claude("ws");
    p.claude.team = team();
    let (dir, _md) = scaffolded(&p);
    let s = settings(dir.path());
    let pre = s["hooks"]["PreToolUse"].as_array().unwrap();
    assert!(
        pre[0].to_string().contains("team-approval-gate"),
        "the approval gate stays first"
    );
    let gate = &pre[1];
    assert!(
        gate.get("matcher").is_none(),
        "every tool with a command: {gate}"
    );
    let hook = &gate["hooks"][0];
    assert!(
        hook["command"]
            .as_str()
            .unwrap()
            .contains("workstreams-merge-gate"),
        "{hook}"
    );
    assert_eq!(hook["onFailure"], "block");
    assert_eq!(hook["shell"], "bash");
    assert!(dir.path().join(HOOK).is_file());
    assert!(dir.path().join(".claude/hooks/loop-guard.sh").is_file());
    // Without Agent Teams the merge gate is the first PreToolUse group.
    let mut p = claude("ws");
    p.claude.team.enabled = false;
    let (dir, _md) = scaffolded(&p);
    let s = settings(dir.path());
    let pre = s["hooks"]["PreToolUse"].as_array().unwrap();
    assert!(
        pre[0].to_string().contains("workstreams-merge-gate"),
        "{pre:?}"
    );
    assert!(pre[0].get("matcher").is_none());
}

#[test]
fn worktrees_branch_from_head_for_workstreams_without_fanout() {
    let mut p = claude("ws");
    p.claude.workflow.fanout = false;
    let (dir, _md) = scaffolded(&p);
    assert_eq!(settings(dir.path())["worktree"]["baseRef"], "head");
    let mut p = claude("ws");
    p.claude.workflow.fanout = false;
    p.claude.workflow.workstreams = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(settings(dir.path()).get("worktree").is_none());
}

#[test]
fn turning_workstreams_off_removes_the_skill_the_rule_and_the_hook() {
    let mut p = claude("ws");
    p.claude.team = team();
    let (dir, _md) = scaffolded(&p);
    p.claude.workflow.workstreams = false;
    p.scaffold(dir.path(), true).unwrap();
    assert!(!dir.path().join(SKILL).exists(), "skill left behind");
    assert!(!dir.path().join(HOOK).exists(), "hook script left behind");
    let rule = read(dir.path(), RULE);
    assert!(
        !rule.contains("## Workstreams") && !rule.contains("/workstreams"),
        "{rule}"
    );
    let s = settings(dir.path());
    assert!(!s["hooks"].to_string().contains("workstreams-merge-gate"));
    assert!(!read(dir.path(), ".claude/skills/deliver/SKILL.md").contains("/workstreams"));
}

#[test]
fn old_state_files_get_workstreams() {
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert!(wf.workstreams);
}

#[test]
fn the_skill_matches_the_gate_it_describes() {
    // What the skill tells the manager to run is what the gate lets through, and
    // what it warns against is what the gate refuses.
    let (dir, md) = scaffolded(&claude("ws"));
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {o:?}");
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    git(&["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&["branch", "worktree-a"]);
    let tip = git(&["rev-parse", "refs/heads/worktree-a"]);
    fs::create_dir_all(dir.path().join(".claude/workstreams")).unwrap();
    fs::write(
        dir.path().join(".claude/workstreams/board.md"),
        format!(
            "# Workstreams — t\nStatus: APPROVED session=s\n\n## Workstreams\n\
             - Workstream: a — Worker: session a — Branch: refs/heads/worktree-a — Worktree: — — Tip: {tip} — State: checked — Gate: ready — After: none\n"
        ),
    )
    .unwrap();
    let env: std::collections::HashMap<String, String> = [(
        "CLAUDE_PROJECT_DIR".to_string(),
        ocgen::paths::for_shell(dir.path()),
    )]
    .into();
    let run = |c: &str| {
        ocgen::hooks::run(
            "workstreams-merge-gate",
            &serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": c } }).to_string(),
            &env,
        )
        .code
    };
    // The skill's merge, with the placeholder filled in.
    let merge = md
        .lines()
        .find_map(|l| {
            l.split('`')
                .find(|s| s.starts_with("git merge --no-ff --no-edit refs/heads/<branch>"))
        })
        .expect("the skill shows the merge command")
        .replace("<branch>", "worktree-a");
    assert_eq!(run(&merge), 0, "{merge}");
    assert_eq!(run("git merge --abort"), 0);
    assert_eq!(
        run("git merge --no-ff refs/heads/worktree-a -m \"a: done\""),
        2,
        "-m reads as a branch"
    );
    assert_eq!(run("git merge worktree-a"), 2, "a short name");
}
