//! `ocgen verify`: check that a generated project actually works — the checks
//! that otherwise get done by hand (or discovered by the user): is it up to date,
//! is settings.json sane, do the hook commands run and does the approval gate
//! block, does the statusline render, is a compatible ocgen on PATH, and does
//! Claude Code's own validator accept the agents, skills and plugin.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::render::{ChangeKind, Project};
use crate::target::Target;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Also run `claude plugin validate` (needs the Claude Code CLI).
    pub run_claude: bool,
    /// Also run the project's check command (may be slow: it is usually the tests).
    pub run_check: bool,
}

fn check(name: &str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}

/// Top-level keys a project `settings.json` may use (Claude Code settings
/// reference, 2026-09). Unknown keys are only warned about — the list evolves.
const SETTINGS_KEYS: &[&str] = &[
    "$schema",
    "model",
    "permissions",
    "env",
    "hooks",
    "statusLine",
    "subagentStatusLine",
    "outputStyle",
    "teammateMode",
    "worktree",
    "sandbox",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "enableAllProjectMcpServers",
    "allowedMcpServers",
    "deniedMcpServers",
    "enabledPlugins",
    "extraKnownMarketplaces",
    "additionalMarketplaces",
    "cleanupPeriodDays",
    "includeCoAuthoredBy",
    "attribution",
    "includeGitInstructions",
    "prUrlTemplate",
    "effortLevel",
    "maxEffortLevel",
    "modelSettings",
    "advisorModel",
    "fastMode",
    "fastModePerSessionOptIn",
    "fallbackModel",
    "availableModels",
    "modelOverrides",
    "alwaysThinkingEnabled",
    "showThinkingSummaries",
    "language",
    "promptCacheTtl",
    "subagentPromptCacheTtl",
    "autoCompactEnabled",
    "autoCompactWindow",
    "autoMemoryEnabled",
    "autoMemoryDirectory",
    "bashOutputMaxChars",
    "claudeMdExcludes",
    "fileCheckpointingEnabled",
    "plansDirectory",
    "skillListingBudgetFraction",
    "skillListingMaxDescChars",
    "skillOverrides",
    "disableBundledSkills",
    "disableSkillShellExecution",
    "spinnerTipsEnabled",
    "spinnerTipsOverride",
    "spinnerVerbs",
    "companyAnnouncements",
    "defaultShell",
    "editorMode",
    "respectGitignore",
    "theme",
    "tui",
    "viewMode",
    "verbose",
    "disableAllHooks",
    "allowedHttpHookUrls",
    "httpHookAllowedEnvVars",
    "disableWorkflows",
    "enableWorkflows",
    "workflowSizeGuideline",
    "agent",
    "crossSessionInbound",
    "disableAgentView",
    "apiKeyHelper",
    "forceLoginMethod",
    "forceLoginOrgUUID",
    "disableAutoMode",
    "autoUpdatesChannel",
    "minimumVersion",
    "enableArtifact",
    "timeFormat",
    "timeZone",
    "maxProseWidth",
    "showTurnDuration",
];

