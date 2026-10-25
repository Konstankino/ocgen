//! The `/brief` workflow skill: an interview informed by the code, a dry run of
//! the target skill for what it would still ask, a live page; the brief ends
//! with the command to type, and `/deliver`, `/inquire` and `/workstreams` read
//! it first and skip what it settles.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::claude::{Output, Skill};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::Value;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/brief/SKILL.md";
const RULE: &str = ".claude/rules/ocgen-workflow.md";
const STATE: &str = ".claude/.ocgen-state.json";
const TARGETS: [&str; 3] = ["deliver", "inquire", "workstreams"];

/// Exactly the tools /brief may use without asking: `git rev-parse`, for the
/// worktree check and the brief's `Commit:` line. The brief is written with the
/// Write tool under the project's own rule; no Write or Edit here, which the
/// skill rules flag on a model-invocable skill.
const BRIEF_TOOLS: [&str; 1] = ["Bash(git rev-parse:*)"];

fn claude(name: &str) -> Project {
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

/// A project with exactly these targets on.
fn with_targets(deliver: bool, inquire: bool, workstreams: bool) -> Project {
    let mut p = claude("br");
    p.claude.workflow.deliver = deliver;
    p.claude.workflow.inquire = inquire;
    p.claude.workflow.workstreams = workstreams;
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn scaffolded(p: &Project) -> (tempfile::TempDir, String) {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    (dir, md)
}

fn settings(dir: &Path) -> Value {
    serde_json::from_str(&read(dir, ".claude/settings.json")).unwrap()
}

fn hook_groups(settings: &Value, event: &str) -> Vec<Value> {
    settings["hooks"][event]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// How many groups of `event` run the notes hook.
fn notes_groups(settings: &Value, event: &str) -> usize {
    hook_groups(settings, event)
        .iter()
        .filter(|g| g.to_string().contains("inquire-notes"))
        .count()
}

/// The SessionStart tip's command, if the project has one.
fn tip(settings: &Value) -> Option<String> {
    hook_groups(settings, "SessionStart")
        .into_iter()
        .find(|g| g.to_string().contains("Tip:"))
        .map(|g| {
            assert_eq!(g["matcher"], "startup");
            g["hooks"][0]["command"].as_str().unwrap().to_string()
        })
}

/// The skill's frontmatter lines (between the `---` fences).
fn frontmatter(md: &str) -> Vec<String> {
    md.strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn tools(md: &str) -> Vec<String> {
    frontmatter(md)
        .iter()
        .find_map(|l| l.strip_prefix("allowed-tools: ").map(String::from))
        .expect("allowed-tools")
        .split(", ")
        .map(String::from)
        .collect()
}

#[test]
fn brief_is_a_model_invocable_skill_with_exact_tools() {
    let (dir, md) = scaffolded(&claude("br"));
    let front = frontmatter(&md);
    assert_eq!(front.first().map(String::as_str), Some("name: brief"));
    // description first in the template: legacy-command cleanup reads it.
    assert!(front[1].starts_with("description: "), "{front:?}");
    assert!(
        front
            .iter()
            .any(|l| l.starts_with("when_to_use: Use when ")),
        "{front:?}"
    );
    assert!(
        !front
            .iter()
            .any(|l| l.starts_with("disable-model-invocation")),
        "Claude may run /brief when asked to prepare one: {front:?}"
    );
    let mut keys: Vec<&str> = front
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, _)| k)
        .collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate frontmatter keys: {front:?}");
    assert_eq!(tools(&md), BRIEF_TOOLS.map(String::from).to_vec());
    assert_eq!(
        ocgen::claude::BRIEF_TOOLS.to_vec(),
        BRIEF_TOOLS.to_vec(),
        "the shipped list is the pinned one"
    );
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    assert!(md.lines().count() < 500, "{} lines", md.lines().count());
    assert!(
        md.len() < 20_000,
        "{} bytes: compaction would cut it",
        md.len()
    );
    assert!(ocgen::claude::WORKFLOW_SKILLS.contains(&"brief"));
    assert!(!ocgen::claude::USER_RUN_WORKFLOWS.contains(&"brief"));

    // The skill rules ocgen checks in yours hold for it too.
    let field = |key: &str| {
        front
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")))
            .unwrap_or_default()
            .to_string()
    };
    let as_skill = Skill {
        name: field("name"),
        description: field("description"),
        when_to_use: field("when_to_use"),
        allowed_tools: field("allowed-tools"),
        body: md.clone(),
        ..Default::default()
    };
    let issues = ocgen::claude::skill_issues(&as_skill);
    assert!(issues.is_empty(), "{issues:?}");
    let issues = ocgen::claude::skill_file_issues(&dir.path().join(".claude/skills/brief"), &md);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn the_skill_teaches_the_loop() {
    let (_dir, md) = scaffolded(&claude("br"));
    for must in [
        "the slug is ASCII `a-z`, `0-9` and hyphens, in English, at most 80",
        // The dry run reads the target's skill where this one was loaded from.
        "${CLAUDE_SKILL_DIR}/../<target>/SKILL.md",
        "at most 3 rounds",
        "Write tool",
        "Read the brief again",
        "say the command and wait",
        "Target: deliver | inquire | workstreams",
        "## Open",
        "## Inferred",
        "`.claude/briefs/.gitignore`",
        "Hand-off: /<target> .claude/briefs/<slug>.md",
        "one question at a time",
        "cannot read the skill",
        "`/improve-prompt` technique",
        "## When the target is /inquire",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    // Resume comes first: compaction keeps a skill's head.
    let at = |s: &str| md.find(s).unwrap_or_else(|| panic!("missing {s:?}"));
    assert!(at("## 0. Resume, or start") < at("## 1. Route"));
    assert!(at("## 1. Route") < at("## 2. Skim"));
    assert!(at("## 4. Dry run") < at("## 5. Polish"));
    assert!(at("## 5. Polish") < at("## 6. Hand off"));
    // The word that opens the page is never an option's label.
    assert!(md.contains("Never label an option with the bare word `brief`"));
}

#[test]
fn the_project_roles_are_rendered_in() {
    let mut p = claude("br");
    p.claude.workflow.check_cmd = "cargo test".into();
    let (_dir, md) = scaffolded(&p);
    for must in [
        "`implementer`",
        "`scope-guard`",
        "`cargo test` unless this piece needs another",
        "(explorer subagents where useful)",
        "(explorer; it gets no conversation context)",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }

    // Without an adversary or a scope guard, no role is named; without a check
    // command, none is suggested; without the explorer, Explore does the reads.
    let mut p = claude("br");
    p.claude.workflow.prefer_explorer = false;
    p.agents
        .retain(|a| a.role != "scope-guard" && a.role != "adversary");
    let (_dir, md) = scaffolded(&p);
    for never in [
        "`implementer`",
        "`scope-guard`",
        "unless this piece needs another",
        "explorer subagents",
    ] {
        assert!(!md.contains(never), "{never:?} rendered without its role");
    }
    assert!(md.contains("(Explore subagents where useful)"), "{md}");
    assert!(md.contains("(Explore; it gets no conversation context)"));
}

/// The route, the `Target:` line and the dry run name exactly the targets the
/// project has; with none, there is no skill at all.
#[test]
fn the_targets_shape_the_route_and_the_dry_run() {
    let on = [true, false];
    for deliver in on {
        for inquire in on {
            for workstreams in on {
                let p = with_targets(deliver, inquire, workstreams);
                let case = format!("deliver={deliver} inquire={inquire} workstreams={workstreams}");
                if !(deliver || inquire || workstreams) {
                    let dir = tempdir().unwrap();
                    p.scaffold(dir.path(), false).unwrap();
                    assert!(p.claude.workflow.brief && !p.brief_enabled(), "{case}");
                    assert!(
                        !dir.path().join(".claude/skills/brief").exists(),
                        "{case}: a brief for no target"
                    );
                    continue;
                }
                assert!(p.brief_enabled(), "{case}");
                let (_dir, md) = scaffolded(&p);
                assert!(!md.contains("{{") && !md.contains("{%"), "{case}");
                let flags = [deliver, inquire, workstreams];
                let words: Vec<&str> = TARGETS
                    .iter()
                    .zip(flags)
                    .filter(|(_, on)| *on)
                    .map(|(w, _)| *w)
                    .collect();
                let line = format!("Target: {}", words.join(" | "));
                assert!(md.lines().any(|l| l == line), "{case}: no {line:?}\n{md}");
                let phases: Vec<&str> = [
                    "/deliver: phases 0–3",
                    "/inquire: orient and sharpen",
                    "/workstreams: step 1",
                ]
                .iter()
                .zip(flags)
                .filter(|(_, on)| *on)
                .map(|(w, _)| *w)
                .collect();
                let dry_run = format!("({})", phases.join("; "));
                assert!(md.contains(&dry_run), "{case}: no {dry_run:?}\n{md}");
                for (word, on) in TARGETS.iter().zip(flags) {
                    assert_eq!(
                        md.contains(&format!("- **{word}** —")),
                        on,
                        "{case}: the route and /{word}"
                    );
                    assert_eq!(
                        md.contains(&format!("`/{word}`")),
                        on,
                        "{case}: /{word} named"
                    );
                }
                assert_eq!(
                    md.contains("## When the target is /inquire"),
                    inquire,
                    "{case}"
                );
                assert_eq!(md.contains("## What I know"), inquire, "{case}");
                assert_eq!(md.contains("## Workstreams"), workstreams, "{case}");
            }
        }
    }
}

/// Each target reads a brief given by its path, and only by its path.
#[test]
fn the_targets_read_a_brief_first() {
    let (dir, _md) = scaffolded(&claude("br"));
    for name in TARGETS {
        let md = read(dir.path(), &format!(".claude/skills/{name}/SKILL.md"));
        for must in [
            "`/brief`",
            ".claude/briefs/<slug>.md",
            "with or without a leading `@`",
            "a brief written in a worktree stays in that worktree",
        ] {
            assert!(md.contains(must), "/{name} lacks {must:?}");
        }
        assert!(
            !md.contains("by its slug alone") && !md.contains("a bare slug"),
            "/{name}: no bare-slug resolution"
        );
    }
    let at = |md: &str, s: &str| md.find(s).unwrap_or_else(|| panic!("missing {s:?}"));

    // /deliver: the brief's Target line routes it, and phase 1 takes it as the
    // answers, before the goal would be rewritten.
    let deliver = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    for must in [
        "`Target:` line is its route — `deliver` continues with phase 1; `inquire` is `/inquire`'s brief and",
        "Read it first; its `Target:` line must name `deliver` (another target's brief belongs to that skill — say",
        "so and stop). Treat it as my answers: its Goal, Scope, Success criteria (keep their slugs),",
        "`Status: draft` is usable — say it is unfinished.",
    ] {
        assert!(deliver.contains(must), "/deliver lacks {must:?}");
    }
    assert!(
        at(&deliver, "Treat it as my answers")
            < at(&deliver, "Rewrite the goal into a crisp brief")
    );

    // /inquire: the brief bullet precedes the arguments bullet (the first match
    // applies), and it never writes the user's first question.
    let inquire = read(dir.path(), ".claude/skills/inquire/SKILL.md");
    assert!(at(&inquire, "- **With a brief**") < at(&inquire, "- **With arguments:**"));
    for must in [
        "never ask for a topic or a first question",
        "its First question as step 1 says",
        "seed the **Mental model** as the brief marks them",
    ] {
        assert!(inquire.contains(must), "/inquire lacks {must:?}");
    }

    // /workstreams: the brief's split is the proposal, and an open board gets
    // the same question as any other argument.
    let ws = read(dir.path(), ".claude/skills/workstreams/SKILL.md");
    for must in [
        "Its Workstreams section is the proposed split — slug, files, `After:`, worker kind,",
        "as the argument gets the same question: add its goal to",
        "then write the",
    ] {
        assert!(ws.contains(must), "/workstreams lacks {must:?}");
    }
    assert!(at(&ws, "## 1. Plan the workstreams") < at(&ws, "Its Workstreams section"));
    assert!(at(&ws, "Its Workstreams section") < at(&ws, "Each workstream gets:"));

    // Off: no skill, and no target mentions it.
    let mut p = claude("br");
    p.claude.workflow.brief = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/brief").exists());
    for name in TARGETS {
        let md = read(dir.path(), &format!(".claude/skills/{name}/SKILL.md"));
        assert!(!md.contains("`/brief`"), "/{name} names /brief with it off");
        assert!(
            !md.contains(".claude/briefs"),
            "/{name} names the folder with it off"
        );
    }
}

#[test]
fn the_rule_explains_briefs() {
    let (dir, _md) = scaffolded(&claude("br"));
    let rule = read(dir.path(), RULE);
    for must in [
        "## Briefs",
        "`/<target> .claude/briefs/<slug>.md`",
        "at most three rounds",
        "`brief` typed alone shows it",
        "never starts the target itself",
        "never writes the user's questions for",
        "run `/brief` yourself: it writes nothing but the brief.",
    ] {
        assert!(rule.contains(must), "the rule lacks {must:?}");
    }
    let at = |s: &str| rule.find(s).unwrap_or_else(|| panic!("missing {s:?}"));
    assert!(at("## Codebase inquiry") < at("## Briefs"));
    assert!(at("## Briefs") < at("## Intents and issues"));
    assert!(
        rule.contains("Before `/deliver`, `/inquire` or `/workstreams` starts cold"),
        "{rule}"
    );

    // The rule names the project's own targets only.
    let (dir, _md) = scaffolded(&with_targets(true, false, true));
    let rule = read(dir.path(), RULE);
    assert!(
        rule.contains("Before `/deliver` or `/workstreams` starts cold"),
        "{rule}"
    );
    assert!(!rule.contains("/inquire"), "{rule}");
    let (dir, _md) = scaffolded(&with_targets(false, true, false));
    let rule = read(dir.path(), RULE);
    assert!(rule.contains("Before `/inquire` starts cold"), "{rule}");
    assert!(!rule.contains("/workstreams"), "{rule}");

    let mut p = claude("br");
    p.claude.workflow.brief = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let rule = read(dir.path(), RULE);
    assert!(!rule.contains("## Briefs"));
    assert!(!rule.contains(".claude/briefs"));
    assert!(rule.contains("## Codebase inquiry") && rule.contains("## Intents and issues"));
}

#[test]
fn the_session_tip_names_brief() {
    const BRIEF_TIP: &str =
        "run /brief to write the brief for /deliver, /inquire or /workstreams before starting one.";
    let (dir, _md) = scaffolded(&claude("br"));
    let cmd = tip(&settings(dir.path())).expect("the tip");
    assert!(cmd.contains(BRIEF_TIP), "{cmd}");
    assert!(
        cmd.contains("/intake") && cmd.contains("/improve-prompt"),
        "{cmd}"
    );

    // /brief alone carries the tip.
    let mut p = claude("br");
    p.claude.workflow.intake = false;
    p.claude.workflow.improve_prompt = false;
    let (dir, _md) = scaffolded(&p);
    let cmd = tip(&settings(dir.path())).expect("the tip for /brief alone");
    assert_eq!(cmd, format!("echo 'Tip: {BRIEF_TIP}'"));

    // The tip names the project's own targets only.
    p.claude.workflow.workstreams = false;
    let (dir, _md) = scaffolded(&p);
    let cmd = tip(&settings(dir.path())).unwrap();
    assert!(
        cmd.contains("the brief for /deliver or /inquire before"),
        "{cmd}"
    );
    p.claude.workflow.inquire = false;
    let (dir, _md) = scaffolded(&p);
    let cmd = tip(&settings(dir.path())).unwrap();
    assert!(cmd.contains("the brief for /deliver before"), "{cmd}");
    assert!(!cmd.contains("/inquire") && !cmd.contains("/workstreams"));

    // Off: no /brief in the tip, and no tip without the other two.
    let mut p = claude("br");
    p.claude.workflow.brief = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let cmd = tip(&settings(dir.path())).expect("the tip");
    assert!(!cmd.contains("/brief"), "{cmd}");
    p.claude.workflow.intake = false;
    p.claude.workflow.improve_prompt = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert_eq!(tip(&settings(dir.path())), None);
}

/// /brief alone: the notes hook opens the page after each write, answers the
/// word, and names the briefs folder to the file watcher; it checks no language
/// versions (a brief has one). Without it and the other notes users, the hook
/// isn't registered at all.
#[test]
fn the_hook_table_registers_the_brief_hooks() {
    let mut p = claude("br");
    p.claude.workflow.inquire = false;
    p.claude.workflow.recap = false;
    p.claude.workflow.intent = false;
    p.claude.workflow.workstreams = false;
    let (dir, _md) = scaffolded(&p);
    let s = settings(dir.path());
    for (event, want) in [
        ("PostToolUse", 1),
        ("UserPromptSubmit", 1),
        ("SessionStart", 1),
        ("CwdChanged", 1),
        ("FileChanged", 1),
        ("Stop", 0),
        ("SubagentStop", 0),
    ] {
        assert_eq!(notes_groups(&s, event), want, "{event}");
    }
    // The watcher groups take every session start and every change.
    for event in ["SessionStart", "CwdChanged", "FileChanged"] {
        let group = hook_groups(&s, event)
            .into_iter()
            .find(|g| g.to_string().contains("inquire-notes"))
            .unwrap();
        assert!(group.get("matcher").is_none(), "{event}: {group}");
        let cmd = group["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("ocgen hook inquire-notes"), "{cmd}");
        assert_eq!(group["hooks"][0]["shell"], "bash");
    }
    assert!(dir.path().join(".claude/hooks/inquire-notes.sh").is_file());
    // Both hooks that watch folders answer SessionStart: the lists must agree.
    p.claude.workflow.intent = true;
    let (dir, _md) = scaffolded(&p);
    let s = settings(dir.path());
    for event in ["SessionStart", "CwdChanged", "FileChanged"] {
        assert_eq!(notes_groups(&s, event), 1, "{event}");
        assert!(
            hook_groups(&s, event)
                .iter()
                .any(|g| g.to_string().contains("intent-draft")),
            "{event}"
        );
    }

    // Off, with everything else that uses the hook off: no group anywhere.
    let mut p = claude("br");
    p.claude.workflow.brief = false;
    p.claude.workflow.inquire = false;
    p.claude.workflow.recap = false;
    p.claude.workflow.intent = false;
    p.claude.workflow.workstreams = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let s = settings(dir.path());
    assert!(!s.to_string().contains("inquire-notes"), "{s}");
    assert!(!dir.path().join(".claude/hooks/inquire-notes.sh").exists());
}

/// What the brief refers back to is named by slugs: the skill carries no codes.
#[test]
fn the_skill_names_things_by_slugs() {
    let (_dir, md) = scaffolded(&claude("br"));
    assert!(md.contains("kebab-case slugs"), "{md}");
    // The same code pattern tests/scaffold.rs applies to every generated file.
    let code = regex::Regex::new(
        r"\b[A-Z][0-9]{1,2}\b|\b[A-Z]#|\b[A-Z][0-9]–[A-Z]n\b|\*\*[A-Z]\.\*\*|\bOption [A-Z]\b",
    )
    .unwrap();
    let found: Vec<&str> = code.find_iter(&md).map(|m| m.as_str()).collect();
    assert!(found.is_empty(), "{found:?}");
}

/// A state from before /brief loads with it on: `doctor` adds the skill.
#[test]
fn an_old_state_backfills_brief() {
    let dir = tempdir().unwrap();
    let mut p = claude("br");
    p.claude.workflow.brief = false;
    p.scaffold(dir.path(), false).unwrap();
    let path = dir.path().join(STATE);
    let mut v: Value = serde_json::from_str(&read(dir.path(), STATE)).unwrap();
    let workflow = v["claude"]["workflow"]
        .as_object_mut()
        .expect("the workflow block");
    assert_eq!(workflow.remove("brief"), Some(Value::Bool(false)));
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let loaded = Project::load_state(dir.path()).unwrap();
    assert!(loaded.claude.workflow.brief, "an old state gains /brief");
    assert!(loaded.brief_enabled());
    // An explicit choice is kept.
    p.scaffold(dir.path(), true).unwrap();
    let kept = Project::load_state(dir.path()).unwrap();
    assert!(!kept.claude.workflow.brief);
}

/// A user's own `brief` skill keeps its name: nothing generated points at it.
#[test]
fn a_user_skill_named_brief_disables_the_generated_one() {
    let mut p = claude("br");
    p.skills.push(Skill {
        name: "brief".into(),
        description: "My own brief writer — use when I ask for a brief of my kind".into(),
        body: "Write my kind of brief.".into(),
        ..Default::default()
    });
    assert!(p.claude.workflow.brief && !p.brief_enabled());
    assert!(
        p.issues()
            .iter()
            .any(|i| i.contains("skill 'brief'") && i.contains("collides")),
        "{:?}",
        p.issues()
    );
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    assert!(md.contains("Write my kind of brief."), "{md}");
    assert!(
        !md.contains("Dry run") && !md.contains("${CLAUDE_SKILL_DIR}"),
        "{md}"
    );
    for name in TARGETS {
        let md = read(dir.path(), &format!(".claude/skills/{name}/SKILL.md"));
        assert!(
            !md.contains("`/brief`"),
            "/{name} hands off to the user's skill"
        );
        assert!(!md.contains(".claude/briefs"), "/{name}");
    }
    assert!(!read(dir.path(), RULE).contains("## Briefs"));
    let s = settings(dir.path());
    assert!(!tip(&s).unwrap().contains("/brief"));
    for event in ["SessionStart", "CwdChanged", "FileChanged"] {
        assert_eq!(notes_groups(&s, event), 0, "{event}");
    }
}

/// A `/brief` command written by hand is shadowed by the generated skill; one an
/// older ocgen wrote is `doctor`'s to remove, and with /brief off nothing shadows.
#[test]
fn a_hand_made_brief_command_is_reported() {
    let dir = tempdir().unwrap();
    let p = claude("br");
    p.scaffold(dir.path(), false).unwrap();
    assert!(p.skill_file_issues(dir.path()).is_empty());
    let command = dir.path().join(".claude/commands/brief.md");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    let mine = "---\ndescription: My brief\n---\nWrite a brief for $ARGUMENTS\n";
    fs::write(&command, mine).unwrap();
    let issues = p.skill_file_issues(dir.path());
    assert!(
        issues
            .iter()
            .any(|i| i.contains("hand-made /brief command") && i.contains("shadowed")),
        "{issues:?}"
    );
    assert!(issues.iter().all(|i| !i.contains("R1")), "{issues:?}");

    // ocgen's own legacy command opens with the generated description.
    let desc = read(dir.path(), SKILL)
        .lines()
        .find(|l| l.starts_with("description: "))
        .unwrap()
        .to_string();
    fs::write(&command, format!("---\n{desc}\n---\nold\n")).unwrap();
    assert!(p.skill_file_issues(dir.path()).is_empty());

    fs::write(&command, mine).unwrap();
    let mut off = claude("br");
    off.claude.workflow.brief = false;
    assert!(off.skill_file_issues(dir.path()).is_empty());
}

#[test]
fn the_plugin_carries_the_skill() {
    let mut p = claude("br");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "br".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(dir.path().join("plugin/br/skills/brief/SKILL.md").is_file());
    let readme = read(dir.path(), "plugin/br/README.md");
    assert!(readme.contains("`/brief`"), "{readme}");
    let hooks: Value =
        serde_json::from_str(&read(dir.path(), "plugin/br/hooks/hooks.json")).unwrap();
    for event in ["SessionStart", "CwdChanged", "FileChanged"] {
        assert_eq!(notes_groups(&hooks, event), 1, "{event}");
    }

    p.claude.workflow.brief = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join("plugin/br/skills/brief").exists());
    assert!(!read(dir.path(), "plugin/br/README.md").contains("/brief"));
}
