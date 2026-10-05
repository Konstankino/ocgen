//! The `adversary` role: a read-only skeptic that hunts accidents in the
//! team's results, and the loop the coordinator runs around it — only when the team
//! has one, so projects without it render exactly as before.

use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::archetype::Archetype;
use ocgen::claude::{Output, Team};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::tempdir;

/// The team every project had before the adversary existed.
const LEGACY: [&str; 4] = ["coordinator", "explorer", "implementer", "reviewer"];

/// A line only the coordinator's adversary loop writes.
const LOOP_MARK: &str = "`Verdict: REWORK`";

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn claude(lang: &str, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "adv".into();
    p.providers.clear();
    p.agents = LEGACY
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, lang).unwrap())
        .collect();
    if adversary {
        p.agents
            .push(Agent::from_archetype_claude("adversary", "adversary", lang).unwrap());
    }
    p
}

fn opencode(lang: &str, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "adv".into();
    p.agents = LEGACY
        .iter()
        .map(|n| Agent::from_archetype(n, n, lang, "mac").unwrap())
        .collect();
    if adversary {
        p.agents
            .push(Agent::from_archetype("adversary", "adversary", lang, "mac").unwrap());
    }
    p
}

fn team_rule(p: &Project) -> String {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    read(dir.path(), ".claude/rules/ocgen-team.md")
}

fn coordinator_prompt(p: &Project) -> String {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    read(dir.path(), ".opencode/prompts/coordinator.txt")
}

// ------------------------------------------------------------- the role --

#[test]
fn the_archetype_seeds_a_read_only_claude_subagent() {
    let a = Agent::from_archetype_claude("adversary", "adversary", "English").unwrap();
    assert_eq!(a.role, "adversary");
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.model, "opus");
    assert_eq!(a.tools, "Read, Grep, Glob, Bash");
    assert_eq!(a.disallowed_tools, "Edit, Write, NotebookEdit");
    assert_eq!(a.steps, Some(40));
    assert_eq!(a.effort, "high");
    assert_eq!(a.color, "red");
    assert_eq!(a.isolation, "");
}

#[test]
fn the_archetype_seeds_an_opencode_subagent_on_another_model() {
    let a = Agent::from_archetype("adversary", "adversary", "English", "mac").unwrap();
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.model, "devstral-24b");
    assert_eq!(a.color, "error");
    assert_eq!(a.steps, Some(40));
    for rule in [
        "edit: deny",
        "\"*\": ask",
        "\"git push*\": deny",
        "\"git diff*\": allow",
        "webfetch: deny",
    ] {
        assert!(a.permissions.contains(rule), "{rule} in {}", a.permissions);
    }
    // The model must be one the shipped provider offers.
    let m = Manifest::load().unwrap();
    assert!(m.providers[0].models.iter().any(|x| x.id == a.model));
}

#[test]
fn the_description_names_the_role_in_both_languages_and_is_plain_yaml() {
    let arch = Archetype::load("adversary").unwrap();
    let uk = arch.description_for("Ukrainian");
    let en = arch.description_for("English");
    assert!(
        uk.starts_with("Скептик — ") && uk.contains("випадковості"),
        "{uk}"
    );
    assert!(
        en.starts_with("Skeptic — ") && en.contains("accidents"),
        "{en}"
    );
    for d in [&uk, &en] {
        // Written unquoted into YAML frontmatter.
        assert!(!d.contains(": ") && !d.contains(" #"), "{d}");
    }
}

#[test]
fn the_body_fixes_the_report_format_in_both_languages() {
    let arch = Archetype::load("adversary").unwrap();
    let uk = arch.body_for("Ukrainian");
    let en = arch.body_for("English");
    assert_ne!(uk, en);
    for body in [&uk, &en] {
        for must in [
            "Verdict: PASS | REWORK",
            "Findings:",
            "- [Severity] file:line",
            "Verified | Inferred (Confidence: NN%)",
            "Challenged claims:",
            "Not checked:",
            "Critical",
            "High",
            "Medium",
            "Low",
            "mktemp -d",
        ] {
            assert!(body.contains(must), "missing {must:?} in:\n{body}");
        }
        // The PoC lives in the report, never in the work tree.
        assert!(!body.contains("tests/poc_"), "{body}");
    }
    assert!(uk.contains("Модель загрози — випадковості"));
    assert!(en.contains("Threat model: accidents"));
}

