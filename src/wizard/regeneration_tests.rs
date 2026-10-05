//! Scripted flows (see `crate::prompt::script`) for rewriting an existing project:
//! `ocgen new` over one, and `doctor` with things added by hand.

use std::fs;
use std::path::{Path, PathBuf};

use dialoguer::theme::ColorfulTheme;
use ocgen::agent;
use ocgen::claude::{McpServer, Skill, Team};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::{json, Value};

use crate::prompt::{script, script_remaining};

const STATE: &str = ".claude/.ocgen-state.json";

fn project(name: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn server(name: &str) -> McpServer {
    McpServer {
        name: name.into(),
        transport: "stdio".into(),
        command: "npx".into(),
        args: vec!["-y".into(), format!("{name}-mcp")],
        ..Default::default()
    }
}

/// A project with a skill and an MCP server: things only its state remembers.
fn existing(dir: &Path) -> String {
    let mut p = project("old");
    p.skills.push(Skill {
        name: "alpha".into(),
        description: "Review a plan. Use when asked to check a plan.".into(),
        body: "1. Read it.\n".into(),
        ..Default::default()
    });
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir, false).unwrap();
    dir.to_string_lossy().into_owned()
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap()
}

fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir.join(".ocgen-backup"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

// ---------- `ocgen new` over an existing project ----------

#[test]
fn new_over_a_project_asks_first_and_no_leaves_it_alone() {
    let tmp = tempfile::tempdir().unwrap();
    existing(tmp.path());
    let state = read(tmp.path(), STATE);
    script(&["n"]); // Replace its configuration?
    super::write_with_review(&ColorfulTheme::default(), &project("new"), tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);
    assert_eq!(read(tmp.path(), STATE), state);
    assert!(tmp.path().join(".claude/skills/alpha/SKILL.md").exists());
    assert!(backups(tmp.path()).is_empty());
}

#[test]
fn new_replacing_a_project_backs_up_its_state_with_the_files() {
    let tmp = tempfile::tempdir().unwrap();
    existing(tmp.path());
    let state = read(tmp.path(), STATE);
    let skill = read(tmp.path(), ".claude/skills/alpha/SKILL.md");
    script(&[
        "y",             // Replace its configuration?
        "Overwrite all", // the skill and .mcp.json are no longer generated
    ]);
    super::write_with_review(&ColorfulTheme::default(), &project("new"), tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);

    let b = backups(tmp.path());
    assert_eq!(b.len(), 1, "one backup folder for everything: {b:?}");
    assert_eq!(read(&b[0], STATE), state);
    assert_eq!(read(&b[0], ".claude/skills/alpha/SKILL.md"), skill);
    assert!(!tmp.path().join(".claude/skills/alpha").exists());
    assert!(!tmp.path().join(".mcp.json").exists());
    assert_eq!(Project::load_state(tmp.path()).unwrap().project_name, "new");
}

#[test]
fn new_without_a_terminal_refuses_to_replace_a_project() {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return; // only meaningful where nothing can be asked
    }
    let tmp = tempfile::tempdir().unwrap();
    existing(tmp.path());
    let state = read(tmp.path(), STATE);
    let err = super::write_with_review(&ColorfulTheme::default(), &project("new"), tmp.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("ocgen doctor"), "{err}");
    assert_eq!(read(tmp.path(), STATE), state);
}

// ---------- doctor ----------

/// An existing project whose `.mcp.json` got a server added by hand.
fn with_hand_added_server(dir: &Path) -> String {
    let mut p = project("doc");
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir, false).unwrap();
    let mut m: Value = serde_json::from_str(&read(dir, ".mcp.json")).unwrap();
    m["mcpServers"]["github"] = json!({ "type": "http", "url": "https://example.com/mcp" });
    fs::write(
        dir.join(".mcp.json"),
        serde_json::to_string_pretty(&m).unwrap(),
    )
    .unwrap();
    dir.to_string_lossy().into_owned()
}

#[test]
fn doctor_keeps_a_hand_added_mcp_server_when_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let path = with_hand_added_server(tmp.path());
    script(&[
        "y",         // keep what was added by hand
        "Apply all", // .mcp.json in ocgen's formatting
    ]);
    super::run_doctor(path, false, false, false).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = Project::load_state(tmp.path()).unwrap();
    assert!(p.claude.mcp_servers.iter().any(|s| s.name == "github"));
    assert!(read(tmp.path(), ".mcp.json").contains("https://example.com/mcp"));
}

#[test]
fn doctor_drops_a_hand_added_mcp_server_with_a_backup_when_declined() {
    let tmp = tempfile::tempdir().unwrap();
    let path = with_hand_added_server(tmp.path());
    let before = read(tmp.path(), ".mcp.json");
    script(&["n", "Apply all"]);
    super::run_doctor(path, false, false, false).unwrap();
    assert_eq!(script_remaining(), 0);
    assert!(!read(tmp.path(), ".mcp.json").contains("github"));
    assert_eq!(read(&backups(tmp.path())[0], ".mcp.json"), before);
}

#[test]
fn doctor_keep_all_leaves_files_no_longer_generated() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project("team");
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.scaffold(tmp.path(), false).unwrap();
    // Turned off the documented way: in the state, then doctor.
    let mut state: Value = serde_json::from_str(&read(tmp.path(), STATE)).unwrap();
    state["claude"]["team"]["enabled"] = json!(false);
    fs::write(tmp.path().join(STATE), state.to_string()).unwrap();

    script(&["Keep all"]);
    super::run_doctor(
        tmp.path().to_string_lossy().into_owned(),
        false,
        false,
        false,
    )
    .unwrap();
    assert_eq!(script_remaining(), 0);
    assert!(tmp.path().join(".claude/skills/team/SKILL.md").exists());
}

#[test]
fn doctor_refuses_a_state_from_a_newer_ocgen_before_asking_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let path = with_hand_added_server(tmp.path());
    let mut state: Value = serde_json::from_str(&read(tmp.path(), STATE)).unwrap();
    state["schema"] = json!(ocgen::render::STATE_SCHEMA + 1);
    fs::write(tmp.path().join(STATE), state.to_string()).unwrap();
    script(&[]);
    let err = super::run_doctor(path, false, false, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("upgrade"), "{err}");
    assert_eq!(script_remaining(), 0);
}
