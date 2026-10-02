//! Regenerating an existing project — what every `ocgen add …` / `ocgen edit …` /
//! `ocgen doctor` run does to files that are already there: nothing the user made
//! is lost silently, files ocgen stops generating go away, names from the state
//! file can't leave the project, and the state survives version skew.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use ocgen::agent;
use ocgen::claude::{McpServer, Output, RuleList, Skill, Team};
use ocgen::manifest::Manifest;
use ocgen::render::{ChangeKind, Project, STATE_SCHEMA};
use ocgen::target::Target;
use predicates::str::contains;
use serde_json::{json, Value};
use tempfile::tempdir;

const SETTINGS: &str = ".claude/settings.json";
const STATE: &str = ".claude/.ocgen-state.json";

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

fn claude(name: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn json_at(dir: &Path, rel: &str) -> Value {
    serde_json::from_str(&read(dir, rel)).unwrap()
}

fn write_json(dir: &Path, rel: &str, v: &Value) {
    fs::write(dir.join(rel), serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn reload(dir: &Path) -> Project {
    Project::load_state(dir).unwrap()
}

/// Every backup folder under `.ocgen-backup/`, oldest first.
fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir.join(".ocgen-backup"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn rules(settings: &Value, list: &str) -> Vec<String> {
    settings["permissions"][list]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

/// Add rules to a list of the project's settings.json, as a user would by hand.
fn hand_add(dir: &Path, list: &str, added: &[&str]) {
    let mut s = json_at(dir, SETTINGS);
    let arr = s["permissions"][list].as_array_mut().unwrap();
    for r in added {
        arr.push((*r).into());
    }
    write_json(dir, SETTINGS, &s);
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

fn skill(name: &str) -> Skill {
    Skill {
        name: name.into(),
        description: "Review a Terraform plan. Use when asked to check a plan.".into(),
        body: "1. Read the plan.\n2. Report problems.\n".into(),
        ..Default::default()
    }
}

fn server_names(dir: &Path) -> BTreeSet<String> {
    if !dir.join(".mcp.json").exists() {
        return BTreeSet::new();
    }
    json_at(dir, ".mcp.json")["mcpServers"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

// ------------------------------------------- add/edit: nothing lost silently -----

#[test]
fn forced_regeneration_keeps_hand_added_rules_and_backs_up_hand_edits() {
    let dir = tempdir().unwrap();
    claude("keep").scaffold(dir.path(), false).unwrap();
    // By hand: a deny rule in settings.json, and a tuned agent prompt.
    hand_add(dir.path(), "deny", &["Bash(curl:*)"]);
    let settings_before = read(dir.path(), SETTINGS);
    fs::write(
        dir.path().join(".claude/agents/reviewer.md"),
        "my tuned reviewer\n",
    )
    .unwrap();

    // `ocgen edit permissions --allow 'Bash(make:*)'`: reload, change, regenerate.
    let mut p = reload(dir.path());
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(make:*)")
        .unwrap();
    p.scaffold(dir.path(), true).unwrap();

    let s = json_at(dir.path(), SETTINGS);
    assert!(rules(&s, "deny").contains(&"Bash(curl:*)".to_string()));
    assert!(rules(&s, "allow").contains(&"Bash(make:*)".to_string()));
    assert_eq!(
        reload(dir.path()).claude.permissions.deny,
        ["Bash(curl:*)"],
        "the hand-added rule is now the user's own"
    );
    // The tuned agent is regenerated; its old version (and settings.json's) is backed up.
    assert_ne!(
        read(dir.path(), ".claude/agents/reviewer.md"),
        "my tuned reviewer\n"
    );
    let b = backups(dir.path());
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(
        read(&b[0], ".claude/agents/reviewer.md"),
        "my tuned reviewer\n"
    );
    assert_eq!(read(&b[0], SETTINGS), settings_before);
}

#[test]
fn forced_regeneration_backs_up_only_hand_edits() {
    let dir = tempdir().unwrap();
    claude("slots").scaffold(dir.path(), false).unwrap();
    // A change that rewrites settings.json, none of it touched by hand: the old
    // version is exactly what ocgen wrote, so no backup slot is used up.
    let mut p = reload(dir.path());
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(make:*)")
        .unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert!(backups(dir.path()).is_empty());

    // A hand edit is still backed up — and only it.
    fs::write(dir.path().join(".claude/agents/reviewer.md"), "mine\n").unwrap();
    let mut p = reload(dir.path());
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(just:*)")
        .unwrap();
    p.scaffold(dir.path(), true).unwrap();
    let b = backups(dir.path());
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(read(&b[0], ".claude/agents/reviewer.md"), "mine\n");
    assert!(!b[0].join(SETTINGS).exists());
}

#[test]
fn forced_regeneration_adopts_mcp_servers_added_by_hand() {
    let dir = tempdir().unwrap();
    let mut p = claude("mcp");
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir.path(), false).unwrap();
    // `claude mcp add --scope project …`, one with a field ocgen doesn't model.
    let mut m = json_at(dir.path(), ".mcp.json");
    m["mcpServers"]["github"] = json!({
        "type": "http",
        "url": "https://api.githubcopilot.com/mcp/",
        "headers": { "Authorization": "Bearer ${GITHUB_PAT}" }
    });
    m["mcpServers"]["sentry"] = json!({
        "command": "npx",
        "args": ["-y", "@sentry/mcp-server"],
        "timeout": 30000
    });
    write_json(dir.path(), ".mcp.json", &m);

    // `ocgen add mcp db`.
    let mut p = reload(dir.path());
    p.claude.mcp_servers.push(server("db"));
    p.scaffold(dir.path(), true).unwrap();

    assert_eq!(
        server_names(dir.path()),
        BTreeSet::from(["db", "github", "sentry", "tf"].map(String::from))
    );
    let m = json_at(dir.path(), ".mcp.json");
    assert_eq!(m["mcpServers"]["github"]["type"], "http");
    assert_eq!(
        m["mcpServers"]["github"]["headers"]["Authorization"],
        "Bearer ${GITHUB_PAT}"
    );
    assert_eq!(
        m["mcpServers"]["sentry"]["args"],
        json!(["-y", "@sentry/mcp-server"])
    );
    assert_eq!(
        m["mcpServers"]["sentry"]["timeout"], 30000,
        "fields ocgen doesn't model survive"
    );
    let state = reload(dir.path());
    let names: Vec<&str> = state
        .claude
        .mcp_servers
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        names.contains(&"github") && names.contains(&"sentry"),
        "{names:?}"
    );
    // Nothing differs afterwards: the adoption is complete.
    assert!(state
        .plan_changes(dir.path())
        .unwrap()
        .iter()
        .all(|c| c.kind == ChangeKind::Unchanged));
}

#[test]
fn a_hand_added_server_with_fields_ocgen_reads_differently_round_trips() {
    let dir = tempdir().unwrap();
    claude("odd").scaffold(dir.path(), false).unwrap();
    // Keys ocgen also uses for its own fields, with values it doesn't model: an
    // argument that isn't a string, a `name` inside the entry, a stdio server
    // with a URL left over, an env value that is a number.
    let odd = json!({
        "odd": { "type": "stdio", "command": "uvx", "args": ["srv", 3] },
        "named": { "type": "stdio", "command": "npx", "args": ["-y", "x"], "name": "other" },
        "leftover": { "type": "stdio", "command": "npx", "url": "https://x/mcp" },
        "envnum": { "type": "stdio", "command": "npx", "env": { "PORT": 3000 } },
    });
    write_json(dir.path(), ".mcp.json", &json!({ "mcpServers": odd }));

    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    // The state still loads (every later command needs it)…
    let state = Project::load_state(dir.path()).expect("the state still loads");
    let mut names: Vec<&str> = state
        .claude
        .mcp_servers
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["envnum", "leftover", "named", "odd"]);
    // …and each server is written back as it was.
    assert_eq!(json_at(dir.path(), ".mcp.json")["mcpServers"], odd);
    // A second regeneration changes nothing.
    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    assert_eq!(json_at(dir.path(), ".mcp.json")["mcpServers"], odd);
}

#[test]
fn a_server_removed_from_the_project_leaves_mcp_json() {
    let dir = tempdir().unwrap();
    let mut p = claude("rm");
    p.claude.mcp_servers.push(server("tf"));
    p.claude.mcp_servers.push(server("db"));
    p.scaffold(dir.path(), false).unwrap();

    // `ocgen edit mcp tf` → Remove it.
    let mut p = reload(dir.path());
    p.claude.mcp_servers.retain(|s| s.name != "tf");
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(server_names(dir.path()), BTreeSet::from(["db".to_string()]));

    // …and the last one: no server is left for Claude Code to start.
    let mut p = reload(dir.path());
    p.claude.mcp_servers.clear();
    p.scaffold(dir.path(), true).unwrap();
    assert!(server_names(dir.path()).is_empty());
    let state = reload(dir.path());
    assert!(state.claude.mcp_servers.is_empty());
    assert!(state
        .plan_changes(dir.path())
        .unwrap()
        .iter()
        .all(|c| c.kind == ChangeKind::Unchanged));
}

#[test]
fn removing_the_last_server_backs_up_a_hand_edited_mcp_json() {
    let dir = tempdir().unwrap();
    let mut p = claude("rm2");
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir.path(), false).unwrap();
    let mut m = json_at(dir.path(), ".mcp.json");
    m["mcpServers"]["tf"]["args"] = json!(["-y", "tf-mcp", "--verbose"]);
    write_json(dir.path(), ".mcp.json", &m);
    let edited = read(dir.path(), ".mcp.json");

    let mut p = reload(dir.path());
    p.claude.mcp_servers.clear();
    p.scaffold(dir.path(), true).unwrap();
    assert!(server_names(dir.path()).is_empty());
    let b = backups(dir.path());
    assert_eq!(read(b.last().unwrap(), ".mcp.json"), edited);
}

// --------------------------------------------- files ocgen stops generating -----

#[test]
fn an_agent_turned_primary_loses_its_subagent_file() {
    let dir = tempdir().unwrap();
    claude("mode").scaffold(dir.path(), false).unwrap();
    assert!(dir.path().join(".claude/agents/reviewer.md").exists());

    let mut p = reload(dir.path());
    p.agents
        .iter_mut()
        .find(|a| a.name == "reviewer")
        .unwrap()
        .mode = "primary".into();
    p.scaffold(dir.path(), true).unwrap();
    assert!(!dir.path().join(".claude/agents/reviewer.md").exists());
}

#[test]
fn renames_leave_no_stale_files_in_either_tree() {
    let dir = tempdir().unwrap();
    let mut p = claude("both");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "both-plugin".into();
    p.skills.push(skill("alpha"));
    p.scaffold(dir.path(), false).unwrap();
    let plugin = "plugin/both-plugin";
    assert!(dir
        .path()
        .join(format!("{plugin}/agents/reviewer.md"))
        .exists());
    assert!(dir
        .path()
        .join(format!("{plugin}/skills/alpha/SKILL.md"))
        .exists());

    // `ocgen edit agent reviewer` → critic, `ocgen edit skill alpha` → beta.
    let mut p = reload(dir.path());
    p.agents
        .iter_mut()
        .find(|a| a.name == "reviewer")
        .unwrap()
        .name = "critic".into();
    p.skills[0].name = "beta".into();
    p.scaffold(dir.path(), true).unwrap();
    Project::remove_claude_agent_file(dir.path(), "reviewer").unwrap();
    Project::rename_claude_skill_dir(dir.path(), "alpha", "beta").unwrap();

    for base in [".claude", plugin] {
        assert!(dir.path().join(format!("{base}/agents/critic.md")).exists());
        assert!(
            !dir.path()
                .join(format!("{base}/agents/reviewer.md"))
                .exists(),
            "{base}: old agent left behind"
        );
        assert!(dir
            .path()
            .join(format!("{base}/skills/beta/SKILL.md"))
            .exists());
        assert!(
            !dir.path().join(format!("{base}/skills/alpha")).exists(),
            "{base}: old skill left behind"
        );
    }
}

fn team() -> Team {
    Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    }
}

#[test]
fn turning_agent_teams_off_removes_its_skills_and_hooks() {
    let dir = tempdir().unwrap();
    let mut p = claude("team");
    p.claude.team = team();
    p.scaffold(dir.path(), false).unwrap();
    assert!(dir.path().join(".claude/skills/team/SKILL.md").exists());
    // A hand edit to one of them: backed up before it goes.
    fs::write(
        dir.path().join(".claude/skills/team-plan/SKILL.md"),
        "my plan skill\n",
    )
    .unwrap();

    let mut p = reload(dir.path());
    p.claude.team.enabled = false;
    let plan = p.plan_changes(dir.path()).unwrap();
    let find = |rel: &str| plan.iter().find(|c| c.rel == rel).unwrap();
    assert_eq!(
        find(".claude/skills/team/SKILL.md").kind,
        ChangeKind::Removed
    );
    assert_eq!(
        find(".claude/skills/team/SKILL.md").hand_edited,
        Some(false)
    );
    assert_eq!(
        find(".claude/skills/team-plan/SKILL.md").hand_edited,
        Some(true)
    );
    p.scaffold(dir.path(), true).unwrap();

    for gone in [
        ".claude/skills/team",
        ".claude/skills/team-plan",
        ".claude/hooks/team-approval-gate.sh",
        ".claude/hooks/team-teammate-idle.sh",
    ] {
        assert!(!dir.path().join(gone).exists(), "{gone} left behind");
    }
    let b = backups(dir.path());
    assert_eq!(
        read(b.last().unwrap(), ".claude/skills/team-plan/SKILL.md"),
        "my plan skill\n"
    );
}

#[test]
fn a_kept_stale_file_is_left_alone() {
    let dir = tempdir().unwrap();
    let mut p = claude("keepstale");
    p.claude.team = team();
    p.scaffold(dir.path(), false).unwrap();
    let mut p = reload(dir.path());
    p.claude.team.enabled = false;
    // doctor → "Keep all changed files": nothing of it is deleted.
    let keep: BTreeSet<String> = p
        .plan_changes(dir.path())
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == ChangeKind::Removed)
        .map(|c| c.rel)
        .collect();
    assert!(keep.contains(".claude/skills/team/SKILL.md"));
    p.scaffold_keeping(dir.path(), &keep).unwrap();
    assert!(dir.path().join(".claude/skills/team/SKILL.md").exists());
}

#[test]
fn legacy_cleanup_removes_only_ocgens_own_commands() {
    let dir = tempdir().unwrap();
    let mut p = claude("legacy");
    p.claude.workflow.deliver = false;
    let cmds = dir.path().join(".claude/commands");
    fs::create_dir_all(&cmds).unwrap();
    // The user's own /deliver (ocgen's is off), with ordinary frontmatter.
    fs::write(
        cmds.join("deliver.md"),
        "---\ndescription: Deploy to staging\n---\nmine\n",
    )
    .unwrap();
    // What an older ocgen generated.
    fs::write(
        cmds.join("intake.md"),
        "---\ndescription: Structured intake interview to gather requirements before work starts\nargument-hint: \"[what we're building]\"\n---\nold\n",
    )
    .unwrap();
    fs::write(
        cmds.join("inquire.md"),
        "---\ndescription: Understand a codebase by asking questions — sharpen each one, answer with evidence, get nudged to the next\n---\nold\n",
    )
    .unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert!(
        cmds.join("deliver.md").exists(),
        "the user's command is kept"
    );
    assert!(!cmds.join("intake.md").exists());
    assert!(!cmds.join("inquire.md").exists());
}

#[test]
fn a_case_only_rename_keeps_the_agent() {
    let dir = tempdir().unwrap();
    let mut p = claude("case");
    p.agents
        .iter_mut()
        .find(|a| a.name == "reviewer")
        .unwrap()
        .name = "Reviewer".into();
    p.scaffold(dir.path(), false).unwrap();

    // `ocgen edit agent Reviewer` → reviewer (what `run_edit_agent` does).
    let mut p = reload(dir.path());
    p.agents
        .iter_mut()
        .find(|a| a.name == "Reviewer")
        .unwrap()
        .name = "reviewer".into();
    p.scaffold(dir.path(), true).unwrap();
    Project::remove_claude_agent_file(dir.path(), "Reviewer").unwrap();

    let agents = entries(&dir.path().join(".claude/agents"));
    assert!(agents.contains(&"reviewer.md".to_string()), "{agents:?}");
    assert!(!agents.contains(&"Reviewer.md".to_string()), "{agents:?}");
    assert!(read(dir.path(), ".claude/agents/reviewer.md").contains("name: reviewer\n"));
}

#[test]
fn a_file_that_is_not_utf8_is_a_conflict_and_is_backed_up() {
    let dir = tempdir().unwrap();
    let mut p = claude("utf16");
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir.path(), false).unwrap();
    // PowerShell 5.1 `>` writes UTF-16LE.
    let mut utf16 = vec![0xFF, 0xFE];
    for u in "{\"mcpServers\":{}}".encode_utf16() {
        utf16.extend(u.to_le_bytes());
    }
    fs::write(dir.path().join(".mcp.json"), &utf16).unwrap();

    let p = reload(dir.path());
    let plan = p.plan_changes(dir.path()).unwrap();
    let mcp = plan.iter().find(|c| c.rel == ".mcp.json").unwrap();
    assert_eq!(mcp.kind, ChangeKind::Modified, "never treated as absent");
    p.scaffold(dir.path(), true).unwrap();
    let b = backups(dir.path());
    assert_eq!(fs::read(b[0].join(".mcp.json")).unwrap(), utf16);
}

// ---------------------------------------------------- names from the state -----

#[test]
fn state_names_that_could_escape_the_project_are_refused() {
    for (pointer, bad) in [
        ("/agents/1/name", "../../../.claude/agents/helper"),
        ("/skills/0/name", "../x"),
        ("/claude/mcp_servers/0/name", "a/b"),
        ("/claude/plugin/repo_name", ".."),
    ] {
        let dir = tempdir().unwrap();
        let mut p = claude("names");
        p.skills.push(skill("good"));
        p.claude.mcp_servers.push(server("tf"));
        p.scaffold(dir.path(), false).unwrap();
        let mut state = json_at(dir.path(), STATE);
        *state.pointer_mut(pointer).unwrap() = json!(bad);
        if pointer.starts_with("/claude/plugin") {
            state["claude"]["output"]["plugin"] = json!(true);
        }
        write_json(dir.path(), STATE, &state);
        let err = format!("{:#}", Project::load_state(dir.path()).unwrap_err());
        assert!(err.contains(bad), "{pointer}: {err}");
    }
    // An OpenCode provider key is checked too.
    let dir = tempdir().unwrap();
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.project_name = "oc".into();
    p.agents = agent::default_pipeline("English", "mac").unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let rel = ".opencode/.ocgen-state.json";
    let mut state = json_at(dir.path(), rel);
    state["providers"][0]["key"] = json!("../mac");
    write_json(dir.path(), rel, &state);
    assert!(Project::load_state(dir.path()).is_err());
}

#[test]
fn a_plugin_repo_cannot_be_a_dot_folder() {
    assert!(ocgen::validate::owner_repo("me/..").is_err());
    assert!(ocgen::validate::owner_repo("me/.").is_err());
    assert!(ocgen::validate::owner_repo("../repo").is_err());
    assert!(ocgen::validate::owner_repo("me/repo.js").is_ok());
}

#[test]
fn generated_paths_from_the_state_never_leave_the_project() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("escape").scaffold(&dir, false).unwrap();
    let victim = outer.path().join("victim.txt");
    fs::write(&victim, "keep me\n").unwrap();
    let mut state = json_at(&dir, STATE);
    state["generated"]["../victim.txt"] = json!("0000000000000000");
    write_json(&dir, STATE, &state);

    reload(&dir).scaffold(&dir, true).unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me\n");
}