// ------------------------------------------------------------ Claude Code --

#[test]
fn claude_writes_the_adversary_agent_file() {
    let dir = tempdir().unwrap();
    claude("English", true).scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/agents/adversary.md");
    for line in [
        "name: adversary\n",
        "description: Skeptic — checks",
        "tools: Read, Grep, Glob, Bash\n",
        "model: opus\n",
        "color: red\n",
        "maxTurns: 40\n",
        "disallowedTools: Edit, Write, NotebookEdit\n",
        "effort: high\n",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    assert!(!md.contains("isolation:"));
}

#[test]
fn claude_coordinator_runs_the_adversary_loop() {
    let rule = team_rule(&claude("English", true));
    let coord = rule.split("## Coordination").nth(1).expect("coordination");
    for must in [
        "Adversary check (adversary)",
        "call adversary last",
        LOOP_MARK,
        "send implementer only the Critical and High findings",
        "call adversary again",
        "At most 2 rework rounds",
        "3 calls to adversary",
        "UNRESOLVED",
        // The implementer works in a worktree: its branch must be merged first.
        "implementer works in its own git worktree",
        "merge its branch",
    ] {
        assert!(coord.contains(must), "missing {must:?} in:\n{coord}");
    }
    assert!(!rule.contains("{{") && !rule.contains("{%"), "{rule}");
    // The coordinator's own prompt is still there, ahead of the loop.
    assert!(coord.find("Your subagents").unwrap() < coord.find("Adversary check").unwrap());
    // The loop ends the section the way the coordinator's prompt did.
    let trailing = |s: &str| s.len() - s.trim_end_matches('\n').len();
    assert_eq!(
        trailing(&rule),
        trailing(&team_rule(&claude("English", false)))
    );
}

#[test]
fn claude_loop_speaks_ukrainian() {
    let rule = team_rule(&claude("Ukrainian", true));
    for must in [
        "Перевірка adversary (adversary)",
        LOOP_MARK,
        "Не більше 2 кіл доопрацювання",
        "UNRESOLVED",
        "злий його гілку",
    ] {
        assert!(rule.contains(must), "missing {must:?} in:\n{rule}");
    }
}

#[test]
fn claude_plugin_carries_the_loop_too() {
    let mut p = claude("English", true);
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "adv-kit".into();
    p.claude.plugin.version = "0.1.0".into();
    p.claude.plugin.display_name = "Adv Kit".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(read(dir.path(), "plugin/adv-kit/rules/ocgen-team.md").contains(LOOP_MARK));
    assert!(dir
        .path()
        .join("plugin/adv-kit/agents/adversary.md")
        .is_file());
}

#[test]
fn a_renamed_adversary_and_implementer_are_named_in_the_loop() {
    let mut p = claude("English", true);
    for a in &mut p.agents {
        match a.role.as_str() {
            "adversary" => a.name = "redteam".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let rule = team_rule(&p);
    assert!(rule.contains("Adversary check (redteam)"), "{rule}");
    assert!(rule.contains("call redteam last"), "{rule}");
    assert!(rule.contains("send builder only"), "{rule}");
}

#[test]
fn a_team_without_an_implementer_hands_findings_to_whoever_changed_the_code() {
    let mut p = claude("English", true);
    p.agents.retain(|a| a.role != "implementer");
    let rule = team_rule(&p);
    assert!(
        rule.contains("send the subagent that made the change only"),
        "{rule}"
    );
    assert!(!rule.contains("worktree:"), "{rule}");
}

#[test]
fn claude_loop_without_a_coordinator_still_gets_a_coordination_section() {
    let mut p = claude("English", true);
    p.agents.retain(|a| a.mode != "primary");
    let rule = team_rule(&p);
    let coord = rule.split("## Coordination").nth(1).expect("coordination");
    assert!(coord.contains(LOOP_MARK), "{rule}");
}

#[test]
fn the_adversary_is_a_read_only_role_for_both_gates() {
    let mut p = claude("English", true);
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.claude.workflow.subagent_confidence = 96;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    for var in ["SUBAGENT_READONLY_ROLES", "TEAM_READONLY_ROLES"] {
        let ro = s["env"][var].as_str().unwrap_or_default();
        assert!(
            ro.split_whitespace().any(|r| r == "adversary"),
            "{var}: {ro:?}"
        );
        assert!(!ro.split_whitespace().any(|r| r == "implementer"), "{var}");
    }
}

// --------------------------------------------------------------- OpenCode --

#[test]
fn opencode_writes_the_adversary_agent_and_lets_the_coordinator_call_it() {
    let dir = tempdir().unwrap();
    opencode("English", true)
        .scaffold(dir.path(), false)
        .unwrap();
    let md = read(dir.path(), ".opencode/agents/adversary.md");
    for line in [
        "mode: subagent\n",
        "model: mac/devstral-24b\n",
        "steps: 40\n",
        "color: error\n",
        "  edit: deny\n",
        "    \"git push*\": deny\n",
        "  webfetch: deny\n",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    let coord = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(coord.contains("\"adversary\": allow"), "{coord}");
}

#[test]
fn opencode_coordinator_prompt_ends_with_the_loop() {
    let prompt = coordinator_prompt(&opencode("English", true));
    let (head, tail) = prompt
        .split_once("Adversary check (adversary)")
        .expect("loop");
    assert!(head.contains("Respond in English.\n\n"), "{prompt}");
    // One server, one agent at a time — the loop doesn't change that.
    assert!(head.contains("do not run in parallel"));
    for must in [
        LOOP_MARK,
        "send implementer only",
        "At most 2 rework rounds",
    ] {
        assert!(tail.contains(must), "missing {must:?} in:\n{tail}");
    }
    // OpenCode has no worktree isolation: nothing to merge.
    assert!(!tail.contains("worktree"), "{tail}");
    assert!(prompt.ends_with('\n') && !prompt.ends_with("\n\n"));
}

#[test]
fn opencode_coordinator_without_a_prompt_file_gets_the_loop_in_its_body() {
    let mut p = opencode("English", true);
    let boss = p.agents.iter_mut().find(|a| a.mode == "primary").unwrap();
    boss.prompt_file = false;
    boss.prompt_body = None;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(md.contains(LOOP_MARK), "{md}");
    assert!(md.ends_with('\n') && !md.ends_with("\n\n"));
    assert!(!dir
        .path()
        .join(".opencode/prompts/coordinator.txt")
        .exists());
}

#[test]
fn a_disabled_adversary_turns_the_loop_off() {
    let mut p = opencode("English", true);
    p.agents
        .iter_mut()
        .find(|a| a.role == "adversary")
        .unwrap()
        .disable = true;
    assert!(!coordinator_prompt(&p).contains(LOOP_MARK));
}

// ------------------------------------------------- existing projects --

#[test]
fn a_team_without_the_adversary_renders_as_before() {
    let rule = team_rule(&claude("English", false));
    assert!(!rule.contains(LOOP_MARK) && !rule.contains("Adversary check"));
    let prompt = coordinator_prompt(&opencode("English", false));
    assert!(prompt.ends_with("Respond in English.\n"), "{prompt}");
    assert!(!prompt.contains("adversary"));
}

#[test]
fn adding_the_adversary_to_an_existing_project_adds_the_loop() {
    for (p, file) in [
        (claude("English", false), ".claude/rules/ocgen-team.md"),
        (
            opencode("English", false),
            ".opencode/prompts/coordinator.txt",
        ),
    ] {
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        assert!(!read(dir.path(), file).contains(LOOP_MARK));

        // What `ocgen add agent` does: reload the state, append, re-render.
        let mut loaded = Project::load_state(dir.path()).unwrap();
        let adv = match loaded.target {
            Target::ClaudeCode => {
                Agent::from_archetype_claude("adversary", "adversary", "English").unwrap()
            }
            Target::OpenCode => {
                Agent::from_archetype("adversary", "adversary", "English", "mac").unwrap()
            }
        };
        loaded.agents.push(adv);
        loaded.scaffold(dir.path(), true).unwrap();
        assert!(read(dir.path(), file).contains(LOOP_MARK), "{file}");
    }
}
