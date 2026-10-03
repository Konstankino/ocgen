//! `ocgen edit language`: switch an existing project's instruction language
//! (re-seeding the preset agents nobody edited) or its answer language.

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::{match_language, Project};
use ocgen::target::Target;
use ocgen::templates;

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

const TEAM: [&str; 5] = [
    "coordinator",
    "explorer",
    "implementer",
    "reviewer",
    "adversary",
];

fn seed(target: Target, role: &str, lang: &str) -> Agent {
    match target {
        Target::ClaudeCode => Agent::from_archetype_claude(role, role, lang),
        Target::OpenCode => Agent::from_archetype(role, role, lang, "mac"),
    }
    .unwrap()
}

fn project(target: Target, prompts: &str, answers: &str) -> Project {
    let mut p = Project::from_manifest(&manifest(), prompts);
    p.target = target;
    p.project_name = "edit-lang".into();
    p.response_language = answers.into();
    p.agents = TEAM.iter().map(|r| seed(target, r, prompts)).collect();
    if target == Target::ClaudeCode {
        p.providers.clear();
    }
    p
}

/// The text an agent's preset gave it.
fn text(a: &Agent) -> (String, String, Option<String>) {
    (a.description.clone(), a.body.clone(), a.prompt_body.clone())
}

const BOTH: [Target; 2] = [Target::ClaudeCode, Target::OpenCode];

#[test]
fn changing_the_answers_leaves_every_agent_text_alone() {
    for target in BOTH {
        let mut p = project(target, "English", "");
        let before: Vec<_> = p.agents.iter().map(text).collect();
        let change = p.change_languages(None, Some("Ukrainian")).unwrap();
        assert!(change.reseeded.is_empty() && change.kept.is_empty());
        assert_eq!(change.pinned_answers, None);
        assert_eq!(p.agents.iter().map(text).collect::<Vec<_>>(), before);
        assert_eq!(p.language, "English");
        assert_eq!(p.response_language(), "Ukrainian");
    }
}

#[test]
fn changing_the_prompts_reseeds_preset_agents_still_as_seeded() {
    for target in BOTH {
        let mut p = project(target, "English", "Ukrainian");
        let change = p.change_languages(Some("Ukrainian"), None).unwrap();
        assert_eq!(change.reseeded, TEAM.to_vec(), "{target:?}");
        assert!(change.kept.is_empty());
        assert_eq!(p.language, "Ukrainian");
        assert_eq!(p.response_language(), "Ukrainian");
        for (a, role) in p.agents.iter().zip(TEAM) {
            assert_eq!(text(a), text(&seed(target, role, "Ukrainian")), "{role}");
        }
        // Everything else about an agent stays as it was.
        assert_eq!(p.agents[1].name, "explorer");
    }
}

#[test]
fn an_edited_preset_agent_keeps_its_text_and_is_reported() {
    let mut p = project(Target::ClaudeCode, "English", "Ukrainian");
    p.agents[2].body = "You implement, then run the tests.".into();
    let mut custom = Agent::blank("scribe", "custom", "");
    custom.body = "You take notes.".into();
    p.agents.push(custom);

    let change = p.change_languages(Some("Ukrainian"), None).unwrap();
    assert_eq!(change.kept, vec!["implementer", "scribe"]);
    assert!(!change.reseeded.contains(&"implementer".to_string()));
    assert_eq!(p.agents[2].body, "You implement, then run the tests.");
    assert_eq!(p.agents[5].body, "You take notes.");
}

#[test]
fn changing_the_prompts_of_an_old_project_keeps_its_answers() {
    for target in BOTH {
        // From before the answer language existed: the field is empty.
        let mut p = project(target, "English", "");
        let change = p.change_languages(Some("Ukrainian"), None).unwrap();
        assert_eq!(change.pinned_answers.as_deref(), Some("English"));
        assert_eq!(p.response_language, "English");
        assert!(p.answers_differ());
        let files = p.render_all().unwrap();
        assert!(
            files
                .iter()
                .any(|(_, t)| t.contains("Відповідай користувачеві англійською.")),
            "{target:?}"
        );
    }
}

#[test]
fn the_same_instruction_language_changes_nothing() {
    let mut p = project(Target::OpenCode, "English", "");
    let change = p.change_languages(Some("English"), None).unwrap();
    assert!(change.reseeded.is_empty() && change.kept.is_empty());
    assert_eq!(change.pinned_answers, None);
    assert_eq!(p.response_language, "");
}

#[test]
fn language_names_are_matched_case_insensitively() {
    let choices = vec!["English".to_string(), "Ukrainian".to_string()];
    assert_eq!(match_language("ukrainian", &choices).unwrap(), "Ukrainian");
    assert_eq!(match_language(" ENGLISH ", &choices).unwrap(), "English");
    let err = match_language("Polish", &choices).unwrap_err().to_string();
    assert!(
        err.contains("Polish") && err.contains("English, Ukrainian"),
        "{err}"
    );
}
