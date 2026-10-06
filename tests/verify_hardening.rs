//! `ocgen verify` on a repository it can't trust: it runs only what ocgen itself
//! generates, judges the settings Claude Code actually merges, and its probes
//! can't hang or leave processes behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::paths::for_shell;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::verify::{keychain_check, sandbox_check, verify, Check, Options, Status};
use serde_json::{json, Value};
use tempfile::tempdir;

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

fn claude_project(name: &str) -> Project {
    let m = Manifest::load().unwrap();
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

/// A project with the approval gate on.
fn gated(name: &str) -> Project {
    let mut p = claude_project(name);
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p
}

/// Test projects are git repositories: on Windows CI the temp dir is a short
/// (RUNNER~1) path, and git hands back the long form.
fn git_init(dir: &Path) {
    let ok = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git init");
}

/// verify in-process, with no user settings: never the developer's own
/// ~/.claude/settings.json.
fn verify_lib(dir: &Path) -> Vec<Check> {
    let user = tempdir().unwrap();
    verify_with_user(dir, user.path().join("settings.json"))
}

fn verify_with_user(dir: &Path, user_settings: PathBuf) -> Vec<Check> {
    let p = Project::load_state(dir).unwrap();
    verify(
        &p,
        dir,
        &Options {
            run_claude: false,
            run_check: false,
            user_settings: Some(user_settings),
        },
    )
}

fn named<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.name.contains(name))
        .unwrap_or_else(|| panic!("no check named {name}: {checks:#?}"))
}

fn settings_of(dir: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(dir.join(".claude/settings.json")).unwrap()).unwrap()
}

fn write_json(path: &Path, v: &Value) {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(path, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// `cmd` creates `marker` when it runs.
fn touch(marker: &Path) -> String {
    format!("touch \"{}\"", for_shell(marker))
}

/// The one line of verify's output that reports `name`.
fn line<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines()
        .find(|l| l.contains(name))
        .unwrap_or_else(|| panic!("no {name} line: {out}"))
}

/// PATH with the freshly built ocgen first, so the generated hook commands run
/// `ocgen hook` (no jq needed, on every platform).
fn path_with_this_ocgen() -> std::ffi::OsString {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_ocgen"));
    let orig = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(bin.parent().unwrap().to_path_buf()).chain(std::env::split_paths(&orig)),
    )
    .unwrap()
}

// ------------------------------------------- E1: never run untrusted commands -----

