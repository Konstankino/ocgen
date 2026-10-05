//! The instruction language (`language`: the agents' prompts) and the answer
//! language (`response_language`: what the user reads) are separate. Only when
//! they differ does the coordinator get an answer line (and a Claude project the
//! `language` setting); a project where they match renders exactly as before.

use std::collections::BTreeMap;
use std::fs;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::{ChangeKind, Project};
use ocgen::target::Target;
use ocgen::templates;
use tempfile::tempdir;

/// The shipped manifest — never a developer's override.
fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

fn project(target: Target, prompts: &str, answers: &str, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&manifest(), prompts);
    p.target = target;
    p.project_name = "lang".into();
    p.response_language = answers.into();
    let mut roles = vec!["coordinator", "explorer", "implementer", "reviewer"];
    if adversary {
        // The default team's checks: the scope guard, then the adversary.
        roles.extend(["scope-guard", "adversary"]);
    }
    p.agents = roles
        .iter()
        .map(|r| match target {
            Target::ClaudeCode => Agent::from_archetype_claude(r, r, prompts),
            Target::OpenCode => Agent::from_archetype(r, r, prompts, "mac"),
        })
        .collect::<anyhow::Result<_>>()
        .unwrap();
    if target == Target::ClaudeCode {
        p.providers.clear();
    }
    p
}

fn rendered(p: &Project) -> BTreeMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

/// The coordinator's text as written: the OpenCode prompt file, or the Claude
/// team rule's coordination section.
fn coordinator(files: &BTreeMap<String, String>) -> &str {
    if let Some(p) = files.get(".opencode/prompts/coordinator.txt") {
        return p;
    }
    let rule = &files[".claude/rules/ocgen-team.md"];
    rule.split("## Coordination").nth(1).expect("coordination")
}

fn settings(files: &BTreeMap<String, String>) -> serde_json::Value {
    serde_json::from_str(&files[".claude/settings.json"]).unwrap()
}

const BOTH: [Target; 2] = [Target::ClaudeCode, Target::OpenCode];

// --------------------------------------------------- existing projects --