#[cfg(unix)]
#[test]
fn a_symlinked_output_file_is_refused() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("link").scaffold(&dir, false).unwrap();
    let victim = outer.path().join("rc");
    fs::write(&victim, "export SAFE=1\n").unwrap();
    let agent = dir.join(".claude/agents/reviewer.md");
    fs::remove_file(&agent).unwrap();
    std::os::unix::fs::symlink(&victim, &agent).unwrap();

    let err = reload(&dir).scaffold(&dir, true).unwrap_err().to_string();
    assert!(err.contains("reviewer.md"), "{err}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "export SAFE=1\n");
}

#[cfg(unix)]
#[test]
fn legacy_cleanup_never_reaches_outside_the_project() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("legacy-link").scaffold(&dir, false).unwrap();
    // `.claude/commands` shared from elsewhere, holding what looks like ocgen's.
    let shared = outer.path().join("shared-commands");
    fs::create_dir_all(&shared).unwrap();
    let old = "---\ndescription: Structured intake interview to gather requirements before work starts\n---\nold\n";
    fs::write(shared.join("intake.md"), old).unwrap();
    std::os::unix::fs::symlink(&shared, dir.join(".claude/commands")).unwrap();

    reload(&dir).scaffold(&dir, true).unwrap();
    assert_eq!(fs::read_to_string(shared.join("intake.md")).unwrap(), old);
}

