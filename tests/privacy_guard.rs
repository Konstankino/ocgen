//! The `privacy-guard` role: a read-only check of every change for personal (PII)
//! or health (PHI) data that could leak by accident, and the loop the coordinator
//! runs around it — only when the team has one, so projects without it render
//! exactly as before.

use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::archetype::Archetype;
use ocgen::claude::{Output, Team};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::tempdir;

/// The default team without the privacy guard.
const TEAM: [&str; 6] = [
    "coordinator",
    "explorer",
    "implementer",
    "scope-guard",
    "reviewer",
    "adversary",
];

/// A line only the coordinator's privacy loop writes.
const LOOP_MARK: &str = "`Verdict: LEAK`";

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn roles(guard: bool) -> Vec<&'static str> {
    let mut v = TEAM.to_vec();
    if guard {
        v.push("privacy-guard");
    }
    v
}

fn claude(lang: &str, guard: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "privacy".into();
    p.providers.clear();
    p.agents = roles(guard)
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, lang).unwrap())
        .collect();
    p
}

fn opencode(lang: &str, guard: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "privacy".into();
    p.agents = roles(guard)
        .iter()
        .map(|n| Agent::from_archetype(n, n, lang, "mac").unwrap())
        .collect();
    p
}

fn scaffold(p: &Project) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    dir
}

fn team_rule(p: &Project) -> String {
    read(scaffold(p).path(), ".claude/rules/ocgen-team.md")
}

fn coordinator_prompt(p: &Project) -> String {
    read(scaffold(p).path(), ".opencode/prompts/coordinator.txt")
}

// ------------------------------------------------------------- the role --

#[test]
fn the_archetype_seeds_a_read_only_claude_subagent() {
    let a = Agent::from_archetype_claude("privacy-guard", "privacy-guard", "English").unwrap();
    assert_eq!(a.role, "privacy-guard");
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.model, "opus");
    assert_eq!(a.tools, "Read, Grep, Glob, Bash");
    assert_eq!(a.disallowed_tools, "Edit, Write, NotebookEdit");
    assert_eq!(a.steps, Some(40));
    assert_eq!(a.effort, "high");
    assert_eq!(a.color, "purple");
    assert_eq!(a.isolation, "");
}

#[test]
fn the_archetype_seeds_an_opencode_subagent_that_runs_only_reads() {
    let a = Agent::from_archetype("privacy-guard", "privacy-guard", "English", "mac").unwrap();
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.color, "info");
    assert_eq!(a.steps, Some(40));
    for rule in [
        "edit: deny",
        "\"*\": deny",
        "\"git diff*\": allow",
        "\"git log*\": allow",
        "\"git status*\": allow",
        "\"git grep*\": allow",
        "webfetch: deny",
    ] {
        assert!(a.permissions.contains(rule), "{rule} in {}", a.permissions);
    }
    // The model must be one the shipped provider offers, and not the implementer's.
    let m = Manifest::load().unwrap();
    assert!(m.providers[0].models.iter().any(|x| x.id == a.model));
    let imp = Agent::from_archetype("implementer", "implementer", "English", "mac").unwrap();
    assert_ne!(a.model, imp.model);
}

#[test]
fn it_is_not_in_the_default_team() {
    // The wizard adds it only for a project that handles personal or health data.
    let m = Manifest::load().unwrap();
    assert!(m.pipeline.iter().all(|p| p.archetype != "privacy-guard"));
}

#[test]
fn the_description_names_the_role_in_both_languages_and_is_plain_yaml() {
    let arch = Archetype::load("privacy-guard").unwrap();
    let uk = arch.description_for("Ukrainian");
    let en = arch.description_for("English");
    assert!(
        en.starts_with("Checks every change for personal or health data"),
        "{en}"
    );
    assert!(
        uk.starts_with("Перевіряє кожну зміну на персональні чи медичні дані"),
        "{uk}"
    );
    for d in [&uk, &en] {
        assert!(d.contains("PII/PHI"), "{d}");
        // Written unquoted into YAML frontmatter.
        assert!(!d.contains(": ") && !d.contains(" #"), "{d}");
    }
}

#[test]
fn the_body_fixes_the_report_format_in_both_languages() {
    let arch = Archetype::load("privacy-guard").unwrap();
    let uk = arch.body_for("Ukrainian");
    let en = arch.body_for("English");
    assert_ne!(uk, en);
    for body in [&uk, &en] {
        for must in [
            "Verdict: CLEAN | LEAK",
            "Findings:",
            "Not checked:",
            "file:line",
            "Verified",
            "Inferred",
            "Confidence: NN%",
            "`.env`",
        ] {
            assert!(body.contains(must), "missing {must:?} in:\n{body}");
        }
    }
    // The report itself must not spread the data it finds.
    assert!(en.contains("never the value"), "{en}");
    assert!(uk.contains("ніколи не значення"), "{uk}");
    // Accidents, not attackers — the threat model every ocgen role shares.
    assert!(en.contains("accidents, not attackers"), "{en}");
    assert!(uk.contains("випадковості, а не зловмисники"), "{uk}");
}

