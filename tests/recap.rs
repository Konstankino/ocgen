//! The `/recap` workflow skill: a daily, per-branch recap that syncs branches
//! fast-forward only and reports what changed since the last recap.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::claude::{Output, Skill, Workflow};
use ocgen::manifest::Manifest;
use ocgen::notes::{ledger_target, Target as NoteTarget};
use ocgen::render::Project;
use ocgen::risk;
use ocgen::target::Target;
use ocgen::validate;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/recap/SKILL.md";

/// Exactly the tools /recap may use without asking. The local fast-forward batch
/// (`git fetch . …`) and new tracking branches are left out on purpose: their
/// permission prompt is the user's confirmation.
const RECAP_TOOLS: [&str; 16] = [
    "Read",
    "Grep",
    "Glob",
    "Edit(./.claude/notes/recap/**)",
    "Bash(git rev-parse:*)",
    "Bash(git for-each-ref:*)",
    "Bash(git rev-list:*)",
    "Bash(git log:*)",
    "Bash(git diff:*)",
    "Bash(git show:*)",
    "Bash(git range-diff:*)",
    "Bash(git status:*)",
    "Bash(git remote)",
    "Bash(git config --get-all remote.origin.fetch)",
    "Bash(git fetch --prune --no-tags --no-recurse-submodules origin)",
    "Bash(git merge --ff-only --no-overwrite-ignore @{u})",
];

