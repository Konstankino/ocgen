//! End-to-end tests of the non-interactive CLI commands via the built binary.
//! These exercise main dispatch, the landscape/fields/doctor output, and the
//! templates management commands (which the interactive wizard tests can't reach).

use std::path::Path;

use assert_cmd::Command;
use ocgen::agent::{self, Agent};
use ocgen::claude::{Output, Skill, Team, Workflow};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::tempdir;

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

fn scaffold_default(dir: &Path) {
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.project_name = "cli-test".into();
    p.agents = agent::default_pipeline("English", "mac").unwrap();
    p.scaffold(dir, false).unwrap();
}

fn scaffold_claude(dir: &Path) {
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "cli-claude".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir, false).unwrap();
}

#[test]
fn help_and_version() {
    ocgen()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("landscape"));
    ocgen().arg("--version").assert().success();
    ocgen().args(["templates", "--help"]).assert().success();
    ocgen().args(["add", "--help"]).assert().success();
    ocgen().args(["edit", "--help"]).assert().success();
}

#[test]
fn fields_reference_runs() {
    ocgen()
        .arg("fields")
        .assert()
        .success()
        .stdout(contains("temperature"))
        .stdout(contains("permissions"))
        .stdout(contains("Provider fields"));
    ocgen().arg("reference").assert().success(); // alias
}

#[test]
fn landscape_on_project() {
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Providers"))
        .stdout(contains("Agents"))
        .stdout(contains("Topology"))
        .stdout(contains("no problems found"));
    ocgen().arg("horizon").arg(dir.path()).assert().success(); // alias
}

#[test]
fn landscape_reports_and_doctor_fixes_issues() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.project_name = "broken".into();
    let mut a = Agent::blank("solo", "custom", "mac");
    a.mode = "primary".into();
    a.model = "ghost-model".into(); // not offered by provider
    p.agents = vec![a];
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Checks"))
        .stdout(contains("ghost-model"));

    ocgen()
        .arg("doctor")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("ghost-model"))
        .stdout(contains("file(s)"));

    // After doctor, the reference is repaired.
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("no problems found"));
}

#[test]
fn discover_from_subdirectory_and_missing_project() {
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());
    // Runs from a nested dir — should discover upward.
    let nested = dir.path().join(".opencode").join("agents");
    ocgen().arg("landscape").arg(&nested).assert().success();

    // A directory with no project errors clearly.
    let empty = tempdir().unwrap();
    ocgen()
        .arg("landscape")
        .arg(empty.path())
        .assert()
        .failure()
        .stderr(contains("no ocgen project"));
}

// Redirects the config dir via HOME, which only `dirs` honors on Unix.
#[cfg(unix)]
#[test]
fn templates_path_init_list() {
    let home = tempdir().unwrap();
    ocgen()
        .env("HOME", home.path())
        .args(["templates", "path"])
        .assert()
        .success()
        .stdout(contains(".config/ocgen/templates"));

    ocgen()
        .env("HOME", home.path())
        .args(["templates", "init"])
        .assert()
        .success();
    assert!(home
        .path()
        .join(".config/ocgen/templates/manifest.toml")
        .is_file());
    assert!(home
        .path()
        .join(".config/ocgen/templates/archetypes/reviewer.toml")
        .is_file());

    // With an override present, list marks it.
    ocgen()
        .env("HOME", home.path())
        .args(["templates", "list"])
        .assert()
        .success()
        .stdout(contains("override"));

    // Rendering through the override dir exercises the override branch of load():
    // doctor re-scaffolds, reading the (identical) override templates.
    let proj = tempdir().unwrap();
    scaffold_default(proj.path());
    ocgen()
        .env("HOME", home.path())
        .arg("doctor")
        .arg(proj.path())
        .assert()
        .success();
    // The generated files still match (override == embedded defaults here).
    assert!(proj.path().join("opencode.json").is_file());
}

