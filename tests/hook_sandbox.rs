//! The hooks' own sandbox, end to end: `ocgen sandbox run` runs a command the
//! way the hooks run the check, in whichever sandbox this machine has —
//! Seatbelt on macOS, bubblewrap or Landlock on Linux; native Windows has none,
//! and refuses. These start a real sandbox, so they can't run inside another
//! one (Claude Code's own, say): CI runs them on every OS. Set
//! `OCGEN_TEST_SANDBOX_EXPECT` to the sandbox the machine must use, and
//! `OCGEN_TEST_NETWORK=1` where the internet is reachable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A project with what the hooks' sandbox protects, and a home folder beside
/// it (not inside: everything in the project is writable) with an approval
/// and a credential in it.
struct Fixture {
    _t: TempDir,
    p: PathBuf,
    home: PathBuf,
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// The env settings.json gives the hooks, as ocgen renders it.
fn hook_env(network: bool) -> serde_json::Value {
    use ocgen::claude::{
        CREDENTIAL_ENV, CREDENTIAL_PATHS, HOOK_ALLOW_EXEC, HOOK_ALLOW_READ, HOOK_ALLOW_WRITE,
        HOOK_DENY_WRITE,
    };
    let mut env = serde_json::json!({
        "OCGEN_SANDBOX": "1",
        "OCGEN_SANDBOX_DENY_WRITE": HOOK_DENY_WRITE.join(" "),
        "OCGEN_SANDBOX_DENY_READ": CREDENTIAL_PATHS.join(" "),
        "OCGEN_SANDBOX_DENY_ENV": CREDENTIAL_ENV.join(" "),
        "OCGEN_SANDBOX_ALLOW_READ": HOOK_ALLOW_READ.join(" "),
        "OCGEN_SANDBOX_ALLOW_EXEC": HOOK_ALLOW_EXEC.join(" "),
        "OCGEN_SANDBOX_ALLOW_WRITE": HOOK_ALLOW_WRITE.join(" "),
    });
    if network {
        env["OCGEN_SANDBOX_NETWORK"] = "1".into();
    }
    env
}

fn fixture(network: bool) -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let root = ocgen::paths::plain(&t.path().canonicalize().unwrap());
    let (p, home) = (root.join("p"), root.join("home"));
    fs::create_dir_all(p.join("src")).unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&p)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git init");
    let settings = serde_json::json!({ "env": hook_env(network) });
    write(&p.join(".claude/settings.json"), &settings.to_string());
    write(&p.join(".claude/hooks/a.sh"), "#!/bin/sh\n");
    write(&p.join(".claude/statusline.sh"), "#!/bin/sh\n");
    write(&home.join(".claude/ocgen/approvals/old"), "1\n");
    write(&home.join(".ssh/id_test"), "secret\n");
    Fixture { _t: t, p, home }
}