#[cfg(unix)]
#[test]
fn a_symlinked_gitattributes_is_left_alone() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("attrs-link").scaffold(&dir, false).unwrap();
    let shared = outer.path().join("gitattributes");
    fs::write(&shared, "*.png binary\n").unwrap();
    fs::remove_file(dir.join(".gitattributes")).unwrap();
    std::os::unix::fs::symlink(&shared, dir.join(".gitattributes")).unwrap();

    // The user's file (wherever it lives) is theirs: no write through the link,
    // and no reason to refuse the whole regeneration either.
    reload(&dir).scaffold(&dir, true).unwrap();
    assert_eq!(fs::read_to_string(&shared).unwrap(), "*.png binary\n");
}

#[test]
fn a_coordinator_body_that_is_not_a_template_is_written_as_is() {
    let mut p = claude("body");
    let body = "Use `${{ secrets.GITHUB_TOKEN }}` in workflows; Helm reads {{ .Values.x }}.\n";
    p.agents
        .iter_mut()
        .find(|a| a.mode == "primary")
        .unwrap()
        .body = body.into();
    let files = p.render_all().expect("renders instead of failing");
    let team = files
        .iter()
        .find(|(rel, _)| rel.ends_with("rules/ocgen-team.md"))
        .unwrap();
    assert!(team.1.contains(body.trim_end()), "{}", team.1);
}

#[test]
fn a_coordinator_body_naming_an_unknown_value_keeps_its_text() {
    let mut p = claude("version");
    // Valid template syntax, but `version` means nothing to ocgen: the text stays.
    let body = "Bump {{ version }} in Cargo.toml before tagging.\n";
    p.agents
        .iter_mut()
        .find(|a| a.mode == "primary")
        .unwrap()
        .body = body.into();
    let files = p.render_all().unwrap();
    let team = files
        .iter()
        .find(|(rel, _)| rel.ends_with("rules/ocgen-team.md"))
        .unwrap();
    assert!(team.1.contains(body.trim_end()), "{}", team.1);
    // The archetype's own body still renders as a template.
    let p = claude("archetype");
    let files = p.render_all().unwrap();
    let team = &files
        .iter()
        .find(|(rel, _)| rel.ends_with("rules/ocgen-team.md"))
        .unwrap()
        .1;
    assert!(
        !team.contains("{%") && team.contains("- reviewer"),
        "{team}"
    );
}

// ------------------------------------------------------- the state's version -----

#[test]
fn the_state_records_which_ocgen_wrote_it() {
    let dir = tempdir().unwrap();
    claude("ver").scaffold(dir.path(), false).unwrap();
    let state = json_at(dir.path(), STATE);
    assert_eq!(state["ocgen_version"], ocgen::VERSION);
    assert_eq!(state["schema"], STATE_SCHEMA);
}