/// Write an executable fake `$EDITOR` script into `dir` and return its path.
/// `body` runs with the file-to-edit as `$1`.
#[cfg(unix)]
fn fake_editor(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn new_advertises_target_and_output_flags() {
    ocgen()
        .args(["new", "--help"])
        .assert()
        .success()
        .stdout(contains("--target"))
        .stdout(contains("--output"))
        .stdout(contains("--repo"));
}

#[cfg(unix)]
#[test]
fn templates_edit_saves_changes_to_override() {
    let home = tempdir().unwrap();
    // Editor that appends a marker line to whatever file it is given.
    let editor = fake_editor(home.path(), "ed.sh", "echo '# edited-by-test' >> \"$1\"");

    ocgen()
        .env("HOME", home.path())
        .env("EDITOR", &editor)
        .args(["templates", "edit", "seeds.toml"])
        .assert()
        .success()
        .stdout(contains("seeds.toml"));

    // The override now exists and holds the edit on top of the embedded content.
    let overridden = home.path().join(".config/ocgen/templates/seeds.toml");
    let body = std::fs::read_to_string(&overridden).unwrap();
    assert!(body.contains("# edited-by-test"), "edit was saved");
    assert!(body.contains("[body]"), "seeded from the embedded default");
}

#[cfg(unix)]
#[test]
fn templates_edit_no_change_writes_nothing() {
    let home = tempdir().unwrap();
    // `true` leaves the temp file untouched → dialoguer reports no change.
    ocgen()
        .env("HOME", home.path())
        .env("EDITOR", "true")
        .args(["templates", "edit", "seeds.toml"])
        .assert()
        .success()
        .stdout(contains("No changes"));

    assert!(!home
        .path()
        .join(".config/ocgen/templates/seeds.toml")
        .exists());
}

#[test]
fn templates_edit_rejects_unknown_template() {
    let home = tempdir().unwrap();
    ocgen()
        .env("HOME", home.path())
        .args(["templates", "edit", "nope.toml"])
        .assert()
        .failure()
        .stderr(contains("unknown template"));
}

#[test]
fn new_advertises_optional_base_url_flag() {
    // The flag exists and is documented.
    ocgen()
        .args(["new", "--help"])
        .assert()
        .success()
        .stdout(contains("--base-url"));
}

#[test]
fn new_rejects_invalid_base_url_before_prompting() {
    // A bad URL fails fast (before any interactive prompt, so no TTY needed).
    ocgen()
        .args(["new", "--base-url", "not-a-url"])
        .assert()
        .failure()
        .stderr(contains("http"));
}

#[test]
fn show_agent_prints_config() {
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());

    // A subagent: shows steps, no task block, and "called by" the coordinator.
    ocgen()
        .args(["show", "agent", "implementer", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Configuration"))
        .stdout(contains("mac/qwen3-coder-30b"))
        .stdout(contains("steps"))
        .stdout(contains("Permissions"))
        .stdout(contains("System prompt"))
        .stdout(contains("called by"));

    // The primary: shows its delegates and the /multi command.
    ocgen()
        .args(["show", "agent", "coordinator", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("delegates to"))
        .stdout(contains("/multi"))
        .stdout(contains("\"explorer\": allow")); // effective task block

    // Unknown agent errors clearly.
    ocgen()
        .args(["show", "agent", "ghost", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("no agent named"));
}

#[test]
fn edit_errors_are_clear() {
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());

    ocgen()
        .args(["edit", "agent", "doesnotexist", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("no agent named"));

    ocgen()
        .args(["edit", "provider", "ghost", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("no provider with key"));
}

#[test]
fn landscape_and_show_on_claude_project() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Claude Code"))
        .stdout(contains("TOOLS"))
        .stdout(contains("Topology"))
        .stdout(contains("no problems found"));
    ocgen()
        .args(["show", "agent", "explorer", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("model: opus"))
        .stdout(contains("tools: Read, Grep, Glob"))
        .stdout(contains("coordinated by"));
}

#[test]
fn doctor_repairs_claude_project() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "broken".into();
    p.providers.clear();
    let mut a = Agent::blank("solo", "custom", "");
    a.mode = "subagent".into();
    a.model = "gpt-4".into(); // invalid alias
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("unknown model alias"));
    ocgen()
        .arg("doctor")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("opus"));
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("no problems found"));
}

#[test]
fn fields_includes_claude_section() {
    ocgen()
        .arg("fields")
        .assert()
        .success()
        .stdout(contains("Claude Code target fields"))
        .stdout(contains("model (alias or ID)"))
        .stdout(contains("tools"))
        .stdout(contains("agent teams"));
}

#[test]
fn add_skill_requires_a_claude_project() {
    // Skills are Claude-only: adding one to an OpenCode project fails clearly,
    // before any interactive prompt.
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());
    ocgen()
        .args(["add", "skill"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("Claude Code feature"));
}

#[test]
fn add_and_edit_help_list_skill() {
    ocgen()
        .args(["add", "--help"])
        .assert()
        .success()
        .stdout(contains("skill"));
    ocgen()
        .args(["edit", "--help"])
        .assert()
        .success()
        .stdout(contains("skill"));
}

#[test]
fn show_claude_coordinator_and_landscape_with_skills() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(Skill {
        name: "commit".into(),
        description: "Craft a commit".into(),
        allowed_tools: "Bash(git commit:*)".into(),
        body: "do it".into(),
        role: None,
        ..Default::default()
    });
    p.scaffold(dir.path(), false).unwrap();

    // Landscape prints the Skills section.
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Skills"))
        .stdout(contains("commit"));

    // Showing the coordinator uses the primary-agent branch.
    ocgen()
        .args(["show", "agent", "coordinator", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("coordinator"))
        .stdout(contains("delegates to"))
        .stdout(contains("CLAUDE.md"));
}

#[test]
fn claude_landscape_and_show_without_coordinator() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "flat".into();
    p.providers.clear();
    let mut a = Agent::blank("solo", "custom", "");
    a.mode = "subagent".into();
    a.model = "opus".into(); // tools intentionally empty → "(all)"
    a.description = "only worker".into();
    a.body = "b".into();
    p.agents = vec![a];
    p.scaffold(dir.path(), false).unwrap();

    // Empty tools render as "(all)"; no coordinator → no Topology section.
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("(all)"))
        .stdout(contains("no problems found"));

    ocgen()
        .args(["show", "agent", "solo", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("coordinated by"))
        .stdout(contains("no coordinator"))
        .stdout(contains("inherit all"));
}