#[test]
fn a_state_without_response_language_answers_in_the_prompt_language() {
    let p: Project = serde_json::from_str(r#"{"language":"Ukrainian"}"#).unwrap();
    assert_eq!(p.response_language, "");
    assert_eq!(p.response_language(), "Ukrainian");
    assert!(!p.answers_differ());
    // No language at all renders English, as before.
    assert_eq!(Project::default().response_language(), "English");
}

#[test]
fn response_language_is_left_out_of_the_state_when_unset() {
    for (answers, written) in [("", false), ("Ukrainian", true)] {
        let p = project(Target::ClaudeCode, "English", answers, false);
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        let state = fs::read_to_string(dir.path().join(".claude/.ocgen-state.json")).unwrap();
        assert_eq!(
            state.contains("\"response_language\""),
            written,
            "{answers:?}"
        );
    }
}

#[test]
fn answering_in_the_prompt_language_renders_as_before() {
    for target in BOTH {
        for lang in ["English", "Ukrainian"] {
            for adversary in [false, true] {
                let before = rendered(&project(target, lang, "", adversary));
                // The same language, however it is spelled, changes nothing.
                for same in [lang.to_string(), lang.to_lowercase()] {
                    let after = rendered(&project(target, lang, &same, adversary));
                    assert_eq!(before, after, "{target:?} {lang} {same} {adversary}");
                }
                assert!(!before.values().any(|t| t.contains("Answer the user in")));
            }
        }
    }
}

#[test]
fn an_existing_project_regenerates_byte_identical() {
    for target in BOTH {
        let p = project(target, "Ukrainian", "Ukrainian", true);
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        // A state from before the field existed.
        let state = dir.path().join(target.state_file());
        let mut json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("response_language");
        fs::write(&state, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let loaded = Project::load_state(dir.path()).unwrap();
        for change in loaded.plan_changes(dir.path()).unwrap() {
            assert_eq!(change.kind, ChangeKind::Unchanged, "{}", change.rel);
        }
    }
}

// ------------------------------------------------------ the answer line --

#[test]
fn english_prompts_with_ukrainian_answers() {
    for target in BOTH {
        let same = rendered(&project(target, "English", "English", true));
        let split = rendered(&project(target, "English", "Ukrainian", true));
        let coord = coordinator(&split);
        assert!(!coord.contains("Respond in English."), "{coord}");
        for must in [
            "Answer the user in Ukrainian. Never answer in Russian.",
            "Write task briefs for subagents in English",
            "`Confidence: NN%`",
        ] {
            assert!(coord.contains(must), "missing {must:?} in:\n{coord}");
        }
        // Only the coordinator's text, Claude's settings and the four skills that
        // write files people read (/inquire's ledger, /intent's and /review-intent's
        // reading copies, /recap's report) change: the subagents and the other
        // commands keep their instructions.
        let changed: Vec<&String> = split
            .iter()
            .filter(|(rel, text)| same.get(*rel) != Some(text))
            .map(|(rel, _)| rel)
            .collect();
        let want: &[&str] = match target {
            Target::ClaudeCode => &[
                ".claude/rules/ocgen-team.md",
                ".claude/settings.json",
                ".claude/skills/inquire/SKILL.md",
                ".claude/skills/intent/SKILL.md",
                ".claude/skills/recap/SKILL.md",
                ".claude/skills/review-intent/SKILL.md",
            ],
            Target::OpenCode => &[".opencode/prompts/coordinator.txt"],
        };
        assert_eq!(changed, want, "{target:?}");
    }
}

#[test]
fn ukrainian_prompts_with_english_answers() {
    for target in BOTH {
        let files = rendered(&project(target, "Ukrainian", "English", false));
        let coord = coordinator(&files);
        assert!(!coord.contains("Відповідай українською."), "{coord}");
        assert!(
            coord.contains("Відповідай користувачеві англійською."),
            "{coord}"
        );
        assert!(
            !coord.contains("Russian") && !coord.contains("російськ"),
            "{coord}"
        );
    }
}

#[test]
fn the_answer_line_comes_before_the_adversary_loop() {
    for target in BOTH {
        let files = rendered(&project(target, "English", "Ukrainian", true));
        let coord = coordinator(&files);
        let line = coord.find("Answer the user in").unwrap();
        let scope = coord.find("Scope check").unwrap();
        let adversary = coord.find("Adversary check").unwrap();
        assert!(coord.find("Your subagents").unwrap() < line);
        assert!(line < scope && scope < adversary, "{coord}");
        // A blank line between sections, none doubled.
        // The mindset closes the coordinator's own text, before the answer.
        assert!(
            coord.contains("</mindset>\n\nAnswer the user in"),
            "{coord}"
        );
        assert!(coord.contains(".\n\nScope check"), "{coord}");
        assert!(!coord.contains("\n\n\n\nAdversary"), "{coord}");
        assert!(!coord.contains("\n\n\n\nScope"), "{coord}");
    }
}

#[test]
fn a_coordinator_without_an_answer_line_gets_one() {
    // A custom coordinator body.
    let mut p = project(Target::ClaudeCode, "English", "Ukrainian", false);
    p.agents[0].body = "You coordinate the team.".into();
    let coord = coordinator(&rendered(&p)).to_string();
    assert!(
        coord.contains("You coordinate the team.\n\n<mindset>")
            && coord.contains("</mindset>\n\nAnswer the user in Ukrainian."),
        "{coord}"
    );

    // An OpenCode coordinator without a prompt file: the line goes in its body.
    let mut p = project(Target::OpenCode, "English", "Ukrainian", false);
    p.agents[0].prompt_file = false;
    p.agents[0].prompt_body = None;
    let files = rendered(&p);
    let md = &files[".opencode/agents/coordinator.md"];
    assert!(md.contains("Answer the user in Ukrainian."), "{md}");
    assert!(md.ends_with('\n') && !md.ends_with("\n\n"));
}

#[test]
fn an_answer_line_left_mid_prompt_is_reported() {
    let mut p = project(Target::OpenCode, "English", "Ukrainian", false);
    let body = p.agents[0].prompt_body.take().unwrap();
    p.agents[0].prompt_body = Some(format!("{}\nAlso keep it short.\n", body.trim_end()));
    let issues = p.issues();
    assert!(
        issues
            .iter()
            .any(|w| w.contains("coordinator") && w.contains("Respond in English.")),
        "{issues:?}"
    );
    // Matching languages: nothing to contradict, nothing reported.
    p.response_language = "English".into();
    assert!(!p.issues().iter().any(|w| w.contains("Respond in English.")));
}

#[test]
fn settings_name_the_answer_language_only_when_it_differs() {
    for (prompts, answers, want) in [
        ("English", "Ukrainian", Some("ukrainian")),
        ("Ukrainian", "English", Some("english")),
        ("English", "", None),
        ("Ukrainian", "ukrainian", None),
    ] {
        let s = settings(&rendered(&project(
            Target::ClaudeCode,
            prompts,
            answers,
            false,
        )));
        assert_eq!(
            s.get("language").and_then(|v| v.as_str()),
            want,
            "{prompts}/{answers}"
        );
    }
}

#[test]
fn the_shipped_manifest_asks_both_languages() {
    let m = manifest();
    let var = |key: &str| m.variables.iter().find(|v| v.key == key).unwrap();
    let prompts = var("language");
    assert_eq!(prompts.default, "English");
    assert_eq!(prompts.prompt, "Instruction language");
    let answers = var("response_language");
    assert_eq!(answers.default, "Ukrainian");
    assert_eq!(answers.prompt, "Answer language");
    for v in [prompts, answers] {
        assert_eq!(v.r#type, "select");
        assert!(v.choices.contains(&"English".to_string()));
        assert!(v.choices.contains(&"Ukrainian".to_string()));
    }
    // Asked right after the instruction language.
    let keys: Vec<&str> = m.variables.iter().map(|v| v.key.as_str()).collect();
    let at = keys.iter().position(|k| *k == "language").unwrap();
    assert_eq!(keys[at + 1], "response_language");
}