/// Run every check. Hook commands are executed with the loop guard switched off
/// (`LOOP_GUARD_MAX_BLOCKS=0`), so verification leaves no state behind.
pub fn verify(project: &Project, root: &Path, opts: &Options) -> Vec<Check> {
    let mut out = vec![up_to_date(project, root), consistency(project)];
    if project.target != Target::ClaudeCode {
        return out;
    }
    let settings: Option<Value> = std::fs::read_to_string(root.join(".claude/settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    out.push(settings_check(root, settings.as_ref()));
    let sh = shell(settings.as_ref().unwrap_or(&Value::Null));
    let shell_ok = match shell_check(&sh) {
        Ok(c) => {
            out.push(c);
            true
        }
        Err(c) => {
            out.push(c);
            false
        }
    };
    if let Some(s) = &settings {
        if shell_ok {
            out.push(hook_scripts(&sh, root, s));
            out.push(hook_commands(&sh, root, s));
        }
        out.push(hook_shell(s, cfg!(windows), find_git_bash(s).as_deref()));
        if shell_ok {
            out.push(approval_gate(&sh, root, s));
            out.push(https_only_fetch(&sh, root, s));
            out.push(notes_view(&sh, root, s));
        }
        out.push(pre_push(project, root));
        if shell_ok {
            out.push(statusline(&sh, root, s));
        }
    }
    if shell_ok {
        out.push(check_command(&sh, project, root, opts.run_check));
    }
    out.push(mcp_json(root));
    out.push(ocgen_on_path());
    out.push(git_tracking(root));
    if opts.run_claude && shell_ok {
        out.push(claude_validate(&sh, project, root));
    }
    out
}

/// The shell verify runs scripts and hook commands with — the one Claude Code
/// uses. On Windows that is Git Bash: a per-user Git install puts only
/// `Git\cmd` on PATH, so a bare `sh` is often missing outside Git Bash.
/// Elsewhere, `sh` from PATH.
fn shell(settings: &Value) -> PathBuf {
    if cfg!(windows) {
        if let Some(bash) = find_git_bash(settings) {
            return bash;
        }
    }
    PathBuf::from("sh")
}

/// Whether `sh` starts at all. Without it every script- and hook-running check
/// would fail for the same reason, so they are skipped and this says why.
fn shell_check(sh: &Path) -> Result<Check, Check> {
    let name = "verify shell";
    match Command::new(sh)
        .args(["-c", "exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(st) if st.success() => Ok(check(
            name,
            Status::Pass,
            format!("scripts and hooks are checked with {}", sh.display()),
        )),
        res => {
            let why = match res {
                Ok(st) => format!("it exited {}", st.code().unwrap_or(-1)),
                Err(e) => e.to_string(),
            };
            let fix = if cfg!(windows) {
                "install Git for Windows, or set CLAUDE_CODE_GIT_BASH_PATH to its bash.exe"
            } else {
                "make sure `sh` is on PATH"
            };
            Err(check(
                name,
                Status::Fail,
                format!(
                    "can't start {} ({why}) — {fix}. The hook script, hook command, approval gate, WebFetch guard, notes view, statusline and check command checks were skipped",
                    sh.display()
                ),
            ))
        }
    }
}

fn up_to_date(project: &Project, root: &Path) -> Check {
    let name = "files up to date";
    match project.plan_changes(root) {
        Err(e) => check(
            name,
            Status::Fail,
            format!("could not render the project: {e}"),
        ),
        Ok(plan) => {
            let differ: Vec<_> = plan
                .iter()
                .filter(|c| c.kind != ChangeKind::Unchanged)
                .collect();
            if differ.is_empty() {
                return check(
                    name,
                    Status::Pass,
                    format!("{} generated file(s) match", plan.len()),
                );
            }
            let hand = differ
                .iter()
                .filter(|c| c.hand_edited == Some(true))
                .count();
            let list: Vec<&str> = differ.iter().take(4).map(|c| c.rel.as_str()).collect();
            check(
                name,
                Status::Warn,
                format!(
                    "{} file(s) differ from what ocgen generates ({hand} edited by hand): {}{} — see `ocgen doctor --dry-run`",
                    differ.len(),
                    list.join(", "),
                    if differ.len() > 4 { ", …" } else { "" }
                ),
            )
        }
    }
}

fn consistency(project: &Project) -> Check {
    let issues = project.issues();
    if issues.is_empty() {
        check("consistency", Status::Pass, "no problems found")
    } else {
        check("consistency", Status::Warn, issues.join("; "))
    }
}

fn settings_check(root: &Path, settings: Option<&Value>) -> Check {
    let name = "settings.json";
    let path = root.join(".claude/settings.json");
    if !path.exists() {
        return check(name, Status::Fail, "missing — run `ocgen doctor`");
    }
    let Some(obj) = settings.and_then(Value::as_object) else {
        return check(
            name,
            Status::Fail,
            "not valid JSON — Claude Code will ignore it",
        );
    };
    let unknown: Vec<&String> = obj
        .keys()
        .filter(|k| !SETTINGS_KEYS.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|k| k.as_str()).collect();
        return check(
            name,
            Status::Warn,
            format!("unknown key(s): {} (typo?)", names.join(", ")),
        );
    }
    check(
        name,
        Status::Pass,
        format!("valid JSON, {} known key(s)", obj.len()),
    )
}