#[test]
fn show_claude_coordinator_without_subagents() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "solo".into();
    p.providers.clear();
    let mut boss = Agent::blank("boss", "custom", "");
    boss.mode = "primary".into();
    boss.body = "coordinate".into();
    p.agents = vec![boss];
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .args(["show", "agent", "boss", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("delegates to"))
        .stdout(contains("(none)"))
        .stdout(contains("main session"));
}

#[test]
fn claude_landscape_plugin_opus_and_no_workflow_no_subagents() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.providers.clear();
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "kit".into();
    p.claude.model = "opus".into();
    p.claude.workflow = Workflow {
        intake: false,
        refine: false,
        improve_prompt: false,
        fanout: false,
        verify_todos: false,
        deliver: false,
        inquire: false,
        intent: false,
        loop_guard_max: 0,
        check_cmd: String::new(),
        subagent_confidence: 0,
    };
    let mut boss = Agent::blank("boss", "custom", "");
    boss.mode = "primary".into();
    boss.body = "coordinate".into();
    p.agents = vec![boss];
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("plugin")) // output includes plugin
        .stdout(contains("opus")) // explicit default model
        .stdout(contains("(none)")) // workflow disabled
        .stdout(contains("no subagents")); // coordinator with no team
}

#[test]
fn claude_landscape_and_show_empty_model_defaults_to_opus() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "e".into();
    p.providers.clear();
    let mut a = Agent::blank("w", "custom", ""); // model + body intentionally empty
    a.mode = "subagent".into();
    a.description = "d".into();
    p.agents = vec![a];
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("opus"));
    ocgen()
        .args(["show", "agent", "w", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("model: opus"))
        .stdout(contains("(none)")); // empty body
}

#[test]
fn edit_team_is_advertised_and_guards_target() {
    // The subcommand shows up in help.
    ocgen()
        .args(["edit", "team", "--help"])
        .assert()
        .success()
        .stdout(contains("Agent Teams"));
    // On an OpenCode project it errors before any prompt (target guard).
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());
    ocgen()
        .args(["edit", "team", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("Claude Code"));
}

#[test]
fn new_advertises_team_flag() {
    ocgen()
        .args(["new", "--help"])
        .assert()
        .success()
        .stdout(contains("--team"))
        .stdout(contains("--team-confidence"))
        .stdout(contains("--no-plan-gate"))
        .stdout(contains("--no-risk-rounds"))
        .stdout(contains("--no-approval-gate"));
}

