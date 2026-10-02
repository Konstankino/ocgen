//! The user's permission rules against the generated ones, the rules the approval
//! gate always brings, and the sandbox profile: what settings.json ends up with.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use ocgen::agent;
use ocgen::claude::{RuleList, SandboxProfile};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use predicates::str::contains;
use tempfile::tempdir;

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

fn claude_default(name: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

/// A project with Agent Teams and the approval gate on.
fn gated(name: &str) -> Project {
    let mut p = claude_default(name);
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p
}

fn settings_of(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(dir.join(".claude/settings.json")).unwrap()).unwrap()
}

/// One permissions list of a settings.json (empty when the list is absent).
fn list(s: &serde_json::Value, key: &str) -> Vec<String> {
    s["permissions"][key]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn has(s: &serde_json::Value, key: &str, rule: &str) -> bool {
    list(s, key).iter().any(|r| r == rule)
}

/// Every rule sits in exactly one list (Claude Code would apply the strictest
/// anyway, but a rule in two lists reads as a contradiction).
fn assert_one_list_per_rule(s: &serde_json::Value) {
    let mut seen = std::collections::BTreeMap::new();
    for key in ["allow", "ask", "deny"] {
        for r in list(s, key) {
            if let Some(other) = seen.insert(r.clone(), key) {
                panic!("{r} is in both {other} and {key}");
            }
        }
    }
}

// ------------------------------------------------ stricter user rules win -----

#[test]
fn tighten_keeps_the_strictest_copy_in_one_list() {
    use ocgen::claude::PermissionRules;
    let mut r = PermissionRules::default();
    assert_eq!(r.tighten(RuleList::Ask, "Bash(git push:*)"), RuleList::Ask);
    // Looser or equal: stays where it is.
    assert_eq!(
        r.tighten(RuleList::Allow, "Bash(git push:*)"),
        RuleList::Ask
    );
    assert_eq!(r.tighten(RuleList::Ask, "Bash(git push:*)"), RuleList::Ask);
    assert_eq!(r.ask, ["Bash(git push:*)"]);
    assert!(r.allow.is_empty());
    // Stricter: moves.
    assert_eq!(
        r.tighten(RuleList::Deny, " Bash(git push:*) "),
        RuleList::Deny
    );
    assert!(r.ask.is_empty());
    assert_eq!(r.deny, ["Bash(git push:*)"]);
    assert_eq!(r.list_of("Bash(git push:*)"), Some(RuleList::Deny));
    assert_eq!(r.list_of("Edit"), None);
    assert!(RuleList::Deny.stricter_than(RuleList::Ask));
    assert!(RuleList::Ask.stricter_than(RuleList::Allow));
    assert!(!RuleList::Allow.stricter_than(RuleList::Allow));
}

#[test]
fn a_user_deny_of_a_generated_ask_rule_lands_in_deny() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("deny-ask");
    p.claude
        .permissions
        .add(RuleList::Deny, "Bash(git push:*)")
        .unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(has(&s, "deny", "Bash(git push:*)"), "{s}");
    assert!(!has(&s, "ask", "Bash(git push:*)"), "the looser copy goes");
    assert!(has(&s, "ask", "Bash(git commit:*)"), "the rest stays");
    assert_one_list_per_rule(&s);
}

#[test]
fn a_user_deny_or_ask_of_a_generated_allow_rule_replaces_it() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("deny-allow");
    let mine = &mut p.claude.permissions;
    mine.add(RuleList::Deny, "Edit").unwrap();
    mine.add(RuleList::Deny, "Write").unwrap();
    mine.add(RuleList::Ask, "Bash(git log:*)").unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    for r in ["Edit", "Write"] {
        assert!(has(&s, "deny", r) && !has(&s, "allow", r), "{r}: {s}");
    }
    assert!(has(&s, "ask", "Bash(git log:*)") && !has(&s, "allow", "Bash(git log:*)"));
    assert!(has(&s, "allow", "Read"), "the rest of allow stays");
    assert_one_list_per_rule(&s);
}

#[test]
fn a_user_rule_looser_than_a_generated_guard_is_left_out() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("looser");
    let mine = &mut p.claude.permissions;
    mine.add(RuleList::Allow, "Bash(git push:*)").unwrap(); // generated ask
    mine.add(RuleList::Ask, "Bash(ocgen approve*)").unwrap(); // generated deny
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(has(&s, "ask", "Bash(git push:*)") && !has(&s, "allow", "Bash(git push:*)"));
    assert!(has(&s, "deny", "Bash(ocgen approve*)") && !has(&s, "ask", "Bash(ocgen approve*)"));
    assert_one_list_per_rule(&s);
}