#[test]
fn a_state_from_a_newer_ocgen_is_never_regenerated() {
    for (schema, version) in [(STATE_SCHEMA + 1, ocgen::VERSION), (STATE_SCHEMA, "99.0.0")] {
        let dir = tempdir().unwrap();
        claude("newer").scaffold(dir.path(), false).unwrap();
        let mut state = json_at(dir.path(), STATE);
        state["schema"] = json!(schema);
        state["ocgen_version"] = json!(version);
        write_json(dir.path(), STATE, &state);
        let before = (read(dir.path(), STATE), read(dir.path(), SETTINGS));

        // It still loads (read-only commands work)…
        let mut p = reload(dir.path());
        // …but nothing regenerates it.
        p.claude.model = "sonnet".into();
        let err = p.scaffold(dir.path(), true).unwrap_err().to_string();
        assert!(err.contains(version) && err.contains("upgrade"), "{err}");
        // Nor does a project built afresh over it.
        let err = claude("fresh")
            .scaffold(dir.path(), true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("upgrade"), "{err}");
        assert_eq!(
            (read(dir.path(), STATE), read(dir.path(), SETTINGS)),
            before
        );

        ocgen()
            .args(["doctor", "--yes"])
            .arg(dir.path())
            .assert()
            .failure()
            .stderr(contains(version));
        ocgen().arg("landscape").arg(dir.path()).assert().success();
    }
}

#[test]
fn a_newer_state_this_ocgen_cannot_read_says_which_version_it_needs() {
    let dir = tempdir().unwrap();
    claude("newer-shape").scaffold(dir.path(), false).unwrap();
    let mut state = json_at(dir.path(), STATE);
    state["ocgen_version"] = json!("99.0.0");
    // A newer ocgen changed a field's shape.
    state["claude"]["team"] = json!("in-process");
    write_json(dir.path(), STATE, &state);
    let err = format!("{:#}", Project::load_state(dir.path()).unwrap_err());
    assert!(err.contains("99.0.0") && err.contains("upgrade"), "{err}");
}

#[test]
fn unknown_state_keys_survive_a_regeneration() {
    let dir = tempdir().unwrap();
    claude("future").scaffold(dir.path(), false).unwrap();
    let mut state = json_at(dir.path(), STATE);
    state["future_top"] = json!({ "x": 1 });
    state["claude"]["future_claude"] = json!([1, 2]);
    state["agents"][1]["future_agent"] = json!("y");
    write_json(dir.path(), STATE, &state);

    let mut p = reload(dir.path());
    p.claude.model = "sonnet".into();
    p.scaffold(dir.path(), true).unwrap();
    let state = json_at(dir.path(), STATE);
    assert_eq!(state["future_top"], json!({ "x": 1 }));
    assert_eq!(state["claude"]["future_claude"], json!([1, 2]));
    assert_eq!(state["agents"][1]["future_agent"], json!("y"));
    assert_eq!(state["claude"]["model"], "sonnet");
}

#[test]
fn a_state_without_a_version_loads_and_gets_one() {
    let dir = tempdir().unwrap();
    claude("old").scaffold(dir.path(), false).unwrap();
    let mut state = json_at(dir.path(), STATE);
    for k in ["ocgen_version", "schema", "generated_rules"] {
        state.as_object_mut().unwrap().remove(k);
    }
    write_json(dir.path(), STATE, &state);
    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    assert_eq!(json_at(dir.path(), STATE)["schema"], STATE_SCHEMA);
}

// --------------------------------------------- hand-added: what really is -----

#[test]
fn rules_ocgen_stopped_generating_are_not_adopted() {
    let dir = tempdir().unwrap();
    let p = claude("explore");
    assert!(p.prefers_explorer());
    p.scaffold(dir.path(), false).unwrap();
    hand_add(dir.path(), "deny", &["Bash(curl:*)"]);
    // The documented route for settings without a CLI: edit the state, run doctor.
    let mut state = json_at(dir.path(), STATE);
    state["claude"]["workflow"]["prefer_explorer"] = json!(false);
    write_json(dir.path(), STATE, &state);

    ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success();
    let s = json_at(dir.path(), SETTINGS);
    assert!(!rules(&s, "deny").contains(&"Agent(Explore)".to_string()));
    assert!(rules(&s, "deny").contains(&"Bash(curl:*)".to_string()));
    assert_eq!(reload(dir.path()).claude.permissions.deny, ["Bash(curl:*)"]);
}

#[test]
fn an_untouched_settings_json_holds_nothing_hand_added() {
    let dir = tempdir().unwrap();
    claude("untouched").scaffold(dir.path(), false).unwrap();
    // A state from before ocgen recorded its rules, with a toggle turned off.
    let mut state = json_at(dir.path(), STATE);
    state.as_object_mut().unwrap().remove("generated_rules");
    state["claude"]["workflow"]["prefer_explorer"] = json!(false);
    write_json(dir.path(), STATE, &state);
    let found = reload(dir.path()).hand_added(dir.path());
    assert!(found.rules.is_empty(), "{found:?}");
}

#[test]
fn a_state_without_recorded_rules_still_tells_generated_from_hand_added() {
    let dir = tempdir().unwrap();
    let mut p = claude("legacy-rules");
    p.claude.workflow.intent = true;
    assert!(p
        .generated_permissions()
        .deny
        .contains(&"Bash(gh issue create*)".to_string()));
    p.scaffold(dir.path(), false).unwrap();
    // A state from before ocgen recorded the rules it generated.
    let mut state = json_at(dir.path(), STATE);
    state.as_object_mut().unwrap().remove("generated_rules");
    write_json(dir.path(), STATE, &state);
    hand_add(dir.path(), "deny", &["Bash(curl:*)"]);

    // `ocgen edit intent --disable`: the rule /intent needed goes with it.
    let mut p = reload(dir.path());
    p.claude.workflow.intent = false;
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(reload(dir.path()).claude.permissions.deny, ["Bash(curl:*)"]);
}

#[test]
fn a_hand_added_rule_that_cannot_be_adopted_is_not_reported_as_kept() {
    let dir = tempdir().unwrap();
    let mut p = claude("conflict");
    p.claude
        .permissions
        .add(RuleList::Deny, "Bash(curl:*)")
        .unwrap();
    p.scaffold(dir.path(), false).unwrap();
    // By hand, the same rule in another list: a rule lives in one list.
    hand_add(dir.path(), "allow", &["Bash(curl:*)"]);

    let mut p = reload(dir.path());
    let notice = p
        .apply(dir.path(), &BTreeSet::new())
        .unwrap()
        .notice(dir.path())
        .join("\n");
    assert!(!notice.contains("kept"), "{notice}");
    assert!(
        notice.contains("Bash(curl:*) is already in the deny list"),
        "{notice}"
    );
    assert_eq!(reload(dir.path()).claude.permissions.deny, ["Bash(curl:*)"]);
}

#[test]
fn a_stricter_hand_added_copy_of_a_generated_rule_is_kept() {
    let dir = tempdir().unwrap();
    let p = claude("stricter");
    let generated = p.generated_permissions();
    assert!(generated.ask.contains(&"Bash(git push:*)".to_string()));
    assert!(generated.allow.contains(&"Edit".to_string()));
    p.scaffold(dir.path(), false).unwrap();
    // By hand: deny what ocgen only asks about (or allows). Claude Code checks
    // deny first, so these are in effect — and they are the user's, not ocgen's.
    hand_add(dir.path(), "deny", &["Bash(git push:*)", "Edit"]);

    let mut p = reload(dir.path());
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(make:*)")
        .unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(
        reload(dir.path()).claude.permissions.deny,
        ["Bash(git push:*)", "Edit"]
    );
}

#[test]
fn a_rule_the_user_removed_is_not_adopted_back() {
    let dir = tempdir().unwrap();
    let mut p = claude("removed");
    p.claude
        .permissions
        .add(RuleList::Allow, "WebSearch")
        .unwrap();
    p.scaffold(dir.path(), false).unwrap();
    hand_add(dir.path(), "allow", &["Bash(head:*)"]);

    // `ocgen edit permissions --remove WebSearch`.
    let mut p = reload(dir.path());
    assert!(p.claude.permissions.remove("WebSearch"));
    p.scaffold(dir.path(), true).unwrap();
    let allow = rules(&json_at(dir.path(), SETTINGS), "allow");
    assert!(!allow.contains(&"WebSearch".to_string()), "{allow:?}");
    assert!(allow.contains(&"Bash(head:*)".to_string()), "{allow:?}");
}

