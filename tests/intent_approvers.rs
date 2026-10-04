//! /intent's drafts always name the project's current approvers. The approvers
//! are baked into the /intent skill when it is generated, so a session started
//! before they changed keeps writing the old list (it once wrote "No approvers
//! configured" a day after an approver was added). ocgen checks the files
//! themselves against the state, whatever the session was told: when Claude
//! writes one (the `intent-approvers` hook), when you review a draft, when the
//! approvers change, and in `ocgen verify`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use ocgen::agent;
use ocgen::intent::{self, Kind};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use predicates::str::contains;

fn project(dir: &Path, approvers: &[&str]) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "appr".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.intent.approvers = approvers.iter().map(|a| a.to_string()).collect();
    p.scaffold(dir, false).unwrap();
    p
}

const STALE_INTENT: &str = "# ADR-0001: Cache\n\nStatus: Proposed\nApprovers: No approvers configured\n\n## Sign-off\n- No approvers configured — pending\n";
const GOOD_INTENT: &str = "# ADR-0001: Cache\n\nStatus: Proposed\nApprovers: @alice, @org/arch\n\n## Sign-off\n- @alice — pending\n- @org/arch — pending\n";
const STALE_DRAFT: &str = "## Intent\nKeep the cache warm.\n\n## Approvers / sign-off\nNo approvers configured — approval pending.\n";
const GOOD_DRAFT: &str = "## Intent\nKeep the cache warm.\n\n## Needs from @alice, @org/arch\n- [ ] @alice — pending\n- [ ] @Org/Arch — pending\n";

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

// --------------------------------------------------------------- the rule --

#[test]
fn a_draft_names_every_approver_as_a_mention() {
    let appr = vec!["@alice".to_string(), "@org/arch".to_string()];
    assert!(intent::gaps(GOOD_DRAFT, &appr).is_empty());
    assert!(intent::gaps(GOOD_INTENT, &appr).is_empty());

    let g = intent::gaps(STALE_DRAFT, &appr);
    assert_eq!(g.missing, appr);
    assert!(g.says_none);

    // A whole handle only: not an email, a longer name, or another team.
    for text in [
        "mail alice@alice.dev",
        "@alicebob",
        "@org/architects",
        "@alice-x",
    ] {
        let g = intent::gaps(&format!("{text} @org/arch"), &appr);
        assert_eq!(g.missing, ["@alice"], "{text}");
    }
    // Named, but the stale line is still there.
    let g = intent::gaps(&format!("{GOOD_DRAFT}\nNo approvers configured\n"), &appr);
    assert!(g.missing.is_empty() && g.says_none);
    // With no approvers there is nothing to name.
    assert!(intent::gaps(STALE_DRAFT, &[]).is_empty());
}

#[test]
fn intent_files_and_drafts_are_recognised_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path(), &["@alice"]);
    let s = &p.claude.intent;
    for (rel, kind) in [
        ("docs/adr/ADR-0001-cache.md", Some(Kind::Intent)),
        ("docs/adr/adr-0012-x.md", Some(Kind::Intent)),
        ("docs/adr/0007-legacy.md", Some(Kind::Intent)),
        (".claude/intent/drafts/adr-0001-cache.md", Some(Kind::Draft)),
        ("docs/adr/README.md", None),
        ("docs/adr/issue-adr-1.md", None),
        ("docs/other/ADR-0001-x.md", None),
        (".claude/intent/view/adr-0001-cache.md", None),
        ("src/main.rs", None),
    ] {
        assert_eq!(intent::kind(s, rel), kind, "{rel}");
    }
    // Only a pending intent is held to the current list: a decided one is history.
    assert!(intent::pending(STALE_INTENT));
    assert!(intent::pending("# ADR-0001: x\nno status line\n"));
    assert!(!intent::pending(
        "# ADR-0001\nStatus: Accepted\nApprovers: @bob\n"
    ));
    assert!(!intent::pending("# ADR-0001\nStatus: Rejected\n"));
}

#[test]
fn stale_lists_pending_intents_and_drafts_that_miss_an_approver() {
    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path(), &["@alice", "@org/arch"]);
    write(dir.path(), "docs/adr/ADR-0001-cache.md", STALE_INTENT);
    write(dir.path(), "docs/adr/ADR-0002-ok.md", GOOD_INTENT);
    write(
        dir.path(),
        "docs/adr/ADR-0003-done.md",
        "Status: Accepted\nApprovers: @bob\n",
    );
    write(
        dir.path(),
        ".claude/intent/drafts/adr-0001-cache.md",
        STALE_DRAFT,
    );
    write(
        dir.path(),
        ".claude/intent/drafts/adr-0002-ok.md",
        GOOD_DRAFT,
    );
    let stale: Vec<String> = intent::stale(&p, dir.path())
        .into_iter()
        .map(|s| s.rel)
        .collect();
    assert_eq!(
        stale,
        [
            "docs/adr/ADR-0001-cache.md",
            ".claude/intent/drafts/adr-0001-cache.md"
        ]
    );
}

