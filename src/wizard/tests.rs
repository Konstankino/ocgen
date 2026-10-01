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

// ---------- `new`: target choice and the existing-file review ----------

use crate::cli::TargetArg;
use dialoguer::theme::ColorfulTheme;

#[test]
fn target_is_asked_only_when_not_given_on_the_command_line() {
    let theme = ColorfulTheme::default();
    script(&[]);
    assert!(matches!(
        super::pick_target(&theme, Some(TargetArg::Opencode)).unwrap(),
        TargetArg::Opencode
    ));
    script(&["OpenCode"]);
    assert!(matches!(
        super::pick_target(&theme, None).unwrap(),
        TargetArg::Opencode
    ));
    script(&["Claude"]);
    assert!(matches!(
        super::pick_target(&theme, None).unwrap(),
        TargetArg::Claude
    ));
    assert_eq!(script_remaining(), 0);
}

/// A Claude project that hasn't been written anywhere yet.
fn unwritten_claude_project() -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "wiz".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

const SETTINGS: &str = ".claude/settings.json";
const STATE: &str = ".claude/.ocgen-state.json";

/// The first generated agent file (relative path), in render order.
fn first_agent(p: &Project) -> String {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, _)| rel.to_string_lossy().replace('\\', "/"))
        .find(|rel| rel.starts_with(".claude/agents/"))
        .unwrap()
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap()
}

/// The single backup folder under `.ocgen-backup/`.
fn backup_dir(dir: &Path) -> std::path::PathBuf {
    let mut dirs: Vec<_> = fs::read_dir(dir.join(".ocgen-backup"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1);
    dirs.remove(0)
}

#[test]
fn new_writes_without_asking_when_nothing_conflicts() {
    let theme = ColorfulTheme::default();
    let p = unwritten_claude_project();

    // An empty directory: no question.
    let tmp = tempfile::tempdir().unwrap();
    script(&[]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert!(tmp.path().join(SETTINGS).exists());

    // Identical files and the user's own CLAUDE.md are not conflicts either.
    fs::write(tmp.path().join("CLAUDE.md"), "mine\n").unwrap();
    script(&[]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert_eq!(read(tmp.path(), "CLAUDE.md"), "mine\n");
    assert!(!tmp.path().join(".ocgen-backup").exists());
}

#[test]
fn new_overwrite_all_backs_up_the_old_versions() {
    let theme = ColorfulTheme::default();
    let p = unwritten_claude_project();
    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let agent = first_agent(&p);
    let generated = read(tmp.path(), SETTINGS);
    fs::write(tmp.path().join(SETTINGS), "{\"mine\": 1}\n").unwrap();
    fs::write(tmp.path().join(&agent), "my agent\n").unwrap();

    script(&["Overwrite all"]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);

    assert_eq!(read(tmp.path(), SETTINGS), generated);
    assert_ne!(read(tmp.path(), &agent), "my agent\n");
    let backup = backup_dir(tmp.path());
    assert_eq!(read(&backup, SETTINGS), "{\"mine\": 1}\n");
    assert_eq!(read(&backup, &agent), "my agent\n");
}

#[test]
fn new_keep_all_writes_only_the_missing_files() {
    let theme = ColorfulTheme::default();
    let p = unwritten_claude_project();
    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let agent = first_agent(&p);
    fs::write(tmp.path().join(SETTINGS), "{\"mine\": 1}\n").unwrap();
    fs::remove_file(tmp.path().join(&agent)).unwrap();

    script(&["Keep all"]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);

    assert_eq!(read(tmp.path(), SETTINGS), "{\"mine\": 1}\n");
    assert!(tmp.path().join(&agent).exists(), "missing file is written");
    assert!(!tmp.path().join(".ocgen-backup").exists());
}

#[test]
fn new_file_by_file_overwrites_and_keeps_as_chosen() {
    let theme = ColorfulTheme::default();
    let p = unwritten_claude_project();
    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let agent = first_agent(&p);
    fs::write(tmp.path().join(&agent), "my agent\n").unwrap();
    fs::write(tmp.path().join(SETTINGS), "{\"mine\": 1}\n").unwrap();

    // Conflicts come in render order: the agent file, then settings.json.
    script(&[
        "file by file",
        "Overwrite", // the agent
        "full diff", // settings.json: look first…
        "Keep",      // …then keep it
    ]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);

    assert_ne!(read(tmp.path(), &agent), "my agent\n");
    assert_eq!(read(tmp.path(), SETTINGS), "{\"mine\": 1}\n");
    let backup = backup_dir(tmp.path());
    assert_eq!(read(&backup, &agent), "my agent\n");
    assert!(
        !backup.join(SETTINGS).exists(),
        "kept files aren't backed up"
    );
}

#[test]
fn new_cancel_writes_nothing() {
    let theme = ColorfulTheme::default();
    let p = unwritten_claude_project();
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    fs::write(tmp.path().join(SETTINGS), "{\"mine\": 1}\n").unwrap();

    script(&["Cancel"]);
    super::write_with_review(&theme, &p, tmp.path()).unwrap();
    assert_eq!(script_remaining(), 0);

    assert_eq!(read(tmp.path(), SETTINGS), "{\"mine\": 1}\n");
    assert!(!tmp.path().join(STATE).exists());
    assert!(!tmp.path().join(".claude/agents").exists());
    assert!(!tmp.path().join(".ocgen-backup").exists());
}

#[test]
fn edit_permissions_walks_add_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&[
        "Add",                 // what now?
        "allow",               // which list
        "Bash(gh run view:*)", // the rule
        "Add",
        "deny",
        "Read(./secrets/**)",
        "Remove",             // what now?
        "Read(./secrets/**)", // which rule
        "List",               // every rule at a glance
        "Done",
    ]);
    super::run_edit_permissions(path, Default::default()).unwrap();
    assert_eq!(script_remaining(), 0);

    let p = reload(tmp.path());
    assert_eq!(p.claude.permissions.allow, ["Bash(gh run view:*)"]);
    assert!(p.claude.permissions.deny.is_empty());
    assert!(read(tmp.path(), SETTINGS).contains("Bash(gh run view:*)"));
}

#[test]
fn edit_intent_walks_every_setting() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    script(&[
        "",         // keep /intent on
        "RFC",      // prefix
        "3",        // digits
        "docs/rfc", // directory
        "200",      // max words
        "",         // branch: the remote default
    ]);
    super::run_edit_intent(path, Default::default()).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert_eq!(p.claude.intent.prefix, "RFC");
    assert_eq!(p.claude.intent.digits, 3);
    assert_eq!(p.claude.intent.dir, "docs/rfc");
    assert_eq!(p.claude.intent.max_words, 200);
    assert!(read(tmp.path(), ".claude/skills/intent/SKILL.md").contains("RFC-001"));
}

