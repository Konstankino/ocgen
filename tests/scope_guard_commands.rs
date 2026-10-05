//! /deliver and /intent use the project's scope guard when it has one: /deliver
//! numbers the plan's criteria, adds a Scope line, and has the guard check the
//! finished change against the plan in both directions before any review; /intent
//! has it check the plan itself before I see it. Without a guard both commands
//! read exactly as before.

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

fn project(guard: bool, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "cmds".into();
    p.providers.clear();
    p.agents = LEGACY
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, "English").unwrap())
        .collect();
    if guard {
        p.agents
            .push(Agent::from_archetype_claude("scope-guard", "scope-guard", "English").unwrap());
    }
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
fn deliver_plans_a_scope_and_numbered_criteria() {
    let f = files(&project(true, false));
    let plan = section(&f[DELIVER], "## 3. Plan", "## 4. Research");
    for must in [
        "Number the success criteria too",
        "a **Scope** line",
        "what is out of scope",
        "`scope-guard` checks the change against",
    ] {
        assert!(plan.contains(must), "missing {must:?} in:\n{plan}");
    }
}

#[test]
fn deliver_has_the_guard_check_the_change_before_any_review() {
    let f = files(&project(true, true));
    let execute = section(&f[DELIVER], "## 5. Execute", "## 6. Synthesize");
    for must in [
        "Before the first task, note the base",
        "git status --porcelain=v2 --branch --untracked-files=all",
        "**Scope check.**",
        "have `scope-guard` check the whole change against the plan in both directions",
        "merge `implementer`'s worktree branch",
        "after a re-plan, the newest one",
        "if phases 1–3 collapsed, the brief",
        "its plan and acceptance criteria",
        "revert each one yourself",
        "undo any revert that breaks one",
        "`Verdict: INCOMPLETE`",
        "send `implementer` only the Partial and Missing items",
        "At most 1 completion round",
        "**not done**",
        "Differs",
        ".claude/rules/ocgen-team.md",
    ] {
        assert!(execute.contains(must), "missing {must:?} in:\n{execute}");
    }
    // The scope check comes first; the adversary then checks the trimmed, complete change.
    let scope = execute.find("**Scope check.**").unwrap();
    let adversary = execute.find("**Adversary check.**").unwrap();
    assert!(scope < adversary, "{execute}");
    assert!(
        execute.contains("and before the adversary check"),
        "{execute}"
    );

    let synth = section(&f[DELIVER], "## 6. Synthesize", "For several independent");
    for must in [
        "plan coverage",
        "blast radius",
        "every cut",
        "Differs",
        "Defer list",
    ] {
        assert!(synth.contains(must), "missing {must:?} in:\n{synth}");
    }
}

#[test]
fn deliver_without_an_adversary_never_mentions_one() {
    let f = files(&project(true, false));
    assert!(f[DELIVER].contains("**Scope check.**"));
    assert!(!f[DELIVER].contains("adversary") && !f[DELIVER].contains("Adversary"));
    assert!(!f[DELIVER].contains("UNRESOLVED"));
}

// -------------------------------------------------------------- /intent --

#[test]
fn intent_has_the_guard_check_the_plan_in_both_directions() {
    let f = files(&project(true, true));
    let plan = section(
        &f[INTENT],
        "## 3. Plan and get my approval",
        "## Writing standard",
    );
    for must in [
        "give `scope-guard` the problem, the findings and the plan",
        "**Gaps:**",
        "every acceptance criterion and every risk needs a task",
        "**Excess:**",
        "**Deferred by the scope check**",
        "Never defer a risk's mitigation",
        "**blast radius**",
    ] {
        assert!(plan.contains(must), "missing {must:?} in:\n{plan}");
    }
    // Before the adversary checks the plan.
    let scope = plan.find("give `scope-guard`").unwrap();
    let adversary = plan.find("give `adversary` the plan").unwrap();
    assert!(scope < adversary, "{plan}");

    let file = section(
        &f[INTENT],
        "## 4. Intent file",
        "## 5. Draft the GitHub issue",
    );
    assert!(
        file.contains("predicted blast radius under **Consequences**"),
        "{file}"
    );
    assert!(file.contains("**Assumption**"), "{file}");
}