#[test]
fn the_ukrainian_texts_have_no_russian_letters() {
    let arch = Archetype::load("privacy-guard").unwrap();
    let rule = team_rule(&claude("Ukrainian", true));
    let dir = scaffold(&claude("Ukrainian", true));
    let deliver = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    for text in [
        arch.body_for("Ukrainian"),
        arch.description_for("Ukrainian"),
        rule,
        deliver,
    ] {
        for c in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
            assert!(!text.contains(c), "{c:?} in:\n{text}");
        }
    }
}

// ------------------------------------------------------------ Claude Code --

#[test]
fn claude_writes_the_privacy_guard_agent_file() {
    let dir = scaffold(&claude("English", true));
    let md = read(dir.path(), ".claude/agents/privacy-guard.md");
    for line in [
        "name: privacy-guard\n",
        "description: Checks every change for personal or health data",
        "tools: Read, Grep, Glob, Bash\n",
        "model: opus\n",
        "color: purple\n",
        "maxTurns: 40\n",
        "disallowedTools: Edit, Write, NotebookEdit\n",
        "effort: high\n",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    assert!(!md.contains("isolation:"));
}

#[test]
fn claude_coordinator_runs_the_privacy_loop() {
    let rule = team_rule(&claude("English", true));
    let coord = rule.split("## Coordination").nth(1).expect("coordination");
    for must in [
        "Privacy check (privacy-guard)",
        "even one that doesn't look like it touches personal data",
        "Never paste personal or health data",
        "`Verdict: CLEAN`",
        LOOP_MARK,
        "send implementer only the Critical and High findings",
        "At most 2 rework rounds",
        "UNRESOLVED",
        // The implementer works in a worktree: its branch must be merged first.
        "implementer works in its own git worktree",
        "merge its branch",
    ] {
        assert!(coord.contains(must), "missing {must:?} in:\n{coord}");
    }
    assert!(!rule.contains("{{") && !rule.contains("{%"), "{rule}");
}

#[test]
fn the_privacy_check_runs_after_the_scope_check_and_before_the_adversary() {
    let rule = team_rule(&claude("English", true));
    let scope = rule.find("Scope check (scope-guard)").expect("scope loop");
    let privacy = rule.find("Privacy check (privacy-guard)").expect("loop");
    let adversary = rule.find("Adversary check (adversary)").expect("adversary");
    assert!(scope < privacy && privacy < adversary, "{rule}");
    assert!(
        rule.contains("after scope-guard and before adversary, call privacy-guard"),
        "{rule}"
    );
    // A blank line between the loops, none doubled.
    assert!(rule.contains(".\n\nPrivacy check"), "{rule}");
    assert!(rule.contains(".\n\nAdversary check"), "{rule}");
}

#[test]
fn without_the_other_checks_the_loop_names_neither() {
    let mut p = claude("English", true);
    p.agents
        .retain(|a| a.role != "scope-guard" && a.role != "adversary");
    let rule = team_rule(&p);
    assert!(rule.contains("Privacy check (privacy-guard)"), "{rule}");
    let coord = rule.split("## Coordination").nth(1).unwrap();
    assert!(
        !coord.contains("scope-guard") && !coord.contains("adversary"),
        "{coord}"
    );
}

#[test]
fn claude_loop_speaks_ukrainian() {
    let rule = team_rule(&claude("Ukrainian", true));
    for must in [
        "Перевірка приватності (privacy-guard)",
        LOOP_MARK,
        "`Verdict: CLEAN`",
        "Не більше 2 кіл доопрацювання",
        "злий його гілку",
    ] {
        assert!(rule.contains(must), "missing {must:?} in:\n{rule}");
    }
}

#[test]
fn deliver_runs_the_privacy_check() {
    let dir = scaffold(&claude("English", true));
    let deliver = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    for must in [
        "**Privacy check.**",
        "have `privacy-guard` check the whole change",
        "the privacy loop in `.claude/rules/ocgen-team.md`",
        "`Verdict: LEAK`",
        "At most 2 rework rounds",
        "From the privacy check",
    ] {
        assert!(deliver.contains(must), "missing {must:?} in:\n{deliver}");
    }
    let privacy = deliver.find("**Privacy check.**").unwrap();
    assert!(deliver.find("**Conformance check.**").unwrap() < privacy);
    assert!(privacy < deliver.find("**Adversary check.**").unwrap());
}

#[test]
fn deliver_without_the_guard_is_unchanged() {
    let deliver = |guard| {
        let dir = scaffold(&claude("English", guard));
        read(dir.path(), ".claude/skills/deliver/SKILL.md")
    };
    let without = deliver(false);
    assert!(!without.contains("Privacy check") && !without.contains("privacy"));
    assert_ne!(without, deliver(true));
}

#[test]
fn claude_plugin_carries_the_loop_too() {
    let mut p = claude("English", true);
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "privacy-kit".into();
    p.claude.plugin.version = "0.1.0".into();
    p.claude.plugin.display_name = "Privacy Kit".into();
    let dir = scaffold(&p);
    assert!(read(dir.path(), "plugin/privacy-kit/rules/ocgen-team.md").contains(LOOP_MARK));
    assert!(dir
        .path()
        .join("plugin/privacy-kit/agents/privacy-guard.md")
        .is_file());
}

#[test]
fn a_renamed_guard_and_implementer_are_named_in_the_loop() {
    let mut p = claude("English", true);
    for a in &mut p.agents {
        match a.role.as_str() {
            "privacy-guard" => a.name = "pii-check".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let rule = team_rule(&p);
    assert!(rule.contains("Privacy check (pii-check)"), "{rule}");
    assert!(
        rule.contains("call pii-check on the whole change"),
        "{rule}"
    );
    assert!(rule.contains("send builder only"), "{rule}");
}

#[test]
fn a_team_without_an_implementer_hands_leaks_to_whoever_changed_the_code() {
    let mut p = claude("English", true);
    p.agents.retain(|a| a.role != "implementer");
    let rule = team_rule(&p);
    assert!(
        rule.contains("send the subagent that made the change only the Critical"),
        "{rule}"
    );
}

#[test]
fn the_guard_is_a_read_only_role_for_both_gates() {
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
    let dir = scaffold(&p);
    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    for var in ["SUBAGENT_READONLY_ROLES", "TEAM_READONLY_ROLES"] {
        let ro = s["env"][var].as_str().unwrap_or_default();
        assert!(
            ro.split_whitespace().any(|r| r == "privacy-guard"),
            "{var}: {ro:?}"
        );
    }
}

// --------------------------------------------------------------- OpenCode --

#[test]
fn opencode_writes_the_guard_and_lets_the_coordinator_call_it() {
    let dir = scaffold(&opencode("English", true));
    let md = read(dir.path(), ".opencode/agents/privacy-guard.md");
    for line in ["mode: subagent\n", "steps: 40\n", "  edit: deny\n"] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    let coord = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(coord.contains("\"privacy-guard\": allow"), "{coord}");
}

#[test]
fn opencode_coordinator_prompt_runs_the_loops_in_order() {
    let prompt = coordinator_prompt(&opencode("English", true));
    let scope = prompt.find("Scope check (scope-guard)").expect("scope");
    let privacy = prompt.find("Privacy check (privacy-guard)").expect("loop");
    let adversary = prompt.find("Adversary check (adversary)").expect("adv");
    assert!(scope < privacy && privacy < adversary, "{prompt}");
    assert!(prompt.contains(LOOP_MARK), "{prompt}");
    // OpenCode has no worktree isolation.
    assert!(!prompt.contains("worktree:"), "{prompt}");
    assert!(prompt.ends_with('\n') && !prompt.ends_with("\n\n"));
}

#[test]
fn a_disabled_guard_turns_the_loop_off() {
    let mut p = opencode("English", true);
    p.agents
        .iter_mut()
        .find(|a| a.role == "privacy-guard")
        .unwrap()
        .disable = true;
    let prompt = coordinator_prompt(&p);
    assert!(
        !prompt.contains(LOOP_MARK) && !prompt.contains("Privacy check"),
        "{prompt}"
    );
}

// ------------------------------------------------- existing projects --

#[test]
fn a_team_without_the_guard_renders_as_before() {
    for lang in ["English", "Ukrainian"] {
        let rule = team_rule(&claude(lang, false));
        assert!(
            !rule.contains(LOOP_MARK) && !rule.contains("privacy"),
            "{rule}"
        );
        let prompt = coordinator_prompt(&opencode(lang, false));
        assert!(!prompt.contains(LOOP_MARK) && !prompt.contains("privacy"));
    }
}

#[test]
fn adding_the_guard_to_an_existing_project_adds_the_loop() {
    for (p, file) in [
        (claude("English", false), ".claude/rules/ocgen-team.md"),
        (
            opencode("English", false),
            ".opencode/prompts/coordinator.txt",
        ),
    ] {
        let dir = scaffold(&p);
        assert!(!read(dir.path(), file).contains(LOOP_MARK));

        // What `ocgen add agent` does: reload the state, append, re-render.
        let mut loaded = Project::load_state(dir.path()).unwrap();
        let guard = match loaded.target {
            Target::ClaudeCode => {
                Agent::from_archetype_claude("privacy-guard", "privacy-guard", "English").unwrap()
            }
            Target::OpenCode => {
                Agent::from_archetype("privacy-guard", "privacy-guard", "English", "mac").unwrap()
            }
        };
        loaded.agents.push(guard);
        loaded.scaffold(dir.path(), true).unwrap();
        let text = read(dir.path(), file);
        assert!(text.contains(LOOP_MARK), "{file}");
        // Appended last in the roster, the loop still runs before the adversary.
        assert!(
            text.find("Privacy check").unwrap() < text.find("Adversary check").unwrap(),
            "{text}"
        );
    }
}
