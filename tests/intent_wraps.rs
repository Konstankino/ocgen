//! /intent's issue drafts keep each paragraph on one line. GitHub shows every
//! newline in an issue as a line break, so a draft wrapped by hand at 100
//! columns reads as broken lines once it is filed — and in the draft editor,
//! which shows it the way GitHub will. The /intent skill says so; ocgen checks
//! every write of a draft (the `intent-approvers` hook), and the editor and
//! `ocgen draft` point out what is left.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::intent;
use ocgen::manifest::Manifest;
use ocgen::notes::draft;
use ocgen::render::Project;
use ocgen::target::Target;

/// A draft wrapped by hand in places, with breaks that are meant in others.
const WRAPPED: &str = include_str!("fixtures/intent-draft-wrapped.md");

/// The lines of WRAPPED that end mid-sentence: each one's newline is a wrap.
const WRAPS: [&str; 9] = [
    "`.claude/` is untracked, so a clone or `--worktree` session runs with no approval, plan, confidence",
    "or idle gate, while the tracked `CLAUDE.md` states the gates are",
    "active — Critical (F1). Evidence and full",
    "High: the generator's only input is untracked (F4); the",
    "approval gate's `aws` branch is incomplete (F8);",
    "`CLAUDE.md` contradicts the rules (F7). Medium: ungated `.git/config`,",
    "`WebSearch` past the fetch guard, *a no-op idle",
    "- **A. Commit `.claude/` in full.** Generated artifacts in history; the",
    "> Quoted from the review: a gate that is wrapped",
];

fn wrapped_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    intent::hard_wraps(text)
        .into_iter()
        .map(|n| lines[n - 1].to_string())
        .collect()
}

// --------------------------------------------------------------- the rule --

#[test]
fn lines_wrapped_mid_sentence_are_found_and_meant_breaks_are_not() {
    // A line that ends in `,` or `;`, or whose next line goes on in lowercase,
    // is a wrap — in a paragraph, a list item, a quote, inside emphasis.
    assert_eq!(wrapped_lines(WRAPPED), WRAPS);
    // Not a wrap: a sentence over another (the template's **Blocks prod?** and
    // **Depends on…** lines), a label or a URL on its own line, hard breaks
    // (two spaces, `\`, `<br>`), code.
    for meant in [
        "**Blocks prod?** No — nothing has been deployed.",
        "Deploys keep the cache warm.",
        "Links: docs/adr/ADR-0001-cache.md",
        "See also",
        "Press <kbd>Ctrl</kbd>+<kbd>S</kbd> to save.<br>",
        "Two spaces end this one  ",
        "code is",
    ] {
        assert!(
            !wrapped_lines(WRAPPED).iter().any(|l| l == meant),
            "{meant}"
        );
    }
    // The team's template, filled in, has none.
    let convention = "## Intent\nDeploys keep the cache warm.\n\n**Blocks prod?** No\n\
                      **Depends on the platform view** (what we need to protect)? Yes\n";
    assert!(intent::hard_wraps(convention).is_empty());
    assert!(intent::hard_wraps("").is_empty());
}

// ------------------------------------------------------------- the hook --

fn project(dir: &Path, approvers: &[&str]) {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "wraps".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.intent.approvers = approvers.iter().map(|a| a.to_string()).collect();
    p.scaffold(dir, false).unwrap();
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// What the `intent-approvers` hook tells Claude after a write to `rel` ("" when nothing).
fn after_write(dir: &Path, rel: &str) -> String {
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
    let o = ocgen::hooks::run("intent-approvers", &payload.to_string(), &env);
    assert_eq!(o.code, 0, "never fails a write: {o:?}");
    if o.stdout.is_empty() {
        return String::new();
    }
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["decision"], "block", "{v}");
    v["reason"].as_str().unwrap().to_string()
}

#[test]
fn the_hook_tells_claude_to_join_lines_wrapped_by_hand_in_a_draft() {
    for approvers in [&["@alice"][..], &[]] {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), approvers);
        let rel = ".claude/intent/drafts/adr-1-cache.md";
        write(dir.path(), rel, WRAPPED);
        let r = after_write(dir.path(), rel);
        for part in [
            rel,
            "wrapped by hand",
            "lines 3, 4, 5, 8, 9, 10, 11, 19, 23",
            "GitHub shows every newline in an issue as a line break",
            "one line",
            "**Blocks prod?**",
        ] {
            assert!(r.contains(part), "{approvers:?}: reason misses {part}: {r}");
        }
        // Approvers named: the wraps are all it is about.
        assert!(!r.contains("@mention"), "{r}");

        // Joined: nothing to say.
        write(
            dir.path(),
            rel,
            "## Intent\nOne line for the whole paragraph, as GitHub shows it.\n\n## Needs from @alice\n- [ ] @alice — pending\n",
        );
        assert_eq!(after_write(dir.path(), rel), "", "{approvers:?}");

        // An intent file is a document in the repository, where a newline is a space.
        write(dir.path(), "docs/adr/ADR-1-cache.md", WRAPPED);
        let r = after_write(dir.path(), "docs/adr/ADR-1-cache.md");
        assert!(!r.contains("wrapped by hand"), "{r}");
    }

    // Both at once: the approvers and the wraps, in one reason.
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &["@alice", "@bob"]);
    let rel = ".claude/intent/drafts/adr-1-cache.md";
    write(dir.path(), rel, WRAPPED);
    let r = after_write(dir.path(), rel);
    assert!(r.contains("@bob") && r.contains("wrapped by hand"), "{r}");
}

// ---------------------------------------------------- the editor, the skill --

#[test]
fn the_editor_points_out_wrapped_lines_and_offers_to_join_them() {
    let page = draft::page(WRAPPED, "d.md", "r", "n", "English", &[]).unwrap();
    assert!(
        page.contains(r#"<div class="banner" id="wraps" role="status">"#),
        "the banner shows"
    );
    assert!(page.contains(r#"<strong id="wraps-count">9</strong>"#));
    assert!(page.contains("GitHub shows every newline in an issue as a line break"));
    assert!(page.contains(r#"<button type="button" id="join-lines">Join them</button>"#));

    let clean = draft::page("One line.\n", "d.md", "r", "n", "English", &[]).unwrap();
    assert!(clean.contains(r#"<div class="banner" id="wraps" role="status" hidden>"#));

    let uk = draft::page(WRAPPED, "d.md", "r", "n", "Ukrainian", &[]).unwrap();
    assert!(uk.contains("GitHub показує кожен перенос рядка в задачі як розрив рядка"));
    assert!(uk.contains("Об’єднати їх"));
}

#[test]
fn the_intent_skill_and_the_issue_template_ask_for_one_line_per_paragraph() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), &["@alice"]);
    let skill = fs::read_to_string(dir.path().join(".claude/skills/intent/SKILL.md")).unwrap();
    assert!(
        skill.contains("GitHub shows every newline in an issue as a line break"),
        "{skill}"
    );
    assert!(skill.contains("never wrap"), "{skill}");

    // The template /intent follows asks for sentences, not lines, and says why.
    let template =
        fs::read_to_string(dir.path().join(ocgen::claude::INTENT_ISSUE_TEMPLATE)).unwrap();
    assert!(!template.contains("2-3 lines"), "{template}");
    assert!(
        template.contains("GitHub shows every newline in an issue as a line break"),
        "{template}"
    );
}
