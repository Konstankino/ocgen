//! Before a change, /deliver's research lists the precedents the change must
//! follow — code to reuse, patterns to match, contracts callers rely on — and
//! hands them to the implementer; afterwards the project's reviewer checks the
//! change against them. How to write the list is a project rule, which
//! `ocgen doctor` keeps current, not a role prompt saved in the project's state.

use std::collections::BTreeMap;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

/// A Claude project with these agents: `(name, archetype)`.
fn project(team: &[(&str, &str)]) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "prec".into();
    p.providers.clear();
    p.agents = team
        .iter()
        .map(|(n, a)| Agent::from_archetype_claude(n, a, "English").unwrap())
        .collect();
    p
}

const FULL: [(&str, &str); 6] = [
    ("coordinator", "coordinator"),
    ("explorer", "explorer"),
    ("implementer", "implementer"),
    ("reviewer", "reviewer"),
    ("scope-guard", "scope-guard"),
    ("adversary", "adversary"),
];

fn without(drop: &str) -> Vec<(&'static str, &'static str)> {
    FULL.iter().copied().filter(|(n, _)| *n != drop).collect()
}

fn files(p: &Project) -> BTreeMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

const DELIVER: &str = ".claude/skills/deliver/SKILL.md";
const RULES: &str = ".claude/rules/ocgen-workflow.md";

/// The text from `from` up to the next `to`.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text.find(from).unwrap_or_else(|| panic!("no {from:?}"));
    let end = text[start + from.len()..]
        .find(to)
        .map_or(text.len(), |i| start + from.len() + i);
    &text[start..end]
}

fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{text}"))
}

// ------------------------------------------------------------------ rule --

#[test]
fn the_rules_say_how_to_write_a_precedent_list() {
    let f = files(&project(&FULL));
    let rule = section(&f[RULES], "## Precedents\n", "\n## ");
    for must in [
        "**reuse**",
        "**pattern**",
        "**contract**",
        "**Naming what you refer back to**",
        "`file:line`",
        "None found",
        "stated reason",
    ] {
        assert!(rule.contains(must), "missing {must:?}:\n{rule}");
    }
    // The pipeline summary names both ends: the list and the check.
    let pipeline = section(&f[RULES], "## Delivery pipeline\n", "\n## ");
    assert!(pipeline.contains("precedent list"), "{pipeline}");
    assert!(pipeline.contains("**Conformance check**"), "{pipeline}");
}

// ------------------------------------------------------------- /deliver --

#[test]
fn deliver_research_writes_the_precedent_list() {
    let f = files(&project(&FULL));
    let research = section(&f[DELIVER], "## 4. Research", "## 5.");
    for must in ["`explorer`", "**Precedents**", "re-plan"] {
        assert!(research.contains(must), "missing {must:?}:\n{research}");
    }
    // The implementer works from it.
    let execute = section(&f[DELIVER], "## 5. Execute", "**Scope check.**");
    assert!(execute.contains("precedents that bear on it"), "{execute}");

    // No explorer: the built-in Explore agent does the sweep.
    let f = files(&project(&without("explorer")));
    let research = section(&f[DELIVER], "## 4. Research", "## 5.");
    assert!(research.contains("built-in `Explore`"), "{research}");
    assert!(!research.contains("`explorer`"), "{research}");
}

#[test]
fn deliver_has_the_reviewer_check_conformance_between_scope_and_adversary() {
    let f = files(&project(&FULL));
    let md = &f[DELIVER];
    assert!(at(md, "**Scope check.**") < at(md, "**Conformance check.**"));
    assert!(at(md, "**Conformance check.**") < at(md, "**Adversary check.**"));
    let check = section(md, "**Conformance check.**", "**Adversary check.**");
    for must in [
        "`reviewer`",
        "Followed",
        "Broken",
        "Not applicable",
        "duplicates",
        "`implementer`",
        "At most one round",
        // The implementer works in its own worktree: its fixes are merged first.
        "Merge `implementer`'s worktree branch",
    ] {
        assert!(check.contains(must), "missing {must:?}:\n{check}");
    }
    let synth = section(md, "## 6. Synthesize", "\nFor several");
    assert!(synth.contains("precedent"), "{synth}");

    // Without a scope guard it follows the final check.
    let f = files(&project(&without("scope-guard")));
    let md = &f[DELIVER];
    assert!(at(md, "**Final check.**") < at(md, "**Conformance check.**"));
    assert!(at(md, "**Conformance check.**") < at(md, "**Adversary check.**"));
}

#[test]
fn deliver_without_a_reviewer_checks_conformance_itself() {
    let f = files(&project(&without("reviewer")));
    let md = &f[DELIVER];
    let check = section(md, "**Conformance check.**", "**Adversary check.**");
    assert!(check.contains("yourself"), "{check}");
    assert!(!check.contains("``"), "{check}");
    assert!(!check.contains("`reviewer`"), "{check}");
}

#[test]
fn deliver_finds_a_renamed_reviewer_by_role() {
    let mut team = without("reviewer");
    team.push(("critic", "reviewer"));
    let f = files(&project(&team));
    let check = section(&f[DELIVER], "**Conformance check.**", "**Adversary check.**");
    assert!(check.contains("`critic`"), "{check}");
}