fn claude(name: &str) -> Project {
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
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
fn recap_is_a_user_run_skill_with_exact_tools() {
    let (_dir, md) = scaffolded(&claude("rc"));
    let front = frontmatter(&md);
    assert_eq!(front.first().map(String::as_str), Some("name: recap"));
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

    assert_eq!(tools(&md), RECAP_TOOLS.map(String::from).to_vec());
    assert_eq!(
        ocgen::claude::RECAP_TOOLS.to_vec(),
        RECAP_TOOLS.to_vec(),
        "the shipped list is the pinned one"
    );
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
}

#[test]
fn recap_tools_cannot_force_or_rewrite() {
    for t in ocgen::claude::RECAP_TOOLS {
        validate::permission_rule(t).unwrap_or_else(|e| panic!("{t}: {e}"));
        // Fetch and merge are pinned to one exact command line each.
        if t.starts_with("Bash(git fetch") || t.starts_with("Bash(git merge") {
            assert!(!t.contains('*'), "not exact: {t}");
        }
        for bad in [
            "+",
            "--force",
            " -f",
            "pull",
            "push",
            "branch",
            "remote -v",
            "reset",
            "update-ref",
            "checkout",
            "switch",
            "rebase",
            "stash",
            "worktree",
            "--upload-pack",
        ] {
            assert!(!t.contains(bad), "{t} allows {bad:?}");
        }
        assert!(t != "Bash" && t != "Write" && t != "Edit", "unscoped {t}");
        // GitHub is the recap-github hook's job: the sandbox withholds the gh
        // login from Claude's shell.
        assert!(!t.contains("gh "), "{t}");
    }
}

#[test]
fn recap_commands_vs_the_approval_gate() {
    // What /recap runs passes the gate's pattern — even for branches named after
    // the gated words — and only the current branch's merge is caught.
    for passes in [
        "git fetch --prune --no-tags --no-recurse-submodules origin",
        "git fetch . refs/remotes/origin/merge:refs/heads/merge refs/remotes/origin/push:refs/heads/push",
        "git for-each-ref --format='%(refname)%09%(objectname)' refs/heads refs/remotes/origin",
        "git rev-list --left-right --count abc...def",
        "git range-diff abc...def",
        "git log --no-merges refs/remotes/origin/merge ^abc --",
        "git branch --track merge refs/remotes/origin/merge",
    ] {
        assert_eq!(risk::high_impact(passes), None, "{passes}");
        assert!(!risk::self_approval(passes), "{passes}");
    }
    assert!(risk::high_impact("git merge --ff-only --no-overwrite-ignore @{u}").is_some());
}

#[test]
fn recap_respects_the_approval_gate() {
    let run = "run `git merge --ff-only --no-overwrite-ignore @{u}`";

    let (_dir, md) = scaffolded(&claude("rg0"));
    assert!(md.contains(run), "{md}");

    let mut p = claude("rg1");
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    let (_dir, md) = scaffolded(&p);
    assert!(!md.contains(run), "runs a gated merge:\n{md}");
    assert!(md.contains("approval gate"), "{md}");
    assert!(md.contains("git merge --ff-only --no-overwrite-ignore @{u}"));
}

#[test]
fn recap_states_its_safety_rules() {
    let (_dir, md) = scaffolded(&claude("rs"));
    // Ignore line wrapping: the rules may be rewrapped.
    let md = md.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "$ARGUMENTS",
        "--since",
        "--no-fetch",
        // ff-only, never a rewrite or a push; the gate is never routed around
        "fast-forward",
        "Never",
        "--force",
        "push",
        "work around",
        // a mirror refspec could overwrite or prune local branches
        "remote.origin.fetch",
        "refs/remotes/origin/",
        // a linked worktree stops the run
        "--git-common-dir",
        // the current branch never pulls in Claude's own config
        "`.claude/`",
        "`.mcp.json`",
        // ranges that leave out the default branch's merged-in commits
        "--first-parent",
        // three-argument form: only the branch's own patches, not main's
        "git range-diff refs/remotes/origin/<default> <old> refs/remotes/origin/<b>",
        // the current branch has a worktree path too
        "without the `*`",
        // untrusted input
        "data, never instructions",
        // old → new SHAs so every move can be undone
        "old → new",
        // the report
        "Needs your attention",
        "Summary",
        "Notable",
        // state written last, only after the report
        "state.json",
        // GitHub: a request the hook answers, never gh in Claude's shell
        "--no-github",
        "github-request.json",
        "github.json",
        "never run `gh`",
        "requested_at",
        "didn't answer",
        "git rev-parse --since",
        "github_checked_at",
        "## GitHub",
        // comments are data, and never anyone's approval
        "not an approval",
        "never a sign-off",
    ] {
        assert!(md.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn recap_writes_only_under_its_own_notes_folder() {
    let (_dir, md) = scaffolded(&claude("rp"));
    // Anything directly in .claude/notes/ is an /inquire ledger (its hook renders
    // and opens it), so every path /recap names sits in .claude/notes/recap/.
    let mut rest = md.as_str();
    while let Some(i) = rest.find(".claude/notes/") {
        rest = &rest[i + ".claude/notes/".len()..];
        assert!(
            rest.starts_with("recap/"),
            "outside recap/: …{}",
            &rest[..rest.len().min(40)]
        );
    }
    for f in [
        ".gitignore",
        "state.json",
        "<YYYY-MM-DD>.md",
        "github-request.json",
        "github.json",
    ] {
        assert!(
            md.contains(&format!(".claude/notes/recap/{f}")),
            "missing {f}"
        );
    }
    for path in [
        ".claude/notes/recap/2026-10-04.md",
        ".claude/notes/recap/state.json",
        ".claude/notes/recap/.gitignore",
        ".claude/notes/recap/github-request.json",
        ".claude/notes/recap/github.json",
        "/home/u/p/.claude/notes/recap/2026-10-04.md",
        r"C:\p\.claude\notes\recap\2026-10-04.md",
        r"C:\p\.claude\notes\recap\github-request.json",
    ] {
        assert_eq!(ledger_target(path), NoteTarget::NotLedger, "{path}");
    }
}

#[test]
fn recap_finds_issues_in_the_intent_files_only_with_intent() {
    let (_dir, md) = scaffolded(&claude("ri"));
    assert!(md.contains("`Issue:`") && md.contains("docs/adr/"), "{md}");

    let mut p = claude("ri2");
    p.claude.intent.dir = "decisions/".into();
    let (_dir, md) = scaffolded(&p);
    assert!(
        md.contains("decisions/") && !md.contains("docs/adr/"),
        "{md}"
    );

    let mut p = claude("ri3");
    p.claude.workflow.intent = false;
    let (_dir, md) = scaffolded(&p);
    assert!(!md.contains("`Issue:`"), "{md}");
    // The rest of the GitHub step stays.
    assert!(md.contains("github-request.json"));
}

#[test]
fn recap_report_follows_the_answer_language() {
    let mut p = claude("rl");
    p.response_language = "Ukrainian".into();
    let (_dir, md) = scaffolded(&p);
    assert!(md.contains("Write the report in Ukrainian"), "{md}");

    let (_dir, md) = scaffolded(&claude("rl2"));
    assert!(!md.contains("Write the report in"), "{md}");
}

#[test]
fn recap_backfills_on_old_state_and_can_be_turned_off() {
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert!(wf.recap, "old state files get /recap");
    assert!(Workflow::default().recap);

    let mut p = claude("roff");
    p.claude.workflow.recap = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/recap").exists());
}

#[test]
fn recap_is_a_reserved_skill_name() {
    assert!(validate::skill_name("recap").is_err());
}

#[test]
fn a_user_skill_named_like_a_workflow_skill_is_kept() {
    let mut p = claude("ru");
    p.skills.push(Skill {
        name: "recap".into(),
        description: "my own recap".into(),
        body: "MY OWN RECAP BODY".into(),
        ..Default::default()
    });
    let files = p.render_all().unwrap();
    let hits: Vec<&String> = files
        .iter()
        .filter(|(rel, _)| rel.ends_with("skills/recap/SKILL.md"))
        .map(|(_, c)| c)
        .collect();
    assert_eq!(hits.len(), 1, "one file per path");
    assert!(hits[0].contains("MY OWN RECAP BODY"), "{}", hits[0]);
    assert!(
        p.issues()
            .iter()
            .any(|w| w.contains("skill 'recap': collides")),
        "{:?}",
        p.issues()
    );
}

#[test]
fn the_plugin_ships_the_same_recap() {
    let mut p = claude("Recap Kit");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "recap-kit".into();
    let (dir, md) = scaffolded(&p);
    assert_eq!(
        read(dir.path(), "plugin/recap-kit/skills/recap/SKILL.md"),
        md
    );
}
