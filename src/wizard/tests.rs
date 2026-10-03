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
        "",                              // keep /intent on
        "RFC",                           // prefix
        "3",                             // digits
        "docs/rfc",                      // directory
        "200",                           // max words
        "",                              // branch: the remote default
        "alice, org/architects, @Alice", // approvers (the same person once)
    ]);
    super::run_edit_intent(path, Default::default()).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert_eq!(p.claude.intent.prefix, "RFC");
    assert_eq!(p.claude.intent.digits, 3);
    assert_eq!(p.claude.intent.dir, "docs/rfc");
    assert_eq!(p.claude.intent.max_words, 200);
    assert_eq!(p.claude.intent.approvers, ["@alice", "@org/architects"]);
    // No CODEOWNERS in the project: nothing to link, so no more questions.
    assert!(p.claude.intent.codeowners.is_empty());
    let skill = read(tmp.path(), ".claude/skills/intent/SKILL.md");
    assert!(skill.contains("RFC-001") && skill.contains("@alice, @org/architects"));
}

#[test]
fn edit_intent_offers_to_link_the_projects_codeowners() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    let owners = tmp.path().join(".github/CODEOWNERS");
    std::fs::create_dir_all(owners.parent().unwrap()).unwrap();
    std::fs::write(&owners, "* @owner\n").unwrap();
    script(&[
        "", "", "", "", "", "",       // /intent and its settings, as they are
        "@alice", // approvers
        "",       // keep them in .github/CODEOWNERS: yes
        "all",    // what they review: every change
    ]);
    super::run_edit_intent(path, Default::default()).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert_eq!(p.claude.intent.codeowners, ".github/CODEOWNERS");
    assert_eq!(
        p.claude.intent.codeowners_scope,
        ocgen::claude::CodeownersScope::All
    );
    let text = std::fs::read_to_string(&owners).unwrap();
    assert!(
        text.starts_with("* @owner\n") && text.contains("\n* @alice\n"),
        "{text}"
    );
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

// ---------- the sandbox: on by default with the approval gate ----------

use crate::cli::{OutputArg, TeamCli};
use ocgen::claude::sandbox_supported;

/// Answers for `new` on a Claude project up to the sandbox question, keeping
/// every default, then `sandbox`.
fn new_claude_answers<'a>(sandbox: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "", "", // agents: the default pipeline, no more
        "", // what the project is for
        "", // CLAUDE.md
        "", // power-user defaults
        "", "", // extra hooks, formatter
        "", "", "", "", "", "", "", // intake … inquire
        "", "", "", "", "", "", "", // /intent, its settings and approvers
        "", "", "", // subagent confidence, check command, loop guard
        "", "", "", "", "", "", "", // Agent Teams and its gates
    ];
    a.extend_from_slice(sandbox);
    a
}

fn build_new(team: bool, approval_gate: bool) -> Project {
    super::build_claude_project(
        &ColorfulTheme::default(),
        &Manifest::load().unwrap(),
        "English",
        "wiz",
        OutputArg::Project,
        None,
        TeamCli {
            enabled: team,
            confidence: None,
            plan_gate: true,
            risk_rounds: true,
            approval_gate,
        },
    )
    .unwrap()
}

#[test]
fn new_with_the_gate_on_sandboxes_by_default() {
    // Enter on the sandbox question, its extra domains and the credentials one.
    let answers = if sandbox_supported() {
        new_claude_answers(&["", "", ""])
    } else {
        new_claude_answers(&[""])
    };
    script(&answers);
    let p = build_new(true, true);
    assert_eq!(
        script_remaining(),
        0,
        "asked exactly the scripted questions"
    );
    assert!(p.claude.team.enabled && p.claude.team.approval_gate);
    assert_eq!(p.claude.sandbox.enabled, sandbox_supported());

    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let s: serde_json::Value = serde_json::from_str(&read(tmp.path(), SETTINGS)).unwrap();
    if !sandbox_supported() {
        assert!(s.get("sandbox").is_none());
        return;
    }
    assert!(!p.claude.sandbox.declined);
    assert_eq!(s["sandbox"]["enabled"], true);
    assert_eq!(
        s["sandbox"]["filesystem"]["denyWrite"][0],
        "~/.claude/ocgen"
    );
    assert!(
        s["sandbox"]["credentials"]["files"].is_array(),
        "credentials withheld: {s}"
    );
}

#[test]
fn new_without_the_gate_keeps_the_sandbox_off_by_default() {
    script(&new_claude_answers(&[""])); // Enter = no, so no follow-ups
    let p = build_new(true, false);
    assert_eq!(script_remaining(), 0);
    assert!(!p.claude.sandbox.enabled);
}

#[test]
fn new_declining_the_sandbox_is_remembered() {
    script(&new_claude_answers(&["n"]));
    let p = build_new(true, true);
    assert_eq!(script_remaining(), 0);
    assert!(!p.claude.sandbox.enabled);
    // Only a no to the recommended yes is an opt-out (native Windows asks with
    // no as the default).
    assert_eq!(
        p.claude.sandbox.declined,
        sandbox_supported(),
        "an explicit no is recorded"
    );

    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let s: serde_json::Value = serde_json::from_str(&read(tmp.path(), SETTINGS)).unwrap();
    assert!(s.get("sandbox").is_none());
    assert_eq!(
        reload(tmp.path()).claude.sandbox.declined,
        sandbox_supported()
    );
}

