//! The `/review-intent` workflow skill: a code review run by the project's own
//! agents in parallel, where a finding reaches the output only after the
//! adversary has checked its evidence, recorded as an /intent issue draft (and an
//! ADR when the user asks). It reuses /intent's settings, templates, writing
//! standard and hooks, so it ships whenever /intent does.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use ocgen::agent::Agent;
use ocgen::claude::{Output, Skill, CLAUDE_CODE_REVIEW_COMMANDS, WORKFLOW_SKILLS};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::{templates, validate};
use predicates::str::contains;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/review-intent/SKILL.md";
const INTENT: &str = ".claude/skills/intent/SKILL.md";

/// Exactly the tools /review-intent may use without asking: /intent's read-only
/// reads plus the few a diff needs. Nothing writes, files an issue or posts.
const REVIEW_TOOLS: [&str; 23] = [
    "Read",
    "Grep",
    "Glob",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git blame:*)",
    "Bash(git grep:*)",
    "Bash(git diff:*)",
    "Bash(git rev-parse:*)",
    "Bash(git fetch:*)",
    "Bash(git ls-tree:*)",
    "Bash(git ls-remote:*)",
    "Bash(gh issue list:*)",
    "Bash(gh issue view:*)",
    "Bash(gh pr list:*)",
    "Bash(gh pr view:*)",
    "Bash(gh pr diff:*)",
    "Bash(gh search issues:*)",
    "Bash(gh search prs:*)",
    "Bash(git merge-base:*)",
    "Bash(git status:*)",
    "Bash(git ls-files:*)",
    "Bash(git rev-list:*)",
];

/// The full default team, built from the shipped archetypes (never a
/// developer's override manifest).
const FULL: [(&str, &str); 6] = [
    ("coordinator", "coordinator"),
    ("explorer", "explorer"),
    ("implementer", "implementer"),
    ("reviewer", "reviewer"),
    ("scope-guard", "scope-guard"),
    ("adversary", "adversary"),
];

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

fn claude_with(answers: &str, team: &[(&str, &str)]) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "review".into();
    p.response_language = answers.into();
    p.providers.clear();
    p.agents = team
        .iter()
        .map(|(name, arch)| Agent::from_archetype_claude(name, arch, "English").unwrap())
        .collect();
    p
}

fn claude() -> Project {
    claude_with("English", &FULL)
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn rendered(p: &Project) -> HashMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

fn skill(p: &Project) -> String {
    rendered(p).remove(SKILL).expect("the review skill")
}

/// The skill's frontmatter lines (between the `---` fences).
fn frontmatter(md: &str) -> Vec<String> {
    md.strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// The text between `from` and `to` (both excluded), or a panic naming them.
fn between<'a>(md: &'a str, from: &str, to: &str) -> &'a str {
    let start = md.find(from).unwrap_or_else(|| panic!("no {from:?}")) + from.len();
    let len = md[start..]
        .find(to)
        .unwrap_or_else(|| panic!("no {to:?} after {from:?}"));
    &md[start..start + len]
}

fn settings(p: &Project) -> serde_json::Value {
    serde_json::from_str(&rendered(p)[".claude/settings.json"]).unwrap()
}

fn listed(v: &serde_json::Value, list: &str, rule: &str) -> bool {
    v["permissions"][list]
        .as_array()
        .is_some_and(|a| a.iter().any(|r| r == rule))
}

// ------------------------------------------------------------ the skill --

#[test]
fn the_skill_ships_with_intent_and_only_with_it() {
    let dir = tempdir().unwrap();
    claude().scaffold(dir.path(), false).unwrap();
    assert!(read(dir.path(), SKILL).starts_with("---\nname: review-intent\n"));

    let mut p = claude();
    p.claude.workflow.intent = false;
    assert!(!rendered(&p).contains_key(SKILL));
}

#[test]
fn the_plugin_ships_the_same_skill() {
    let mut p = claude();
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "review-kit".into();
    let files = rendered(&p);
    assert_eq!(
        files["plugin/review-kit/skills/review-intent/SKILL.md"],
        files[SKILL]
    );
}