#[test]
fn a_rule_the_user_moved_by_hand_to_a_stricter_list_is_kept_by_doctor() {
    let dir = tempdir().unwrap();
    claude_default("moved").scaffold(dir.path(), false).unwrap();
    // Move `git push` from ask to deny by hand.
    let path = dir.path().join(".claude/settings.json");
    let mut s = settings_of(dir.path());
    let ask = s["permissions"]["ask"].as_array_mut().unwrap();
    ask.retain(|r| r != "Bash(git push:*)");
    s["permissions"]["deny"]
        .as_array_mut()
        .unwrap()
        .push("Bash(git push:*)".into());
    fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).unwrap();

    ocgen()
        .args(["doctor", "--yes"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("kept 1 rule(s) as yours"));
    let s = settings_of(dir.path());
    assert!(has(&s, "deny", "Bash(git push:*)"), "{s}");
    assert!(!has(&s, "ask", "Bash(git push:*)"));
}

#[test]
fn edit_permissions_deny_of_a_generated_rule_is_written_and_explained() {
    let dir = tempdir().unwrap();
    claude_default("cli").scaffold(dir.path(), false).unwrap();
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--deny", "Bash(git push:*)", "--deny", "Edit"])
        .assert()
        .success()
        .stdout(contains("replaces ocgen's ask rule"))
        .stdout(contains("replaces ocgen's allow rule"));
    let s = settings_of(dir.path());
    assert!(has(&s, "deny", "Bash(git push:*)") && !has(&s, "ask", "Bash(git push:*)"));
    assert!(has(&s, "deny", "Edit") && !has(&s, "allow", "Edit"));

    // A looser rule keeps the generated guard, with a warning.
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--allow", "Bash(ssh *)"])
        .assert()
        .success()
        .stdout(contains("no effect"));
    let s = settings_of(dir.path());
    assert!(has(&s, "ask", "Bash(ssh *)") && !has(&s, "allow", "Bash(ssh *)"));
}

/// The lines of one list's block in `--list` output.
fn block<'a>(out: &'a str, key: &str) -> Vec<&'a str> {
    let mut lines = out
        .lines()
        .skip_while(|l| !l.trim_start().starts_with(&format!("{key} (")));
    let mut v = vec![lines.next().unwrap_or_else(|| panic!("no {key} block"))];
    v.extend(lines.take_while(|l| !l.trim().is_empty()));
    v
}

