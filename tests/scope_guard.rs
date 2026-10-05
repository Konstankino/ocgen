//! The `scope-guard` role: a read-only check that the change does everything the
//! plan asks and nothing it doesn't, with its blast radius stated plainly, and the
//! loop the coordinator runs around it — only when the team has one, so projects
//! without it render exactly as before.

use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::archetype::Archetype;
use ocgen::claude::{Output, Team};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::tempdir;

/// The team every project had before the scope guard and the adversary existed.
const LEGACY: [&str; 4] = ["coordinator", "explorer", "implementer", "reviewer"];

/// A line only the coordinator's scope loop writes.
const LOOP_MARK: &str = "`Verdict: INCOMPLETE`";

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn roles(guard: bool, adversary: bool) -> Vec<&'static str> {
    let mut v = LEGACY.to_vec();
    if guard {
        v.push("scope-guard");
    }
    if adversary {
        v.push("adversary");
    }
    v
}

fn claude(lang: &str, guard: bool, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "scope".into();
    p.providers.clear();
    p.agents = roles(guard, adversary)
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, lang).unwrap())
        .collect();
    p
}

fn opencode(lang: &str, guard: bool, adversary: bool) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "scope".into();
    p.agents = roles(guard, adversary)
        .iter()
        .map(|n| Agent::from_archetype(n, n, lang, "mac").unwrap())
        .collect();
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
    let a = Agent::from_archetype_claude("scope-guard", "scope-guard", "English").unwrap();
    assert_eq!(a.role, "scope-guard");
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.model, "sonnet");
    assert_eq!(a.tools, "Read, Grep, Glob, Bash");
    assert_eq!(a.disallowed_tools, "Edit, Write, NotebookEdit");
    assert_eq!(a.steps, Some(30));
    assert_eq!(a.effort, "high");
    assert_eq!(a.color, "green");
    assert_eq!(a.isolation, "");
}