#[test]
fn it_is_user_run_at_high_effort_with_read_only_tools() {
    let md = skill(&claude());
    let front = frontmatter(&md);
    assert!(front.iter().any(|l| l == "disable-model-invocation: true"));
    assert!(front.iter().any(|l| l == "effort: high"), "{front:?}");
    assert!(front.iter().any(|l| l.starts_with("description: ")));
    assert!(front.iter().any(|l| l.starts_with("argument-hint: ")));
    let tools: Vec<&str> = front
        .iter()
        .find_map(|l| l.strip_prefix("allowed-tools: "))
        .expect("allowed-tools")
        .split(", ")
        .collect();
    assert_eq!(tools, REVIEW_TOOLS);
}

#[test]
fn it_never_writes_code_files_an_issue_or_posts_on_a_pull_request() {
    let md = skill(&claude());
    for must in [
        "**read-only**",
        "Never create the issue yourself",
        "Never post on the pull request",
        "`gh pr comment`",
        "`gh pr review`",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
}

// ------------------------------------------------------------- the agents --

#[test]
fn it_runs_the_projects_own_agents_in_parallel_waves() {
    let md = skill(&claude());
    // Find: the scope check, one reviewer per area and the explorer, at once.
    let find = between(&md, "## 2.", "## 3.");
    assert!(find.contains("one message"), "{find}");
    for area in ["Correctness", "Security", "Tests", "Cross-platform"] {
        assert!(find.contains(&format!("**{area}:**")), "{area}: {find}");
    }
    assert!(find.contains("`scope-guard`") && find.contains("`reviewer`"));
    assert!(find.contains("`explorer`"));
    assert!(
        find.contains("can't run git"),
        "shell-less agents get the pack"
    );
    // Verify: the adversary, one candidate per run, at most 8 at once, without
    // the reviewer's reasoning.
    let verify = between(&md, "## 4.", "## Writing standard");
    assert!(verify.contains("`adversary`"), "{verify}");
    assert!(verify.contains("one message") && verify.contains("at most 8 at a time"));
    assert!(
        verify.contains("**not** the reviewer's reasoning"),
        "{verify}"
    );
    for verdict in [
        "**Confirmed (reproduced)**",
        "**Confirmed (by reading)**",
        "**Rejected**",
        "**Unsettled**",
    ] {
        assert!(verify.contains(verdict), "{verdict}");
    }
    assert!(verify.contains("Only confirmed candidates become findings"));
}

#[test]
fn a_renamed_adversary_and_guard_are_named() {
    let team = [
        ("coordinator", "coordinator"),
        ("explorer", "explorer"),
        ("implementer", "implementer"),
        ("reviewer", "reviewer"),
        ("scope-check", "scope-guard"),
        ("red-team", "adversary"),
    ];
    let md = skill(&claude_with("English", &team));
    assert!(md.contains("`red-team`") && md.contains("`scope-check`"));
    assert!(!md.contains("`adversary`") && !md.contains("`scope-guard`"));
    // The finding line credits the agent that checked it.
    assert!(md.contains("red-team: confirmed (reproduced"), "{md}");
}

#[test]
fn without_an_adversary_or_guard_it_says_what_it_can_vouch_for() {
    let team = [
        ("coordinator", "coordinator"),
        ("explorer", "explorer"),
        ("implementer", "implementer"),
        ("reviewer", "reviewer"),
    ];
    let md = skill(&claude_with("English", &team));
    let verify = between(&md, "## 4.", "## Writing standard");
    assert!(verify.contains("No adversary"), "{verify}");
    assert!(verify.contains("every surviving finding is **Inferred**"));
    assert!(!verify.contains("**Confirmed (reproduced)**"));
    let find = between(&md, "## 2.", "## 3.");
    assert!(find.contains("**Scope, yourself:**"), "{find}");
}

#[test]
fn without_a_reviewer_a_general_purpose_agent_is_told_to_stay_read_only() {
    let team = [
        ("coordinator", "coordinator"),
        ("explorer", "explorer"),
        ("implementer", "implementer"),
    ];
    let md = skill(&claude_with("English", &team));
    assert!(md.contains("`general-purpose`"), "{md}");
    assert!(md.contains("it can edit files"), "{md}");
}

#[test]
fn without_the_explorer_preferred_it_uses_the_built_in_explore() {
    let mut p = claude();
    p.claude.workflow.prefer_explorer = false;
    let md = skill(&p);
    let find = between(&md, "## 2.", "## 3.");
    assert!(find.contains("built-in `Explore`"), "{find}");
}

// ------------------------------------------------------------ the output --

#[test]
fn findings_follow_the_agreed_format() {
    let md = skill(&claude());
    for must in [
        "**R1, R2, …**",
        "**C1, C2, …**",
        "**R3 · High · Possible · Verified** (cross-platform)",
        "(provisional)",
        "To verify:",
        "**Not confirmed**",
        "<details><summary>",
        "**What works**",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
}

#[test]
fn the_draft_is_an_issue_draft_reviewed_in_the_browser() {
    let mut p = claude();
    p.claude.intent.max_words = 180;
    let md = skill(&p);
    assert!(md.contains(".claude/intent/drafts/issue-review-<slug>.md"));
    assert!(md.contains("180 words"));
    assert!(md.contains("the blocks don't count"));
    assert!(md.contains(".claude/intent/issue-template.md"));
    assert!(md.contains("Type `draft` to review it in your browser"));
    assert!(md.contains("gh issue create --title"));
    // Sign-off and the issue link stay /intent's.
    assert!(md.contains("/intent issue-review-<slug>"), "{md}");
}

#[test]
fn an_adr_is_written_only_when_asked_and_linked_both_ways() {
    let mut p = claude();
    p.claude.intent.prefix = "RFC".into();
    p.claude.intent.digits = 3;
    p.claude.intent.dir = "docs/rfc".into();
    p.claude.intent.branch = "trunk".into();
    let md = skill(&p);
    let adr = &md[md.find("## 6.").expect("an ADR step")..];
    assert!(adr.contains("only when I ask"), "{adr}");
    assert!(adr.contains("docs/rfc/RFC-") && adr.contains("3 digits"));
    assert!(adr.contains("git fetch --quiet origin trunk") && adr.contains("git ls-tree"));
    assert!(adr.contains(".claude/intent/intent-template.md"));
    assert!(adr.contains("Issue: not filed yet — draft"), "{adr}");
    assert!(adr.contains("`/intent`, `/deliver` and `/recap`"), "{adr}");
    assert!(adr.contains("type `draft`"), "the stale tab: {adr}");
}

#[test]
fn it_reuses_intents_text_instead_of_copying_it() {
    let files = rendered(&claude());
    let (intent, review) = (&files[INTENT], &files[SKILL]);
    // The writing standard: the same text, findings numbered R# instead of F#.
    let standard =
        |md: &str, heading: &str, next: &str| between(md, heading, next).replace("R#", "F#");
    assert_eq!(
        standard(intent, "## Writing standard (steps 4 and 5)\n", "\n\n## 4."),
        standard(review, "## Writing standard (steps 5 and 6)\n", "\n\n## 5.")
    );
    // The browser review, and how an intent file is numbered.
    for (from, to) in [
        ("  - **The browser review:**", "let me decide."),
        ("1. **Pick the number**", "2. Name it"),
        ("- **Tone check**", "(c) **Approvers**"),
    ] {
        let a = between(intent, from, to);
        let b = between(review, from, to);
        if from.contains("Tone check") {
            // The one clause that says where a finding appears in full.
            let cut = |s: &str| s.split("(in full").next().unwrap().to_string();
            assert_eq!(cut(a), cut(b), "{from}");
        } else {
            assert_eq!(a, b, "{from}");
        }
    }
}

#[test]
fn reading_copies_only_when_the_answers_are_not_english() {
    assert!(!skill(&claude()).contains("**reading copy**"));
    let md = skill(&claude_with("Ukrainian", &FULL));
    assert!(
        md.contains("**reading copy**") && md.contains("Ukrainian"),
        "{md}"
    );
}

#[test]
fn the_rule_points_to_it() {
    let rule = &rendered(&claude())[".claude/rules/ocgen-workflow.md"];
    assert!(rule.contains("/review-intent"), "{rule}");
}

// ------------------------------------------------------------- settings --

#[test]
fn posting_on_a_pull_request_asks_first_while_intent_is_on() {
    let rules = ["Bash(gh pr comment*)", "Bash(gh pr review*)"];
    let mut p = claude();
    for r in rules {
        assert!(listed(&settings(&p), "ask", r), "{r}");
    }
    // Kept even without the permission defaults: the skill relies on it.
    p.claude.powerups.permissions = false;
    for r in rules {
        assert!(listed(&settings(&p), "ask", r), "{r} without defaults");
    }
    p.claude.workflow.intent = false;
    for r in rules {
        assert!(!listed(&settings(&p), "ask", r), "{r} with intent off");
    }
}

// ----------------------------------------------------------------- name --

#[test]
fn the_name_is_reserved_and_clear_of_claude_codes_own_reviews() {
    assert!(WORKFLOW_SKILLS.contains(&"review-intent"));
    assert!(validate::skill_name("review-intent").is_err());
    for name in WORKFLOW_SKILLS {
        assert!(!CLAUDE_CODE_REVIEW_COMMANDS.contains(&name), "{name}");
    }
    for builtin in ["review", "code-review", "security-review"] {
        assert!(CLAUDE_CODE_REVIEW_COMMANDS.contains(&builtin));
    }
}

#[test]
fn a_user_skill_named_review_intent_is_kept_and_warned_about() {
    let mut p = claude();
    p.skills.push(Skill {
        name: "review-intent".into(),
        description: "my own review".into(),
        body: "MY OWN REVIEW BODY".into(),
        ..Default::default()
    });
    assert!(rendered(&p)[SKILL].contains("MY OWN REVIEW BODY"));
    assert!(p
        .issues()
        .iter()
        .any(|w| w.contains("skill 'review-intent': collides")));
}

// -------------------------------------------- the hooks /intent shipped --

const STALE_DRAFT: &str =
    "## Intent\nReview of feat/x.\n\n## Needs from the approvers\nNo approvers configured.\n";

fn env(dir: &Path) -> HashMap<String, String> {
    [
        (
            "CLAUDE_PROJECT_DIR".to_string(),
            ocgen::paths::for_shell(dir),
        ),
        ("OCGEN_NOTES_OPEN".to_string(), "0".to_string()),
    ]
    .into()
}

#[test]
fn a_review_draft_is_checked_and_opened_like_any_issue_draft() {
    let dir = tempdir().unwrap();
    let mut p = claude();
    p.claude.intent.approvers = vec!["@alice".into()];
    p.scaffold(dir.path(), false).unwrap();
    let rel = ".claude/intent/drafts/issue-review-feat-x.md";
    let file = dir.path().join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, STALE_DRAFT).unwrap();

    // After a write: the approvers check names who is missing.
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&file) }
    });
    let o = ocgen::hooks::run("intent-approvers", &payload.to_string(), &env(dir.path()));
    assert!(o.stdout.contains("@alice"), "{o:?}");

    // The word `draft`: the newest draft is the review's.
    let o = ocgen::hooks::run("intent-draft", r#"{"prompt":"draft"}"#, &env(dir.path()));
    assert!(o.stdout.contains("issue-review-feat-x.md"), "{o:?}");
}

#[test]
fn with_no_draft_yet_both_skills_are_named() {
    let dir = tempdir().unwrap();
    claude().scaffold(dir.path(), false).unwrap();
    let o = ocgen::hooks::run("intent-draft", r#"{"prompt":"draft"}"#, &env(dir.path()));
    assert!(o.stdout.contains("no issue draft"), "{o:?}");
    assert!(o.stdout.contains("/review-intent"), "{o:?}");

    fs::create_dir_all(dir.path().join(".claude/intent/drafts")).unwrap();
    Command::cargo_bin("ocgen")
        .unwrap()
        .arg("draft")
        .current_dir(dir.path())
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .failure()
        .stderr(contains("/review-intent"));
}

#[test]
fn landscape_lists_it() {
    let dir = tempdir().unwrap();
    claude().scaffold(dir.path(), false).unwrap();
    Command::cargo_bin("ocgen")
        .unwrap()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("/review-intent"));
}