#[test]
fn keeping_the_sandbox_off_without_the_gate_is_no_opt_out() {
    // `new` without the gate: Enter keeps the sandbox off, its default — not a
    // no to the recommendation, so turning the gate on later offers it as yes.
    script(&new_claude_answers(&[""]));
    let p = build_new(true, false);
    assert_eq!(script_remaining(), 0);
    assert!(!p.claude.sandbox.enabled);
    assert!(!p.claude.sandbox.declined, "nothing was declined");

    let tmp = tempfile::tempdir().unwrap();
    p.scaffold(tmp.path(), false).unwrap();
    let path = tmp.path().to_string_lossy().into_owned();
    let mut answers = team_with_gate();
    answers[0] = ""; // Agent Teams is on already: Enter keeps it
    if sandbox_supported() {
        answers.extend(["", "", ""]); // Enter on the sandbox: yes
    }
    script(&answers);
    super::run_edit_team(path).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert!(p.claude.team.approval_gate);
    assert_eq!(p.claude.sandbox.enabled, sandbox_supported());
}

/// Answers for `edit team` turning Agent Teams on with the approval gate.
fn team_with_gate() -> Vec<&'static str> {
    vec![
        "y", // enable Agent Teams
        "",  // quality-gate hooks
        "",  // display mode
        "",  // plan gate
        "",  // confidence
        "",  // risk rounds
        "y", // the approval gate
    ]
}

#[test]
fn edit_team_turning_the_gate_on_offers_the_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    let mut answers = team_with_gate();
    if sandbox_supported() {
        answers.extend(["", "", ""]); // Enter = yes, extra domains, credentials
    }
    script(&answers);
    super::run_edit_team(path).unwrap();
    assert_eq!(script_remaining(), 0, "the sandbox was offered");

    let p = reload(tmp.path());
    assert!(p.claude.team.approval_gate);
    assert_eq!(p.claude.sandbox.enabled, sandbox_supported());
    let s: serde_json::Value = serde_json::from_str(&read(tmp.path(), SETTINGS)).unwrap();
    assert_eq!(s.get("sandbox").is_some(), sandbox_supported());
}

#[test]
fn edit_team_declining_the_sandbox_is_remembered() {
    if !sandbox_supported() {
        return; // never offered where it can't run
    }
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    let mut answers = team_with_gate();
    answers.push("n");
    script(&answers);
    super::run_edit_team(path).unwrap();
    assert_eq!(script_remaining(), 0);
    let p = reload(tmp.path());
    assert!(!p.claude.sandbox.enabled && p.claude.sandbox.declined);
}

#[test]
fn edit_team_without_the_gate_does_not_ask_about_the_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let path = claude_project(tmp.path());
    let mut answers = team_with_gate();
    *answers.last_mut().unwrap() = "n"; // no approval gate
    script(&answers);
    super::run_edit_team(path).unwrap();
    assert_eq!(script_remaining(), 0);
    assert!(!reload(tmp.path()).claude.sandbox.enabled);
}

#[test]
fn the_sandbox_help_names_the_gate_only_when_it_is_on() {
    use super::sandbox_help_for;
    // Supported: with the gate it says what the gate is without the sandbox.
    assert!(sandbox_help_for(true, true).contains("approval gate is advisory only"));
    // …and claims no more than it keeps away.
    assert!(sandbox_help_for(true, true).contains("the credentials ocgen knows about"));
    assert!(!sandbox_help_for(true, true).contains("however"));
    assert!(!sandbox_help_for(true, false).contains("approval gate"));
    // Native Windows: the sandbox isn't there, and the gate is advisory only
    // when there is a gate.
    assert!(sandbox_help_for(false, true).contains("isn't available on native Windows"));
    assert!(sandbox_help_for(false, true).contains("approval gate is advisory"));
    let off = sandbox_help_for(false, false);
    assert!(off.contains("isn't available on native Windows"), "{off}");
    assert!(!off.contains("approval gate"), "{off}");
}

// ---------- the adversary: offered first to a team that lacks it ----------

/// A project with the team every project had before the adversary existed.
fn legacy_team(dir: &Path, target: Target) -> String {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = target;
    p.project_name = "legacy".into();
    p.agents = ["coordinator", "explorer", "implementer", "reviewer"]
        .iter()
        .map(|n| match target {
            Target::ClaudeCode => agent::Agent::from_archetype_claude(n, n, "English"),
            Target::OpenCode => agent::Agent::from_archetype(n, n, "English", "mac"),
        })
        .collect::<anyhow::Result<_>>()
        .unwrap();
    if target == Target::ClaudeCode {
        p.providers.clear();
    }
    p.scaffold(dir, false).unwrap();
    dir.to_string_lossy().into_owned()
}