#[test]
fn landscape_shows_agent_teams_when_enabled() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "teamed".into();
    p.providers.clear();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("agent teams"))
        .stdout(contains("in-process"))
        .stdout(contains("plan-gate"))
        .stdout(contains("≥96%"))
        .stdout(contains("risk rounds"))
        .stdout(contains("approval-gate"));
}

#[test]
fn landscape_warns_about_a_vague_skill() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(Skill {
        name: "helper".into(),
        description: "TF helper".into(),
        allowed_tools: "Bash".into(),
        body: "do it".into(),
        ..Default::default()
    });
    p.scaffold(dir.path(), false).unwrap();

    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("skill 'helper'"))
        .stdout(contains("too short to trigger"))
        .stdout(contains("Bash(git status:*)"));
}

#[test]
fn add_mcp_requires_a_claude_project_and_help_lists_it() {
    let dir = tempdir().unwrap();
    scaffold_default(dir.path());
    ocgen()
        .args(["add", "mcp"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("Claude Code feature"));
    ocgen()
        .args(["add", "--help"])
        .assert()
        .success()
        .stdout(contains("mcp"));
    ocgen()
        .args(["edit", "--help"])
        .assert()
        .success()
        .stdout(contains("mcp"));
}

#[test]
fn landscape_lists_mcp_servers() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.mcp_servers.push(ocgen::claude::McpServer {
        name: "tf".into(),
        transport: "stdio".into(),
        command: "npx".into(),
        ..Default::default()
    });
    p.scaffold(dir.path(), false).unwrap();
    ocgen()
        .arg("landscape")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("MCP servers"))
        .stdout(contains("npx"));
}

#[test]
fn doctor_dry_run_shows_the_plan_and_writes_nothing() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "dr".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir.path(), false).unwrap();
    std::fs::write(
        dir.path().join(".claude/settings.json"),
        "{\"model\":\"mine\"}\n",
    )
    .unwrap();

    ocgen()
        .args(["doctor", "--dry-run"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains(".claude/settings.json"))
        .stdout(contains("edited by hand"))
        .stdout(contains("settings.local.json"))
        .stdout(contains("dry run"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
        "{\"model\":\"mine\"}\n",
        "dry run leaves files alone"
    );
    assert!(!dir.path().join(".ocgen-backup").exists());

    ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains(".ocgen-backup/"));
    let backups: Vec<_> = std::fs::read_dir(dir.path().join(".ocgen-backup"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(backups[0].path().join(".claude/settings.json")).unwrap(),
        "{\"model\":\"mine\"}\n"
    );
}

#[test]
fn managed_settings_prints_a_deployable_policy() {
    ocgen()
        .arg("managed-settings")
        .assert()
        .success()
        .stdout(contains("disableBypassPermissionsMode"))
        // The deploy note goes to stderr so stdout stays pipeable JSON.
        .stderr(contains("managed-settings.json"));
}

#[test]
fn verify_exits_nonzero_on_failure() {
    let dir = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "vc".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir.path(), false).unwrap();
    ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("up to date"));
    std::fs::write(dir.path().join(".claude/statusline.sh"), "exit 3\n").unwrap();
    ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains("statusline"));
}

#[test]
fn approve_refuses_to_run_for_an_agent() {
    let dir = tempdir().unwrap();
    let home = tempdir().unwrap();
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = "ap".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir.path(), false).unwrap();

    // Under Claude Code (CLAUDECODE set) — refused, nothing written.
    ocgen()
        .arg("approve")
        .arg(dir.path())
        .env("HOME", home.path())
        .env("CLAUDECODE", "1")
        .assert()
        .failure()
        .stderr(contains("human"));
    // Not a terminal (piped, as any tool call would be) — refused too.
    ocgen()
        .arg("approve")
        .arg(dir.path())
        .env("HOME", home.path())
        .env_remove("CLAUDECODE")
        .assert()
        .failure()
        .stderr(contains("terminal"));
    assert!(!home.path().join(".claude/ocgen").exists());

    // Reading the status is always allowed.
    ocgen()
        .args(["approve", "--status"])
        .arg(dir.path())
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(contains("locked"));
}