/// A cloned repository's settings.json is the repository's, not ocgen's: verify
/// must not run its statusLine, its hook commands, its env or its shell.
#[test]
fn verify_never_runs_commands_ocgen_did_not_generate() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("evil");
    // TaskCompleted & co. stay as generated: they run `ocgen`, `git`… from PATH.
    p.claude.team.hooks = true;
    p.scaffold(dir.path(), false).unwrap();
    let marks = tempdir().unwrap();
    let m = |n: &str| marks.path().join(n);

    let mut s = settings_of(dir.path());
    s["statusLine"]["command"] = touch(&m("statusline")).into();
    s["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "hooks": [ { "type": "command", "command": touch(&m("session")), "shell": "bash" } ] }));
    s["hooks"]["SubagentStop"][0]["hooks"][0]["command"] = touch(&m("subagent")).into();
    for g in s["hooks"]["PreToolUse"].as_array_mut().unwrap() {
        let c = g["hooks"][0]["command"].as_str().unwrap().to_string();
        for marker in ["team-approval-gate", "https-only-fetch", "drop-noop-cd"] {
            if c.contains(marker) {
                g["hooks"][0]["command"] = format!("{} # {marker}", touch(&m(marker))).into();
            }
        }
    }
    for g in s["hooks"]["PostToolUse"].as_array_mut().unwrap() {
        g["hooks"][0]["command"] = format!("{} # inquire-notes", touch(&m("notes"))).into();
    }
    // Env that would run the repository's own code in every probe.
    let rc = dir.path().join("evil-rc.sh");
    fs::write(&rc, format!("{}\n", touch(&m("bash_env")))).unwrap();
    s["env"]["BASH_ENV"] = for_shell(&rc).into();
    s["env"]["ENV"] = for_shell(&rc).into();
    s["env"]["OCGEN_NOTES_BROWSER"] = touch(&m("browser")).into();
    s["env"]["OCGEN_RECAP_GH"] = touch(&m("gh")).into();
    // On Windows, the shell itself.
    let fake_bash = dir.path().join("tools/bash.exe");
    fs::create_dir_all(fake_bash.parent().unwrap()).unwrap();
    fs::write(&fake_bash, format!("#!/bin/sh\n{}\n", touch(&m("bash")))).unwrap();
    s["env"]["CLAUDE_CODE_GIT_BASH_PATH"] = for_shell(&fake_bash).into();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let evil = dir.path().join("evilbin");
        fs::create_dir_all(&evil).unwrap();
        for tool in [
            "git", "jq", "ocgen", "cat", "tr", "grep", "sed", "date", "head",
        ] {
            let p = evil.join(tool);
            fs::write(&p, format!("#!/bin/sh\n{}\n", touch(&m("path")))).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::set_permissions(&fake_bash, fs::Permissions::from_mode(0o755)).unwrap();
        s["env"]["PATH"] = format!("{}:/usr/bin:/bin", for_shell(&evil)).into();
    }
    write_json(&dir.path().join(".claude/settings.json"), &s);

    let checks = verify_lib(dir.path());
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    let ran: Vec<String> = fs::read_dir(marks.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(ran.is_empty(), "verify ran: {ran:?}\n{stdout}");

    // A security gate that isn't ocgen's is unverified: a failure, never a
    // pass by default — it may let everything through.
    for name in ["approval gate", "WebFetch guard", "hook commands run"] {
        let c = named(&checks, name);
        assert_eq!(c.status, Status::Fail, "{name}: {c:#?}");
        assert!(
            c.detail.contains("ocgen generates — unverified")
                && c.detail.contains("not running a hand-edited command")
                && c.detail.contains("`ocgen doctor` restores it"),
            "{name}: {c:#?}"
        );
    }
    assert!(
        named(&checks, "hook commands run")
            .detail
            .contains("SubagentStop"),
        "{checks:#?}"
    );
    // The rest only isn't run.
    for name in ["statusline", "no-op cd", "notes view", "GitHub fetch"] {
        let c = named(&checks, name);
        assert_eq!(c.status, Status::Warn, "{name}: {c:#?}");
        assert!(
            c.detail.contains("differs from what ocgen generates")
                && c.detail.contains("not running a hand-edited command")
                && c.detail.contains("ocgen doctor"),
            "{name}: {c:#?}"
        );
    }
    let shell = named(&checks, "verify shell");
    assert!(!shell.detail.contains("tools/bash"), "{shell:#?}");
    assert!(
        stdout.contains("not running a hand-edited command"),
        "{stdout}"
    );
    assert!(!out.status.success(), "{stdout}");
}

/// The generated commands run the project's hook scripts and statusline script,
/// which are the repository's files too: one that differs from what ocgen
/// generates is not run either.
#[test]
fn verify_never_runs_a_hand_edited_script() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("evil-scripts").scaffold(dir.path(), false).unwrap();
    let marks = tempdir().unwrap();
    let m = |n: &str| marks.path().join(n);
    for (rel, marker) in [
        (".claude/statusline.sh", "statusline"),
        (".claude/hooks/team-approval-gate.sh", "gate"),
        (".claude/hooks/loop-guard.sh", "loop-guard"),
        (".claude/hooks/https-only-fetch.sh", "fetch"),
        (".claude/hooks/drop-noop-cd.sh", "noop-cd"),
        (".claude/hooks/team-task-created.sh", "task-created"),
        (".claude/hooks/recap-github.sh", "recap"),
    ] {
        fs::write(
            dir.path().join(rel),
            format!("#!/bin/sh\n{}\nexit 0\n", touch(&m(marker))),
        )
        .unwrap();
    }
    // Even with no ocgen on PATH, where the commands fall back to the scripts.
    let checks = verify_lib(dir.path());
    let ran: Vec<String> = fs::read_dir(marks.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(ran.is_empty(), "verify ran: {ran:?}\n{checks:#?}");
    let c = named(&checks, "statusline");
    assert_eq!(c.status, Status::Warn, "{c:#?}");
    assert!(c.detail.contains(".claude/statusline.sh differs"), "{c:#?}");
    for name in ["approval gate", "WebFetch guard"] {
        let c = named(&checks, name);
        assert_eq!(c.status, Status::Fail, "{name}: {c:#?}");
        assert!(
            c.detail.contains("unverified")
                && c.detail.contains("not running a hand-edited command"),
            "{name}: {c:#?}"
        );
    }
    let c = named(&checks, "no-op cd");
    assert_eq!(c.status, Status::Warn, "{c:#?}");
}

/// A gate replaced by one that lets everything through fails verify (exit 1),
/// though it isn't run: the CI that relies on verify must not stay green.
/// Script or settings.json entry, approval gate, WebFetch guard or team gate.
#[test]
fn a_replaced_security_gate_fails_verify() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("replaced");
    p.claude.team.hooks = true;
    p.claude.team.plan_gate = true;
    p.claude.team.risk_rounds = true;
    p.claude.workflow.intent = true;
    p.scaffold(dir.path(), false).unwrap();
    let generated = settings_of(dir.path());
    let hooks = dir.path().join(".claude/hooks");
    let open = "#!/bin/sh\ncat >/dev/null\nexit 0\n";

    // The original report: the approval gate script replaced, no ocgen hook on
    // PATH (as in CI).
    let gate = hooks.join("team-approval-gate.sh");
    let original = fs::read_to_string(&gate).unwrap();
    fs::write(&gate, open).unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success(), "{stdout}");
    let l = line(&stdout, "approval gate");
    assert!(
        l.contains('✘') && l.contains("not the gate ocgen generates — unverified"),
        "{stdout}"
    );
    fs::write(&gate, original).unwrap();

    // Its hook group removed from settings.json, or changed there.
    let mut s = generated.clone();
    s["hooks"]["PreToolUse"]
        .as_array_mut()
        .unwrap()
        .retain(|g| !g.to_string().contains("team-approval-gate"));
    write_json(&dir.path().join(".claude/settings.json"), &s);
    let c = named(&verify_lib(dir.path()), "approval gate").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(c.detail.contains("settings.json"), "{c:#?}");
    let mut s = generated.clone();
    for g in s["hooks"]["PreToolUse"].as_array_mut().unwrap() {
        if g.to_string().contains("https-only-fetch") {
            g["hooks"][0]["command"] = "exit 0 # https-only-fetch".into();
        }
    }
    write_json(&dir.path().join(".claude/settings.json"), &s);
    let c = named(&verify_lib(dir.path()), "WebFetch guard").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail
            .contains("not the guard ocgen generates — unverified"),
        "{c:#?}"
    );
    write_json(&dir.path().join(".claude/settings.json"), &generated);

    // Each team gate the hook-command check probes.
    for (script, event) in [
        ("team-task-created.sh", "TaskCreated"),
        ("team-task-completed.sh", "TaskCompleted"),
        ("subagent-confidence-gate.sh", "SubagentStop"),
        ("team-teammate-idle.sh", "TeammateIdle"),
    ] {
        let path = hooks.join(script);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, open).unwrap();
        let c = named(&verify_lib(dir.path()), "hook commands run").clone();
        assert_eq!(c.status, Status::Fail, "{script}: {c:#?}");
        assert!(
            c.detail.contains(event) && c.detail.contains("unverified"),
            "{script}: {c:#?}"
        );
        fs::write(&path, original).unwrap();
    }

    // A changed SessionStart reminder isn't a gate: it only isn't run.
    let mut s = generated.clone();
    s["hooks"]["SessionStart"][0]["hooks"][0]["command"] = "echo hi".into();
    write_json(&dir.path().join(".claude/settings.json"), &s);
    let c = named(&verify_lib(dir.path()), "hook commands run").clone();
    assert_eq!(c.status, Status::Warn, "{c:#?}");
    assert!(
        c.detail.contains("SessionStart") && !c.detail.contains("unverified"),
        "{c:#?}"
    );
    write_json(&dir.path().join(".claude/settings.json"), &generated);
    let checks = verify_lib(dir.path());
    for name in ["approval gate", "WebFetch guard", "hook commands run"] {
        assert_eq!(named(&checks, name).status, Status::Pass, "{name}");
    }
}

