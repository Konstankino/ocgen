//! Scripted wizard flows (see `crate::prompt::script`): each test answers the real
//! prompts in order and checks what the flow wrote.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;

use crate::prompt::{script, script_remaining};

fn claude_project(dir: &Path) -> String {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "wiz".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.scaffold(dir, false).unwrap();
    dir.to_string_lossy().into_owned()
}

fn reload(dir: &Path) -> Project {
    Project::load_state(dir).unwrap()
}

#[test]
fn add_skill_walks_every_prompt_and_writes_the_skill_and_stubs() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&[
        "",                  // What do you need? → a skill
        "knowledge",         // preset
        "tf-plan-review",    // name
        "Review a Terraform plan for S3/KMS misconfigurations. Use when asked to check a plan or bucket policy.",
        "",                  // when to use
        "Read,Grep",         // allowed tools
        "Bash(terraform show:*)", // extra patterns
        "",                  // argument hint
        "",                  // user-run only → no
        "",                  // hide → no
        "",                  // fork → no
        "",                  // model
        "high",              // effort
        "Write",             // disallowed tools
        "**/*.tf",           // paths
        "",                  // named arguments
        "",                  // body → keep the preset's
        "y",                 // reference.md stub
        "n",                 // scripts/ stub
    ]);
    super::run_add_skill(Some(path)).unwrap();
    assert_eq!(
        script_remaining(),
        0,
        "the flow asked exactly the scripted questions"
    );

    let p = reload(tmp.path());
    let s = p
        .skills
        .iter()
        .find(|s| s.name == "tf-plan-review")
        .unwrap();
    assert_eq!(s.allowed_tools, "Read, Grep, Bash(terraform show:*)");
    assert_eq!(s.effort, "high");
    assert_eq!(s.paths, "**/*.tf");
    let md = fs::read_to_string(tmp.path().join(".claude/skills/tf-plan-review/SKILL.md")).unwrap();
    assert!(md.contains("effort: high") && md.contains("disallowed-tools: Write"));
    assert!(tmp
        .path()
        .join(".claude/skills/tf-plan-review/reference.md")
        .exists());
    assert!(!tmp
        .path()
        .join(".claude/skills/tf-plan-review/scripts")
        .exists());
}

#[test]
fn add_skill_routes_always_on_guidance_to_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&["rule"]);
    super::run_add_skill(Some(path)).unwrap();
    assert_eq!(script_remaining(), 0);
    assert!(reload(tmp.path()).skills.is_empty());
}

#[test]
fn add_skill_rejects_a_workflow_name() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&["", "blank", "deliver"]);
    let err = super::run_add_skill(Some(path)).unwrap_err().to_string();
    assert!(err.contains("workflow"), "{err}");
}

#[test]
fn add_skill_offers_revision_when_checks_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&[
        "",
        "blank",
        "helper",
        "TF helper",
        "",
        "none",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",  // body
        "n", // Revise now? → keep as is
        "n",
        "n",
    ]);
    super::run_add_skill(Some(path)).unwrap();
    assert_eq!(script_remaining(), 0, "the revise prompt appeared");
    let p = reload(tmp.path());
    assert_eq!(p.skills[0].description, "TF helper");
    assert!(p.issues().iter().any(|i| i.contains("skill 'helper'")));
}

#[test]
fn add_mcp_defaults_to_pre_approved_and_assigns_agents() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&[
        "tf",                   // name
        "stdio",                // transport
        "npx",                  // command
        "-y terraform-mcp",     // args
        "TF_TOKEN=${TF_TOKEN}", // env
        "",                     // pre-approve → default (yes for a new server)
        "explorer",             // subagents that may use it
    ]);
    super::run_add_mcp(Some(path)).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    let m = &p.claude.mcp_servers[0];
    assert!(m.pre_approve, "new servers default to pre-approved");
    assert_eq!(m.args, vec!["-y", "terraform-mcp"]);
    let explorer = p.agents.iter().find(|a| a.name == "explorer").unwrap();
    assert_eq!(explorer.mcp_servers, "tf");
    assert!(tmp.path().join(".mcp.json").exists());
}

#[test]
fn edit_mcp_remove_also_detaches_it_from_agents() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&["tf", "stdio", "npx", "", "", "", "explorer"]);
    super::run_add_mcp(Some(path.clone())).unwrap();
    script(&["Remove"]);
    super::run_edit_mcp(path, Some("tf".into())).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert!(p.claude.mcp_servers.is_empty());
    assert!(p.agents.iter().all(|a| a.mcp_servers.is_empty()));
}