#[test]
fn windows_installer_extracts_from_a_zip_named_file() {
    // Windows PowerShell's Expand-Archive rejects any path that doesn't end in
    // `.zip` — and New-TemporaryFile makes a `.tmp`, which broke the installer.
    let ps1 = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.ps1")).unwrap();
    assert!(
        !ps1.lines()
            .any(|l| !l.trim_start().starts_with('#') && l.contains("New-TemporaryFile")),
        "New-TemporaryFile yields a .tmp file that Expand-Archive refuses"
    );
    let assign = ps1
        .lines()
        .find(|l| l.trim_start().starts_with("$tmpZip ="))
        .expect("the installer names its download");
    assert!(
        assign.contains(".zip"),
        "the download must be saved as *.zip: {assign}"
    );
    assert!(ps1.contains("Expand-Archive"));
    assert!(
        ps1.is_ascii(),
        "keep the script ASCII: Windows PowerShell 5.1 misreads UTF-8 without a BOM"
    );
}

#[test]
fn help_is_not_tied_to_one_target() {
    // ocgen bootstraps for several tools: the description is generic…
    ocgen()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("bootstrapper for LLM multi-agent projects"));
    // …and `new` has no built-in target: the wizard asks when --target is omitted.
    ocgen()
        .args(["new", "--help"])
        .assert()
        .success()
        .stdout(contains("opencode"))
        .stdout(contains("claude"))
        .stdout(contains("[default: opencode]").not());
}

#[test]
fn edit_permissions_adds_and_removes_rules_from_flags() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let settings = |d: &Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(d.join(".claude/settings.json")).unwrap())
            .unwrap()
    };

    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args([
            "--allow",
            "Bash(gh run view:*)",
            "--allow",
            "Bash(gh run list:*)",
        ])
        .args(["--deny", "Read(./secrets/**)"])
        .assert()
        .success()
        .stdout(contains("Bash(gh run view:*)"));
    let s = settings(dir.path());
    let allow = s["permissions"]["allow"].as_array().unwrap();
    assert!(allow.contains(&"Bash(gh run view:*)".into()));
    assert!(allow.contains(&"Bash(gh run list:*)".into()));
    assert!(s["permissions"]["deny"]
        .as_array()
        .unwrap()
        .contains(&"Read(./secrets/**)".into()));

    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--remove", "Bash(gh run list:*)"])
        .assert()
        .success();
    let allow = settings(dir.path())["permissions"]["allow"].clone();
    assert!(!allow
        .as_array()
        .unwrap()
        .contains(&"Bash(gh run list:*)".into()));
    assert!(allow
        .as_array()
        .unwrap()
        .contains(&"Bash(gh run view:*)".into()));

    // An allow that a generated ask/deny rule overrides is accepted, with a warning.
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--allow", "Bash(git push:*)"])
        .assert()
        .success()
        .stdout(contains("no effect"));
}

#[test]
fn edit_permissions_rejects_bad_input_and_guards_target() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let before = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--allow", "Bash(gh"])
        .assert()
        .failure()
        .stderr(contains("Bash(gh"));
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--remove", "Bash(nothing:*)"])
        .assert()
        .failure()
        .stderr(contains("not one of"));
    let after = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    assert_eq!(before, after, "nothing written on error");

    // Without flags it is interactive, so with no terminal it says what to pass.
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("--allow"));

    // OpenCode permissions are per agent.
    let oc = tempdir().unwrap();
    scaffold_default(oc.path());
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(oc.path())
        .args(["--allow", "Read"])
        .assert()
        .failure()
        .stderr(contains("ocgen edit agent"));
}

#[test]
fn edit_permissions_help_explains_rules_and_shows_examples() {
    // The parent `edit` help points at it with an example too.
    ocgen()
        .args(["edit", "--help"])
        .assert()
        .success()
        .stdout(contains("Examples:"))
        .stdout(contains("ocgen edit permissions --allow"));
    // Examples show in both the short (-h) and the full (--help) help.
    ocgen()
        .args(["edit", "permissions", "-h"])
        .assert()
        .success()
        .stdout(contains("Examples:"));
    ocgen()
        .args(["edit", "permissions", "--help"])
        .assert()
        .success()
        .stdout(contains("Tool(specifier)"))
        .stdout(contains("deny, then ask, then allow"))
        .stdout(contains("Examples:"))
        .stdout(contains("ocgen edit permissions --remove"))
        .stdout(contains("--allow <RULE>"));
}