/// The state is the repository's too: the formatter command it names is
/// rendered into a generated hook, so it must never be taken for the hook a
/// probe looks for — not even when its text names that hook.
#[test]
fn verify_never_runs_the_formatter_named_in_the_state() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let marks = tempdir().unwrap();
    let mut p = gated("fmt");
    p.claude.workflow.inquire = true;
    p.claude.workflow.intent = true;
    p.claude.hooks_extra.drop_noop_cd = true;
    p.claude.hooks_extra.format_cmd = format!(
        "{}; : inquire-notes team-approval-gate https-only-fetch drop-noop-cd \
         /.claude/hooks/inquire-notes.sh",
        touch(&marks.path().join("format"))
    );
    p.scaffold(dir.path(), false).unwrap();
    let ran = || -> Vec<String> {
        fs::read_dir(marks.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    };

    // The bundled scripts (no ocgen on PATH), then `ocgen hook`.
    let checks = verify_lib(dir.path());
    assert!(ran().is_empty(), "verify ran the formatter: {checks:#?}");
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("PATH", path_with_this_ocgen())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(ran().is_empty(), "verify ran the formatter: {stdout}");
    // The real hooks were found and probed.
    for name in ["notes view", "approval gate", "WebFetch guard", "no-op cd"] {
        assert!(line(&stdout, name).contains('✔'), "{name}: {stdout}");
    }
}

/// With a current ocgen on PATH, the generated hooks run as `ocgen hook` and
/// every probe of a fresh project passes — the notes view renders too.
#[test]
fn with_a_current_ocgen_every_probe_of_a_fresh_project_passes() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("fresh");
    p.claude.team.hooks = true;
    p.scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("PATH", path_with_this_ocgen())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    for name in [
        "line endings",
        "hook commands run",
        "effective settings",
        "approval gate blocks git push",
        "WebFetch guard",
        "no-op cd",
        "notes view",
        "git pre-push hook",
        "statusline renders",
    ] {
        assert!(line(&stdout, name).contains('✔'), "{name}: {stdout}");
    }
}

/// With a current ocgen on PATH, a project in two languages asks for each
/// local document's other version — probed beside the project, not in it.
#[test]
fn with_a_current_ocgen_the_language_check_passes() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("bi");
    p.response_language = "Ukrainian".into();
    p.scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("PATH", path_with_this_ocgen())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let l = line(&stdout, "language versions");
    assert!(l.contains('✔') && l.contains("English"), "{stdout}");
    assert!(!dir.path().join(".claude/notes/.versions").exists());
}

// --------------------------------------------- E2: output and timeouts -----

/// A check command that prints more than a pipe holds (64 KiB) used to block
/// until the 10-minute limit and then fail.
#[test]
fn run_check_with_a_lot_of_output_finishes_and_passes() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = claude_project("loud");
    p.claude.workflow.check_cmd =
        "head -c 300000 /dev/zero; head -c 300000 /dev/zero >&2; echo done".into();
    p.scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude", "--run-check"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .timeout(Duration::from_secs(120))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let l = line(&stdout, "check command");
    assert!(l.contains("passes"), "{stdout}");
}

/// Ctrl-C stops the check verify runs, not only verify. The check runs in a
/// process group of its own (so a timeout can stop all of it), which the
/// terminal's Ctrl-C doesn't reach by itself.
#[cfg(unix)]
#[test]
fn interrupting_verify_stops_the_check_it_runs() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let marks = tempdir().unwrap();
    let started = marks.path().join("started");
    let finished = marks.path().join("finished");
    let mut p = claude_project("interrupted");
    p.claude.workflow.check_cmd = format!("{}; sleep 3; {}", touch(&started), touch(&finished));
    p.scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    // A job of its own, as a command typed at a terminal is.
    let mut job = std::process::Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["verify", "--no-claude", "--run-check"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while !started.exists() && start.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(started.exists(), "the check never started");
    // Ctrl-C a moment later (as a person would): SIGINT to the foreground job.
    std::thread::sleep(Duration::from_millis(300));
    let sent = std::process::Command::new("kill")
        .args(["-INT", "--", &format!("-{}", job.id())])
        .status()
        .unwrap();
    assert!(sent.success());
    job.wait().unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert!(!finished.exists(), "the check kept running after Ctrl-C");
}

// ------------------------------------- E3: the settings Claude Code merges -----