// ------------------------------------------------------------- the hook --

/// Run the `intent-approvers` hook after a write to `rel`.
fn hook(dir: &Path, rel: &str) -> ocgen::hooks::Outcome {
    let env: HashMap<String, String> = [(
        "CLAUDE_PROJECT_DIR".to_string(),
        ocgen::paths::for_shell(dir),
    )]
    .into();
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&dir.join(rel)) }
    });
    ocgen::hooks::run("intent-approvers", &payload.to_string(), &env)
}

/// The reason the hook gives Claude, or "" when it lets the write be.
fn reason(o: &ocgen::hooks::Outcome) -> String {
    assert_eq!(o.code, 0, "never fails a write: {o:?}");
    if o.stdout.is_empty() {
        return String::new();
    }
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["decision"], "block", "{v}");
    v["reason"].as_str().unwrap().to_string()
}

#[test]
fn a_draft_without_an_intent_file_keeps_its_own_status() {
    // With no intent file, /intent records the status in the draft as a comment
    // GitHub hides; a decided one is history, like a decided intent file.
    let decided = format!(
        "<!-- Issue: https://github.com/o/r/issues/7 -->\n<!-- Status: Accepted -->\n{STALE_DRAFT}"
    );
    let rejected = format!("<!-- Status: Rejected -->\n{STALE_DRAFT}");
    let proposed = format!("<!-- Status: Proposed -->\n{STALE_DRAFT}");
    assert!(!intent::pending(&decided));
    assert!(!intent::pending(&rejected));
    assert!(intent::pending(&proposed));
    assert!(
        intent::pending(STALE_DRAFT),
        "no status: pending, as before"
    );

    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path(), &["@alice", "@org/arch"]);
    let drafts = ".claude/intent/drafts";
    write(dir.path(), &format!("{drafts}/issue-done.md"), &decided);
    write(dir.path(), &format!("{drafts}/issue-no.md"), &rejected);
    write(dir.path(), &format!("{drafts}/issue-open.md"), &proposed);
    write(
        dir.path(),
        &format!("{drafts}/adr-0001-cache.md"),
        STALE_DRAFT,
    );
    let stale: Vec<String> = intent::stale(&p, dir.path())
        .into_iter()
        .map(|s| s.rel)
        .collect();
    assert_eq!(
        stale,
        [
            ".claude/intent/drafts/adr-0001-cache.md",
            ".claude/intent/drafts/issue-open.md"
        ]
    );
    assert_eq!(
        reason(&hook(dir.path(), &format!("{drafts}/issue-done.md"))),
        ""
    );
    assert!(reason(&hook(dir.path(), &format!("{drafts}/issue-open.md"))).contains("@alice"));
}

#[test]
fn the_hook_tells_claude_the_current_approvers_on_every_stale_write() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &["@alice", "@org/arch"]);

    write(
        dir.path(),
        ".claude/intent/drafts/adr-0001-cache.md",
        STALE_DRAFT,
    );
    let r = reason(&hook(dir.path(), ".claude/intent/drafts/adr-0001-cache.md"));
    for part in [
        ".claude/intent/drafts/adr-0001-cache.md",
        "@alice",
        "@org/arch",
        "No approvers configured",
        "Needs from",
    ] {
        assert!(r.contains(part), "reason misses {part}: {r}");
    }

    write(dir.path(), "docs/adr/ADR-0001-cache.md", STALE_INTENT);
    let r = reason(&hook(dir.path(), "docs/adr/ADR-0001-cache.md"));
    assert!(r.contains("@alice") && r.contains("Sign-off"), "{r}");

    // Fixed, decided, or not an intent file at all: nothing to say.
    write(
        dir.path(),
        ".claude/intent/drafts/adr-0001-cache.md",
        GOOD_DRAFT,
    );
    write(
        dir.path(),
        "docs/adr/ADR-0002-done.md",
        "Status: Accepted\n",
    );
    write(dir.path(), "docs/adr/notes.md", STALE_DRAFT);
    for rel in [
        ".claude/intent/drafts/adr-0001-cache.md",
        "docs/adr/ADR-0002-done.md",
        "docs/adr/notes.md",
        "src/gone.rs",
    ] {
        assert_eq!(reason(&hook(dir.path(), rel)), "", "{rel}");
    }
}