// ------------------------------------------------------------ line endings -----

#[test]
fn claude_projects_pin_lf_line_endings() {
    let dir = tempdir().unwrap();
    claude("lf").scaffold(dir.path(), false).unwrap();
    let attrs = read(dir.path(), ".gitattributes");
    assert!(attrs.contains(".claude/** text=auto eol=lf"), "{attrs}");
    assert!(attrs.contains(".mcp.json text=auto eol=lf"), "{attrs}");
    assert!(reload(dir.path())
        .plan_changes(dir.path())
        .unwrap()
        .iter()
        .all(|c| c.kind == ChangeKind::Unchanged));
}

#[test]
fn an_existing_gitattributes_gets_ocgens_block_once() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".gitattributes"), "*.png binary").unwrap();
    claude("lf2").scaffold(dir.path(), false).unwrap();
    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    let attrs = read(dir.path(), ".gitattributes");
    assert!(attrs.starts_with("*.png binary\n"), "{attrs}");
    assert_eq!(
        attrs.matches(".claude/** text=auto eol=lf").count(),
        1,
        "{attrs}"
    );
}

#[test]
fn the_plugin_tree_pins_lf_too() {
    let dir = tempdir().unwrap();
    let mut p = claude("lfp");
    p.claude.output = Output {
        project: false,
        plugin: true,
    };
    p.claude.plugin.repo_name = "lf-plugin".into();
    p.scaffold(dir.path(), false).unwrap();
    assert!(read(dir.path(), "plugin/lf-plugin/.gitattributes").contains("* text=auto eol=lf"));
    assert!(!dir.path().join(".gitattributes").exists());
}

#[test]
fn opencode_projects_get_no_gitattributes() {
    let dir = tempdir().unwrap();
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.project_name = "oc".into();
    p.agents = agent::default_pipeline("English", "mac").unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".gitattributes").exists());
}

#[test]
fn a_crlf_checkout_is_not_a_change() {
    let dir = tempdir().unwrap();
    let mut p = claude("crlf");
    p.claude.team = team();
    p.scaffold(dir.path(), false).unwrap();
    let state = reload(dir.path());
    // What git's autocrlf does on Windows.
    for rel in state.generated.keys() {
        let lf = read(dir.path(), rel);
        fs::write(dir.path().join(rel), lf.replace('\n', "\r\n")).unwrap();
    }
    let plan = state.plan_changes(dir.path()).unwrap();
    let differ: Vec<&str> = plan
        .iter()
        .filter(|c| c.kind != ChangeKind::Unchanged)
        .map(|c| c.rel.as_str())
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
    // A regeneration writes LF again and backs nothing up.
    state.scaffold(dir.path(), true).unwrap();
    assert!(!read(dir.path(), ".claude/hooks/team-approval-gate.sh").contains('\r'));
    assert!(backups(dir.path()).is_empty());
}

// ----------------------------------------------------------- the real binary -----

#[test]
fn edit_permissions_keeps_a_hand_added_rule_and_says_so() {
    let dir = tempdir().unwrap();
    claude("cli").scaffold(dir.path(), false).unwrap();
    hand_add(dir.path(), "deny", &["Bash(curl:*)"]);
    ocgen()
        .args(["edit", "permissions", "--allow", "Bash(make:*)", "--path"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("Bash(curl:*)"))
        .stdout(contains(".ocgen-backup/"))
        // The closing list of the user's rules includes the adopted one.
        .stdout(contains("Your permission rules (2)"));
    let s = json_at(dir.path(), SETTINGS);
    assert!(rules(&s, "deny").contains(&"Bash(curl:*)".to_string()));
    assert!(rules(&s, "allow").contains(&"Bash(make:*)".to_string()));
}

#[test]
fn doctor_keeps_mcp_servers_added_by_hand() {
    let dir = tempdir().unwrap();
    let mut p = claude("doc");
    p.claude.mcp_servers.push(server("tf"));
    p.scaffold(dir.path(), false).unwrap();
    let mut m = json_at(dir.path(), ".mcp.json");
    m["mcpServers"]["github"] = json!({ "type": "http", "url": "https://example.com/mcp" });
    write_json(dir.path(), ".mcp.json", &m);
    ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("github"));
    assert_eq!(
        server_names(dir.path()),
        BTreeSet::from(["github", "tf"].map(String::from))
    );
}

// ------------------------------------------------- links and leftovers -----

#[cfg(unix)]
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[cfg(unix)]
fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The user-owned files ocgen never writes: a link there (CLAUDE.md → AGENTS.md,
/// even a dangling one) is the user's business, not a reason to refuse.
#[cfg(unix)]
#[test]
fn a_linked_claude_md_is_left_alone() {
    use ocgen::claude::{INTENT_FILE_TEMPLATE, INTENT_ISSUE_TEMPLATE};
    let dir = tempdir().unwrap();
    let mut p = claude("agents-md");
    p.claude.workflow.intent = true;
    p.scaffold(dir.path(), false).unwrap();
    fs::rename(dir.path().join("CLAUDE.md"), dir.path().join("AGENTS.md")).unwrap();
    std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();
    for rel in [INTENT_ISSUE_TEMPLATE, INTENT_FILE_TEMPLATE] {
        fs::remove_file(dir.path().join(rel)).unwrap();
        std::os::unix::fs::symlink("gone.md", dir.path().join(rel)).unwrap();
    }
    let agents_md = read(dir.path(), "AGENTS.md");

    let mut p = reload(dir.path());
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(make:*)")
        .unwrap();
    let plan = p.plan_changes(dir.path()).unwrap();
    assert!(!plan.iter().any(|c| c.rel == "CLAUDE.md"));
    p.scaffold(dir.path(), true).unwrap();
    assert!(is_link(&dir.path().join("CLAUDE.md")));
    assert_eq!(read(dir.path(), "AGENTS.md"), agents_md);
    for rel in [INTENT_ISSUE_TEMPLATE, INTENT_FILE_TEMPLATE] {
        assert!(is_link(&dir.path().join(rel)), "{rel}");
    }
    assert!(rules(&json_at(dir.path(), SETTINGS), "allow").contains(&"Bash(make:*)".to_string()));
}

/// `.claude/hooks/git-pre-push.sh` may be chained from someone else's pre-push
/// hook, so it outlives the gate (it lets every push through then).
#[test]
fn turning_the_approval_gate_off_keeps_the_pre_push_script() {
    let dir = tempdir().unwrap();
    let mut p = claude("chain");
    p.claude.team = team();
    p.scaffold(dir.path(), false).unwrap();
    let script = ".claude/hooks/git-pre-push.sh";
    assert!(dir.path().join(script).exists());

    let mut p = reload(dir.path());
    p.claude.team.approval_gate = false;
    let plan = p.plan_changes(dir.path()).unwrap();
    assert!(!plan.iter().any(|c| c.rel == script), "{script} planned");
    p.scaffold(dir.path(), true).unwrap();
    assert!(dir.path().join(script).exists(), "{script} removed");
    assert!(!dir
        .path()
        .join(".claude/hooks/team-approval-gate.sh")
        .exists());
    // Teams off altogether: still there.
    let mut p = reload(dir.path());
    p.claude.team.enabled = false;
    p.scaffold(dir.path(), true).unwrap();
    assert!(dir.path().join(script).exists(), "{script} removed");
}

/// The line ocgen tells users to add to their own pre-push hook: it runs the
/// script when it is there, and passes when it isn't.
#[cfg(unix)]
#[test]
fn the_pre_push_chain_line_tolerates_a_missing_script() {
    let dir = tempdir().unwrap();
    let chain = ocgen::render::pre_push_chain(dir.path());
    let run = || {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{chain}\necho after"))
            .current_dir(dir.path())
            .output()
            .unwrap()
    };
    let out = run();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "after\n");
    fs::create_dir_all(dir.path().join(".claude/hooks")).unwrap();
    let script = dir.path().join(".claude/hooks/git-pre-push.sh");
    fs::write(&script, "exit 1\n").unwrap();
    let out = run();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty());
    fs::write(&script, "exit 0\n").unwrap();
    assert!(run().status.success());
}