#[test]
fn a_local_override_that_turns_the_gate_off_is_reported() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("local").scaffold(dir.path(), false).unwrap();
    let local = dir.path().join(".claude/settings.local.json");

    write_json(&local, &json!({ "env": { "TEAM_APPROVAL_GATE": "0" } }));
    let checks = verify_lib(dir.path());
    let gate = named(&checks, "approval gate");
    assert_eq!(gate.status, Status::Fail, "{gate:#?}");
    assert!(gate.detail.contains("settings.local.json"), "{gate:#?}");
    let eff = named(&checks, "effective settings");
    assert_eq!(eff.status, Status::Fail, "{eff:#?}");
    assert!(
        eff.detail.contains(".claude/settings.local.json")
            && eff.detail.contains("TEAM_APPROVAL_GATE"),
        "{eff:#?}"
    );
    // The fix is in that file: `ocgen doctor` doesn't touch it.
    for c in [gate, eff] {
        assert!(
            c.detail
                .contains("remove it from .claude/settings.local.json")
                && !c.detail.contains("ocgen doctor"),
            "{c:#?}"
        );
    }

    write_json(&local, &json!({ "disableAllHooks": true }));
    let checks = verify_lib(dir.path());
    assert_eq!(named(&checks, "approval gate").status, Status::Fail);
    let eff = named(&checks, "effective settings");
    assert_eq!(eff.status, Status::Fail, "{eff:#?}");
    assert!(eff.detail.contains("disableAllHooks"), "{eff:#?}");

    // A lowered bar is a weaker gate; a changed loop budget only differs.
    write_json(
        &local,
        &json!({ "env": { "SUBAGENT_CONFIDENCE_THRESHOLD": "10", "LOOP_GUARD_MAX_BLOCKS": "9" } }),
    );
    let eff = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(eff.status, Status::Fail, "{eff:#?}");
    assert!(
        eff.detail.contains("SUBAGENT_CONFIDENCE_THRESHOLD=10"),
        "{eff:#?}"
    );
    write_json(&local, &json!({ "env": { "LOOP_GUARD_MAX_BLOCKS": "9" } }));
    let eff = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(eff.status, Status::Warn, "{eff:#?}");

    // The gate switched off in settings.json itself: `ocgen doctor` restores it.
    fs::remove_file(&local).unwrap();
    let generated = settings_of(dir.path());
    let mut s = generated.clone();
    s["env"]["TEAM_APPROVAL_GATE"] = "0".into();
    write_json(&dir.path().join(".claude/settings.json"), &s);
    let checks = verify_lib(dir.path());
    for name in ["approval gate", "effective settings"] {
        let c = named(&checks, name);
        assert_eq!(c.status, Status::Fail, "{c:#?}");
        assert!(c.detail.contains("ocgen doctor"), "{c:#?}");
    }
    write_json(&dir.path().join(".claude/settings.json"), &generated);

    // Nothing overriding: a pass.
    let checks = verify_lib(dir.path());
    assert_eq!(named(&checks, "effective settings").status, Status::Pass);
    assert_eq!(named(&checks, "approval gate").status, Status::Pass);
}

/// The exemption lists, the loop budget and the gate switches ocgen doesn't set
/// loosen the gates as surely as a switch turned off: a role added to a
/// read-only list is never gated, a lower budget releases every quality gate
/// sooner, the probe switch makes the gates change nothing.
#[test]
fn widening_an_exemption_or_the_loop_budget_weakens_the_gates() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("widened");
    p.claude.team.hooks = true;
    p.claude.team.risk_rounds = true;
    p.scaffold(dir.path(), false).unwrap();
    let env = settings_of(dir.path())["env"].clone();
    let ro = env["SUBAGENT_READONLY_ROLES"].as_str().unwrap().to_string();
    let team_ro = env["TEAM_READONLY_ROLES"].as_str().unwrap().to_string();
    assert!(
        ro.contains("explorer") && team_ro.contains("explorer"),
        "{env:#?}"
    );
    let local = dir.path().join(".claude/settings.local.json");
    let effective = |v: Value| {
        write_json(&local, &v);
        named(&verify_lib(dir.path()), "effective settings").clone()
    };

    // The reported override: implementers exempt from the worker and risk
    // gates, the loop guard releasing after one block.
    let c = effective(json!({ "env": {
        "SUBAGENT_READONLY_ROLES": format!("{ro} implementer general-purpose"),
        "TEAM_READONLY_ROLES": format!("{team_ro} implementer"),
        "LOOP_GUARD_MAX_BLOCKS": "1",
    } }));
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    for key in [
        "SUBAGENT_READONLY_ROLES",
        "TEAM_READONLY_ROLES",
        "LOOP_GUARD_MAX_BLOCKS=1",
        "remove it from .claude/settings.local.json",
    ] {
        assert!(c.detail.contains(key), "{key}: {c:#?}");
    }
    // Each on its own.
    for (key, value) in [
        ("SUBAGENT_READONLY_ROLES", format!("{ro} implementer")),
        ("TEAM_READONLY_ROLES", "implementer".to_string()),
        ("LOOP_GUARD_MAX_BLOCKS", "2".to_string()),
        ("OCGEN_HOOK_PROBE", "1".to_string()),
        ("OCGEN_HOOK_PROBE", "0".to_string()),
        ("OCGEN_RECAP_GH", "sh ./fake-gh".to_string()),
    ] {
        let c = effective(json!({ "env": { key: value } }));
        assert_eq!(c.status, Status::Fail, "{key}={value}: {c:#?}");
        assert!(c.detail.contains(key), "{key}: {c:#?}");
    }
    // Stricter or merely different: reported, not a failure. Fewer read-only
    // roles gate more; a loop budget of 0 never releases a quality gate; the
    // team task gate off has SubagentStop judge teammates too.
    for (key, value) in [
        ("SUBAGENT_READONLY_ROLES", "explorer"),
        ("TEAM_READONLY_ROLES", ""),
        ("LOOP_GUARD_MAX_BLOCKS", "0"),
        ("LOOP_GUARD_MAX_BLOCKS", "9"),
        ("TEAM_TASK_GATE", "0"),
    ] {
        let c = effective(json!({ "env": { key: value } }));
        assert_eq!(c.status, Status::Warn, "{key}={value}: {c:#?}");
    }

    // From any file Claude Code merges: the user's settings too.
    fs::remove_file(&local).unwrap();
    let user = tempdir().unwrap();
    let file = user.path().join("settings.json");
    write_json(&file, &json!({ "env": { "OCGEN_HOOK_PROBE": "1" } }));
    let c = named(
        &verify_with_user(dir.path(), file.clone()),
        "effective settings",
    )
    .clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail.contains("OCGEN_HOOK_PROBE") && c.detail.contains(&for_shell(&file)),
        "{c:#?}"
    );
}

