//! The mindset every agent ocgen sets up is given: safety work goes to protecting
//! against accidents, not to defending against insiders as adversaries. ocgen
//! adds it when it writes the agents, so a project made before it existed picks
//! it up on regeneration, and it overrides any threat model in a role's own text.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ocgen::agent::{self, Agent};
use ocgen::archetype::Archetype;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::{tempdir, TempDir};

const EN: &str = "safety work goes to protecting against accidents, not to defending against insiders as adversaries";
const UK: &str = "робота з безпеки спрямована на захист від випадковостей, а не на оборону від інсайдерів як від супротивників";

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn scaffolded(p: &Project) -> TempDir {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    dir
}

fn claude(lang: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "ms".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline(lang).unwrap();
    p
}

fn opencode(lang: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "ms".into();
    p.agents = agent::default_pipeline(lang, "mac").unwrap();
    p
}

/// `text` on one line, so a phrase wrapped across lines still matches.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many times the mindset appears in `text`.
fn told(text: &str, mark: &str) -> usize {
    flat(text).matches(mark).count()
}

#[test]
fn every_claude_agent_and_the_coordinator_are_told() {
    for (lang, mark) in [("English", EN), ("Ukrainian", UK)] {
        let p = claude(lang);
        let dir = scaffolded(&p);
        let subs: Vec<_> = p.agents.iter().filter(|a| a.mode != "primary").collect();
        assert!(!subs.is_empty());
        for a in subs {
            let md = read(dir.path(), &format!(".claude/agents/{}.md", a.name));
            assert_eq!(told(&md, mark), 1, "{lang} {}:\n{md}", a.name);
            assert!(md.contains("<mindset>") && md.trim_end().ends_with("</mindset>"));
        }
        let rules = read(dir.path(), ".claude/rules/ocgen-team.md");
        assert_eq!(told(&rules, mark), 1, "{lang} coordinator:\n{rules}");
    }
}

#[test]
fn every_opencode_agent_is_told_and_the_answer_line_stays_last() {
    for (lang, mark, answer) in [
        ("English", EN, "Respond in English."),
        ("Ukrainian", UK, "Відповідай українською."),
    ] {
        let p = opencode(lang);
        let dir = scaffolded(&p);
        for a in &p.agents {
            let md = read(dir.path(), &format!(".opencode/agents/{}.md", a.name));
            assert_eq!(told(&md, mark), 1, "{lang} {}:\n{md}", a.name);
            if a.prompt_file {
                let txt = read(dir.path(), &format!(".opencode/prompts/{}.txt", a.name));
                assert_eq!(told(&txt, mark), 1, "{lang} {} prompt:\n{txt}", a.name);
            }
        }
        let coordinator = p.agents.iter().find(|a| a.mode == "primary").unwrap();
        let rel = if coordinator.prompt_file {
            format!(".opencode/prompts/{}.txt", coordinator.name)
        } else {
            format!(".opencode/agents/{}.md", coordinator.name)
        };
        let text = read(dir.path(), &rel);
        let at = text
            .rfind(answer)
            .expect("the coordinator keeps its answer line");
        assert!(
            text.find("</mindset>").unwrap() < at,
            "the answer line no longer closes the text:\n{text}"
        );
    }
}

#[test]
fn a_custom_agent_is_told_too() {
    let mut p = claude("English");
    let mut own = Agent::blank("docs-writer", "docs", "");
    own.body = "You write the docs.".into();
    p.agents.push(own);
    let md = read(scaffolded(&p).path(), ".claude/agents/docs-writer.md");
    // Its own text first, then what ocgen adds, the mindset last.
    let own = md.find("You write the docs.").unwrap();
    assert!(own < md.find("<mindset>").unwrap(), "{md}");
    assert!(md.trim_end().ends_with("</mindset>"), "{md}");
    assert_eq!(told(&md, EN), 1, "{md}");
}