/// In a monorepo the line names the project's own script, as git runs hooks
/// from the repository's top — so a nested project's gate isn't skipped as
/// "missing", even with a space in its path.
#[cfg(unix)]
#[test]
fn the_pre_push_chain_line_names_a_nested_projects_script() {
    let top = tempdir().unwrap();
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(top.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let project = top.path().join("services/my api");
    fs::create_dir_all(project.join(".claude/hooks")).unwrap();
    fs::write(project.join(".claude/hooks/git-pre-push.sh"), "exit 1\n").unwrap();
    let chain = ocgen::render::pre_push_chain(&project);
    assert!(
        chain.contains("services/my api/.claude/hooks/git-pre-push.sh"),
        "{chain}"
    );
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{chain}\necho after"))
        .current_dir(top.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{chain}: {out:?}");
}

/// The command form of a workflow skill ocgen renders: what an older ocgen
/// wrote to `.claude/commands/<name>.md`.
fn legacy_form(skill: &str, name: &str) -> String {
    skill.replacen(&format!("name: {name}\n"), "", 1).replacen(
        "disable-model-invocation: true\n",
        "",
        1,
    )
}

#[test]
fn a_hand_edited_legacy_command_is_backed_up_before_it_goes() {
    let dir = tempdir().unwrap();
    claude("legacy-edit").scaffold(dir.path(), false).unwrap();
    let cmds = dir.path().join(".claude/commands");
    fs::create_dir_all(&cmds).unwrap();
    let mine = "---\ndescription: Structured intake interview to gather requirements before work starts\n---\nOUR TEAM INTAKE: ask about compliance first.\n";
    fs::write(cmds.join("intake.md"), mine).unwrap();

    let plan = reload(dir.path()).plan_changes(dir.path()).unwrap();
    let c = plan
        .iter()
        .find(|c| c.rel == ".claude/commands/intake.md")
        .unwrap();
    assert_eq!(c.kind, ChangeKind::Removed);
    assert_eq!(c.hand_edited, None, "not known to be ocgen's");
    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    assert!(!cmds.join("intake.md").exists());
    let b = backups(dir.path());
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(read(&b[0], ".claude/commands/intake.md"), mine);
}

#[test]
fn an_untouched_legacy_command_goes_without_a_backup() {
    let dir = tempdir().unwrap();
    claude("legacy-same").scaffold(dir.path(), false).unwrap();
    let cmds = dir.path().join(".claude/commands");
    fs::create_dir_all(&cmds).unwrap();
    for name in ["intake", "fanout"] {
        let skill = read(dir.path(), &format!(".claude/skills/{name}/SKILL.md"));
        fs::write(cmds.join(format!("{name}.md")), legacy_form(&skill, name)).unwrap();
    }

    let plan = reload(dir.path()).plan_changes(dir.path()).unwrap();
    for name in ["intake", "fanout"] {
        let rel = format!(".claude/commands/{name}.md");
        let c = plan.iter().find(|c| c.rel == rel).unwrap();
        assert_eq!(c.hand_edited, Some(false), "{rel}");
    }
    reload(dir.path()).scaffold(dir.path(), true).unwrap();
    assert!(!cmds.exists());
    assert!(backups(dir.path()).is_empty());
}

/// A `.mcp.json` linked in from elsewhere (a shared config): ocgen neither adopts
/// its servers nor writes it — and says so.
#[cfg(unix)]
#[test]
fn a_linked_mcp_json_is_neither_adopted_nor_written() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("mcp-link").scaffold(&dir, false).unwrap();
    let shared = outer.path().join("shared-mcp.json");
    let body =
        "{\"mcpServers\":{\"github\":{\"type\":\"http\",\"url\":\"https://example.com/mcp\"}}}\n";
    fs::write(&shared, body).unwrap();
    std::os::unix::fs::symlink(&shared, dir.join(".mcp.json")).unwrap();

    let mut p = reload(&dir);
    assert!(p.hand_added(&dir).servers.is_empty());
    let applied = p.apply(&dir, &BTreeSet::new()).unwrap();
    assert!(applied.adopted.servers.is_empty());
    let notice = applied.notice(&dir).join("\n");
    assert!(
        notice.contains(".mcp.json") && notice.contains("symbolic link"),
        "{notice}"
    );
    assert!(reload(&dir).claude.mcp_servers.is_empty());
    assert!(is_link(&dir.join(".mcp.json")));
    assert_eq!(fs::read_to_string(&shared).unwrap(), body);
}

#[cfg(unix)]
#[test]
fn a_linked_settings_json_is_not_adopted_from() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("settings-link").scaffold(&dir, false).unwrap();
    hand_add(&dir, "deny", &["Bash(curl:*)"]);
    let shared = outer.path().join("settings.json");
    fs::rename(dir.join(SETTINGS), &shared).unwrap();
    std::os::unix::fs::symlink(&shared, dir.join(SETTINGS)).unwrap();

    let found = reload(&dir).hand_added(&dir);
    assert!(found.rules.is_empty(), "{found:?}");
    assert_eq!(found.linked, [SETTINGS]);
}

/// A loosening rule moved by hand (an ask ocgen generates, put under allow) is
/// kept as the user's, but settings.json leaves it out — so the report says it
/// has no effect instead of listing it as kept.
#[test]
fn a_hand_added_rule_with_no_effect_is_reported_as_such() {
    let dir = tempdir().unwrap();
    claude("noeffect").scaffold(dir.path(), false).unwrap();
    let mut s = json_at(dir.path(), SETTINGS);
    let push = "Bash(git push:*)";
    s["permissions"]["ask"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r != push);
    s["permissions"]["allow"]
        .as_array_mut()
        .unwrap()
        .extend([json!(push), json!("Bash(make:*)")]);
    write_json(dir.path(), SETTINGS, &s);

    let mut p = reload(dir.path());
    p.claude.intent.prefix = "RFC".into();
    let notice = p
        .apply(dir.path(), &BTreeSet::new())
        .unwrap()
        .notice(dir.path())
        .join("\n");
    assert!(
        notice.contains(
            "kept 1 permission rule(s) added to settings.json by hand as yours: Bash(make:*)\n"
        ),
        "{notice}"
    );
    assert!(
        notice.contains(&format!(
            "{push} (allow) is kept in your rules but has no effect: ocgen's ask rule wins"
        )),
        "{notice}"
    );
    let s = json_at(dir.path(), SETTINGS);
    assert!(rules(&s, "ask").contains(&push.to_string()));
    assert!(!rules(&s, "allow").contains(&push.to_string()));
}