#[test]
fn edit_permissions_list_shows_what_settings_json_holds() {
    let dir = tempdir().unwrap();
    claude_default("list").scaffold(dir.path(), false).unwrap();
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--deny", "Bash(git push:*)", "--allow", "Bash(ssh *)"])
        .assert()
        .success();
    let s = settings_of(dir.path());
    let out = ocgen()
        .args(["edit", "permissions", "--list", "-p"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();

    let deny = block(&out, "deny");
    let ask = block(&out, "ask");
    let allow = block(&out, "allow");
    // Headers count the rules settings.json holds.
    for (key, b) in [("deny", &deny), ("ask", &ask), ("allow", &allow)] {
        assert_eq!(
            b[0].trim(),
            format!("{key} ({})", list(&s, key).len()),
            "{out}"
        );
    }
    let line = |b: &[&str], rule: &str| -> String {
        b.iter()
            .find(|l| l.trim_start().starts_with(rule))
            .unwrap_or_else(|| panic!("{rule} not listed:\n{out}"))
            .to_string()
    };
    assert!(line(&deny, "Bash(git push:*)").contains("yours"));
    assert!(line(&deny, "Bash(git push:*)").contains("replaces ocgen's ask rule"));
    assert!(
        !ask.iter().any(|l| l.contains("Bash(git push:*)")),
        "moved out of ask:\n{out}"
    );
    // Yours that settings.json doesn't hold are flagged with the rule that wins.
    let ssh = line(&allow, "Bash(ssh *)");
    assert!(
        ssh.contains("no effect") && ssh.contains("ocgen's ask rule wins"),
        "{ssh}"
    );
}

// ------------------------------------------------ approval-gate guard rules -----

#[test]
fn the_approval_gate_always_guards_its_own_files() {
    use ocgen::claude::{GATE_ASK, GATE_DENY};
    // Even without the permission defaults: the sandbox covers Bash, not the
    // file tools.
    let dir = tempdir().unwrap();
    let mut p = gated("gate-bare");
    p.claude.powerups.permissions = false;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(has(&s, "deny", "Edit(~/.claude/ocgen/**)"), "{s}");
    for r in [
        "Edit(/.claude/hooks/**)",
        "Edit(/.claude/settings.json)",
        "Edit(/.claude/settings.local.json)",
        "Edit(/.claude/.ocgen-state.json)",
        "Edit(//**/.git/hooks/**)",
    ] {
        assert!(has(&s, "ask", r), "missing {r}: {s}");
        assert!(GATE_ASK.contains(&r));
    }
    assert!(GATE_DENY.contains(&"Edit(~/.claude/ocgen/**)"));
    assert!(list(&s, "allow").is_empty(), "nothing pre-allowed: {s}");
    assert_one_list_per_rule(&s);

    // With the defaults the guard deny isn't repeated.
    let dir = tempdir().unwrap();
    gated("gate-full").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let deny = list(&s, "deny");
    assert_eq!(
        deny.iter()
            .filter(|r| *r == "Edit(~/.claude/ocgen/**)")
            .count(),
        1
    );
    assert!(has(&s, "ask", "Edit(/.claude/settings.json)"));
    assert!(has(&s, "allow", "Edit"), "edits elsewhere stay pre-allowed");
    assert_one_list_per_rule(&s);

    // The state-file rule names the real state file.
    assert!(GATE_ASK.contains(&format!("Edit(/{})", Target::ClaudeCode.state_file()).as_str()));
}

#[test]
fn the_approval_gate_keeps_agents_off_the_store_and_ocgens_own_controls() {
    // Without the permission defaults too: an agent can't mint an approval, and
    // a human sees it when an agent runs `ocgen edit …` or `ocgen doctor`,
    // which rewrite the rules and hooks the gate relies on.
    let dir = tempdir().unwrap();
    let mut p = gated("gate-cli");
    p.claude.powerups.permissions = false;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    for r in ["Edit(~/.claude/ocgen/**)", "Bash(ocgen approve*)"] {
        assert!(has(&s, "deny", r), "missing deny {r}: {s}");
    }
    for r in ["Bash(ocgen edit*)", "Bash(ocgen doctor*)"] {
        assert!(has(&s, "ask", r), "missing ask {r}: {s}");
    }
    assert_one_list_per_rule(&s);

    // With the defaults, nothing is listed twice.
    let dir = tempdir().unwrap();
    gated("gate-cli-full").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(has(&s, "ask", "Bash(ocgen edit*)"), "{s}");
    assert_one_list_per_rule(&s);
}

#[test]
fn without_the_gate_no_gate_rules_are_added() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("nogate");
    p.claude.team.enabled = true; // teams on, gate off
    p.claude.powerups.permissions = false;
    p.claude.workflow.intent = false;
    p.claude.workflow.prefer_explorer = false;
    p.scaffold(dir.path(), false).unwrap();
    assert!(settings_of(dir.path()).get("permissions").is_none());
}

#[test]
fn a_user_allow_of_a_gate_ask_rule_has_no_effect() {
    let dir = tempdir().unwrap();
    gated("gate-cli").scaffold(dir.path(), false).unwrap();
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--allow", "Edit(/.claude/settings.json)"])
        .assert()
        .success()
        .stdout(contains("no effect"));
    let s = settings_of(dir.path());
    assert!(has(&s, "ask", "Edit(/.claude/settings.json)"));
    assert!(!has(&s, "allow", "Edit(/.claude/settings.json)"));
}

// ------------------------------------------------------------------ sandbox -----

#[test]
fn sandbox_support_follows_the_platform() {
    let expected = cfg!(any(target_os = "macos", target_os = "linux"));
    assert_eq!(ocgen::claude::sandbox_supported(), expected);
}

#[test]
fn a_declined_sandbox_is_remembered_and_old_states_load() {
    // States written before the field existed load as "never asked".
    let old: SandboxProfile = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
    assert!(!old.declined);

    let dir = tempdir().unwrap();
    let mut p = gated("declined");
    p.claude.sandbox.declined = true;
    p.scaffold(dir.path(), false).unwrap();
    assert!(settings_of(dir.path()).get("sandbox").is_none());
    let (_, back) = Project::discover(dir.path()).unwrap();
    assert!(back.claude.sandbox.declined);
    assert!(!back.claude.sandbox.enabled);
}

#[test]
fn your_rules_are_labelled_alike_after_an_edit_and_in_the_list() {
    let dir = tempdir().unwrap();
    claude_default("labels")
        .scaffold(dir.path(), false)
        .unwrap();
    // Printed after the change: which rule wins, as `--list` words it.
    ocgen()
        .args(["edit", "permissions", "-p"])
        .arg(dir.path())
        .args(["--allow", "Bash(ssh *)"])
        .assert()
        .success()
        .stdout(contains("Bash(ssh *)  no effect: ocgen's ask rule wins"));
}

#[test]
fn edit_permissions_list_reads_padded_rules_from_a_hand_edited_state() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("padded");
    // A state edited by hand: the rules carry stray spaces.
    p.claude.permissions.allow = vec![" Grep ".into()];
    p.claude.permissions.deny = vec![" Bash(git push:*) ".into()];
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(has(&s, "deny", "Bash(git push:*)") && !has(&s, "ask", "Bash(git push:*)"));

    let out = ocgen()
        .args(["edit", "permissions", "--list", "-p"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    // Both are in effect (Grep is ocgen's allow already): neither is flagged.
    assert!(!out.contains("no effect"), "{out}");
    let deny = block(&out, "deny");
    assert_eq!(
        deny.iter()
            .filter(|l| l.contains("Bash(git push:*)"))
            .count(),
        1,
        "listed once:\n{out}"
    );
}

// ------------------------------------------- what the sandbox protects -----

/// A string list in a settings.json (empty when absent).
fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// A gated project with the sandbox on.
fn sandboxed(name: &str) -> Project {
    let mut p = gated(name);
    p.claude.sandbox.enabled = true;
    p
}

#[test]
fn the_sandbox_write_protects_what_runs_outside_it() {
    // Claude Code runs the status line, and ocgen regenerates the hooks from
    // its state file, outside the sandbox; a human's own `git push` runs
    // .git/hooks unsandboxed. Paths are written as the sandbox reads them: `~/`
    // the home folder, `./` the project root.
    let dir = tempdir().unwrap();
    sandboxed("sb-protect").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let no_write = strings(&s["sandbox"]["filesystem"]["denyWrite"]);
    for want in [
        "~/.claude/ocgen",
        "./.claude/statusline.sh",
        "./.claude/.ocgen-state.json",
        "./.git/hooks",
    ] {
        assert!(no_write.iter().any(|w| w == want), "missing {want}: {s}");
    }
    // Without the status line there is no script to protect.
    let dir = tempdir().unwrap();
    let mut p = sandboxed("sb-nostatus");
    p.claude.powerups.statusline = false;
    p.scaffold(dir.path(), false).unwrap();
    let no_write = strings(&settings_of(dir.path())["sandbox"]["filesystem"]["denyWrite"]);
    assert!(
        !no_write.iter().any(|w| w.contains("statusline")),
        "{no_write:?}"
    );
    assert!(no_write.iter().any(|w| w == "~/.claude/ocgen"));
}

#[test]
fn the_approval_gate_asks_before_the_status_line_changes() {
    use ocgen::claude::GATE_ASK;
    assert!(GATE_ASK.contains(&"Edit(/.claude/statusline.sh)"));
    let dir = tempdir().unwrap();
    gated("gate-status").scaffold(dir.path(), false).unwrap();
    assert!(has(
        &settings_of(dir.path()),
        "ask",
        "Edit(/.claude/statusline.sh)"
    ));
}

#[test]
fn the_approval_store_stays_readable_so_the_pre_push_hook_sees_an_approval() {
    // Claude Code feeds Read deny rules into the sandbox, so neither a denyRead
    // nor a Read rule may cover ~/.claude/ocgen: the sandboxed pre-push hook
    // reads the approval there. Writes stay denied.
    use ocgen::claude::{GATE_DENY, GUARD_DENY};
    for r in GATE_DENY.iter().chain(GUARD_DENY.iter()) {
        assert!(!r.starts_with("Read(~/.claude/ocgen"), "{r}");
    }
    let mut bare = sandboxed("sb-read-bare");
    bare.claude.powerups.permissions = false;
    for p in [sandboxed("sb-read"), bare] {
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        let s = settings_of(dir.path());
        assert!(
            s["sandbox"]["filesystem"].get("denyRead").is_none(),
            "no read-deny of the store: {s}"
        );
        for key in ["allow", "ask", "deny"] {
            assert!(
                !list(&s, key)
                    .iter()
                    .any(|r| r.starts_with("Read") && r.contains(".claude/ocgen")),
                "{key}: {s}"
            );
        }
        assert!(has(&s, "deny", "Edit(~/.claude/ocgen/**)"));
        assert!(strings(&s["sandbox"]["filesystem"]["denyWrite"])
            .iter()
            .any(|w| w == "~/.claude/ocgen"));
    }
}

/// The paths a list of permission rules for `tool` names, in sandbox syntax:
/// `Read(~/.ssh/**)` → `~/.ssh` (rules with other wildcards are left out).
#[cfg(target_os = "macos")]
fn rule_paths(rules: &[String], tool: &str) -> Vec<String> {
    rules
        .iter()
        .filter_map(|r| r.strip_prefix(tool)?.strip_prefix('(')?.strip_suffix(')'))
        .map(|p| p.strip_suffix("/**").unwrap_or(p).to_string())
        .filter(|p| !p.contains('*'))
        .collect()
}

/// The pre-push backstop runs inside the sandbox (a sandboxed `git push` starts
/// it), so it must still read an approval there. Rebuilds what the generated
/// settings deny — denyWrite and Edit deny rules for writes; denyRead, the
/// withheld credential files and Read deny rules for reads — as a Seatbelt
/// profile, and runs the installed hook under it.
#[cfg(target_os = "macos")]
#[test]
fn an_approved_push_passes_the_pre_push_hook_inside_the_sandbox() {
    use ocgen::hooks::{seatbelt_profile, SandboxSpec};
    let repo = tempdir().unwrap();
    let home = tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    sandboxed("sb-push").scaffold(repo.path(), false).unwrap();
    let s = settings_of(repo.path());
    let fs_ = &s["sandbox"]["filesystem"];
    let mut write = strings(&fs_["denyWrite"]);
    write.extend(rule_paths(&list(&s, "deny"), "Edit"));
    let mut read = strings(&fs_["denyRead"]);
    read.extend(
        s["sandbox"]["credentials"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string()),
    );
    read.extend(rule_paths(&list(&s, "deny"), "Read"));
    ocgen::approval::grant(home.path(), repo.path(), 10).unwrap();

    let push = |deny_read: &str| {
        let profile = seatbelt_profile(&SandboxSpec {
            deny_write: &write.join(" "),
            deny_read,
            home: home.path().to_str().unwrap(),
            project: repo.path(),
            dir: repo.path(),
        });
        std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", &profile, "/bin/sh", ".git/hooks/pre-push"])
            .current_dir(repo.path())
            .env("HOME", home.path())
            .env("CLAUDECODE", "1")
            .output()
            .unwrap()
    };
    let o = push(&read.join(" "));
    assert_eq!(
        o.status.code(),
        Some(0),
        "an approved push goes through: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    // What a read-deny of the store did: the hook couldn't see the approval.
    let o = push(&format!("{} ~/.claude/ocgen", read.join(" ")));
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn the_sandbox_withholds_package_registry_and_git_credentials() {
    use ocgen::claude::{CREDENTIAL_ENV, CREDENTIAL_PATHS};
    for f in [
        "~/.npmrc",
        "~/.yarnrc.yml",
        "~/.cargo/credentials",
        "~/.cargo/credentials.toml",
        "~/.git-credentials",
        "~/.config/git/credentials",
        "~/.pypirc",
        "~/.gem/credentials",
    ] {
        assert!(CREDENTIAL_PATHS.contains(&f), "{f}");
    }
    for v in [
        "YARN_NPM_AUTH_TOKEN",
        "TWINE_PASSWORD",
        "TWINE_USERNAME",
        "GEM_HOST_API_KEY",
        "PYPI_TOKEN",
    ] {
        assert!(CREDENTIAL_ENV.contains(&v), "{v}");
    }
    let dir = tempdir().unwrap();
    sandboxed("sb-creds").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let files: Vec<&str> = s["sandbox"]["credentials"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, CREDENTIAL_PATHS);
}

#[test]
fn verify_fails_a_local_override_that_unsandboxes_the_hooks() {
    use ocgen::verify::{verify, Options, Status};
    let dir = tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    let mut p = sandboxed("sb-local");
    p.claude.workflow.check_cmd = "true".into();
    p.scaffold(dir.path(), false).unwrap();
    let user = tempdir().unwrap();
    let effective = |local: serde_json::Value| {
        fs::write(
            dir.path().join(".claude/settings.local.json"),
            local.to_string(),
        )
        .unwrap();
        let checks = verify(
            &Project::load_state(dir.path()).unwrap(),
            dir.path(),
            &Options {
                run_claude: false,
                run_check: false,
                user_settings: Some(user.path().join("settings.json")),
            },
        );
        checks
            .into_iter()
            .find(|c| c.name.contains("effective settings"))
            .unwrap()
    };
    for env in [
        serde_json::json!({ "OCGEN_SANDBOX": "0" }),
        serde_json::json!({ "OCGEN_SANDBOX_DENY_WRITE": "~/.claude" }),
        serde_json::json!({ "OCGEN_SANDBOX_DENY_ENV": "" }),
    ] {
        let c = effective(serde_json::json!({ "env": env }));
        assert_eq!(c.status, Status::Fail, "{env}: {c:#?}");
        assert!(c.detail.contains("OCGEN_SANDBOX"), "{c:#?}");
    }
    // More protection only differs.
    let more = format!("{} ~/.extra", ocgen::claude::HOOK_DENY_WRITE.join(" "));
    let c = effective(serde_json::json!({ "env": { "OCGEN_SANDBOX_DENY_WRITE": more } }));
    assert_eq!(c.status, Status::Warn, "{c:#?}");
}

/// The env a project's hooks read, from its settings.json.
fn hook_env(dir: &Path) -> serde_json::Map<String, serde_json::Value> {
    settings_of(dir)["env"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn hooks_learn_the_sandbox_from_the_settings_env() {
    // The check and the formatter run in a hook, outside Claude Code's sandbox:
    // the hook needs to know what to sandbox them with.
    use ocgen::claude::{CREDENTIAL_ENV, CREDENTIAL_PATHS, HOOK_DENY_WRITE};
    let dir = tempdir().unwrap();
    let mut p = sandboxed("sb-env");
    p.claude.workflow.check_cmd = "cargo test".into();
    p.scaffold(dir.path(), false).unwrap();
    let env = hook_env(dir.path());
    assert_eq!(env["OCGEN_SANDBOX"], "1");
    let words = |k: &str| -> Vec<String> {
        env[k]
            .as_str()
            .unwrap()
            .split_whitespace()
            .map(String::from)
            .collect()
    };
    let write = words("OCGEN_SANDBOX_DENY_WRITE");
    assert_eq!(write, HOOK_DENY_WRITE);
    for w in [
        "~/.claude",
        "./.claude/settings.json",
        "./.claude/settings.local.json",
        "./.claude/hooks",
        "./.claude/statusline.sh",
        "./.claude/.ocgen-state.json",
        "./.git/hooks",
        "./.git/config",
    ] {
        assert!(write.iter().any(|x| x == w), "missing {w}: {write:?}");
    }
    assert_eq!(words("OCGEN_SANDBOX_DENY_READ"), CREDENTIAL_PATHS);
    assert_eq!(words("OCGEN_SANDBOX_DENY_ENV"), CREDENTIAL_ENV);

    // A formatter alone needs it too (format.sh reads the settings env).
    let dir = tempdir().unwrap();
    let mut p = claude_default("sb-fmt");
    p.claude.sandbox.enabled = true;
    p.claude.workflow.check_cmd.clear();
    p.claude.workflow.subagent_confidence = 0;
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    p.scaffold(dir.path(), false).unwrap();
    assert_eq!(hook_env(dir.path())["OCGEN_SANDBOX"], "1");

    // Credentials allowed: only the write protection is left.
    let dir = tempdir().unwrap();
    let mut p = sandboxed("sb-env-creds");
    p.claude.workflow.check_cmd = "cargo test".into();
    p.claude.sandbox.allow_credentials = true;
    p.scaffold(dir.path(), false).unwrap();
    let env = hook_env(dir.path());
    assert_eq!(env["OCGEN_SANDBOX"], "1");
    assert!(env.contains_key("OCGEN_SANDBOX_DENY_WRITE"));
    assert!(!env.contains_key("OCGEN_SANDBOX_DENY_READ"));
    assert!(!env.contains_key("OCGEN_SANDBOX_DENY_ENV"));

    // No sandbox, or nothing for a hook to run: no sandbox env.
    let dir = tempdir().unwrap();
    let mut p = gated("sb-env-off");
    p.claude.workflow.check_cmd = "cargo test".into();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!hook_env(dir.path()).contains_key("OCGEN_SANDBOX"));
    let dir = tempdir().unwrap();
    let mut p = sandboxed("sb-env-idle");
    p.claude.workflow.check_cmd.clear();
    p.claude.hooks_extra.format_cmd.clear();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!hook_env(dir.path()).contains_key("OCGEN_SANDBOX"));
}