#[test]
fn the_main_session_is_told_whenever_it_coordinates() {
    // Without a coordinator agent, the scope and adversary loops are still the
    // main session's to run.
    let mut p = claude("English");
    p.agents.retain(|a| a.mode != "primary");
    assert!(p.agents.iter().any(|a| a.role == "scope-guard"));
    let rules = read(scaffolded(&p).path(), ".claude/rules/ocgen-team.md");
    assert!(rules.contains("## Coordination"), "{rules}");
    assert_eq!(told(&rules, EN), 1, "{rules}");

    // With nothing to coordinate, no section appears just to carry it.
    p.agents
        .retain(|a| a.role != "scope-guard" && a.role != "adversary");
    let rules = read(scaffolded(&p).path(), ".claude/rules/ocgen-team.md");
    assert!(!rules.contains("## Coordination"), "{rules}");
    assert_eq!(told(&rules, EN), 0, "{rules}");
}

#[test]
fn it_says_what_to_do_with_each_and_overrides_a_role_s_threat_model() {
    let en = flat(&read(
        scaffolded(&claude("English")).path(),
        ".claude/agents/explorer.md",
    ));
    for must in [
        "Insiders are not adversaries.",
        "Deliberate attacks are not what you defend against.",
        "This mindset takes precedence over any threat model your role's own text describes.",
        "never get around a safeguard",
        "ask for confirmation before anything destructive or irreversible",
        "never rate one above Low",
        "a plain `git push`",
        "`git -c credential.helper=osxkeychain push`",
        "Keep your role's own report format.",
        "Insider (out of scope):",
        "before a closing `Confidence:` line",
    ] {
        assert!(en.contains(must), "missing {must:?} in:\n{en}");
    }
    let uk = flat(&read(
        scaffolded(&claude("Ukrainian")).path(),
        ".claude/agents/explorer.md",
    ));
    for must in [
        "Інсайдери — не супротивники.",
        "Цей підхід має пріоритет над будь-якою моделлю загрози з тексту твоєї ролі.",
        "Insider (out of scope):",
    ] {
        assert!(uk.contains(must), "missing {must:?} in:\n{uk}");
    }
}

#[test]
fn the_adversary_preset_hunts_accidents_not_attackers() {
    let arch = Archetype::load("adversary").unwrap();
    let en = arch.body_for("English");
    let uk = arch.body_for("Ukrainian");
    assert!(
        flat(&en).contains("Threat model: accidents, not attackers."),
        "{en}"
    );
    assert!(
        flat(&uk).contains("Модель загрози — випадковості, а не зловмисники."),
        "{uk}"
    );
    for gone in [
        "outside attacker",
        "attack scenario",
        "evil.zip",
        "the team's skeptic and attacker",
    ] {
        assert!(!en.contains(gone), "{gone:?} still in:\n{en}");
    }
    for gone in [
        "зовнішній зловмисник",
        "сценарій атаки",
        "evil.zip",
        "скептик і зловмисник",
    ] {
        assert!(!uk.contains(gone), "{gone:?} still in:\n{uk}");
    }
    for lang in ["English", "Ukrainian"] {
        let d = arch.description_for(lang);
        assert!(!d.contains("attacker") && !d.contains("зловмисник"), "{d}");
    }
}

#[test]
fn no_other_role_or_skill_casts_the_adversary_as_an_attacker() {
    let scope = Archetype::load("scope-guard").unwrap();
    assert!(!scope.body_for("English").contains("can be attacked"));
    assert!(!scope.body_for("Ukrainian").contains("можна атакувати"));

    let dir = scaffolded(&claude("English"));
    let mut files = BTreeMap::new();
    for rel in [
        ".claude/skills/intent/SKILL.md",
        ".claude/skills/deliver/SKILL.md",
    ] {
        files.insert(rel, read(dir.path(), rel));
    }
    let intent = &files[".claude/skills/intent/SKILL.md"];
    assert!(intent.contains("for accidents"), "{intent}");
    for text in files.values() {
        assert!(!text.contains("outside attacker"), "{text}");
    }
}