#[test]
fn doctor_reports_a_hand_added_rule_with_no_effect() {
    let dir = tempdir().unwrap();
    claude("doc-noeffect").scaffold(dir.path(), false).unwrap();
    let mut s = json_at(dir.path(), SETTINGS);
    let push = "Bash(git push:*)";
    s["permissions"]["ask"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r != push);
    s["permissions"]["allow"]
        .as_array_mut()
        .unwrap()
        .push(json!(push));
    write_json(dir.path(), SETTINGS, &s);
    let out = ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "{push} (allow) is kept in your rules but has no effect: ocgen's ask rule wins"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("kept 1 rule(s)"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn doctor_leaves_a_linked_mcp_json_alone() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("doc-mcp-link").scaffold(&dir, false).unwrap();
    let shared = outer.path().join("shared-mcp.json");
    let body =
        "{\"mcpServers\":{\"github\":{\"type\":\"http\",\"url\":\"https://example.com/mcp\"}}}\n";
    fs::write(&shared, body).unwrap();
    std::os::unix::fs::symlink(&shared, dir.join(".mcp.json")).unwrap();
    let out = ocgen()
        .args(["doctor", "--yes"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains(".mcp.json is a symbolic link: ocgen adopts nothing from it"),
        "{stdout}"
    );
    assert!(!stdout.contains("kept MCP server"), "{stdout}");
    assert!(is_link(&dir.join(".mcp.json")));
    assert_eq!(fs::read_to_string(&shared).unwrap(), body);
}

/// `.claude` kept in a dotfiles store and linked in: the user made that link, so
/// ocgen writes through it — in a git repository (the link untracked or
/// git-ignored) and outside one.
#[cfg(unix)]
#[test]
fn a_claude_dir_the_user_linked_in_is_regenerated_through_the_link() {
    for (repo, ignored) in [(true, false), (true, true), (false, false)] {
        let outer = tempdir().unwrap();
        let dir = outer.path().join("proj");
        fs::create_dir_all(&dir).unwrap();
        if repo {
            git(&dir, &["init", "-q"]);
        }
        claude("dotfiles").scaffold(&dir, false).unwrap();
        if ignored {
            fs::write(dir.join(".gitignore"), ".claude\n").unwrap();
        }
        let store = outer.path().join("store/proj-claude");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::rename(dir.join(".claude"), &store).unwrap();
        std::os::unix::fs::symlink(&store, dir.join(".claude")).unwrap();

        let mut p = reload(&dir);
        p.claude
            .permissions
            .add(RuleList::Allow, "Bash(make:*)")
            .unwrap();
        p.plan_changes(&dir).unwrap();
        p.scaffold(&dir, true)
            .unwrap_or_else(|e| panic!("repo={repo} ignored={ignored}: {e:#}"));
        assert!(is_link(&dir.join(".claude")));
        let s: Value =
            serde_json::from_str(&fs::read_to_string(store.join("settings.json")).unwrap())
                .unwrap();
        assert!(rules(&s, "allow").contains(&"Bash(make:*)".to_string()));
    }
}

/// A linked folder the repository itself brings in (git tracks the link) may
/// point anywhere — e.g. into `~/.claude` — so ocgen won't write through it.
#[cfg(unix)]
#[test]
fn a_linked_folder_git_tracks_is_refused_when_it_leads_outside() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    claude("tracked-link").scaffold(&dir, false).unwrap();
    let elsewhere = outer.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("reviewer.md"), "theirs\n").unwrap();
    fs::remove_dir_all(dir.join(".claude/agents")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join(".claude/agents")).unwrap();
    assert!(git(&dir, &["add", ".claude/agents"]).status.success());

    let err = format!("{:#}", reload(&dir).scaffold(&dir, true).unwrap_err());
    assert!(
        err.contains(".claude/agents") && err.contains("git tracks"),
        "{err}"
    );
    assert_eq!(entries(&elsewhere), ["reviewer.md"]);
    assert_eq!(
        fs::read_to_string(elsewhere.join("reviewer.md")).unwrap(),
        "theirs\n"
    );
    // verify names the refusal and what to do — not "could not render".
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(!stdout.contains("could not render"), "{stdout}");
    assert!(stdout.contains("git tracks"), "{stdout}");
}

/// A submodule — any repository checked out inside the project — brings its
/// links in too: one it tracks that leads out of it is refused like the
/// project's own.
#[cfg(unix)]
#[test]
fn a_linked_folder_a_nested_repository_tracks_is_refused() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    claude("nested-link").scaffold(&dir, false).unwrap();
    let elsewhere = outer.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    // `.claude` checked out from a repository of its own, its `agents` a link out.
    let claude_dir = dir.join(".claude");
    git(&claude_dir, &["init", "-q"]);
    fs::remove_dir_all(claude_dir.join("agents")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, claude_dir.join("agents")).unwrap();
    assert!(git(&claude_dir, &["add", "agents"]).status.success());

    let err = format!("{:#}", reload(&dir).scaffold(&dir, true).unwrap_err());
    assert!(
        err.contains(".claude/agents") && err.contains("git tracks"),
        "{err}"
    );
    assert!(entries(&elsewhere).is_empty(), "{:?}", entries(&elsewhere));
}

/// Past a link the user made, the tree is theirs: a dotfiles store may track
/// links of its own, and ocgen writes through them.
#[cfg(unix)]
#[test]
fn links_inside_a_store_the_user_linked_in_are_theirs() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    claude("store-links").scaffold(&dir, false).unwrap();
    let dotfiles = outer.path().join("dotfiles");
    let store = dotfiles.join("proj-claude");
    fs::create_dir_all(&dotfiles).unwrap();
    fs::rename(dir.join(".claude"), &store).unwrap();
    std::os::unix::fs::symlink(&store, dir.join(".claude")).unwrap();
    let shared = dotfiles.join("shared-agents");
    fs::rename(store.join("agents"), &shared).unwrap();
    std::os::unix::fs::symlink("../shared-agents", store.join("agents")).unwrap();
    git(&dotfiles, &["init", "-q"]);
    assert!(git(&dotfiles, &["add", "proj-claude/agents"])
        .status
        .success());
    fs::remove_file(shared.join("reviewer.md")).unwrap();

    let p = reload(&dir);
    p.plan_changes(&dir).unwrap();
    p.scaffold(&dir, true).unwrap();
    assert!(shared.join("reviewer.md").exists());
}

/// In a monorepo a project may link a folder shared across the repository: git
/// tracks that link, but it stays in the repository, so ocgen writes through it.
#[cfg(unix)]
#[test]
fn a_tracked_link_that_stays_in_the_repository_is_written_through() {
    let outer = tempdir().unwrap();
    let repo = outer.path().join("mono");
    let dir = repo.join("services/api");
    fs::create_dir_all(&dir).unwrap();
    git(&repo, &["init", "-q"]);
    claude("mono").scaffold(&dir, false).unwrap();
    let shared = repo.join("shared/agents");
    fs::create_dir_all(shared.parent().unwrap()).unwrap();
    fs::rename(dir.join(".claude/agents"), &shared).unwrap();
    std::os::unix::fs::symlink("../../../shared/agents", dir.join(".claude/agents")).unwrap();
    assert!(
        git(&repo, &["add", "services/api/.claude/agents", "shared"])
            .status
            .success()
    );
    fs::remove_file(shared.join("reviewer.md")).unwrap();

    let p = reload(&dir);
    p.plan_changes(&dir).unwrap();
    p.scaffold(&dir, true).unwrap();
    assert!(shared.join("reviewer.md").exists());
    assert!(is_link(&dir.join(".claude/agents")));
    // verify agrees.
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("✔ files up to date"), "{stdout}");

    // Out of the repository, the same tracked link is refused.
    let outside = outer.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_file(dir.join(".claude/agents")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join(".claude/agents")).unwrap();
    assert!(git(&repo, &["add", "services/api/.claude/agents"])
        .status
        .success());
    let err = format!("{:#}", reload(&dir).scaffold(&dir, true).unwrap_err());
    assert!(err.contains("git tracks"), "{err}");
    assert!(entries(&outside).is_empty());
}

/// The state file is an output like the others: never written through a link.
#[cfg(unix)]
#[test]
fn a_linked_state_file_is_refused() {
    let outer = tempdir().unwrap();
    let dir = outer.path().join("proj");
    claude("state-link").scaffold(&dir, false).unwrap();
    let state = dir.join(".claude/.ocgen-state.json");
    let elsewhere = outer.path().join("state.json");
    fs::rename(&state, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &state).unwrap();
    let before = fs::read_to_string(&elsewhere).unwrap();

    let mut p = reload(&dir);
    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(make:*)")
        .unwrap();
    let err = format!("{:#}", p.plan_changes(&dir).unwrap_err());
    assert!(err.contains("symbolic link"), "{err}");
    let err = format!("{:#}", p.scaffold(&dir, true).unwrap_err());
    assert!(err.contains("symbolic link"), "{err}");
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), before);
    assert!(!rules(&json_at(&dir, SETTINGS), "allow").contains(&"Bash(make:*)".to_string()));
}

// ------------------------------------------------------------ CODEOWNERS -----

const OWNERS: &str = ".github/CODEOWNERS";
const MINE: &str = "* @owner\n# the team's own rules\n";
const OCGEN_MARK: &str =
    "# ocgen: /intent approvers (ocgen edit intent --approver / --codeowners-scope)";

