//! /deliver and /intent use the project's adversary when it has one: /deliver
//! has it check the whole change before synthesizing, /intent has it attack the
//! findings (Pass 4) and the plan, and records what stays open in both drafts.
//! Without an adversary both commands read exactly as before.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::claude::Output;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;
use tempfile::tempdir;

const LEGACY: [&str; 4] = ["coordinator", "explorer", "implementer", "reviewer"];

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

fn project(adversary: bool) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "cmds".into();
    p.providers.clear();
    p.agents = LEGACY
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, "English").unwrap())
        .collect();
    if adversary {
        p.agents
            .push(Agent::from_archetype_claude("adversary", "adversary", "English").unwrap());
    }
    p
}

fn files(p: &Project) -> BTreeMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

const DELIVER: &str = ".claude/skills/deliver/SKILL.md";
const INTENT: &str = ".claude/skills/intent/SKILL.md";

/// The text between two headings of a command.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text.find(from).unwrap_or_else(|| panic!("no {from:?}"));
    let end = text[start..].find(to).map_or(text.len(), |i| start + i);
    &text[start..end]
}

// ------------------------------------------------------------- /deliver --

#[test]
fn deliver_has_the_adversary_check_the_change_before_synthesizing() {
    let f = files(&project(true));
    let execute = section(&f[DELIVER], "## 5. Execute", "## 6. Synthesize");
    for must in [
        "**Adversary check.**",
        "have `adversary` check the whole change",
        "merge `implementer`'s worktree branch",
        "`Verdict: REWORK`",
        "send `implementer` only its Critical and High findings",
        "At most 2 rework rounds",
        "**UNRESOLVED**",
        ".claude/rules/ocgen-team.md",
    ] {
        assert!(execute.contains(must), "missing {must:?} in:\n{execute}");
    }
}

// -------------------------------------------------------------- /intent --

#[test]
fn intent_has_the_adversary_attack_the_findings_and_the_plan() {
    let f = files(&project(true));
    let text = &f[INTENT];

    // Pass 4: the adversary challenges the findings, not the explorer.
    let pass4 = section(text, "**Pass 4 — Challenge.**", "**Analysis report**");
    assert!(
        pass4.contains("Give a fresh `adversary` subagent the findings' claims"),
        "{pass4}"
    );

    // Step 3: it attacks the plan before I see it.
    let plan = section(
        text,
        "## 3. Plan and get my approval",
        "## Writing standard",
    );
    for must in [
        "give `adversary` the plan",
        "`Verdict: REWORK`",
        "at most 2 rounds",
        "**UNRESOLVED**",
    ] {
        assert!(plan.contains(must), "missing {must:?} in:\n{plan}");
    }

    // What stays open goes into both drafts, and the tone check looks for it.
    let file = section(text, "## 4. Intent file", "## 5. Draft the GitHub issue");
    assert!(
        file.contains("under **Risks**, marked **UNRESOLVED**"),
        "{file}"
    );
    let issue = section(text, "## 5. Draft the GitHub issue", "## 6. Sign-off");
    assert!(
        issue.contains("every UNRESOLVED adversary finding"),
        "{issue}"
    );
    assert!(issue.contains("(d) **Unresolved**"), "{issue}");
}

// ------------------------------------------------------------- renamed --

#[test]
fn a_renamed_adversary_and_implementer_are_named_in_both_commands() {
    let mut p = project(true);
    for a in &mut p.agents {
        match a.role.as_str() {
            "adversary" => a.name = "redteam".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let f = files(&p);
    assert!(f[DELIVER].contains("have `redteam` check the whole change"));
    assert!(f[DELIVER].contains("send `builder` only its Critical and High findings"));
    assert!(f[INTENT].contains("Give a fresh `redteam` subagent"));
    assert!(!f[DELIVER].contains("`adversary`") && !f[INTENT].contains("`adversary`"));
}

#[test]
fn without_an_implementer_findings_go_to_whoever_changed_the_code() {
    let mut p = project(true);
    p.agents.retain(|a| a.role != "implementer");
    let deliver = &files(&p)[DELIVER];
    assert!(
        deliver.contains("send the subagent that made the change only its Critical"),
        "{deliver}"
    );
    assert!(!deliver.contains("worktree branch"), "{deliver}");
}

// ---------------------------------------------------- without an adversary --

#[test]
fn without_an_adversary_both_commands_read_as_before() {
    let f = files(&project(false));
    assert!(!f[DELIVER].contains("Adversary") && !f[DELIVER].contains("adversary"));
    assert!(!f[INTENT].contains("adversary") && !f[INTENT].contains("UNRESOLVED"));
    // Pass 4 still goes to the explorer (preferred by default).
    assert!(f[INTENT].contains("Give a fresh `explorer` subagent the findings' claims"));
    // A disabled adversary is no adversary.
    let mut p = project(true);
    p.agents
        .iter_mut()
        .find(|a| a.role == "adversary")
        .unwrap()
        .disable = true;
    let g = files(&p);
    assert_eq!(f[DELIVER], g[DELIVER]);
    assert_eq!(f[INTENT], g[INTENT]);
}

// ------------------------------------------------------ other outputs --

#[test]
fn ukrainian_answers_keep_both_the_adversary_steps_and_the_reading_copy() {
    let mut p = project(true);
    p.response_language = "Ukrainian".into();
    let intent = &files(&p)[INTENT];
    assert!(intent.contains("Give a fresh `adversary` subagent"));
    assert!(intent.contains(".claude/intent/view/"));
}

#[test]
fn the_plugin_carries_the_adversary_steps_too() {
    let mut p = project(true);
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "cmd-kit".into();
    p.claude.plugin.version = "0.1.0".into();
    p.claude.plugin.display_name = "Cmd Kit".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let read = |rel: &str| fs::read_to_string(Path::new(dir.path()).join(rel)).unwrap();
    assert!(read("plugin/cmd-kit/skills/deliver/SKILL.md").contains("**Adversary check.**"));
    assert!(
        read("plugin/cmd-kit/skills/intent/SKILL.md").contains("Give a fresh `adversary` subagent")
    );
}