#[test]
fn edit_permissions_list_shows_every_rule_at_a_glance() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args([
            "--allow",
            "Bash(gh run view:*)",
            "--allow",
            "Bash(git push:*)",
        ])
        .assert()
        .success();
    let before = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();

    ocgen()
        .args(["edit", "permissions", "--list", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        // Every list, generated rules and yours, each with where it comes from.
        .stdout(contains("allow (10)"))
        .stdout(contains("ask (17)"))
        .stdout(contains("deny ("))
        .stdout(contains("Bash(ocgen approve*)"))
        .stdout(contains("Bash(gh run view:*)"))
        .stdout(contains("ocgen"))
        .stdout(contains("yours"))
        // An allow that a generated ask rule overrides is flagged.
        .stdout(contains("no effect"));
    let after = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    assert_eq!(before, after, "--list writes nothing");

    // Listing is read-only: it can't be combined with changes.
    ocgen()
        .args(["edit", "permissions", "--list", "--allow", "Read", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("--list"));
    // And it's in the examples.
    ocgen()
        .args(["edit", "permissions", "--help"])
        .assert()
        .success()
        .stdout(contains("ocgen edit permissions --list"));
}

#[test]
fn edit_intent_updates_settings_from_flags() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    ocgen()
        .args(["edit", "intent", "-p"])
        .arg(dir.path())
        .args(["--prefix", "RFC", "--digits", "3", "--dir", "docs/rfc"])
        .args(["--max-words", "200", "--branch", "trunk"])
        .assert()
        .success();
    let skill = std::fs::read_to_string(dir.path().join(".claude/skills/intent/SKILL.md")).unwrap();
    assert!(skill.contains("docs/rfc/RFC-001") && skill.contains("200 words"));
    assert!(skill.contains("trunk"));

    // --show prints the settings and writes nothing.
    ocgen()
        .args(["edit", "intent", "--show", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("RFC"))
        .stdout(contains("docs/rfc"))
        .stdout(contains("200"))
        .stdout(contains("docs/rfc/RFC-001-<slug>.md"))
        .stdout(contains(".claude/intent/issue-template.md"));

    // Off and on again.
    ocgen()
        .args(["edit", "intent", "--disable", "-p"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(!dir.path().join(".claude/skills/intent").exists());
    ocgen()
        .args(["edit", "intent", "--enable", "-p"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(dir.path().join(".claude/skills/intent/SKILL.md").exists());
}

#[test]
fn edit_intent_resets_a_template_to_the_default() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let issue = dir.path().join(".claude/intent/issue-template.md");
    std::fs::write(&issue, "mine\n").unwrap();
    ocgen()
        .args(["edit", "intent", "--reset-issue-template", "-p"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(std::fs::read_to_string(&issue)
        .unwrap()
        .contains("Acceptance criteria"));
}

#[cfg(unix)]
#[test]
fn edit_intent_opens_a_template_in_the_editor() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let home = tempdir().unwrap();
    let editor = fake_editor(home.path(), "ed.sh", "echo '## Edited by test' >> \"$1\"");
    ocgen()
        .args(["edit", "intent", "--intent-template", "-p"])
        .arg(dir.path())
        .env("EDITOR", &editor)
        .assert()
        .success();
    let t = std::fs::read_to_string(dir.path().join(".claude/intent/intent-template.md")).unwrap();
    assert!(
        t.contains("Status:") && t.contains("## Edited by test"),
        "{t}"
    );
}

#[test]
fn edit_intent_rejects_bad_values_and_guards_target() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let state = dir.path().join(".claude/.ocgen-state.json");
    let before = std::fs::read_to_string(&state).unwrap();
    ocgen()
        .args(["edit", "intent", "--prefix", "A-B", "--digits", "3", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("prefix"));
    ocgen()
        .args(["edit", "intent", "--dir", "../outside", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("dir"));
    assert_eq!(
        before,
        std::fs::read_to_string(&state).unwrap(),
        "nothing written"
    );

    // No flags and no terminal: say which flags to pass.
    ocgen()
        .args(["edit", "intent", "-p"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("--prefix"));

    let oc = tempdir().unwrap();
    scaffold_default(oc.path());
    ocgen()
        .args(["edit", "intent", "--show", "-p"])
        .arg(oc.path())
        .assert()
        .failure()
        .stderr(contains("Claude Code"));
}

#[test]
fn edit_intent_help_has_examples() {
    ocgen()
        .args(["edit", "intent", "--help"])
        .assert()
        .success()
        .stdout(contains("Examples:"))
        .stdout(contains("ocgen edit intent --prefix"))
        .stdout(contains("--max-words"));
    ocgen()
        .args(["edit", "--help"])
        .assert()
        .success()
        .stdout(contains("intent"));
}

#[test]
fn doctor_keeps_permission_rules_added_by_hand() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    let path = dir.path().join(".claude/settings.json");
    let mut s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for r in ["Bash(gh run view:*)", "Bash(head:*)"] {
        s["permissions"]["allow"]
            .as_array_mut()
            .unwrap()
            .push(r.into());
    }
    std::fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    // The dry run lists them and writes nothing.
    ocgen()
        .args(["doctor", "--dry-run"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("added by hand"))
        .stdout(contains("Bash(head:*)"))
        .stdout(contains("dry run"));
    assert_eq!(before, std::fs::read_to_string(&path).unwrap());

    // --yes keeps them as your rules: in settings.json and in the state.
    ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("kept 2 rule(s)"));
    let allow = |d: &Path| -> Vec<String> {
        let s: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(d.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        s["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    assert!(allow(dir.path()).contains(&"Bash(gh run view:*)".to_string()));
    let state = std::fs::read_to_string(dir.path().join(".claude/.ocgen-state.json")).unwrap();
    assert!(state.contains("Bash(head:*)"), "saved as your rule");

    // A second doctor has nothing to change in settings.json.
    ocgen()
        .args(["doctor", "--dry-run"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("added by hand").not())
        .stdout(contains("every generated file is up to date"));
    assert!(allow(dir.path()).contains(&"Bash(head:*)".to_string()));
}

#[test]
fn doctor_help_has_examples() {
    ocgen()
        .args(["doctor", "--help"])
        .assert()
        .success()
        .stdout(contains("Examples:"))
        .stdout(contains("ocgen doctor --dry-run"))
        .stdout(contains("hand-added permission rules"));
}

#[test]
fn version_flag_reports_the_build_version() {
    // Release builds take the tag's version (OCGEN_BUILD_VERSION, set by the
    // release workflow); other builds fall back to Cargo.toml's.
    let expected = option_env!("OCGEN_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    assert_eq!(ocgen::VERSION, expected);
    ocgen()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(format!("ocgen {expected}")));
}

#[test]
fn release_workflow_stamps_the_tag_version_into_the_binary() {
    let wf = include_str!("../.github/workflows/release.yml");
    // The tag (vX.Y.Z) becomes the version the binary reports…
    assert!(wf.contains("OCGEN_BUILD_VERSION"), "{wf}");
    assert!(wf.contains("GITHUB_REF_NAME#v"));
    // …a tag that isn't a version fails the release…
    assert!(wf.contains("is not a version tag"));
    // …and each native build is checked to report it.
    assert!(wf.contains("--version"));
}

#[test]
fn edit_intent_trusts_and_untrusts_domains() {
    let dir = tempdir().unwrap();
    scaffold_claude(dir.path());
    ocgen()
        .args(["edit", "intent", "-p"])
        .arg(dir.path())
        .args([
            "--trust-domain",
            "docs.example.org",
            "--untrust-domain",
            "go.dev",
            "--trust-domain",
            "*.amazon.com",
        ])
        .assert()
        .success();
    let skill = std::fs::read_to_string(dir.path().join(".claude/skills/intent/SKILL.md")).unwrap();
    assert!(skill.contains("WebFetch(domain:docs.example.org)"));
    assert!(skill.contains("WebFetch(domain:*.amazon.com)"));
    assert!(!skill.contains("WebFetch(domain:go.dev)"));
    ocgen()
        .args(["edit", "intent", "--show", "-p"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("docs.example.org"))
        .stdout(contains("trusted docs"));

    let state = dir.path().join(".claude/.ocgen-state.json");
    // https:// is accepted (stored as the host); http:// is refused.
    ocgen()
        .args([
            "edit",
            "intent",
            "--trust-domain",
            "https://docs.example.net",
            "-p",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let skill = std::fs::read_to_string(dir.path().join(".claude/skills/intent/SKILL.md")).unwrap();
    assert!(skill.contains("WebFetch(domain:docs.example.net)"));
    let before = std::fs::read_to_string(&state).unwrap();
    ocgen()
        .args([
            "edit",
            "intent",
            "--trust-domain",
            "http://docs.example.com",
            "-p",
        ])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(contains("https://"));
    assert_eq!(
        before,
        std::fs::read_to_string(&state).unwrap(),
        "nothing written"
    );

    for bad in ["https://evil.example/x", "*.com"] {
        ocgen()
            .args(["edit", "intent", "--trust-domain", bad, "-p"])
            .arg(dir.path())
            .assert()
            .failure()
            .stderr(contains("domain"));
    }
    assert_eq!(
        before,
        std::fs::read_to_string(&state).unwrap(),
        "nothing written"
    );
    ocgen()
        .args(["edit", "intent", "--help"])
        .assert()
        .success()
        .stdout(contains("--trust-domain"));
}

// ------------------------------------------------------------ ocgen notes --

const NOTES_LEDGER: &str = "Topic: Request flow\nUpdated: 2026-10-01   Commit: abc1234\n\
## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\n";

fn notes_project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let notes = dir.path().join(".claude/notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("request-flow.md"), NOTES_LEDGER).unwrap();
    std::fs::write(
        notes.join("older.md"),
        "Topic: Something older\n## Q&A log\n",
    )
    .unwrap();
    (dir, notes)
}

#[test]
fn notes_help_lists_open_and_render() {
    ocgen()
        .args(["notes", "--help"])
        .assert()
        .success()
        .stdout(contains("open"))
        .stdout(contains("render"))
        .stdout(contains("OCGEN_NOTES_OPEN"))
        .stdout(contains("serve").not());
    ocgen()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("notes"));
}

#[test]
fn notes_render_writes_the_html() {
    let (_dir, notes) = notes_project();
    let md = notes.join("request-flow.md");
    ocgen()
        .args(["notes", "render"])
        .arg(&md)
        .assert()
        .success()
        .stdout(contains("request-flow.html"));
    let page = std::fs::read_to_string(notes.join("request-flow.html")).unwrap();
    assert!(page.contains("Request flow") && page.contains("ev verified"));
}

#[test]
fn notes_open_matches_topic_line_or_latest_and_prints_the_url() {
    let (dir, notes) = notes_project();
    let log = dir.path().join("opened.log");
    let fake = dir.path().join("fake.sh");
    std::fs::write(
        &fake,
        format!(
            "printf '%s\\n' \"$1\" >> '{}'\n",
            ocgen::paths::for_shell(&log)
        ),
    )
    .unwrap();
    let browser = format!("sh '{}'", ocgen::paths::for_shell(&fake));
    let run = |args: &[&str]| {
        ocgen()
            .args(["notes", "open"])
            .args(args)
            .arg("--path")
            .arg(dir.path())
            .env("OCGEN_NOTES_BROWSER", &browser)
            .env("OCGEN_NOTES_IDLE_SECS", "30")
            .env_remove("OCGEN_NOTES_OPEN")
            .assert()
            .success()
    };
    // By Topic: line (a substring), even in CI: an explicit open always shows.
    run(&["something older"]).stdout(contains("Opened http://127.0.0.1:"));
    assert!(notes.join("older.html").is_file());
    // By slug, then the most recently changed one.
    run(&["request-flow"]).stdout(contains("request-flow.html"));
    std::fs::write(
        notes.join("older.md"),
        "Topic: Something older\n## Q&A log\n- x\n",
    )
    .unwrap();
    run(&[]).stdout(contains("older.html"));
    let opened = std::fs::read_to_string(&log).unwrap();
    assert_eq!(opened.lines().count(), 3, "{opened}");
    assert!(opened.lines().all(ocgen::notes::browser::is_viewer_url));
    // Unknown topics name the ledgers there are.
    ocgen()
        .args(["notes", "open", "nope"])
        .arg("--path")
        .arg(dir.path())
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .failure()
        .stderr(contains("request-flow"));
    if let Some(info) = ocgen::notes::viewer::read_info(&notes) {
        ocgen::notes::viewer::quit(&info);
    }
}