/// A gate switch ocgen didn't generate is judged too: the team task gate added
/// where there are no team task hooks leaves in-process teammates ungated by
/// the worker gate; a read-only list added where the risk gate has none exempts
/// a role it holds.
#[test]
fn a_gate_switch_ocgen_does_not_set_is_judged_too() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("nohooks");
    p.claude.team.hooks = false;
    p.claude.team.risk_rounds = false;
    p.claude.workflow.subagent_confidence = 96;
    p.scaffold(dir.path(), false).unwrap();
    let env = settings_of(dir.path())["env"].clone();
    assert!(env.get("TEAM_TASK_GATE").is_none(), "{env:#?}");
    assert!(env.get("TEAM_READONLY_ROLES").is_none(), "{env:#?}");
    let local = dir.path().join(".claude/settings.local.json");
    write_json(&local, &json!({ "env": { "TEAM_TASK_GATE": "1" } }));
    let c = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail
            .contains(".claude/settings.local.json sets TEAM_TASK_GATE=1 (not set by ocgen)"),
        "{c:#?}"
    );
    // Unset keys that don't loosen a gate aren't worth a word — nor a read-only
    // list no gate here reads (no risk gate).
    write_json(
        &local,
        &json!({ "env": {
            "TEAM_TASK_GATE": "0",
            "OCGEN_NO_JQ": "1",
            "TEAM_READONLY_ROLES": "implementer",
        } }),
    );
    let c = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(c.status, Status::Pass, "{c:#?}");

    // The risk gate on, with no read-only role for ocgen to list.
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("norole");
    p.claude.team.hooks = true;
    p.claude.team.risk_rounds = true;
    p.agents.retain(|a| {
        let t = a.tools.trim();
        let d = &a.disallowed_tools;
        !((!t.is_empty() && !t.contains("Write") && !t.contains("Edit"))
            || (d.contains("Write") && d.contains("Edit")))
    });
    p.scaffold(dir.path(), false).unwrap();
    let env = settings_of(dir.path())["env"].clone();
    assert_eq!(env["TEAM_RISK_ROUNDS"], "1", "{env:#?}");
    assert!(env.get("TEAM_READONLY_ROLES").is_none(), "{env:#?}");
    let local = dir.path().join(".claude/settings.local.json");
    write_json(
        &local,
        &json!({ "env": { "TEAM_READONLY_ROLES": "implementer" } }),
    );
    let c = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail
            .contains(".claude/settings.local.json sets TEAM_READONLY_ROLES=implementer"),
        "{c:#?}"
    );
}

/// A key only a gate ocgen doesn't generate reads loosens nothing: no worker
/// gate, no risk gate here. Not from settings.local.json, nor from the user's
/// settings, which apply to every project. (The WebFetch guard is in every
/// Claude project, so a site added to its list always counts — see
/// `values_are_judged_as_either_hook_twin_reads_them`.)
#[test]
fn a_key_no_generated_gate_reads_is_not_a_failure() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("nogate");
    p.claude.team.hooks = false;
    p.claude.team.risk_rounds = false;
    p.claude.workflow.subagent_confidence = 0;
    p.claude.workflow.check_cmd = String::new();
    p.claude.workflow.intent = false;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(s["hooks"].get("SubagentStop").is_none(), "{s:#?}");
    assert!(s["hooks"].get("PreToolUse").is_some(), "{s:#?}");
    let unread = json!({ "env": {
        "SUBAGENT_READONLY_ROLES": "implementer general-purpose",
        "TEAM_TASK_GATE": "1",
        "TEAM_READONLY_ROLES": "implementer",
    } });
    write_json(&dir.path().join(".claude/settings.local.json"), &unread);
    let c = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(c.status, Status::Pass, "{c:#?}");
    fs::remove_file(dir.path().join(".claude/settings.local.json")).unwrap();
    let user = tempdir().unwrap();
    let file = user.path().join("settings.json");
    write_json(&file, &unread);
    let c = named(&verify_with_user(dir.path(), file), "effective settings").clone();
    assert_eq!(c.status, Status::Pass, "{c:#?}");
}

/// Values are judged as the hooks read them, by either twin: the sh scripts
/// read only plain digits (`+99` is 0: the bar off), the Rust hook a u32 with
/// an optional `+` (5000000000 is 0); roles and documentation sites match
/// whatever their letter case. A site added to the WebFetch guard's list
/// loosens it, as the ConfigChange audit judges it.
#[test]
fn values_are_judged_as_either_hook_twin_reads_them() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("twins");
    p.claude.team.hooks = true;
    p.claude.team.confidence_threshold = 96;
    p.claude.workflow.subagent_confidence = 96;
    p.claude.workflow.loop_guard_max = 3;
    p.claude.workflow.intent = true;
    p.scaffold(dir.path(), false).unwrap();
    let env = settings_of(dir.path())["env"].clone();
    let ro = env["SUBAGENT_READONLY_ROLES"].as_str().unwrap().to_string();
    let sites = env["OCGEN_WEBFETCH_DOMAINS"].as_str().unwrap().to_string();
    assert!(!sites.is_empty(), "{env:#?}");
    let local = dir.path().join(".claude/settings.local.json");
    let effective = |key: &str, value: &str| {
        write_json(&local, &json!({ "env": { key: value } }));
        named(&verify_lib(dir.path()), "effective settings").clone()
    };
    for (key, value) in [
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "+99"),
        ("TEAM_CONFIDENCE_THRESHOLD", "+99"),
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "5000000000"),
        ("LOOP_GUARD_MAX_BLOCKS", "+2"),
        ("OCGEN_WEBFETCH_DOMAINS", &format!("{sites} evil.example")),
    ] {
        let c = effective(key, value);
        assert_eq!(c.status, Status::Fail, "{key}={value}: {c:#?}");
    }
    for (key, value) in [
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "099"),
        ("LOOP_GUARD_MAX_BLOCKS", "+3"),
        ("SUBAGENT_READONLY_ROLES", &ro.to_ascii_lowercase()),
        ("OCGEN_WEBFETCH_DOMAINS", &sites.to_ascii_uppercase()),
        (
            "OCGEN_WEBFETCH_DOMAINS",
            sites.split_whitespace().next().unwrap(),
        ),
    ] {
        let c = effective(key, value);
        assert_eq!(c.status, Status::Warn, "{key}={value}: {c:#?}");
    }
}