#[test]
fn the_default_preset_is_the_next_pipeline_role_the_team_lacks() {
    use super::default_preset;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let presets = s(&[
        "adversary",
        "coordinator",
        "explorer",
        "implementer",
        "reviewer",
        "verifier",
        "blank (custom role)",
    ]);
    let pipeline = s(&[
        "coordinator",
        "explorer",
        "implementer",
        "reviewer",
        "adversary",
    ]);
    // A new project starts from the coordinator, as before.
    assert_eq!(
        presets[default_preset(&presets, &[], &pipeline)],
        "coordinator"
    );
    // A team from before the adversary is offered the adversary.
    let legacy = s(&["coordinator", "explorer", "implementer", "reviewer"]);
    assert_eq!(
        presets[default_preset(&presets, &legacy, &pipeline)],
        "adversary"
    );
    // A renamed agent still counts by its role.
    let renamed = s(&[
        "boss",
        "coordinator",
        "explorer",
        "implementer",
        "reviewer",
        "redteam",
        "adversary",
    ]);
    assert_eq!(
        presets[default_preset(&presets, &renamed, &pipeline)],
        "verifier"
    );
    // Every preset used: the blank role; no pipeline at all: the first unused.
    let all = s(&[
        "adversary",
        "coordinator",
        "explorer",
        "implementer",
        "reviewer",
        "verifier",
    ]);
    assert_eq!(
        presets[default_preset(&presets, &all, &pipeline)],
        "blank (custom role)"
    );
    assert_eq!(
        presets[default_preset(&presets, &s(&["adversary"]), &[])],
        "coordinator"
    );
}

#[test]
fn add_agent_offers_the_adversary_to_a_claude_team_that_lacks_it() {
    let tmp = tempfile::tempdir().unwrap();
    let path = legacy_team(tmp.path(), Target::ClaudeCode);
    // Enter on every prompt: preset, name, then the agent's fields.
    script(&[""; 13]);
    super::run_add_agent(Some(path)).unwrap();
    assert_eq!(
        script_remaining(),
        0,
        "asked exactly the scripted questions"
    );

    let p = reload(tmp.path());
    let adv = p.agents.iter().find(|a| a.name == "adversary").unwrap();
    assert_eq!(adv.role, "adversary");
    assert_eq!(adv.tools, "Read, Grep, Glob, Bash");
    assert!(read(tmp.path(), ".claude/agents/adversary.md").contains("maxTurns: 40"));
    assert!(read(tmp.path(), ".claude/rules/ocgen-team.md").contains("`Verdict: REWORK`"));
    let s: serde_json::Value = serde_json::from_str(&read(tmp.path(), SETTINGS)).unwrap();
    let ro = s["env"]["SUBAGENT_READONLY_ROLES"]
        .as_str()
        .unwrap_or_default();
    assert!(ro.split_whitespace().any(|r| r == "adversary"), "{ro:?}");
}

#[test]
fn add_agent_named_adversary_starts_an_opencode_agent_from_its_preset() {
    let tmp = tempfile::tempdir().unwrap();
    let path = legacy_team(tmp.path(), Target::OpenCode);
    // The name comes first in OpenCode; the preset of that name is preselected.
    let mut answers = vec!["adversary"];
    answers.extend([""; 17]);
    script(&answers);
    super::run_add_agent(Some(path)).unwrap();
    assert_eq!(
        script_remaining(),
        0,
        "asked exactly the scripted questions"
    );

    let p = reload(tmp.path());
    let adv = p.agents.iter().find(|a| a.name == "adversary").unwrap();
    assert_eq!(adv.role, "adversary");
    assert_eq!(adv.model, "devstral-24b");
    let prompt = read(tmp.path(), ".opencode/prompts/coordinator.txt");
    assert!(prompt.contains("`Verdict: REWORK`"), "{prompt}");
    assert!(read(tmp.path(), ".opencode/agents/coordinator.md").contains("\"adversary\": allow"));
}

// ---------- languages: instructions in one, answers in another ----------

fn shipped_manifest() -> Manifest {
    toml::from_str(&ocgen::templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

#[test]
fn basics_default_to_english_prompts_and_ukrainian_answers() {
    // Enter on the project name, the instruction language and the answer language.
    script(&["", "", ""]);
    let b = super::ask_basics(&ColorfulTheme::default(), &shipped_manifest()).unwrap();
    assert_eq!(
        script_remaining(),
        0,
        "asked exactly the scripted questions"
    );
    assert_eq!(b.project_name, "my-project");
    assert_eq!(b.language, "English");
    assert_eq!(b.response_language, "Ukrainian");
}

#[test]
fn basics_without_an_answer_question_answer_in_the_prompt_language() {
    // An override manifest from before the answer language existed.
    let mut m = shipped_manifest();
    m.variables.retain(|v| v.key != "response_language");
    script(&["", "Ukrainian"]);
    let b = super::ask_basics(&ColorfulTheme::default(), &m).unwrap();
    assert_eq!(
        script_remaining(),
        0,
        "asked exactly the scripted questions"
    );
    assert_eq!(b.language, "Ukrainian");
    assert_eq!(b.response_language, "Ukrainian");
}