#[test]
fn the_hook_is_quiet_without_approvers_and_registered_with_intent() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &[]);
    write(dir.path(), ".claude/intent/drafts/x.md", STALE_DRAFT);
    assert_eq!(reason(&hook(dir.path(), ".claude/intent/drafts/x.md")), "");

    let s: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let group = s["hooks"]["PostToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g.to_string().contains("intent-approvers"))
        .expect("a PostToolUse group for the approvers check")
        .clone();
    assert_eq!(group["matcher"], "Write|Edit|MultiEdit");
    let cmd = group["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains("ocgen hook intent-approvers") && cmd.contains(ocgen::hooks::PROTOCOL),
        "{cmd}"
    );
    assert!(dir
        .path()
        .join(".claude/hooks/intent-approvers.sh")
        .is_file());
    assert!(ocgen::hooks::NAMES.contains(&"intent-approvers"));

    // Off with /intent.
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "appr2".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.workflow.intent = false;
    p.scaffold(dir.path(), false).unwrap();
    assert!(
        !fs::read_to_string(dir.path().join(".claude/settings.json"))
            .unwrap()
            .contains("intent-approvers")
    );
}

// ------------------------------------------------------- review and edit --

#[test]
fn the_editor_page_warns_about_missing_approvers() {
    let appr = vec!["@alice".to_string()];
    let page = ocgen::notes::draft::page(STALE_DRAFT, "d.md", "r", "n", "English", &appr).unwrap();
    assert!(page.contains(r#"id="approvers""#), "{page}");
    assert!(page.contains(r#"data-approvers="@alice""#), "{page}");
    let banner = page.split(r#"id="approvers""#).nth(1).unwrap();
    let banner = &banner[..banner.find("</div>").unwrap()];
    assert!(!banner.contains("hidden"), "shown: {banner}");
    assert!(banner.contains("@alice"), "{banner}");

    let ok = ocgen::notes::draft::page(GOOD_DRAFT, "d.md", "r", "n", "English", &appr).unwrap();
    let banner = ok.split(r#"id="approvers""#).nth(1).unwrap();
    assert!(banner[..banner.find('>').unwrap()].contains("hidden"));
}

#[test]
fn edit_intent_lists_the_files_that_miss_an_approver() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &[]);
    write(dir.path(), "docs/adr/ADR-0001-cache.md", STALE_INTENT);
    write(
        dir.path(),
        ".claude/intent/drafts/adr-0001-cache.md",
        STALE_DRAFT,
    );
    Command::cargo_bin("ocgen")
        .unwrap()
        .args(["edit", "intent", "--approver", "@alice", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("docs/adr/ADR-0001-cache.md"))
        .stdout(contains(".claude/intent/drafts/adr-0001-cache.md"))
        .stdout(contains("/intent ADR-0001"));
}

#[test]
fn verify_warns_about_a_pending_intent_that_misses_an_approver() {
    use ocgen::verify::Status;
    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path(), &["@alice"]);
    let run = || {
        let user = tempfile::tempdir().unwrap();
        ocgen::verify::verify(
            &p,
            dir.path(),
            &ocgen::verify::Options {
                run_claude: false,
                run_check: false,
                user_settings: Some(user.path().join("settings.json")),
            },
        )
    };
    let status = |checks: &[ocgen::verify::Check]| {
        checks
            .iter()
            .find(|c| c.name.contains("approvers in drafts"))
            .unwrap_or_else(|| panic!("no check: {checks:#?}"))
            .clone()
    };
    assert_eq!(status(&run()).status, Status::Pass);
    write(dir.path(), "docs/adr/ADR-0001-cache.md", STALE_INTENT);
    let c = status(&run());
    assert_eq!(c.status, Status::Warn, "{c:#?}");
    assert!(c.detail.contains("ADR-0001-cache.md"), "{c:#?}");
}

#[test]
fn opening_a_draft_says_which_approvers_it_misses() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &["@alice"]);
    write(
        dir.path(),
        ".claude/intent/drafts/adr-0001-cache.md",
        STALE_DRAFT,
    );

    // From a terminal.
    Command::cargo_bin("ocgen")
        .unwrap()
        .arg("draft")
        .current_dir(dir.path())
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .success()
        .stdout(contains("@alice"));

    // And the note Claude gets when you type `draft`.
    let env: HashMap<String, String> = [
        (
            "CLAUDE_PROJECT_DIR".to_string(),
            ocgen::paths::for_shell(dir.path()),
        ),
        ("OCGEN_NOTES_OPEN".to_string(), "0".to_string()),
    ]
    .into();
    let o = ocgen::hooks::run("intent-draft", r#"{"prompt":"draft"}"#, &env);
    assert!(o.stdout.contains("@alice"), "{o:?}");
}