/// Every `(event, command)` in the hooks table.
fn hook_entries(settings: &Value) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Some(hooks) = settings.get("hooks").and_then(Value::as_object) {
        for (event, groups) in hooks {
            for g in groups.as_array().into_iter().flatten() {
                for h in g
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(c) = h.get("command").and_then(Value::as_str) {
                        v.push((event.clone(), c.to_string()));
                    }
                }
            }
        }
    }
    v
}

/// Whether each command hook pins `"shell": "bash"` (in table order).
fn hook_shells_pinned(settings: &Value) -> Vec<bool> {
    let mut v = Vec::new();
    if let Some(hooks) = settings.get("hooks").and_then(Value::as_object) {
        for groups in hooks.values() {
            for g in groups.as_array().into_iter().flatten() {
                for h in g
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if h.get("command").is_some() {
                        v.push(h.get("shell").and_then(Value::as_str) == Some("bash"));
                    }
                }
            }
        }
    }
    v
}

/// Git Bash on Windows, the way Claude Code looks for it: the
/// `CLAUDE_CODE_GIT_BASH_PATH` override (environment or settings `env`), the
/// usual install locations, then PATH (skipping the WSL launcher in System32).
fn find_git_bash(settings: &Value) -> Option<PathBuf> {
    let configured = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            settings
                .pointer("/env/CLAUDE_CODE_GIT_BASH_PATH")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
    let installs = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .flat_map(|base| {
            let base = PathBuf::from(base);
            [
                base.join("Git/bin/bash.exe"),
                base.join("Programs/Git/bin/bash.exe"),
            ]
        });
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| {
        std::env::split_paths(&p)
            .filter(|d| {
                let d = d.to_string_lossy().to_lowercase();
                !d.contains("system32") && !d.contains("windowsapps")
            })
            .map(|d| d.join("bash.exe"))
            .collect::<Vec<_>>()
    });
    configured
        .into_iter()
        .chain(installs)
        .chain(on_path)
        .find(|p| p.is_file())
}

/// The generated hook commands are POSIX sh. Claude Code runs them in bash only
/// when the hook says so and — on Windows — Git Bash exists; otherwise they go to
/// PowerShell, fail to parse, and every gate fails open (a hook error never blocks).
pub fn hook_shell(settings: &Value, windows: bool, bash: Option<&Path>) -> Check {
    let name = "hook shell";
    let pinned = hook_shells_pinned(settings);
    if pinned.is_empty() {
        return check(name, Status::Skip, "no hooks configured");
    }
    if windows && bash.is_none() {
        return check(
            name,
            Status::Fail,
            "Git Bash not found — the hooks cannot run here, so the gates (including the approval gate) let everything through. Install Git for Windows, or set CLAUDE_CODE_GIT_BASH_PATH to its bash.exe",
        );
    }
    let unpinned = pinned.iter().filter(|p| !**p).count();
    if unpinned > 0 {
        return check(
            name,
            Status::Warn,
            format!(
                "{unpinned} hook(s) don't set \"shell\": \"bash\" — on Windows without Git Bash they run in PowerShell and fail open. Run `ocgen doctor`"
            ),
        );
    }
    let via = bash.map_or_else(|| "sh".to_string(), |b| b.display().to_string());
    check(
        name,
        Status::Pass,
        format!("{} hook(s) run in bash ({via})", pinned.len()),
    )
}