#[test]
fn the_archetype_seeds_an_opencode_subagent_that_runs_only_git_reads() {
    let a = Agent::from_archetype("scope-guard", "scope-guard", "English", "mac").unwrap();
    assert_eq!(a.mode, "subagent");
    assert_eq!(a.model, "gemma-26b");
    assert_eq!(a.color, "success");
    assert_eq!(a.steps, Some(30));
    for rule in [
        "edit: deny",
        "\"*\": deny",
        "\"git diff*\": allow",
        "\"git log*\": allow",
        "\"git status*\": allow",
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
fn the_description_names_the_role_in_both_languages_and_is_plain_yaml() {
    let arch = Archetype::load("scope-guard").unwrap();
    let uk = arch.description_for("Ukrainian");
    let en = arch.description_for("English");
    assert!(en.starts_with("Checks the change against the plan"), "{en}");
    assert!(uk.starts_with("Звіряє зміну з планом"), "{uk}");
    for d in [&uk, &en] {
        // Written unquoted into YAML frontmatter.
        assert!(!d.contains(": ") && !d.contains(" #"), "{d}");
    }
}

#[test]
fn the_body_fixes_the_report_format_in_both_languages() {
    let arch = Archetype::load("scope-guard").unwrap();
    let uk = arch.body_for("Ukrainian");
    let en = arch.body_for("English");
    assert_ne!(uk, en);
    for body in [&uk, &en] {
        for must in [
            "Verdict: CLEAN | TRIM | INCOMPLETE",
            "Plan:",
            "Done:",
            "Partial:",
            "Missing",
            "Differs:",
            "Also needed:",
            "Cut:",
            "Defer:",
            "Blast radius:",
            "Not checked:",
            "git diff --stat <base>",
            "--untracked-files=all",
            "git log -n 100 --name-only --format= <base>",
            "Confidence: NN%",
        ] {
            assert!(body.contains(must), "missing {must:?} in:\n{body}");
        }
    }
    assert!(en.contains("nothing it doesn't"), "{en}");
    assert!(uk.contains("нічого зайвого"), "{uk}");
}

#[test]
fn the_ukrainian_texts_have_no_russian_letters() {
    let arch = Archetype::load("scope-guard").unwrap();
    let rule = team_rule(&claude("Ukrainian", true, true));
    for text in [
        arch.body_for("Ukrainian"),
        arch.description_for("Ukrainian"),
        rule,
    ] {
        for c in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
            assert!(!text.contains(c), "{c:?} in:\n{text}");
        }
    }
}

// ------------------------------------------------------------ Claude Code --

#[test]
fn claude_writes_the_scope_guard_agent_file() {
    let dir = tempdir().unwrap();
    claude("English", true, false)
        .scaffold(dir.path(), false)
        .unwrap();
    let md = read(dir.path(), ".claude/agents/scope-guard.md");
    for line in [
        "name: scope-guard\n",
        "description: Checks the change against the plan",
        "tools: Read, Grep, Glob, Bash\n",
        "model: sonnet\n",
        "color: green\n",
        "maxTurns: 30\n",
        "disallowedTools: Edit, Write, NotebookEdit\n",
        "effort: high\n",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    assert!(!md.contains("isolation:"));
}

#[test]
fn claude_coordinator_runs_the_scope_loop() {
    let rule = team_rule(&claude("English", true, false));
    let coord = rule.split("## Coordination").nth(1).expect("coordination");
    for must in [
        "Scope check (scope-guard)",
        "git status --porcelain=v2 --branch --untracked-files=all",
        "change only what the task needs",
        "call scope-guard before any review",
        "the latest approved plan or brief, verbatim",
        "the decisions the user confirmed",
        "`Verdict: TRIM`",
        "the one edit you make yourself",
        "git checkout <base> -- <path>",
        "undo that revert",
        LOOP_MARK,
        "send implementer only the Partial and Missing items",
        "At most 1 completion round:",
        "not done, never as done",
        "Differs",
        "the blast radius",
        // The implementer works in a worktree: its branch must be merged first.
        "implementer works in its own git worktree",
        "merge its branch",
    ] {
        assert!(coord.contains(must), "missing {must:?} in:\n{coord}");
    }
    assert!(!rule.contains("{{") && !rule.contains("{%"), "{rule}");
    // Without an adversary or a formatter, neither is mentioned.
    assert!(!coord.contains("adversary"), "{coord}");
    assert!(!coord.contains("formatter"), "{coord}");
    // The coordinator's own prompt is still there, ahead of the loop.
    assert!(coord.find("Your subagents").unwrap() < coord.find("Scope check").unwrap());
    // The loop ends the section the way the coordinator's prompt did.
    let trailing = |s: &str| s.len() - s.trim_end_matches('\n').len();
    assert_eq!(
        trailing(&rule),
        trailing(&team_rule(&claude("English", false, false)))
    );
}

#[test]
fn the_scope_check_runs_before_the_adversary() {
    let rule = team_rule(&claude("English", true, true));
    let scope = rule.find("Scope check (scope-guard)").expect("scope loop");
    let adversary = rule
        .find("Adversary check (adversary)")
        .expect("adversary loop");
    assert!(scope < adversary, "{rule}");
    assert!(
        rule.contains("call scope-guard before any review and before adversary"),
        "{rule}"
    );
    // A blank line between the loops, none doubled.
    assert!(rule.contains(".\n\nAdversary check"), "{rule}");
}

#[test]
fn the_formatter_is_named_when_the_project_formats_on_edit() {
    let mut p = claude("English", true, false);
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    let rule = team_rule(&p);
    assert!(
        rule.contains("the formatter that runs after every edit (`cargo fmt`)"),
        "{rule}"
    );
}

#[test]
fn claude_loop_speaks_ukrainian() {
    let rule = team_rule(&claude("Ukrainian", true, false));
    for must in [
        "Перевірка обсягу (scope-guard)",
        LOOP_MARK,
        "`Verdict: TRIM`",
        "Не більше 1 кола доопрацювання",
        "злий його гілку",
        "git checkout <base> -- <path>",
    ] {
        assert!(rule.contains(must), "missing {must:?} in:\n{rule}");
    }
}

#[test]
fn claude_plugin_carries_the_loop_too() {
    let mut p = claude("English", true, false);
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "scope-kit".into();
    p.claude.plugin.version = "0.1.0".into();
    p.claude.plugin.display_name = "Scope Kit".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(read(dir.path(), "plugin/scope-kit/rules/ocgen-team.md").contains(LOOP_MARK));
    assert!(dir
        .path()
        .join("plugin/scope-kit/agents/scope-guard.md")
        .is_file());
}

#[test]
fn a_renamed_guard_and_implementer_are_named_in_the_loop() {
    let mut p = claude("English", true, false);
    for a in &mut p.agents {
        match a.role.as_str() {
            "scope-guard" => a.name = "tracer".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let rule = team_rule(&p);
    assert!(rule.contains("Scope check (tracer)"), "{rule}");
    assert!(rule.contains("call tracer before any review"), "{rule}");
    assert!(rule.contains("send builder only"), "{rule}");
}

#[test]
fn a_team_without_an_implementer_hands_gaps_to_whoever_changed_the_code() {
    let mut p = claude("English", true, false);
    p.agents.retain(|a| a.role != "implementer");
    let rule = team_rule(&p);
    assert!(
        rule.contains("send the subagent that made the change only"),
        "{rule}"
    );
    assert!(!rule.contains("worktree:"), "{rule}");
}

#[test]
fn the_guard_is_a_read_only_role_for_both_gates() {
    let mut p = claude("English", true, false);
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
            ro.split_whitespace().any(|r| r == "scope-guard"),
            "{var}: {ro:?}"
        );
    }
}

// --------------------------------------------------------------- OpenCode --

#[test]
fn opencode_writes_the_guard_and_lets_the_coordinator_call_it() {
    let dir = tempdir().unwrap();
    opencode("English", true, false)
        .scaffold(dir.path(), false)
        .unwrap();
    let md = read(dir.path(), ".opencode/agents/scope-guard.md");
    for line in [
        "mode: subagent\n",
        "model: mac/gemma-26b\n",
        "steps: 30\n",
        "color: success\n",
        "  edit: deny\n",
        "  webfetch: deny\n",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    let coord = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(coord.contains("\"scope-guard\": allow"), "{coord}");
}

#[test]
fn opencode_coordinator_prompt_ends_with_the_loops_in_order() {
    let prompt = coordinator_prompt(&opencode("English", true, true));
    let (head, tail) = prompt
        .split_once("Scope check (scope-guard)")
        .expect("loop");
    assert!(head.contains("Respond in English.\n\n"), "{prompt}");
    for must in [
        LOOP_MARK,
        "send implementer only",
        "At most 1 completion round",
    ] {
        assert!(tail.contains(must), "missing {must:?} in:\n{tail}");
    }
    assert!(tail.contains("Adversary check (adversary)"), "{tail}");
    // OpenCode has no worktree isolation and no format hook.
    assert!(
        !tail.contains("worktree:") && !tail.contains("formatter"),
        "{tail}"
    );
    assert!(prompt.ends_with('\n') && !prompt.ends_with("\n\n"));
}

#[test]
fn a_disabled_guard_turns_the_loop_off() {
    let mut p = opencode("English", true, false);
    p.agents
        .iter_mut()
        .find(|a| a.role == "scope-guard")
        .unwrap()
        .disable = true;
    let prompt = coordinator_prompt(&p);
    assert!(
        !prompt.contains(LOOP_MARK) && !prompt.contains("Scope check"),
        "{prompt}"
    );
}

// ------------------------------------------------- existing projects --

#[test]
fn a_team_without_the_guard_renders_as_before() {
    let rule = team_rule(&claude("English", false, true));
    assert!(!rule.contains(LOOP_MARK) && !rule.contains("Scope check"));
    let prompt = coordinator_prompt(&opencode("English", false, false));
    assert!(prompt.ends_with("Respond in English.\n"), "{prompt}");
    assert!(
        !prompt.contains(LOOP_MARK) && !prompt.contains("Scope check"),
        "{prompt}"
    );
}

#[test]
fn adding_the_guard_to_an_existing_project_adds_the_loop() {
    for (p, file) in [
        (
            claude("English", false, true),
            ".claude/rules/ocgen-team.md",
        ),
        (
            opencode("English", false, true),
            ".opencode/prompts/coordinator.txt",
        ),
    ] {
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        assert!(!read(dir.path(), file).contains(LOOP_MARK));

        // What `ocgen add agent` does: reload the state, append, re-render.
        let mut loaded = Project::load_state(dir.path()).unwrap();
        let guard = match loaded.target {
            Target::ClaudeCode => {
                Agent::from_archetype_claude("scope-guard", "scope-guard", "English").unwrap()
            }
            Target::OpenCode => {
                Agent::from_archetype("scope-guard", "scope-guard", "English", "mac").unwrap()
            }
        };
        loaded.agents.push(guard);
        loaded.scaffold(dir.path(), true).unwrap();
        let text = read(dir.path(), file);
        assert!(text.contains(LOOP_MARK), "{file}");
        // Appended last in the roster, the loop still runs it before the adversary.
        assert!(
            text.find("Scope check").unwrap() < text.find("Adversary check").unwrap(),
            "{text}"
        );
    }
}