/// `ocgen <args>` in the project, with the fixture's home folder.
fn ocgen(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(args)
        .current_dir(&f.p)
        .env("HOME", &f.home)
        .env("USERPROFILE", &f.home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("GH_TOKEN", "ghp_test")
        .output()
        .unwrap()
}

/// Run `cmd` in the hooks' sandbox: its exit code and output.
fn run(f: &Fixture, cmd: &str) -> (i32, String) {
    let p = ocgen::paths::for_shell(&f.p);
    let o = ocgen(f, &["sandbox", "run", "-p", &p, cmd]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    (o.status.code().unwrap_or(-1), text)
}

/// What `ocgen sandbox run` says when it finds the hooks' sandbox off for the
/// project: it ran nothing.
#[cfg(unix)]
const OFF: &str = "the hooks' sandbox is off";

/// The sandbox the machine must be using, when the environment says.
#[cfg(unix)]
fn expect_backend(f: &Fixture) {
    let Ok(want) = std::env::var("OCGEN_TEST_SANDBOX_EXPECT") else {
        return;
    };
    let p = ocgen::paths::for_shell(&f.p);
    let o = ocgen(f, &["sandbox", "status", "-p", &p, "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["on"], true, "{v}");
    assert_eq!(v["backend"], want.as_str(), "{v}");
}

#[cfg(unix)]
#[test]
fn a_sandboxed_command_does_its_work_and_nothing_else() {
    let f = fixture(false);
    expect_backend(&f);
    // Its own work goes ahead, without the withheld credentials in its env
    // (in a folder of the project: under Landlock nothing new can be created
    // right at its top).
    let o = run(
        &f,
        r#"echo built > src/out.txt && test -f src/out.txt && test -z "${GH_TOKEN+x}""#,
    );
    assert_eq!(o.0, 0, "{o:?}");
    assert!(f.p.join("src/out.txt").exists());

    let settings_path = f.p.join(".claude/settings.json");
    let settings_was = fs::read_to_string(&settings_path).unwrap();
    let mut ran = Vec::new();
    for cmd in [
        // What Claude Code and git run unsandboxed: written, created, renamed.
        "echo x >> .claude/settings.json",
        "echo x > .claude/settings.local.json",
        "echo x > .claude/hooks/a.sh",
        "echo x > .claude/statusline.sh",
        "echo x > .git/hooks/pre-push",
        "mv .claude c2",
        // The approval store and a credential, outside the project.
        r#"echo 9999999999 > "$HOME/.claude/ocgen/approvals/old""#,
        r#"cat "$HOME/.ssh/id_test""#,
        // A protected file that doesn't exist yet: refused, or moved aside.
        "echo {} > .mcp.json",
    ] {
        let o = run(&f, cmd);
        assert!(!o.1.contains(OFF), "{cmd}: ocgen didn't run it: {o:?}");
        if o.0 == 0 {
            ran.push(format!("{cmd}: {o:?}"));
        }
        // A write that went through is put back, or ocgen would find the
        // sandbox off and refuse every command after it.
        match fs::read_to_string(&settings_path) {
            Ok(settings_now) if settings_now != settings_was => {
                ran.push(format!("{cmd}: changed settings.json"));
                fs::write(&settings_path, &settings_was).unwrap();
            }
            Err(e) => {
                ran.push(format!("{cmd}: settings.json is missing: {e}"));
                let moved = f.p.join("c2");
                if moved.join("settings.json").is_file() {
                    fs::rename(moved, f.p.join(".claude")).unwrap();
                }
            }
            _ => {}
        }
    }
    assert!(
        ran.is_empty(),
        "refused, yet ended with 0:\n{}",
        ran.join("\n")
    );
    let settings = fs::read_to_string(f.p.join(".claude/settings.json")).unwrap();
    assert!(settings.starts_with("{\"env\""), "{settings}");
    assert!(!f.p.join(".claude/settings.local.json").exists());
    assert_eq!(
        fs::read_to_string(f.p.join(".claude/hooks/a.sh")).unwrap(),
        "#!/bin/sh\n"
    );
    assert!(!f.p.join(".git/hooks/pre-push").exists());
    assert!(f.p.join(".claude").is_dir() && !f.p.join("c2").exists());
    let old = f.home.join(".claude/ocgen/approvals/old");
    assert_eq!(fs::read_to_string(old).unwrap(), "1\n");
    assert!(!f.p.join(".mcp.json").exists());
}

/// Native Windows has no sandbox for the check: it is refused, with what to
/// do instead, and never run.
#[cfg(windows)]
#[test]
fn native_windows_refuses_and_says_where_the_sandbox_works() {
    let f = fixture(false);
    let o = run(&f, "cmd /d /c echo built> src\\out.txt");
    assert_ne!(o.0, 0, "{o:?}");
    assert!(o.1.contains("WSL2"), "{o:?}");
    assert!(!f.p.join("src/out.txt").exists());
}

/// A worker's worktree inside the project's `.claude/` (as Claude Code makes
/// them) is writable, and its own `.claude` protected.
#[cfg(unix)]
#[test]
fn a_worktree_inside_dot_claude_stays_writable() {
    let f = fixture(false);
    let wt = f.p.join(".claude/worktrees/w1");
    fs::create_dir_all(wt.join(".claude/hooks")).unwrap();
    fs::create_dir_all(wt.join("src")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["sandbox", "run", "-p", &ocgen::paths::for_shell(&f.p)])
        .arg("echo built > src/out.txt && ! echo x > .claude/hooks/b.sh")
        .current_dir(&wt)
        .env("HOME", &f.home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(wt.join("src/out.txt").exists());
    assert!(!wt.join(".claude/hooks/b.sh").exists());
}

/// The check-broker hook runs the full check in this sandbox, in a real linked
/// worktree inside `.claude/`. The check's own work goes ahead in a folder the
/// commit has (under Landlock nothing new can be created right at the worktree's
/// top), a protected path stays shut, and exit 125 is the helper's own "couldn't
/// apply": not run, never failed.
#[cfg(unix)]
#[test]
fn the_check_broker_runs_its_check_in_the_sandbox() {
    use std::io::Write as _;
    use std::process::Stdio;

    let f = fixture(false);
    expect_backend(&f);
    write(&f.p.join("src/keep.txt"), "kept\n");
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(args)
            .current_dir(&f.p)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {o:?}");
    };
    git(&["config", "user.name", "t"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "core.autocrlf", "false"]);
    git(&["add", "src"]);
    git(&["commit", "-qm", "init"]);

    let project = ocgen::paths::for_shell(&f.p);
    let records = f.p.join(".claude/deliver/s1");
    let request = records.join("check-request.json");
    let env = hook_env(false);
    // One request under a new id: the record the hook leaves.
    let broker = |id: &str, check: &str| -> String {
        let body = serde_json::json!({ "id": id, "commit": "HEAD" }).to_string();
        write(&request, &body);
        let event = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": { "file_path": ocgen::paths::for_shell(&request) },
        });
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        cmd.args(["hook", "check-broker"])
            .current_dir(&f.p)
            .env("CLAUDE_PROJECT_DIR", &project)
            .env("HOME", &f.home)
            .env("USERPROFILE", &f.home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("OCGEN_HOOK_PROBE")
            .env("OCGEN_FULL_CHECK_CMD", check)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env.as_object().unwrap() {
            cmd.env(k, v.as_str().unwrap());
        }
        let mut child = cmd.spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes());
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        fs::read_to_string(records.join(format!("check-{id}.md"))).unwrap()
    };

    let rec = broker("built", "echo built > src/out.txt && test -f src/out.txt");
    assert!(rec.contains("Status: passed\n"), "{rec}");
    assert!(!f.p.join("src/out.txt").exists(), "not in the checkout");
    assert!(!f.p.join(".claude/worktrees/broker-s1-built").exists());

    let settings = f.p.join(".claude/settings.json");
    let settings_was = fs::read_to_string(&settings).unwrap();
    let append = format!("echo x >> '{project}/.claude/settings.json'");
    let rec = broker("settings", &append);
    assert!(rec.contains("Status: failed\n"), "{rec}");
    assert_eq!(fs::read_to_string(&settings).unwrap(), settings_was);

    let store = "echo 9999999999 > \"$HOME/.claude/ocgen/approvals/old\"";
    let rec = broker("approval", store);
    assert!(rec.contains("Status: failed\n"), "{rec}");
    let old = f.home.join(".claude/ocgen/approvals/old");
    assert_eq!(fs::read_to_string(old).unwrap(), "1\n");

    // `$CLAUDE_PROJECT_DIR` is the worktree for the check, while the policy, as
    // above, still protects the real project.
    let through_env = "echo built > \"$CLAUDE_PROJECT_DIR/src/env.txt\"";
    let rec = broker("fromenv", through_env);
    assert!(rec.contains("Status: passed\n"), "{rec}");
    assert!(!f.p.join("src/env.txt").exists(), "not in the checkout");

    let rec = broker("notapplied", "exit 125");
    assert!(rec.contains("Status: not-run\n"), "{rec}");
}

/// Network off by default, on with `hook_network` (where the internet is
/// reachable at all: OCGEN_TEST_NETWORK=1).
///
/// Plain HTTP on purpose: the claim is about reaching the network, and an
/// HTTPS fetch also checks the site's certificate chain against the machine's
/// trust store. On the macOS runners that check began to fail (`curl` exit 60,
/// "unable to get local issuer certificate") between two commits that changed
/// no Rust code: something outside the repository, the site's chain or the
/// runner's trust store, turned the test red.
#[cfg(unix)]
#[test]
fn the_network_is_off_unless_the_project_turns_it_on() {
    if std::env::var("OCGEN_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("skipped: OCGEN_TEST_NETWORK isn't 1");
        return;
    }
    let fetch = "curl -sS -m 10 -o /dev/null http://example.com";
    let off = run(&fixture(false), fetch);
    assert_ne!(off.0, 0, "{off:?}");
    let on = run(&fixture(true), fetch);
    assert_eq!(on.0, 0, "{on:?}");
}

/// wasmtime runs a WASI module with its folder as `/`: the module can create a
/// file in `src/` but can't truncate a protected file or create one at the top
/// (a protected path lies there). No OS sandbox: it runs anywhere.
#[test]
fn a_wasi_module_gets_its_folder_and_no_more() {
    use ocgen::sandbox::{wasm, Policy};
    let t = tempfile::tempdir().unwrap();
    let p = ocgen::sandbox::policy::real_path(t.path());
    fs::create_dir_all(p.join("src")).unwrap();
    write(&p.join(".claude/settings.json"), "{}");
    let wat = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sandbox/check.wat"),
    )
    .unwrap();
    fs::write(p.join("check.wasm"), wat::parse_str(&wat).unwrap()).unwrap();
    let policy = Policy {
        project: p.clone(),
        dir: p.clone(),
        write: vec![p.clone()],
        protect: vec![p.join(".claude/settings.json")],
        ..Policy::default()
    };
    // Its folders: `/` read-only, `/.claude` read-only, `/src` writable — in
    // that order (fds 3, 4, 5 in the module).
    let pre: Vec<(String, bool)> = wasm::preopens(&policy)
        .into_iter()
        .map(|(_, g, w)| (g, w))
        .collect();
    assert_eq!(
        pre,
        [
            ("/".to_string(), false),
            ("/.claude".to_string(), false),
            ("/src".to_string(), true)
        ]
    );
    assert_eq!(wasm::run(&policy, "check.wasm"), Ok(0));
    assert!(p.join("src/out.txt").exists());
    assert_eq!(
        fs::read_to_string(p.join(".claude/settings.json")).unwrap(),
        "{}"
    );
    assert!(!p.join("top.txt").exists());
    // A command that isn't a module is refused before anything runs.
    assert!(wasm::run(&policy, "cargo test").is_err());
}