fn hook_scripts(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "hook scripts";
    let re = regex::Regex::new(r"/\.claude/hooks/([A-Za-z0-9_.-]+\.sh)").unwrap();
    let mut scripts: Vec<String> = hook_entries(settings)
        .iter()
        .flat_map(|(_, c)| {
            re.captures_iter(c)
                .map(|m| m[1].to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    const GATES: [&str; 5] = [
        "subagent-confidence-gate.sh",
        "team-task-completed.sh",
        "team-task-created.sh",
        "team-teammate-idle.sh",
        "team-approval-gate.sh",
    ];
    if scripts.iter().any(|s| GATES.contains(&s.as_str())) {
        scripts.push("loop-guard.sh".into()); // the gates source it
    }
    scripts.sort();
    scripts.dedup();
    if scripts.is_empty() {
        return check(name, Status::Skip, "no hook scripts configured");
    }
    let mut problems = Vec::new();
    for s in &scripts {
        let p = root.join(".claude/hooks").join(s);
        if !p.is_file() {
            problems.push(format!("{s} is missing"));
        } else if !Command::new(sh)
            // Run from the hooks dir with a bare file name: no path for the
            // shell to mangle (Windows verbatim paths lose their backslashes).
            .current_dir(root.join(".claude/hooks"))
            .arg("-n")
            .arg(s)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|st| st.success())
        {
            problems.push(format!("{s} has a syntax error"));
        }
    }
    if problems.is_empty() {
        check(
            name,
            Status::Pass,
            format!("{} script(s) present and parse", scripts.len()),
        )
    } else {
        check(
            name,
            Status::Fail,
            format!("{} — run `ocgen doctor`", problems.join("; ")),
        )
    }
}

/// The env a hook sees: settings `env`, the project dir, and no loop guard.
fn hook_env(root: &Path, settings: &Value) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = settings
        .get("env")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    env.insert("CLAUDE_PROJECT_DIR".into(), crate::paths::for_shell(root));
    env.insert("LOOP_GUARD_MAX_BLOCKS".into(), "0".into());
    // Probing the gates must not run the project's test suite.
    env.insert("OCGEN_CHECK_CMD".into(), String::new());
    env
}

/// Run `sh -c cmd` with `stdin` and `env` from `cwd`, up to `limit`.
fn run_sh(
    sh: &Path,
    cmd: &str,
    stdin: &str,
    env: &HashMap<String, String>,
    cwd: &Path,
    limit: Duration,
) -> Option<(i32, String, String)> {
    let mut child = Command::new(sh)
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let _ = child.stdin.take()?.write_all(stdin.as_bytes());
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    Some((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Run the side-effect-free hook commands with a harmless event and check they
/// exit as expected (a missing script or interpreter shows up as 126/127).
fn hook_commands(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "hook commands run";
    let env = hook_env(root, settings);
    let not_git = std::env::temp_dir();
    let plan_gate = env.get("TEAM_PLAN_GATE").is_some_and(|v| v == "1");
    let mut ran = 0;
    let mut problems = Vec::new();
    for (event, cmd) in hook_entries(settings) {
        // Notification/format/audit hooks have real side effects; skip them here.
        let (payload, expect) = match event.as_str() {
            "SessionStart" => ("{}".to_string(), 0),
            "SubagentStop" => (
                serde_json::json!({ "session_id": "ocgen-verify", "cwd": crate::paths::for_shell(&not_git) }).to_string(),
                0,
            ),
            "TaskCompleted" => (
                r#"{"session_id":"ocgen-verify","task_id":"ocgen-verify","summary":"Confidence: 100%"}"#.into(),
                0,
            ),
            "TeammateIdle" => (r#"{"session_id":"ocgen-verify","agent_type":"ocgen-verify-probe"}"#.into(), 0),
            "TaskCreated" => {
                let approved = std::fs::read_to_string(root.join(".claude/team/plan.md"))
                    .is_ok_and(|p| p.lines().any(|l| l.starts_with("Status: APPROVED")));
                (
                    r#"{"session_id":"ocgen-verify","agent_type":"ocgen-verify-probe"}"#.into(),
                    if plan_gate && !approved { 2 } else { 0 },
                )
            }
            _ => continue,
        };
        ran += 1;
        match run_sh(sh, &cmd, &payload, &env, root, Duration::from_secs(20)) {
            None => problems.push(format!("{event}: did not finish")),
            Some((code, _, err)) if code != expect => problems.push(format!(
                "{event}: exit {code}, expected {expect} ({})",
                err.lines().next().unwrap_or("").trim()
            )),
            Some(_) => {}
        }
    }
    if ran == 0 {
        check(name, Status::Skip, "no side-effect-free hooks to run")
    } else if problems.is_empty() {
        check(
            name,
            Status::Pass,
            format!("{ran} hook command(s) ran as expected"),
        )
    } else {
        check(name, Status::Fail, problems.join("; "))
    }
}

/// The approval gate must block pushes — including phrasings with flags and via
/// a deploy script — and allow ordinary work. Tested with an empty HOME so a
/// current approval can't hide a broken gate.
/// The WebFetch guard must allow only https:// fetches to trusted documentation
/// sites: plain http://, untrusted hosts and look-alikes are blocked.
fn https_only_fetch(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "WebFetch guard";
    let Some((_, cmd)) = hook_entries(settings)
        .into_iter()
        .find(|(e, c)| e == "PreToolUse" && c.contains("https-only-fetch"))
    else {
        return check(name, Status::Skip, "/intent not enabled");
    };
    let env = hook_env(root, settings);
    let run = |url: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "WebFetch",
            "tool_input": { "url": url, "prompt": "verify" }
        })
        .to_string();
        run_sh(sh, &cmd, &ev, &env, root, Duration::from_secs(20)).map(|x| x.0)
    };
    // A trusted host to probe with: the first entry (a `*.` entry → a subdomain).
    let trusted: Option<String> = env
        .get("OCGEN_WEBFETCH_DOMAINS")
        .and_then(|l| l.split_whitespace().next())
        .map(|d| match d.strip_prefix("*.") {
            Some(rest) => format!("ocgen-verify.{rest}"),
            None => d.to_string(),
        });
    // A `*.` entry over a shared-hosting suffix trusts strangers' sites.
    let shared: Vec<String> = env
        .get("OCGEN_WEBFETCH_DOMAINS")
        .map(|l| {
            l.split_whitespace()
                .filter(|d| crate::validate::shared_hosting_wildcard(d))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if let Some(d) = shared.first() {
        return check(
            name,
            Status::Fail,
            format!(
                "trusts {} — anyone can register a subdomain there; drop it with `ocgen edit intent --untrust-domain '{d}'` and trust exact hosts",
                shared.join(", ")
            ),
        );
    }
    let mut must_block = vec![
        "http://example.com/".to_string(),
        "HTTP://example.com/".to_string(),
        "https://ocgen-verify.invalid/".to_string(),
    ];
    if let Some(t) = &trusted {
        must_block.push(format!("http://{t}/"));
        must_block.push(format!("https://{t}.ocgen-verify.invalid/"));
        // Host parts a URL parser reads differently: a backslash is a path
        // separator to the fetcher, so these really go to ocgen-verify.invalid.
        must_block.push(format!("https://ocgen-verify.invalid\\@{t}/"));
        must_block.push(format!("https://ocgen-verify.invalid\\.{t}/"));
        must_block.push(format!("https://ocgen-verify.invalid@{t}/"));
    }
    let leaks: Vec<String> = must_block
        .into_iter()
        .filter(|u| run(u) != Some(2))
        .collect();
    if !leaks.is_empty() {
        return check(
            name,
            Status::Fail,
            format!("let {} through — run `ocgen doctor`", leaks.join(", ")),
        );
    }
    match trusted {
        None => check(
            name,
            Status::Pass,
            "no documentation sites trusted — every WebFetch is blocked",
        ),
        Some(t) if run(&format!("https://{t}/")) != Some(0) => check(
            name,
            Status::Fail,
            format!("blocks the trusted https://{t}/ too — run `ocgen doctor`"),
        ),
        Some(_) => check(
            name,
            Status::Pass,
            "only https:// to trusted documentation sites; http:// and other hosts are blocked",
        ),
    }
}

/// The /inquire notes view: after a ledger is written, the hook renders its
/// HTML page. Probed on a scratch ledger with the browser switched off.
fn notes_view(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "/inquire notes view";
    let Some((_, cmd)) = hook_entries(settings)
        .into_iter()
        .find(|(e, c)| e == "PostToolUse" && c.contains("inquire-notes"))
    else {
        return check(name, Status::Skip, "/inquire not enabled");
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch =
        std::env::temp_dir().join(format!("ocgen-verify-notes-{}-{nanos}", std::process::id()));
    let notes = scratch.join(".claude/notes");
    let md = notes.join("verify.md");
    if std::fs::create_dir_all(&notes).is_err()
        || std::fs::write(
            &md,
            "Topic: ocgen verify ledger\n## Q&A log\n### Q1 · Flow · Verified\nQ: Does the view render?\n",
        )
        .is_err()
    {
        let _ = std::fs::remove_dir_all(&scratch);
        return check(name, Status::Warn, "could not create a scratch ledger to probe with");
    }
    let mut env = hook_env(root, settings);
    env.insert("OCGEN_NOTES_OPEN".into(), "0".into());
    let ev = serde_json::json!({
        "session_id": "ocgen-verify", "tool_name": "Write",
        "tool_input": { "file_path": crate::paths::for_shell(&md) }
    })
    .to_string();
    let ran = run_sh(sh, &cmd, &ev, &env, root, Duration::from_secs(20));
    let rendered = std::fs::read_to_string(notes.join("verify.html"))
        .is_ok_and(|h| h.contains("ocgen verify ledger"));
    let _ = std::fs::remove_dir_all(&scratch);
    match ran {
        None => check(name, Status::Fail, "the hook did not finish"),
        Some((code, _, err)) if code != 0 => check(
            name,
            Status::Fail,
            format!(
                "the hook exited {code} ({}) — it must never fail a write; run `ocgen doctor`",
                err.lines().next().unwrap_or("").trim()
            ),
        ),
        Some(_) if rendered => check(
            name,
            Status::Pass,
            "each ledger gets an HTML page that refreshes the tab showing it",
        ),
        Some(_) if !ocgen_hook_ok() => check(
            name,
            Status::Warn,
            format!(
                "needs ocgen ({}) on PATH — without it ledgers stay Markdown only",
                crate::hooks::PROTOCOL
            ),
        ),
        Some(_) => check(
            name,
            Status::Fail,
            "the hook ran but wrote no HTML page — run `ocgen doctor`",
        ),
    }
}

fn approval_gate(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "approval gate blocks git push";
    let Some((_, cmd)) = hook_entries(settings)
        .into_iter()
        .find(|(e, c)| e == "PreToolUse" && c.contains("team-approval-gate"))
    else {
        return check(name, Status::Skip, "approval gate not enabled");
    };
    let mut env = hook_env(root, settings);
    let empty_home = std::env::temp_dir().join(format!("ocgen-verify-home-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&empty_home);
    env.insert("HOME".into(), crate::paths::for_shell(&empty_home));
    let run = |c: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "Bash", "tool_input": { "command": c }
        })
        .to_string();
        run_sh(sh, &cmd, &ev, &env, root, Duration::from_secs(20)).map(|x| x.0)
    };
    let must_block = [
        "git push origin main",
        "git -C . push origin main",
        "terraform -chdir=infra apply",
        "ocgen approve",
    ];
    let must_allow = ["git status", "git log --grep=push"];
    let leaks: Vec<&str> = must_block
        .iter()
        .copied()
        .filter(|c| run(c) != Some(2))
        .collect();
    let blocked: Vec<&str> = must_allow
        .iter()
        .copied()
        .filter(|c| run(c) != Some(0))
        .collect();
    let _ = std::fs::remove_dir_all(&empty_home);
    let via = if ocgen_hook_ok() {
        "via `ocgen hook`"
    } else {
        "via the bundled script"
    };
    if !leaks.is_empty() || !blocked.is_empty() {
        return check(
            name,
            Status::Fail,
            format!(
                "not blocked: [{}]; wrongly blocked: [{}] ({via}) — run `ocgen doctor`",
                leaks.join(", "),
                blocked.join(", ")
            ),
        );
    }
    if root.join(".claude/team/execution-approved").exists() {
        return check(
            name,
            Status::Warn,
            "an old .claude/team/execution-approved file is present — it no longer unlocks anything; delete it and use `ocgen approve`",
        );
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match home.and_then(|h| crate::approval::remaining(&h, root)) {
        Some(left) => check(
            name,
            Status::Warn,
            format!("gate works, but execution is APPROVED for another {} min (`ocgen approve --revoke` re-locks)", left.div_ceil(60)),
        ),
        None => check(name, Status::Pass, format!("blocks pushes (also with flags), deploys and self-approval; allows normal work ({via})")),
    }
}

/// The git half of the gate: a pre-push hook that stops unapproved pushes however
/// they are launched.
fn pre_push(project: &Project, root: &Path) -> Check {
    let name = "git pre-push hook";
    if !(project.claude.team.enabled && project.claude.team.approval_gate) {
        return check(name, Status::Skip, "approval gate not enabled");
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) else {
        return check(name, Status::Skip, "not a git repository");
    };
    if git(&["config", "core.hooksPath"]).is_some_and(|p| !p.is_empty()) {
        return check(name, Status::Warn, "this repo uses core.hooksPath — add `sh .claude/hooks/git-pre-push.sh || exit 1` to its pre-push hook");
    }
    match std::fs::read_to_string(PathBuf::from(common).join("hooks/pre-push")) {
        Ok(h) if h.contains("ocgen:pre-push") => check(
            name,
            Status::Pass,
            "installed — unapproved pushes are blocked even from scripts",
        ),
        Ok(_) => check(
            name,
            Status::Warn,
            "another pre-push hook exists — add `sh .claude/hooks/git-pre-push.sh || exit 1` to it",
        ),
        Err(_) => check(name, Status::Warn, "not installed — run `ocgen doctor`"),
    }
}

fn statusline(sh: &Path, root: &Path, settings: &Value) -> Check {
    let name = "statusline renders";
    let Some(cmd) = settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
    else {
        return check(name, Status::Skip, "no statusLine configured");
    };
    let payload = serde_json::json!({
        "model": { "display_name": "ocgen verify" },
        "workspace": {
            "current_dir": crate::paths::for_shell(root),
            "project_dir": crate::paths::for_shell(root)
        },
        "context_window": { "remaining_percentage": 50 },
    })
    .to_string();
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_PROJECT_DIR".to_string(),
        crate::paths::for_shell(root),
    );
    match run_sh(sh, cmd, &payload, &env, root, Duration::from_secs(10)) {
        Some((0, out, _)) if !out.trim().is_empty() => {
            let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
                .unwrap()
                .replace_all(out.trim(), "");
            check(name, Status::Pass, format!("\"{plain}\""))
        }
        Some((code, _, err)) => check(
            name,
            Status::Fail,
            format!(
                "exit {code}, no output ({})",
                err.lines().next().unwrap_or("").trim()
            ),
        ),
        None => check(name, Status::Fail, "did not finish within 10s"),
    }
}

/// The objective gate: is a check command configured, and (on request) does it pass?
fn check_command(sh: &Path, project: &Project, root: &Path, run: bool) -> Check {
    let name = "check command";
    let cmd = project.claude.workflow.check_cmd.trim();
    if cmd.is_empty() {
        return check(
            name,
            Status::Warn,
            "none configured — the gates rely on self-reported confidence. Set one (e.g. `cargo test`) for an objective check",
        );
    }
    if !run {
        return check(
            name,
            Status::Pass,
            format!("`{cmd}` gates finished work (not run here; add --run-check)"),
        );
    }
    match run_sh(sh, cmd, "", &HashMap::new(), root, Duration::from_secs(600)) {
        Some((0, _, _)) => check(name, Status::Pass, format!("`{cmd}` passes")),
        Some((code, out, err)) => check(
            name,
            Status::Fail,
            format!(
                "`{cmd}` exits {code}: {}",
                format!("{out}{err}")
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
            ),
        ),
        None => check(
            name,
            Status::Fail,
            format!("`{cmd}` did not finish within 10 minutes"),
        ),
    }
}

fn mcp_json(root: &Path) -> Check {
    let name = ".mcp.json";
    match std::fs::read_to_string(root.join(".mcp.json")) {
        Err(_) => check(name, Status::Skip, "no project MCP servers"),
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) if v.get("mcpServers").is_some_and(Value::is_object) => check(
                name,
                Status::Pass,
                format!(
                    "{} server(s)",
                    v["mcpServers"].as_object().map_or(0, |m| m.len())
                ),
            ),
            _ => check(
                name,
                Status::Fail,
                "not valid (needs a top-level mcpServers object)",
            ),
        },
    }
}

/// The first `ocgen` on PATH, if any.
fn which_ocgen() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ocgen.exe" } else { "ocgen" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

fn ocgen_hook_ok() -> bool {
    which_ocgen().is_some_and(|p| {
        Command::new(p)
            .args(["hook", "--check"])
            .output()
            .is_ok_and(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout).trim() == crate::hooks::PROTOCOL
            })
    })
}