/// The user's own ~/.claude/settings.json applies to every project: a
/// `disableAllHooks` there switches the gates off here too.
#[test]
fn a_user_setting_that_disables_hooks_is_reported() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("user").scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    write_json(
        &home.path().join(".claude/settings.json"),
        &json!({ "disableAllHooks": true }),
    );
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success(), "{stdout}");
    let l = line(&stdout, "effective settings");
    assert!(
        l.contains("disableAllHooks") && l.contains("~/.claude/settings.json"),
        "{stdout}"
    );
}

/// The user settings verify merges can be given — the in-process checks then
/// don't depend on whoever runs them.
#[test]
fn verify_merges_the_user_settings_it_is_given() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("given").scaffold(dir.path(), false).unwrap();
    let user = tempdir().unwrap();
    let file = user.path().join("settings.json");
    write_json(
        &file,
        &json!({ "sandbox": { "enabled": true }, "disableAllHooks": true }),
    );
    let checks = verify_with_user(dir.path(), file.clone());
    let eff = named(&checks, "effective settings");
    assert_eq!(eff.status, Status::Fail, "{eff:#?}");
    assert!(eff.detail.contains(&for_shell(&file)), "{eff:#?}");
    let sandbox = named(&checks, "sandbox");
    assert!(
        !sandbox.detail.contains("without the sandbox"),
        "{sandbox:#?}"
    );
    // Without it, nothing of the user's applies.
    let checks = verify_lib(dir.path());
    assert_eq!(named(&checks, "effective settings").status, Status::Pass);
}

/// `claude plugin validate` gets each folder as an argument — a folder name is
/// the repository's to choose, so it never goes through a shell.
#[cfg(unix)]
#[test]
fn claude_validation_passes_plugin_folders_as_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = claude_project("plug");
    p.claude.output = ocgen::claude::Output {
        project: true,
        plugin: true,
    };
    p.scaffold(dir.path(), false).unwrap();
    let evil = dir.path().join("plugin/x$(touch pwned)");
    fs::create_dir_all(&evil).unwrap();

    let bin = tempdir().unwrap();
    let log = bin.path().join("args.log");
    let claude = bin.path().join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"{}\"\necho 'Validation passed'\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .arg("verify")
        .arg(dir.path())
        .env("PATH", path)
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !dir.path().join("pwned").exists(),
        "the folder name ran: {stdout}"
    );
    let args = fs::read_to_string(&log).unwrap_or_default();
    assert!(args.contains("x$(touch pwned)"), "{args}\n{stdout}");
    assert!(
        line(&stdout, "Claude Code validation").contains("pass"),
        "{stdout}"
    );
}

/// A project path with a space: the no-op cd probe quotes it, as Claude does,
/// instead of reporting a missing jq.
#[test]
fn the_noop_cd_probe_quotes_a_path_with_spaces() {
    let base = tempdir().unwrap();
    let dir = base.path().join("John Doe/my proj");
    fs::create_dir_all(&dir).unwrap();
    git_init(&dir);
    claude_project("spaced").scaffold(&dir, false).unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(&dir)
        .env("PATH", path_with_this_ocgen())
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let l = line(&stdout, "no-op cd");
    assert!(l.contains("drops"), "{stdout}");
}

// ------------------------------------------------------------ new checks -----

#[test]
fn the_sandbox_check_says_when_the_gate_is_advisory() {
    // Gate on, no sandbox: advisory, and how to fix it.
    let c = sandbox_check(true, false, None, "macos", false);
    assert_eq!(c.status, Status::Warn);
    assert!(
        c.detail
            .contains("the approval gate is advisory without the sandbox")
            && c.detail.contains("ocgen edit team"),
        "{c:#?}"
    );
    // Turned off by another file: named.
    let c = sandbox_check(
        true,
        false,
        Some(".claude/settings.local.json"),
        "linux",
        true,
    );
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains(".claude/settings.local.json"), "{c:#?}");
    // Native Windows has no sandbox at all.
    for on in [false, true] {
        let c = sandbox_check(true, on, None, "windows", false);
        assert_eq!(c.status, Status::Warn, "{c:#?}");
        assert!(
            c.detail.contains("Windows") && c.detail.contains("advisory"),
            "{c:#?}"
        );
    }
    // Linux: the sandbox needs bubblewrap.
    let c = sandbox_check(false, true, None, "linux", false);
    assert_eq!(c.status, Status::Warn);
    assert!(
        c.detail.contains("bwrap") && c.detail.contains("bubblewrap"),
        "{c:#?}"
    );
    assert_eq!(
        sandbox_check(true, true, None, "linux", true).status,
        Status::Pass
    );
    assert_eq!(
        sandbox_check(true, true, None, "macos", false).status,
        Status::Pass
    );
    // Neither gate nor sandbox: nothing to say.
    assert_eq!(
        sandbox_check(false, false, None, "macos", false).status,
        Status::Skip
    );
}