// ---------- doctor: hand-added rules and the per-file review ----------

/// A Claude project on disk with two allow rules added by hand to settings.json
/// and a hand-edited agent file. Returns (path, agent rel path, old settings).
fn hand_edited_project(dir: &Path) -> (String, String, String) {
    let path = claude_project(dir);
    let p = reload(dir);
    let agent = first_agent(&p);
    fs::write(dir.join(&agent), "my agent\n").unwrap();
    let settings = dir.join(SETTINGS);
    let mut s: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    for r in ["Bash(gh run view:*)", "Bash(head:*)"] {
        s["permissions"]["allow"]
            .as_array_mut()
            .unwrap()
            .push(r.into());
    }
    let old = serde_json::to_string_pretty(&s).unwrap();
    fs::write(&settings, &old).unwrap();
    (path, agent, old)
}

#[test]
fn doctor_adopts_hand_added_rules_and_keeps_files_chosen_one_by_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, agent, _) = hand_edited_project(tmp.path());
    script(&[
        "y",            // keep the hand-added rules as mine
        "file by file", // what now?
        "Keep",         // the agent file
        "Overwrite",    // settings.json (regenerated with the adopted rules)
    ]);
    super::run_doctor(path, false, false).unwrap();
    assert_eq!(script_remaining(), 0);

    // settings.json was regenerated only for the rules, which it now holds as mine.
    let p = reload(tmp.path());
    assert_eq!(
        p.claude.permissions.allow,
        ["Bash(gh run view:*)", "Bash(head:*)"]
    );
    assert!(read(tmp.path(), SETTINGS).contains("Bash(head:*)"));
    assert_eq!(read(tmp.path(), &agent), "my agent\n", "kept");
}

#[test]
fn doctor_apply_all_without_adopting_drops_the_rules_with_a_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, agent, old) = hand_edited_project(tmp.path());
    script(&["n", "Apply all"]);
    super::run_doctor(path, false, false).unwrap();
    assert_eq!(script_remaining(), 0);

    assert!(!read(tmp.path(), SETTINGS).contains("Bash(head:*)"));
    assert_ne!(read(tmp.path(), &agent), "my agent\n");
    let backup = backup_dir(tmp.path());
    assert_eq!(read(&backup, SETTINGS), old);
    assert_eq!(read(&backup, &agent), "my agent\n");
}

#[test]
fn doctor_cancel_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, agent, old) = hand_edited_project(tmp.path());
    let state = read(tmp.path(), STATE);
    script(&["y", "Cancel"]);
    super::run_doctor(path, false, false).unwrap();
    assert_eq!(script_remaining(), 0);
    assert_eq!(read(tmp.path(), SETTINGS), old);
    assert_eq!(read(tmp.path(), &agent), "my agent\n");
    assert_eq!(
        read(tmp.path(), STATE),
        state,
        "rules not adopted on cancel"
    );
    assert!(!tmp.path().join(".ocgen-backup").exists());
}
