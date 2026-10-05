//! `ocgen doctor` brings a preset agent nobody edited up to its current preset:
//! when an agent's text is one an older ocgen shipped for its role, the text is
//! re-seeded. An edited agent, a custom role and every other field are kept.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use ocgen::agent::{self, Agent};
use ocgen::archetype::Archetype;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::Value;
use tempfile::tempdir;

/// The adversary preset as ocgen 0.8.3 shipped it.
const OLD_ADVERSARY: &str = include_str!("fixtures/adversary-0.8.3.toml");

/// The 0.8.3 adversary's (description, body) in `lang`.
fn old_adversary(lang: &str) -> (String, String) {
    let t: toml::Value = toml::from_str(OLD_ADVERSARY).unwrap();
    let get = |key: &str| t[key][lang].as_str().unwrap().to_string();
    (get("description"), get("body"))
}

fn project(target: Target, lang: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "pr".into();
    p.target = target;
    p.agents = match target {
        Target::ClaudeCode => {
            p.providers.clear();
            agent::claude_default_pipeline(lang).unwrap()
        }
        Target::OpenCode => agent::default_pipeline(lang, "mac").unwrap(),
    };
    // The adversary under another name, as `ocgen new` lets you call it.
    let adv = p.agents.iter_mut().find(|a| a.role == "adversary").unwrap();
    adv.name = "red-team".into();
    p
}

fn red_team(p: &mut Project) -> &mut Agent {
    p.agents.iter_mut().find(|a| a.name == "red-team").unwrap()
}

fn mentions(fixes: &[String], name: &str) -> bool {
    fixes.iter().any(|f| f.contains(&format!("'{name}'")))
}

#[test]
fn an_unedited_agent_from_an_older_preset_gets_the_current_one() {
    for target in [Target::ClaudeCode, Target::OpenCode] {
        for lang in ["English", "Ukrainian"] {
            let mut p = project(target, lang);
            let current = red_team(&mut p).clone();
            let (desc, body) = old_adversary(lang);
            let a = red_team(&mut p);
            a.description = desc;
            a.body = body;

            let fixes = p.doctor();
            let a = red_team(&mut p);
            assert_eq!(a.body, current.body, "{target:?} {lang}");
            assert_eq!(a.description, current.description, "{target:?} {lang}");
            assert!(
                mentions(&fixes, "red-team") && fixes.iter().any(|f| f.contains("adversary")),
                "{target:?} {lang}: {fixes:?}"
            );
        }
    }
}

#[test]
fn only_the_text_changes() {
    let mut p = project(Target::ClaudeCode, "English");
    let (desc, body) = old_adversary("English");
    let a = red_team(&mut p);
    a.description = desc;
    a.body = body;
    a.model = "sonnet".into();
    a.tools = "Read, Grep".into();
    a.steps = Some(12);
    a.effort = "max".into();
    p.doctor();
    let a = red_team(&mut p);
    assert!(a.body.contains("Threat model: accidents, not attackers."));
    assert_eq!(
        (
            a.model.as_str(),
            a.tools.as_str(),
            a.steps,
            a.effort.as_str()
        ),
        ("sonnet", "Read, Grep", Some(12), "max")
    );
}

#[test]
fn an_edited_agent_keeps_its_text() {
    for edit in ["\nAlso check the migrations.", ""] {
        let mut p = project(Target::ClaudeCode, "English");
        let (desc, body) = old_adversary("English");
        let a = red_team(&mut p);
        // Either field edited is enough to keep both.
        if edit.is_empty() {
            a.description = format!("{desc} (ours)");
            a.body = body;
        } else {
            a.description = desc;
            a.body = format!("{body}{edit}");
        }
        let before = red_team(&mut p).clone();
        let fixes = p.doctor();
        let a = red_team(&mut p);
        assert_eq!(
            (&a.description, &a.body),
            (&before.description, &before.body)
        );
        assert!(!mentions(&fixes, "red-team"), "{fixes:?}");
    }
}

#[test]
fn current_agents_and_custom_roles_are_left_alone() {
    let mut p = project(Target::ClaudeCode, "English");
    let mut own = Agent::blank("docs-writer", "docs", "");
    // A custom role whose text happens to be an old preset's is still its own.
    let (_, body) = old_adversary("English");
    own.body = body;
    p.agents.push(own);
    let before: Vec<Agent> = p.agents.clone();
    let fixes = p.doctor();
    for (a, b) in p.agents.iter().zip(&before) {
        assert_eq!(
            (&a.description, &a.body),
            (&b.description, &b.body),
            "{}",
            a.name
        );
    }
    // (A blank agent gets doctor's usual fixes: a model and a colour.)
    assert!(!fixes.iter().any(|f| f.contains("preset")), "{fixes:?}");
}

#[test]
fn the_shipped_presets_are_the_ones_fingerprinted() {
    // The adversary's 0.8.3 text and today's are both known; an edit is not.
    let (desc, body) = old_adversary("Ukrainian");
    let arch = Archetype::load("adversary").unwrap();
    for text in [
        desc,
        body,
        arch.body_for("English"),
        arch.description_for("Ukrainian"),
    ] {
        assert!(
            ocgen::render::shipped_preset_text("adversary", &text),
            "{text}"
        );
    }
    assert!(!ocgen::render::shipped_preset_text(
        "adversary",
        "You hunt bugs."
    ));
    // A text is known for its own role only.
    assert!(!ocgen::render::shipped_preset_text(
        "reviewer",
        &arch.body_for("English")
    ));
}

#[test]
fn ocgen_doctor_rewrites_the_agent_file() {
    let dir = tempdir().unwrap();
    project(Target::ClaudeCode, "English")
        .scaffold(dir.path(), false)
        .unwrap();
    let state = dir.path().join(".claude/.ocgen-state.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
    let (desc, body) = old_adversary("English");
    for a in v["agents"].as_array_mut().unwrap() {
        if a["name"] == "red-team" {
            a["description"] = desc.clone().into();
            a["body"] = body.clone().into();
        }
    }
    fs::write(&state, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    Command::cargo_bin("ocgen")
        .unwrap()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success();

    let md = fs::read_to_string(dir.path().join(".claude/agents/red-team.md")).unwrap();
    assert!(
        md.contains("Threat model: accidents, not attackers."),
        "{md}"
    );
    assert!(!md.contains("outside attacker"), "{md}");
    let saved: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
    let a = saved["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "red-team")
        .unwrap();
    assert!(a["body"]
        .as_str()
        .unwrap()
        .contains("accidents, not attackers"));
    assert!(Path::new(&state).is_file());
}