/// A project with an existing CODEOWNERS of its own (not linked yet).
fn with_codeowners(name: &str, rel: &str) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    claude(name).scaffold(dir.path(), false).unwrap();
    let p = dir.path().join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, MINE).unwrap();
    dir
}

/// Reload, change the /intent settings, regenerate as `ocgen edit intent` does.
fn edit_intent(dir: &Path, f: impl FnOnce(&mut ocgen::claude::IntentSettings)) -> Vec<String> {
    let mut p = reload(dir);
    f(&mut p.claude.intent);
    p.apply(dir, &BTreeSet::new()).unwrap().notice(dir)
}

fn block(rule: &str) -> String {
    format!("{MINE}\n{OCGEN_MARK}\n{rule}\n# ocgen: end\n")
}

#[test]
fn codeowners_is_never_created() {
    let dir = tempdir().unwrap();
    claude("co-none").scaffold(dir.path(), false).unwrap();
    edit_intent(dir.path(), |s| s.approvers = vec!["@alice".into()]);
    for rel in [
        OWNERS,
        "CODEOWNERS",
        "docs/CODEOWNERS",
        ".claude/CODEOWNERS",
    ] {
        assert!(fs::symlink_metadata(dir.path().join(rel)).is_err(), "{rel}");
    }
}

#[test]
fn a_linked_codeowners_gets_the_approvers_block_and_a_link() {
    let dir = with_codeowners("co-link", OWNERS);
    edit_intent(dir.path(), |s| {
        s.approvers = vec!["@alice".into(), "@org/architects".into()];
        s.codeowners = OWNERS.into();
    });
    assert_eq!(
        read(dir.path(), OWNERS),
        block("/docs/adr/ @alice @org/architects")
    );
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(dir.path().join(".claude/CODEOWNERS")).unwrap(),
        Path::new("../.github/CODEOWNERS")
    );
    // Nothing more to do on the next run.
    let plan = reload(dir.path()).plan_changes(dir.path()).unwrap();
    let owners = plan.iter().find(|c| c.rel == OWNERS).unwrap();
    assert_eq!(owners.kind, ChangeKind::Unchanged);

    // The scope `all`: every change.
    edit_intent(dir.path(), |s| {
        s.codeowners_scope = ocgen::claude::CodeownersScope::All
    });
    assert_eq!(read(dir.path(), OWNERS), block("* @alice @org/architects"));
}

#[test]
fn the_codeowners_block_follows_the_approvers_and_leaves_your_lines() {
    let dir = with_codeowners("co-follow", OWNERS);
    edit_intent(dir.path(), |s| {
        s.approvers = vec!["@alice".into()];
        s.codeowners = OWNERS.into();
    });
    // A rule of the user's after the block, which takes precedence over it.
    let path = dir.path().join(OWNERS);
    let text = format!("{}/docs/ @docs-team\n", read(dir.path(), OWNERS));
    fs::write(&path, &text).unwrap();
    let notice = edit_intent(dir.path(), |s| s.approvers.push("@bob".into()));
    assert_eq!(
        read(dir.path(), OWNERS),
        format!("{}/docs/ @docs-team\n", block("/docs/adr/ @alice @bob"))
    );
    assert!(
        notice
            .iter()
            .any(|l| l.contains("take precedence") && l.contains("/docs/ @docs-team")),
        "{notice:?}"
    );
}

#[test]
fn the_codeowners_block_goes_and_your_file_stays() {
    let dir = with_codeowners("co-off", OWNERS);
    let link = |s: &mut ocgen::claude::IntentSettings| {
        s.approvers = vec!["@alice".into()];
        s.codeowners = OWNERS.into();
        s.codeowners_scope = ocgen::claude::CodeownersScope::Intents;
    };
    edit_intent(dir.path(), link);
    edit_intent(dir.path(), |s| {
        s.codeowners_scope = ocgen::claude::CodeownersScope::Off
    });
    assert_eq!(read(dir.path(), OWNERS), MINE, "scope off");
    edit_intent(dir.path(), link);
    edit_intent(dir.path(), |s| s.approvers.clear());
    assert_eq!(read(dir.path(), OWNERS), MINE, "no approvers");
    edit_intent(dir.path(), link);
    edit_intent(dir.path(), |s| s.codeowners.clear());
    assert_eq!(read(dir.path(), OWNERS), MINE, "unlinked");
    assert!(fs::symlink_metadata(dir.path().join(".claude/CODEOWNERS")).is_err());
}

#[test]
fn a_root_codeowners_can_be_linked() {
    let dir = with_codeowners("co-root", "CODEOWNERS");
    edit_intent(dir.path(), |s| {
        s.approvers = vec!["@alice".into()];
        s.codeowners = "CODEOWNERS".into();
    });
    assert_eq!(read(dir.path(), "CODEOWNERS"), block("/docs/adr/ @alice"));
    assert!(!dir.path().join(OWNERS).exists());
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(dir.path().join(".claude/CODEOWNERS")).unwrap(),
        Path::new("../CODEOWNERS")
    );
}

#[test]
fn a_linked_codeowners_that_went_away_is_reported_not_recreated() {
    let dir = with_codeowners("co-gone", OWNERS);
    edit_intent(dir.path(), |s| {
        s.approvers = vec!["@alice".into()];
        s.codeowners = OWNERS.into();
    });
    fs::remove_file(dir.path().join(OWNERS)).unwrap();
    let notice = edit_intent(dir.path(), |s| s.approvers.push("@bob".into()));
    assert!(!dir.path().join(OWNERS).exists());
    assert!(
        notice
            .iter()
            .any(|l| l.contains(OWNERS) && l.contains("--codeowners")),
        "{notice:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_linked_codeowners_replaced_by_a_symlink_is_left_alone() {
    let dir = with_codeowners("co-sym", OWNERS);
    edit_intent(dir.path(), |s| {
        s.approvers = vec!["@alice".into()];
        s.codeowners = OWNERS.into();
    });
    let elsewhere = tempdir().unwrap();
    let theirs = elsewhere.path().join("CODEOWNERS");
    fs::write(&theirs, "* @someone\n").unwrap();
    fs::remove_file(dir.path().join(OWNERS)).unwrap();
    std::os::unix::fs::symlink(&theirs, dir.path().join(OWNERS)).unwrap();
    let notice = edit_intent(dir.path(), |s| s.approvers.push("@bob".into()));
    assert_eq!(fs::read_to_string(&theirs).unwrap(), "* @someone\n");
    assert!(
        notice.iter().any(|l| l.contains("symbolic link")),
        "{notice:?}"
    );
}

#[test]
fn an_old_state_without_approvers_loads() {
    let dir = tempdir().unwrap();
    claude("co-old").scaffold(dir.path(), false).unwrap();
    let mut state = json_at(dir.path(), STATE);
    let intent = state["claude"]["intent"].as_object_mut().unwrap();
    for k in ["approvers", "codeowners", "codeowners_scope"] {
        intent.remove(k);
    }
    write_json(dir.path(), STATE, &state);
    let p = reload(dir.path());
    assert!(p.claude.intent.approvers.is_empty());
    assert!(p.claude.intent.codeowners.is_empty());
    assert_eq!(
        p.claude.intent.codeowners_scope,
        ocgen::claude::CodeownersScope::Intents
    );
}

/// An older ocgen doesn't know the approvers: it refuses to rewrite a state a
/// newer one wrote rather than drop them.
#[test]
fn approvers_written_by_a_newer_ocgen_are_never_dropped() {
    let dir = tempdir().unwrap();
    claude("co-newer").scaffold(dir.path(), false).unwrap();
    let mut state = json_at(dir.path(), STATE);
    state["ocgen_version"] = json!("99.0.0");
    state["claude"]["intent"]["approvers"] = json!(["@alice"]);
    write_json(dir.path(), STATE, &state);
    let before = read(dir.path(), STATE);
    let err = reload(dir.path())
        .scaffold(dir.path(), true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("upgrade"), "{err}");
    assert_eq!(read(dir.path(), STATE), before);
}