fn ocgen_on_path() -> Check {
    let name = "ocgen on PATH";
    match which_ocgen() {
        None => check(name, Status::Warn, "not found — hooks use the bundled sh scripts (install ocgen for the Rust hooks and Windows support)"),
        Some(p) if ocgen_hook_ok() => check(name, Status::Pass, format!("{} (hooks run in Rust)", p.display())),
        Some(p) => check(
            name,
            Status::Warn,
            format!(
                "{} is too old for `ocgen hook` — hooks fall back to the sh scripts. Reinstall ocgen so this path has a current build",
                p.display()
            ),
        ),
    }
}

fn git_tracking(root: &Path) -> Check {
    let name = ".claude/ tracked by git";
    match crate::gitcheck::claude_config_ignored(root) {
        None => check(name, Status::Skip, "not a git repository"),
        Some(false) => check(
            name,
            Status::Pass,
            "worktree sessions get the settings, hooks and rules",
        ),
        Some(true) => check(name, Status::Warn, crate::gitcheck::IGNORED_CONFIG_WARNING),
    }
}

fn claude_validate(sh: &Path, project: &Project, root: &Path) -> Check {
    let name = "Claude Code validation";
    let has_claude = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|d| d.join("claude").is_file());
    if !has_claude {
        return check(name, Status::Skip, "claude CLI not found");
    }
    let mut targets: Vec<PathBuf> = [".claude/agents", ".claude/skills"]
        .iter()
        .map(|d| root.join(d))
        .filter(|d| d.is_dir())
        .collect();
    if project.claude.output.plugin {
        if let Ok(entries) = std::fs::read_dir(root.join("plugin")) {
            targets.extend(
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir()),
            );
        }
    }
    let mut failed = Vec::new();
    for t in &targets {
        let cmd = format!(
            "claude plugin validate \"{}\" </dev/null",
            crate::paths::for_shell(t)
        );
        match run_sh(sh, &cmd, "", &HashMap::new(), root, Duration::from_secs(90)) {
            Some((0, out, _)) if out.contains("Validation passed") => {}
            Some((_, out, err)) => failed.push(format!(
                "{}: {}",
                t.strip_prefix(root).unwrap_or(t).display(),
                format!("{out}{err}")
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
            )),
            None => failed.push(format!("{}: timed out", t.display())),
        }
    }
    if failed.is_empty() {
        check(
            name,
            Status::Pass,
            format!("{} target(s) pass `claude plugin validate`", targets.len()),
        )
    } else {
        check(name, Status::Fail, failed.join("; "))
    }
}