#[test]
fn verify_warns_that_a_gate_without_the_sandbox_is_advisory() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("nosandbox").scaffold(dir.path(), false).unwrap();
    let c = named(&verify_lib(dir.path()), "sandbox").clone();
    assert_eq!(c.status, Status::Warn, "{c:#?}");
    assert!(c.detail.contains("advisory"), "{c:#?}");

    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("sandboxed");
    p.claude.sandbox.enabled = true;
    p.scaffold(dir.path(), false).unwrap();
    let c = named(&verify_lib(dir.path()), "sandbox").clone();
    if cfg!(target_os = "macos") {
        assert_eq!(c.status, Status::Pass, "{c:#?}");
    } else {
        assert_ne!(c.status, Status::Fail, "{c:#?}");
    }
}

/// The pre-push check runs the installed hook as Claude Code would (no real
/// approval can count) instead of trusting that it is there.
#[test]
fn the_pre_push_check_runs_the_installed_hook() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("push").scaffold(dir.path(), false).unwrap();
    let c = named(&verify_lib(dir.path()), "pre-push").clone();
    assert_eq!(c.status, Status::Pass, "{c:#?}");
    assert!(c.detail.contains("blocked"), "{c:#?}");

    // A hook that carries ocgen's marker but lets the push through.
    let hook = dir.path().join(".git/hooks/pre-push");
    fs::write(&hook, "#!/bin/sh\n# ocgen:pre-push\nexit 0\n").unwrap();
    let c = named(&verify_lib(dir.path()), "pre-push").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(c.detail.contains("let"), "{c:#?}");

    // One that fails every push blocks the user's own pushes too: not a pass.
    fs::write(
        &hook,
        "#!/bin/sh\n# ocgen:pre-push\necho 'pre-push: broken' >&2\nexit 3\n",
    )
    .unwrap();
    let c = named(&verify_lib(dir.path()), "pre-push").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail.contains("every push") && c.detail.contains("pre-push: broken"),
        "{c:#?}"
    );
}

/// The probe hides the real approvals from the hook (a scratch HOME), not git's
/// own trust: a repository owned by someone else that the global config trusts
/// (`safe.directory`, common in containers and CI) is still probed for real.
#[cfg(unix)]
#[test]
fn the_pre_push_probe_keeps_the_global_safe_directory() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("owner").scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    fs::write(home.path().join(".gitconfig"), "[safe]\n\tdirectory = *\n").unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("HOME", home.path())
        .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let l = line(&stdout, "git pre-push hook");
    assert!(l.contains('✔') && l.contains("blocked"), "{stdout}");
}