// ------------------------------------------------------------- renamed --

#[test]
fn a_renamed_guard_and_implementer_are_named_in_both_commands() {
    let mut p = project(true, true);
    for a in &mut p.agents {
        match a.role.as_str() {
            "scope-guard" => a.name = "tracer".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let f = files(&p);
    assert!(f[DELIVER].contains("have `tracer` check the whole change"));
    assert!(f[DELIVER].contains("send `builder` only the Partial and Missing items"));
    assert!(f[INTENT].contains("give `tracer` the problem"));
    assert!(!f[DELIVER].contains("`scope-guard`") && !f[INTENT].contains("`scope-guard`"));
}

#[test]
fn without_an_implementer_gaps_go_to_whoever_changed_the_code() {
    let mut p = project(true, false);
    p.agents.retain(|a| a.role != "implementer");
    let deliver = &files(&p)[DELIVER];
    assert!(
        deliver.contains("send the subagent that made the change only the Partial and Missing"),
        "{deliver}"
    );
    assert!(!deliver.contains("worktree branch"), "{deliver}");
}

// ----------------------------------------------------- without a guard --

#[test]
fn without_a_guard_both_commands_read_as_before() {
    for adversary in [false, true] {
        let f = files(&project(false, adversary));
        assert!(!f[DELIVER].contains("Scope check") && !f[DELIVER].contains("**Scope**"));
        assert!(!f[INTENT].contains("scope check") && !f[INTENT].contains("blast radius"));
        // A disabled guard is no guard: byte for byte.
        let mut p = project(true, adversary);
        p.agents
            .iter_mut()
            .find(|a| a.role == "scope-guard")
            .unwrap()
            .disable = true;
        let g = files(&p);
        assert_eq!(f[DELIVER], g[DELIVER]);
        assert_eq!(f[INTENT], g[INTENT]);
    }
}

/// With no scope guard, nothing checked the finished change against the plan:
/// /deliver does it itself before reporting, and loops back on a miss.
#[test]
fn without_a_guard_deliver_checks_the_criteria_before_reporting() {
    for adversary in [false, true] {
        let f = files(&project(false, adversary));
        let execute = section(&f[DELIVER], "## 5. Execute", "## 6. Synthesize");
        for must in [
            "**Final check.**",
            "every success criterion",
            "run the tests",
            "go back to the task",
            "at most 2 rounds",
            "**not done**",
            // The tests must see the isolated implementer's change.
            "after merging `implementer`'s worktree branch",
        ] {
            assert!(execute.contains(must), "missing {must:?} in:\n{execute}");
        }
        if adversary {
            // The adversary then checks the finished change, as the scope check does.
            let fin = execute.find("**Final check.**").unwrap();
            let adv = execute.find("**Adversary check.**").unwrap();
            assert!(fin < adv, "{execute}");
            assert!(
                execute.contains("and before the adversary check"),
                "{execute}"
            );
        }
    }
    // An implementer in the main checkout has nothing to merge.
    let mut p = project(false, false);
    p.agents
        .iter_mut()
        .find(|a| a.role == "implementer")
        .unwrap()
        .isolation
        .clear();
    assert!(!files(&p)[DELIVER].contains("after merging"));
    // The guard's scope check already does it: no second check.
    let f = files(&project(true, true));
    assert!(!f[DELIVER].contains("**Final check.**"));
}

// ------------------------------------------------------ other outputs --

#[test]
fn ukrainian_answers_keep_the_scope_steps() {
    let mut p = project(true, false);
    p.response_language = "Ukrainian".into();
    let f = files(&p);
    assert!(f[DELIVER].contains("**Scope check.**"));
    assert!(f[INTENT].contains("give `scope-guard` the problem"));
}

#[test]
fn the_plugin_carries_the_scope_steps_too() {
    let mut p = project(true, false);
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
    assert!(read("plugin/cmd-kit/skills/deliver/SKILL.md").contains("**Scope check.**"));
    assert!(
        read("plugin/cmd-kit/skills/intent/SKILL.md").contains("give `scope-guard` the problem")
    );
}
