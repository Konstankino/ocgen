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

/// Network off by default, on with `hook_network` (where the internet is
/// reachable at all: OCGEN_TEST_NETWORK=1).
#[cfg(unix)]
#[test]
fn the_network_is_off_unless_the_project_turns_it_on() {
    if std::env::var("OCGEN_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("skipped: OCGEN_TEST_NETWORK isn't 1");
        return;
    }
    let fetch = "curl -sS -m 10 -o /dev/null https://example.com";
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