/// When git can't read the repository inside the probed hook, the hook lets the
/// push through without deciding anything — verify couldn't run it, which is
/// not the hook letting pushes through.
#[cfg(unix)]
#[test]
fn a_pre_push_probe_whose_git_cannot_run_is_only_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("nogit").scaffold(dir.path(), false).unwrap();
    let real = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    assert!(!real.is_empty());
    // A git that fails only under verify's scratch HOME (the probe's).
    let bin = tempdir().unwrap();
    let git = bin.path().join("git");
    fs::write(
        &git,
        format!(
            "#!/bin/sh\ncase \"$HOME\" in *ocgen-verify-*) echo 'fatal: ocgen-test: no repository' >&2; exit 128 ;; esac\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let home = tempdir().unwrap();
    let out = ocgen()
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .env("PATH", path)
        .env("HOME", home.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let l = line(&stdout, "git pre-push hook");
    assert!(
        l.contains('▲') && l.contains("ocgen-test: no repository") && !l.contains("let a push"),
        "{stdout}"
    );
}

/// A Windows checkout without LF pinned turns the hook scripts CRLF; sh then
/// fails on them, so every gate breaks.
#[test]
fn crlf_hook_scripts_fail_with_the_fix() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    gated("crlf").scaffold(dir.path(), false).unwrap();
    assert_eq!(
        named(&verify_lib(dir.path()), "line endings").status,
        Status::Pass
    );
    let gate = dir.path().join(".claude/hooks/team-approval-gate.sh");
    let crlf = fs::read_to_string(&gate).unwrap().replace('\n', "\r\n");
    fs::write(&gate, crlf).unwrap();
    let c = named(&verify_lib(dir.path()), "line endings").clone();
    assert_eq!(c.status, Status::Fail, "{c:#?}");
    assert!(
        c.detail.contains("team-approval-gate.sh")
            && c.detail.contains("git add --renormalize .")
            && c.detail.contains(".gitattributes"),
        "{c:#?}"
    );
}

/// A hook script left in the override dir (an older `ocgen templates init`) does
/// nothing now — verify says so, so nobody trusts a tweak that isn't in effect.
#[cfg(unix)]
#[test]
fn verify_warns_about_ignored_hook_script_overrides() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    claude_project("ov").scaffold(dir.path(), false).unwrap();
    let home = tempdir().unwrap();
    let run = || {
        let out = ocgen()
            .env("HOME", home.path())
            .args(["verify", "--no-claude"])
            .arg(dir.path())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(!run().contains("template overrides"));

    let hooks = home.path().join(".config/ocgen/templates/claude/hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("team-approval-gate.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let out = run();
    let l = line(&out, "template overrides");
    assert!(l.contains("claude/hooks/team-approval-gate.sh"), "{out}");
    assert!(l.contains("ignored"), "{out}");
}

/// The CODEOWNERS ocgen is linked to: verify says it's there and whether
/// GitHub can enforce it, and warns when it went away.
#[test]
fn verify_reports_the_linked_codeowners() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    claude_project("co").scaffold(dir.path(), false).unwrap();
    let skip = verify_lib(dir.path());
    assert_eq!(named(&skip, "CODEOWNERS").status, Status::Skip);

    let owners = dir.path().join(".github/CODEOWNERS");
    fs::create_dir_all(owners.parent().unwrap()).unwrap();
    fs::write(&owners, "* @owner\n").unwrap();
    let mut p = Project::load_state(dir.path()).unwrap();
    p.claude.intent.approvers = vec!["@alice".into()];
    p.claude.intent.codeowners = ".github/CODEOWNERS".into();
    p.scaffold(dir.path(), true).unwrap();
    let ok = verify_lib(dir.path());
    let c = named(&ok, "CODEOWNERS");
    // No symbolic link involved, so it passes on every platform.
    assert_eq!(c.status, Status::Pass, "{c:?}");
    assert!(
        c.detail.contains("Require review from Code Owners"),
        "{c:?}"
    );
    assert!(!c.detail.contains(".claude/CODEOWNERS"), "{c:?}");
    // GitHub assigns no code owner without explicit write access.
    assert!(c.detail.contains("write access"), "{c:?}");

    // Nothing owns the CODEOWNERS file itself: GitHub's advice is to.
    fs::write(&owners, "/src/ @dev\n").unwrap();
    p.scaffold(dir.path(), true).unwrap();
    let unowned = verify_lib(dir.path());
    let c = named(&unowned, "CODEOWNERS");
    assert_eq!(c.status, Status::Warn, "{c:?}");
    assert!(
        c.detail.contains("nothing owns .github/CODEOWNERS"),
        "{c:?}"
    );

    fs::remove_file(&owners).unwrap();
    let gone = verify_lib(dir.path());
    let c = named(&gone, "CODEOWNERS");
    assert_eq!(c.status, Status::Warn, "{c:?}");
    assert!(c.detail.contains("is gone"), "{c:?}");
}

fn helpers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The sandbox hides credential files, not an OS keychain: a helper that asks
/// one hands a sandboxed `git push` a token. verify names it.
#[test]
fn verify_warns_when_git_can_ask_a_keychain_the_sandbox_cannot_hide() {
    let plain = |v: &str| helpers(&[("credential.helper", v)]);
    for helper in [
        "osxkeychain",
        "manager",
        "manager-core",
        "/usr/local/share/gcm-core/git-credential-manager",
        "C:/Program Files/Git/mingw64/bin/git-credential-manager.exe",
        "wincred",
        "libsecret",
        "/usr/share/git/credential/libsecret/git-credential-libsecret",
        "!/opt/homebrew/bin/gh auth git-credential",
    ] {
        let c = keychain_check(true, &plain(helper));
        assert_eq!(c.status, Status::Warn, "{helper}: {c:#?}");
        assert!(
            c.detail.contains(helper) && c.detail.contains("git -c credential.helper"),
            "{c:#?}"
        );
    }
    // What `gh auth setup-git` writes clears github.com's helpers only:
    // osxkeychain still answers for every other host.
    let gh = helpers(&[
        ("credential.helper", "osxkeychain"),
        ("credential.https://github.com.helper", ""),
        (
            "credential.https://github.com.helper",
            "!/opt/homebrew/bin/gh auth git-credential",
        ),
    ]);
    let c = keychain_check(true, &gh);
    assert_eq!(c.status, Status::Warn, "{c:#?}");
    assert!(
        c.detail.contains("osxkeychain") && c.detail.contains("gh auth git-credential"),
        "{c:#?}"
    );
    // A reset read last leaves none; a file store the sandbox hides is no keychain.
    let reset = helpers(&[
        ("credential.helper", "osxkeychain"),
        ("credential.helper", ""),
    ]);
    assert_eq!(keychain_check(true, &reset).status, Status::Pass);
    assert_eq!(keychain_check(true, &plain("store")).status, Status::Pass);
    assert_eq!(keychain_check(true, &[]).status, Status::Pass);
    // Credentials not withheld (no sandbox, or allowed): nothing to say.
    assert_eq!(
        keychain_check(false, &plain("osxkeychain")).status,
        Status::Skip
    );
}

/// verify reads the helpers git would use in the repository (its own config
/// included) — and only reads them: a helper is never run.
#[test]
fn verify_reads_the_credential_helpers_git_uses_here() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("keychain");
    p.claude.sandbox.enabled = true;
    p.scaffold(dir.path(), false).unwrap();
    let marker = dir.path().join("helper-ran");
    let helper = format!("!{}", touch(&marker));
    let ok = std::process::Command::new("git")
        .args(["config", "--add", "credential.helper", "osxkeychain"])
        .current_dir(dir.path())
        .status()
        .unwrap()
        .success()
        && std::process::Command::new("git")
            .args(["config", "--add", "credential.helper", &helper])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success();
    assert!(ok, "git config");
    let c = named(&verify_lib(dir.path()), "git credentials").clone();
    if ocgen::claude::sandbox_supported() {
        assert_eq!(c.status, Status::Warn, "{c:#?}");
        assert!(c.detail.contains("osxkeychain"), "{c:#?}");
    } else {
        assert_eq!(c.status, Status::Skip, "{c:#?}");
    }
    assert!(!marker.exists(), "verify ran a credential helper");
}

/// The reset is part of withholding credentials: a settings file that takes it
/// back hands the keychain to the agent again.
#[test]
fn a_local_override_that_gives_git_its_helpers_back_fails_verify() {
    let dir = tempdir().unwrap();
    git_init(dir.path());
    let mut p = gated("helper-back");
    p.claude.sandbox.enabled = true;
    p.scaffold(dir.path(), false).unwrap();
    let local = dir.path().join(".claude/settings.local.json");
    for env in [
        json!({ "GIT_CONFIG_COUNT": "0" }),
        json!({ "GIT_CONFIG_VALUE_0": "osxkeychain" }),
    ] {
        write_json(&local, &json!({ "env": env }));
        let eff = named(&verify_lib(dir.path()), "effective settings").clone();
        assert_eq!(eff.status, Status::Fail, "{env}: {eff:#?}");
        assert!(eff.detail.contains("GIT_CONFIG_"), "{eff:#?}");
    }
    fs::remove_file(&local).unwrap();
    let eff = named(&verify_lib(dir.path()), "effective settings").clone();
    assert_eq!(eff.status, Status::Pass, "{eff:#?}");
}
