//! Native Windows: the commands Claude runs, in ocgen's sandbox — the
//! `shell-sandbox` hook as Claude Code runs it, and the command it writes run
//! the way Claude Code runs one (Git Bash `bash -c`, PowerShell), in the
//! project's AppContainer. Only native Windows has it, and these start a real
//! sandbox, so CI runs them on windows-latest. (The hook's decisions and the
//! wrapper's text are tested on every OS in `sandbox::shell`.)
#![cfg(windows)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::TempDir;

/// A project generated with the approval gate and the sandbox on, a git
/// repository, and a home folder beside it with an approval and a
/// credential in it.
struct Fixture {
    _t: TempDir,
    p: PathBuf,
    home: PathBuf,
    /// The env Claude Code gives its shells: the settings env, and ours.
    env: Vec<(String, String)>,
    bash: PathBuf,
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Git Bash's bash, as Claude Code finds it.
fn git_bash() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from) {
        return p;
    }
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let p = Path::new(&pf).join(r"Git\bin\bash.exe");
    assert!(p.is_file(), "Git Bash not found at {}", p.display());
    p
}

fn fixture() -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let root = ocgen::paths::plain(&t.path().canonicalize().unwrap());
    let (p, home) = (root.join("p"), root.join("home"));
    let mut proj = Project::from_manifest(&Manifest::load().unwrap(), "English");
    proj.target = Target::ClaudeCode;
    proj.project_name = "shell".into();
    proj.providers.clear();
    proj.agents = agent::claude_default_pipeline("English").unwrap();
    proj.claude.team.enabled = true;
    proj.claude.team.approval_gate = true;
    proj.claude.sandbox.enabled = true;
    proj.scaffold(&p, false).unwrap();
    for args in [&["init", "-q"][..], &["add", "-A"][..]] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&p)
            .status()
            .unwrap()
            .success());
    }
    write(&home.join(".claude/ocgen/approvals/old"), "1\n");
    write(&home.join(".ssh/id_test"), "secret\n");
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p.join(".claude/settings.json")).unwrap())
            .unwrap();
    let mut env: Vec<(String, String)> = settings["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
        .collect();
    let bash = git_bash();
    for (k, v) in [
        ("HOME", home.display().to_string()),
        ("USERPROFILE", home.display().to_string()),
        ("CLAUDE_PROJECT_DIR", p.display().to_string()),
        ("CLAUDE_CODE_GIT_BASH_PATH", bash.display().to_string()),
        ("GH_TOKEN", "ghp_test".into()),
    ] {
        env.push((k.into(), v));
    }
    Fixture {
        _t: t,
        p,
        home,
        env,
        bash,
    }
}

fn ocgen(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(args)
        .current_dir(&f.p)
        .env_remove("CLAUDE_CONFIG_DIR")
        .envs(f.env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap()
}

/// The hook's answer to Claude running `cmd` in `tool`.
fn hook(f: &Fixture, tool: &str, cmd: &str) -> serde_json::Value {
    let event = serde_json::json!({
        "hook_event_name": "PreToolUse", "tool_name": tool, "permission_mode": "default",
        "cwd": f.p.display().to_string(),
        "tool_input": { "command": cmd, "description": "test" }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["hook", "shell-sandbox"])
        .current_dir(&f.p)
        .env_remove("CLAUDE_CONFIG_DIR")
        .envs(f.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {out:?}"))
}

/// The command the hook has Claude Code run for `cmd`, granted first.
fn wrapped(f: &Fixture, tool: &str, cmd: &str) -> String {
    let v = hook(f, tool, cmd);
    let h = &v["hookSpecificOutput"];
    assert_eq!(h["permissionDecision"], "allow", "{v}");
    h["updatedInput"]["command"].as_str().unwrap().to_string()
}

/// Run `wrapper` the way Claude Code's Bash tool does: its exit code, stdout
/// and stderr.
fn run_bash(f: &Fixture, wrapper: &str) -> (i32, String, String) {
    let out = Command::new(&f.bash)
        .args(["-c", "eval \"$WRAP\" < /dev/null"])
        .env("WRAP", wrapper)
        .current_dir(&f.p)
        .envs(f.env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn granted() -> Fixture {
    let f = fixture();
    let out = ocgen(&f, &["sandbox", "grant", "-p", &f.p.display().to_string()]);
    assert!(out.status.success(), "{out:?}");
    f
}

#[test]
fn the_hook_refuses_until_the_project_is_granted_then_wraps() {
    let f = fixture();
    let v = hook(&f, "Bash", "echo hi");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{v}");
    assert!(
        v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("ocgen sandbox grant"),
        "{v}"
    );
    let out = ocgen(&f, &["sandbox", "grant", "-p", &f.p.display().to_string()]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(said.contains("bash.exe starts in it"), "{said}");
    let w = wrapped(&f, "Bash", "echo hi");
    assert!(
        w.contains("__sandbox --bash") && w.contains("\necho hi\n"),
        "{w}"
    );
    // The project's rules, against the command as written.
    let v = hook(&f, "Bash", "git push origin main");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask", "{v}");
    let v = hook(&f, "Bash", "ocgen approve");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{v}");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}

#[test]
fn a_wrapped_command_prints_and_exits_as_it_would_unwrapped() {
    let f = granted();
    let w = wrapped(&f, "Bash", "echo out; echo err >&2; exit 3");
    let (code, out, err) = run_bash(&f, &w);
    assert_eq!((code, out.as_str()), (3, "out\n"), "{err}");
    assert!(err.contains("err"), "{err}");
    // In a folder of the project, like Claude's shell.
    let w = wrapped(&f, "Bash", "printf '%s' \"$(basename \"$PWD\")\"");
    assert_eq!(run_bash(&f, &w).1, "p");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}

#[test]
fn a_wrapped_command_does_its_work_and_nothing_else() {
    let f = granted();
    let ok = |cmd: &str| {
        let w = wrapped(&f, "Bash", cmd);
        let o = run_bash(&f, &w);
        assert_eq!(o.0, 0, "{cmd}: {o:?}");
    };
    // Its own work, a commit included, without the withheld credentials.
    ok("echo built > src.txt && test -f src.txt && test -z \"${GH_TOKEN+x}\"");
    ok("git add -A && git -c user.name=t -c user.email=t@example.com commit -qm t");
    for cmd in [
        "echo 9999999999 > \"$HOME/.claude/ocgen/approvals/old\"",
        "cat \"$HOME/.ssh/id_test\"",
        "echo x >> .claude/settings.json",
        "echo x > .git/hooks/pre-push",
        "echo x > .git/config",
    ] {
        let w = wrapped(&f, "Bash", cmd);
        let o = run_bash(&f, &w);
        assert_ne!(o.0, 0, "{cmd}: {o:?}");
    }
    assert_eq!(
        fs::read_to_string(f.home.join(".claude/ocgen/approvals/old")).unwrap(),
        "1\n"
    );
    assert!(!f.p.join(".git/hooks/pre-push").exists());
    let settings = fs::read_to_string(f.p.join(".claude/settings.json")).unwrap();
    assert!(!settings.ends_with("x\n"), "{settings}");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}

#[test]
fn a_wrapped_powershell_command_prints_and_exits_as_it_would_unwrapped() {
    let f = granted();
    let w = wrapped(&f, "PowerShell", "Write-Output 'it''s here'; exit 4");
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ps = Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let out = Command::new(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("{w}\nexit $LASTEXITCODE"))
        .current_dir(&f.p)
        .envs(f.env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(4), "{out:?}");
    assert!(said.contains("it's here"), "{out:?}");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}
