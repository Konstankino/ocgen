//! Native Windows: the commands Claude runs, in ocgen's sandbox — the
//! `shell-sandbox` hook as Claude Code runs it, and the command it writes run
//! the way Claude Code's PowerShell tool runs one, in the project's
//! AppContainer. Bash commands are refused there: Git Bash can't run reliably
//! in an AppContainer. Only native Windows has it, and these start a real
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
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
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
    for (k, v) in [
        ("HOME", home.display().to_string()),
        ("USERPROFILE", home.display().to_string()),
        ("CLAUDE_PROJECT_DIR", p.display().to_string()),
        ("GH_TOKEN", "ghp_test".into()),
    ] {
        env.push((k.into(), v));
    }
    Fixture {
        _t: t,
        p,
        home,
        env,
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

/// Run `wrapper` the way Claude Code's PowerShell tool does (its exit code
/// read as `$LASTEXITCODE`): its exit code, stdout and stderr.
fn run_ps(f: &Fixture, wrapper: &str) -> (i32, String, String) {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ps = Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let out = Command::new(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("{wrapper}\nexit $LASTEXITCODE"))
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

fn reason(v: &serde_json::Value) -> String {
    v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn bash_is_refused_and_powershell_waits_for_the_grant_then_is_wrapped() {
    let f = fixture();
    // Bash: refused, saying why and what to use instead — never run as is.
    let v = hook(&f, "Bash", "echo hi");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{v}");
    assert!(
        reason(&v).contains("Git Bash") && reason(&v).contains("PowerShell tool"),
        "{v}"
    );
    // PowerShell: refused until the project is granted.
    let v = hook(&f, "PowerShell", "Write-Output hi");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{v}");
    assert!(reason(&v).contains("ocgen sandbox grant"), "{v}");
    let out = ocgen(&f, &["sandbox", "grant", "-p", &f.p.display().to_string()]);
    let said = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(out.status.success(), "{out:?}");
    assert!(
        said.contains("powershell") && said.contains("starts in it"),
        "{said}"
    );
    let w = wrapped(&f, "PowerShell", "Write-Output hi");
    assert!(
        w.contains("__sandbox --powershell") && w.contains("\nWrite-Output hi\n'@"),
        "{w}"
    );
    // The project's rules, against the command as written.
    let v = hook(&f, "PowerShell", "git push origin main");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask", "{v}");
    let v = hook(&f, "PowerShell", "ocgen approve");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{v}");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}

#[test]
fn a_wrapped_command_prints_and_exits_as_it_would_unwrapped() {
    let f = granted();
    let w = wrapped(
        &f,
        "PowerShell",
        "Write-Output 'it''s here'; [Console]::Error.WriteLine('err'); exit 3",
    );
    let (code, out, err) = run_ps(&f, &w);
    assert_eq!(code, 3, "{out}{err}");
    assert!(
        out.contains("it's here") && err.contains("err"),
        "{out}{err}"
    );
    // In the project's folder, like Claude's shell: through the `Project:`
    // drive, which leads there.
    let w = wrapped(
        &f,
        "PowerShell",
        "(Get-Location).Path; Split-Path -Leaf (Get-Location).ProviderPath.TrimEnd('\\')",
    );
    let out = run_ps(&f, &w).1;
    assert_eq!(
        out.lines().map(str::trim).collect::<Vec<_>>(),
        ["Project:\\", "p"],
        "{out}"
    );
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}

#[test]
fn a_wrapped_command_does_its_work_and_nothing_else() {
    let f = granted();
    let ok = |cmd: &str| {
        let w = wrapped(&f, "PowerShell", cmd);
        let o = run_ps(&f, &w);
        assert_eq!(o.0, 0, "{cmd}: {o:?}");
    };
    // Its own work, a commit included, without the withheld credentials.
    ok("Set-Content -Path built.txt -Value built; if (-not (Test-Path built.txt)) { exit 1 }; if ($env:GH_TOKEN) { exit 2 }");
    ok("git add -A; git -c user.name=t -c user.email=t@example.com commit -qm t; exit $LASTEXITCODE");
    for cmd in [
        "Set-Content -Path \"$env:USERPROFILE\\.claude\\ocgen\\approvals\\old\" -Value 9999999999 -ErrorAction Stop",
        "Get-Content -Path \"$env:USERPROFILE\\.ssh\\id_test\" -ErrorAction Stop",
        "Add-Content -Path .claude\\settings.json -Value x -ErrorAction Stop",
        "Set-Content -Path .git\\hooks\\pre-push -Value x -ErrorAction Stop",
        "Set-Content -Path .git\\config -Value x -ErrorAction Stop",
    ] {
        let w = wrapped(&f, "PowerShell", cmd);
        let o = run_ps(&f, &w);
        assert_ne!(o.0, 0, "{cmd}: {o:?}");
    }
    assert_eq!(
        fs::read_to_string(f.home.join(".claude/ocgen/approvals/old")).unwrap(),
        "1\n"
    );
    assert!(!f.p.join(".git/hooks/pre-push").exists());
    let settings = fs::read_to_string(f.p.join(".claude/settings.json")).unwrap();
    assert!(!settings.trim_end().ends_with('x'), "{settings}");
    let _ = ocgen(&f, &["sandbox", "reset", "-p", &f.p.display().to_string()]);
}
