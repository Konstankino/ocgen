//! `ocgen verify`: check that a generated project actually works — the checks
//! that otherwise get done by hand (or discovered by the user): is it up to date,
//! is settings.json sane, do the hook commands run and does the approval gate
//! block, does the statusline render, is a compatible ocgen on PATH, and does
//! Claude Code's own validator accept the agents, skills and plugin.
//!
//! A repository's `.claude/` files are the repository's, not ocgen's, so verify
//! runs only what ocgen itself generates for the project: hook commands and
//! scripts are re-rendered from the state and compared with the files, and one
//! that differs is reported, never run. Only `--run-check` runs the project's
//! own check command, on request.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
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
    /// The user's Claude Code settings file, merged under the project's. `None`:
    /// the one Claude Code reads (`$CLAUDE_CONFIG_DIR/settings.json`, else
    /// `~/.claude/settings.json`).
    pub user_settings: Option<PathBuf>,
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

/// The project's settings file, relative to its root.
const SETTINGS: &str = ".claude/settings.json";
/// The git-ignored per-user override Claude Code merges over it.
const LOCAL: &str = ".claude/settings.local.json";

/// Run every check. Hook commands are executed with the loop guard switched off
/// (`LOOP_GUARD_MAX_BLOCKS=0`), so verification leaves no state behind.
pub fn verify(project: &Project, root: &Path, opts: &Options) -> Vec<Check> {
    let mut out = vec![up_to_date(project, root), consistency(project, root)];
    if project.target != Target::ClaudeCode {
        return out;
    }
    // ocgen's own render of the project: the only commands and scripts verify
    // runs. The repository's files are compared with it, never run as found.
    let rendered: HashMap<String, String> = project
        .render_all()
        .map(|files| {
            files
                .into_iter()
                .map(|(rel, c)| (rel.to_string_lossy().replace('\\', "/"), c))
                .collect()
        })
        .unwrap_or_default();
    let expected: Value = rendered
        .get(SETTINGS)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let user_file = match &opts.user_settings {
        Some(p) => Some((p.clone(), crate::paths::for_shell(p))),
        None => user_settings(),
    };
    let layers = Layers::load(root, user_file);
    let settings = layers.of(Scope::Project).cloned();
    out.push(settings_check(root, settings.as_ref()));
    let user = layers.of(Scope::User).cloned().unwrap_or(Value::Null);
    let sh = shell(&user);
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
    out.push(line_endings(root, &rendered));
    if let Some(s) = &settings {
        let probe = Probe {
            sh: &sh,
            root,
            rendered: &rendered,
            expected: &expected,
            disk: s,
            env: layers.hook_env(root),
        };
        if shell_ok {
            out.push(hook_scripts(&sh, root, s));
            out.push(hook_commands(&probe));
        }
        out.push(hook_shell(
            s,
            cfg!(windows),
            find_git_bash(&user).as_deref(),
        ));
        out.push(effective(&layers, &expected));
        if shell_ok {
            out.push(approval_gate(&probe, layers.gate_off()));
            out.push(https_only_fetch(&probe));
            out.push(noop_cd(&probe));
            out.push(notes_view(&probe));
            out.push(notes_word(&probe));
            out.push(doc_versions(&probe, project));
            out.push(draft_review(&probe));
            out.push(recap_github(&probe));
        }
        out.push(sandbox(project, &layers));
        out.push(git_credentials(project, root, &layers));
        out.push(pre_push(shell_ok.then_some(sh.as_path()), project, root));
        if shell_ok {
            out.push(statusline(&probe, &layers));
        }
    }
    if shell_ok {
        out.push(check_command(&sh, project, root, opts.run_check));
    }
    out.push(mcp_json(root));
    out.push(codeowners(project, root));
    out.push(intent_approvers(project, root));
    out.push(ocgen_on_path());
    out.extend(ignored_overrides());
    out.push(git_tracking(root));
    if opts.run_claude {
        out.push(claude_validate(project, root));
    }
    out
}

/// The shell verify runs scripts and hook commands with — the one Claude Code
/// uses. On Windows that is Git Bash: a per-user Git install puts only
/// `Git\cmd` on PATH, so a bare `sh` is often missing outside Git Bash.
/// Elsewhere, `sh` from PATH. `user` is the user's own settings.
fn shell(user: &Value) -> PathBuf {
    if cfg!(windows) {
        if let Some(bash) = find_git_bash(user) {
            return bash;
        }
    }
    PathBuf::from("sh")
}

/// Where a settings file comes from, least specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    User,
    Project,
    Local,
    Managed,
}

/// The settings files Claude Code merges — the user's, the project's, the
/// git-ignored local one and the organisation's managed policy — least specific
/// first, each with the name to report it by. The more specific file wins key
/// by key (`env` too), so any of them can switch off what settings.json turns on.
struct Layers(Vec<(Scope, String, Value)>);

impl Layers {
    /// The layers for the project at `root`, with the user's settings file (and
    /// the name to report it by) from `user`.
    fn load(root: &Path, user: Option<(PathBuf, String)>) -> Self {
        let read = |p: &Path| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        };
        let mut files = Vec::new();
        if let Some((path, label)) = user {
            if let Some(v) = read(&path) {
                files.push((Scope::User, label, v));
            }
        }
        for (scope, rel) in [(Scope::Project, SETTINGS), (Scope::Local, LOCAL)] {
            if let Some(v) = read(&root.join(rel)) {
                files.push((scope, rel.to_string(), v));
            }
        }
        let managed = managed_settings();
        if let Some(v) = read(&managed) {
            let label = format!("the managed policy ({})", managed.display());
            files.push((Scope::Managed, label, v));
        }
        Layers(files)
    }

    fn of(&self, scope: Scope) -> Option<&Value> {
        self.0.iter().find(|(s, ..)| *s == scope).map(|(.., v)| v)
    }

    /// The value at `pointer` that wins, and the file it comes from.
    fn get(&self, pointer: &str) -> Option<(&Value, &str)> {
        self.0
            .iter()
            .rev()
            .find_map(|(_, label, v)| v.pointer(pointer).map(|x| (x, label.as_str())))
    }

    /// Why no project hook runs at all, if none does, and the file that says so.
    fn hooks_off(&self) -> Option<(String, &str)> {
        if let Some((v, from)) = self.get("/disableAllHooks") {
            if v.as_bool() == Some(true) {
                return Some((format!("{from} sets disableAllHooks"), from));
            }
        }
        // Only a managed policy can limit hooks to its own.
        self.0
            .iter()
            .find(|(s, _, v)| {
                *s == Scope::Managed && v.get("allowManagedHooksOnly") == Some(&Value::Bool(true))
            })
            .map(|(_, label, _)| {
                (
                    format!("{label} sets allowManagedHooksOnly"),
                    label.as_str(),
                )
            })
    }

    /// Why the approval gate doesn't run in Claude Code, if it doesn't, and the
    /// file that says so.
    fn gate_off(&self) -> Option<(String, &str)> {
        self.hooks_off()
            .or_else(|| match self.get("/env/TEAM_APPROVAL_GATE") {
                Some((v, _)) if text(v) == "1" => None,
                Some((v, from)) => {
                    Some((format!("{from} sets TEAM_APPROVAL_GATE={}", text(v)), from))
                }
                None => Some((
                    format!("TEAM_APPROVAL_GATE is missing from {SETTINGS}"),
                    SETTINGS,
                )),
            })
    }

    /// The env a hook probe gets: ocgen's gate settings from every file, the
    /// most specific winning as in Claude Code, plus the project dir, and no loop
    /// guard or check command. Nothing else from a settings file reaches a probe:
    /// PATH, BASH_ENV or a browser command there would run the repository's code.
    fn hook_env(&self, root: &Path) -> HashMap<String, String> {
        let mut env = HashMap::new();
        for (_, _, v) in &self.0 {
            for (k, val) in v
                .get("env")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if probe_env_key(k) {
                    env.insert(k.clone(), text(val));
                }
            }
        }
        env.insert("CLAUDE_PROJECT_DIR".into(), crate::paths::for_shell(root));
        env.insert("LOOP_GUARD_MAX_BLOCKS".into(), "0".into());
        // Probing the gates must not run the project's test suite.
        env.insert("OCGEN_CHECK_CMD".into(), String::new());
        // …nor change state: the gates decide but write nothing (no plan stamp).
        env.insert("OCGEN_HOOK_PROBE".into(), "1".into());
        env
    }
}

/// How to undo a setting that weakens the gates, given the file it is in:
/// ocgen restores its own settings.json; the other files are the user's.
fn undo(from: &str) -> String {
    if from == SETTINGS {
        "`ocgen doctor` restores the generated settings.json".into()
    } else {
        format!("remove it from {from}")
    }
}

/// Env keys a hook probe may take from the settings files: ocgen's gate
/// settings, minus the keys that name a command to run.
fn probe_env_key(key: &str) -> bool {
    const PREFIXES: [&str; 4] = ["OCGEN_", "TEAM_", "LOOP_GUARD_", "SUBAGENT_"];
    const COMMANDS: [&str; 4] = [
        "OCGEN_CHECK_CMD",
        "OCGEN_FORMAT_CMD",
        "OCGEN_NOTES_BROWSER",
        "OCGEN_RECAP_GH",
    ];
    (PREFIXES.iter().any(|p| key.starts_with(p)) || key == "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")
        && !COMMANDS.contains(&key)
}

/// A settings value as the text a hook sees in its env.
fn text(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

/// The user's own settings file — `$CLAUDE_CONFIG_DIR/settings.json`, else
/// `~/.claude/settings.json` — and the name to report it by.
fn user_settings() -> Option<(PathBuf, String)> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        let path = PathBuf::from(dir).join("settings.json");
        let label = crate::paths::for_shell(&path);
        return Some((path, label));
    }
    // Claude Code's home (node's os.homedir): USERPROFILE on Windows, else HOME.
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    Some((
        home.join(".claude/settings.json"),
        "~/.claude/settings.json".into(),
    ))
}

/// Where Claude Code reads an organisation's managed policy.
fn managed_settings() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("ProgramFiles")
            .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
            .join(r"ClaudeCode\managed-settings.json")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    }
}

/// What the hook probes run: the hooks ocgen generates for the project — never
/// a command or script as found in the repository, which may not be ocgen's.
struct Probe<'a> {
    sh: &'a Path,
    root: &'a Path,
    /// Every file ocgen generates for the project, by relative path.
    rendered: &'a HashMap<String, String>,
    /// The settings.json ocgen generates.
    expected: &'a Value,
    /// The project's settings.json as found.
    disk: &'a Value,
    /// The env the hooks get (see [`Layers::hook_env`]).
    env: HashMap<String, String>,
}

impl Probe<'_> {
    /// Every generated `(event, group, command)`, in table order.
    fn generated(&self) -> Vec<(String, Value, String)> {
        let mut v = Vec::new();
        for (event, groups) in self
            .expected
            .get("hooks")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            for g in groups.as_array().into_iter().flatten() {
                for h in g
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(c) = h.get("command").and_then(Value::as_str) {
                        v.push((event.clone(), g.clone(), c.to_string()));
                    }
                }
            }
        }
        v
    }

    /// The generated hook for `event` that runs `.claude/hooks/<script>.sh` and
    /// no other script. Mentioning the name isn't enough: the formatter hook
    /// carries the state's own command, which may name any hook.
    fn find(&self, event: &str, script: &str) -> Option<(String, Value, String)> {
        let want = format!(".claude/hooks/{script}.sh");
        self.generated().into_iter().find(|(e, _, c)| {
            let mut runs = scripts_in(c);
            runs.dedup();
            e == event && runs == [want.as_str()]
        })
    }

    /// The file that keeps a generated hook from being run here, if one does:
    /// settings.json when it no longer holds the hook exactly as generated, else
    /// the first script it runs that was edited.
    fn hand_edited(&self, event: &str, group: &Value, cmd: &str) -> Option<String> {
        let kept = self
            .disk
            .pointer(&format!("/hooks/{event}"))
            .and_then(Value::as_array)
            .is_some_and(|groups| groups.contains(group));
        if !kept {
            return Some("settings.json".into());
        }
        self.edited_script(cmd)
    }

    /// The first script `cmd` runs (with the loop guard the gates source) that
    /// exists but isn't what ocgen generates. A missing one is fine: nothing of
    /// it runs, and the probe reports the failure.
    fn edited_script(&self, cmd: &str) -> Option<String> {
        let mut scripts: Vec<String> = scripts_in(cmd).into_iter().map(String::from).collect();
        let guard = ".claude/hooks/loop-guard.sh";
        if scripts.iter().any(|s| s.starts_with(".claude/hooks/"))
            && self.rendered.contains_key(guard)
        {
            scripts.push(guard.into());
        }
        scripts.into_iter().find(|rel| {
            std::fs::read(self.root.join(rel)).is_ok_and(|found| {
                self.rendered
                    .get(rel)
                    .is_none_or(|want| want.as_bytes() != found.as_slice())
            })
        })
    }
}

/// The project scripts `cmd` names (`.claude/hooks/x.sh`, `.claude/statusline.sh`),
/// in order, relative to the project root.
fn scripts_in(cmd: &str) -> Vec<&str> {
    let re =
        regex::Regex::new(r"/(\.claude/(?:hooks/[A-Za-z0-9_.-]+\.sh|statusline\.sh))").unwrap();
    re.captures_iter(cmd)
        .filter_map(|m| m.get(1))
        .map(|m| m.as_str())
        .collect()
}

/// Why verify leaves a hook alone: `what` isn't what ocgen generates.
fn not_run(what: &str) -> String {
    format!(
        "{what} differs from what ocgen generates — not running a hand-edited command; run `ocgen doctor`"
    )
}

/// Why a security `gate` (the approval gate, the WebFetch guard, a team gate)
/// fails unrun: `what` isn't what ocgen generates, so nothing shows it still
/// blocks — the one that let everything through would look the same.
fn unverified(gate: &str, what: &str) -> String {
    format!(
        "not the {gate} ocgen generates — unverified: {what} differs from what ocgen generates (not running a hand-edited command); `ocgen doctor` restores it"
    )
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
                    "can't start {} ({why}) — {fix}. The hook script, hook command, approval gate, WebFetch guard, no-op cd, notes view, language versions, pre-push, statusline and check command checks were skipped",
                    sh.display()
                ),
            ))
        }
    }
}

fn up_to_date(project: &Project, root: &Path) -> Check {
    let name = "files up to date";
    match project.plan_changes(root) {
        // A path ocgen won't write: the message says what to change.
        Err(e) if e.is::<crate::render::Refused>() => check(name, Status::Fail, e.to_string()),
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

fn consistency(project: &Project, root: &Path) -> Check {
    let mut issues = project.issues();
    issues.extend(project.skill_file_issues(root));
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
/// `CLAUDE_CODE_GIT_BASH_PATH` override, the usual install locations, then PATH
/// (skipping the WSL launcher in System32). The override comes from the
/// environment or the user's own settings (`user`) — never the project's, which
/// would let a repository choose the program verify runs.
fn find_git_bash(user: &Value) -> Option<PathBuf> {
    let configured = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            user.pointer("/env/CLAUDE_CODE_GIT_BASH_PATH")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        // A relative path would resolve against wherever verify runs.
        .filter(|p| p.is_absolute());
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

/// The sandbox behind the approval gate. Without it the gate is a text match an
/// agent can phrase around — advisory; with it, sandboxed commands can't reach
/// the approval store or (by default) push and deploy credentials. `on` is
/// whether it is on in the settings Claude Code merges, `off_by` the file that
/// turns it off, `os` is `std::env::consts::OS` and `bwrap` whether bubblewrap
/// is on PATH.
pub fn sandbox_check(gate: bool, on: bool, off_by: Option<&str>, os: &str, bwrap: bool) -> Check {
    let name = "sandbox";
    if os == "windows" && gate {
        return check(
            name,
            Status::Warn,
            "Claude Code's sandbox isn't available on native Windows, so the approval gate is advisory here — a guard-rail, not a boundary (WSL2 has the sandbox)",
        );
    }
    if os == "windows" && on {
        return check(
            name,
            Status::Warn,
            "on, but Claude Code's sandbox isn't available on native Windows — commands run unsandboxed here (WSL2 has it)",
        );
    }
    if on && os == "linux" && !bwrap {
        return check(
            name,
            Status::Warn,
            "on, but `bwrap` isn't on PATH — Claude Code's Linux sandbox needs bubblewrap (e.g. `sudo apt install bubblewrap`)",
        );
    }
    match (on, gate, off_by) {
        (true, true, _) => check(
            name,
            Status::Pass,
            "on — sandboxed commands, and the check and formatter the hooks run, can't write the approval store",
        ),
        (true, false, _) => check(name, Status::Pass, "on"),
        (false, true, Some(file)) => check(
            name,
            Status::Warn,
            format!("{file} turns the sandbox off, so the approval gate is advisory — remove that, or enable it with `ocgen edit team`"),
        ),
        (false, true, None) => check(
            name,
            Status::Warn,
            "the approval gate is advisory without the sandbox — enable it with `ocgen edit team`",
        ),
        (false, false, _) => check(name, Status::Skip, "off"),
    }
}

/// [`sandbox_check`] for this project, on this machine.
fn sandbox(project: &Project, layers: &Layers) -> Check {
    let gate = project.claude.team.enabled && project.claude.team.approval_gate;
    let (on, off_by) = match layers.get("/sandbox/enabled") {
        Some((v, from)) => {
            let on = v.as_bool() == Some(true);
            (on, (!on).then_some(from))
        }
        None => (false, None),
    };
    sandbox_check(
        gate,
        on,
        off_by,
        std::env::consts::OS,
        find_on_path("bwrap").is_some(),
    )
}

/// The credential helpers in `entries` that ask an OS keychain or a login
/// service: osxkeychain, Git Credential Manager, wincred, libsecret,
/// gnome-keyring and the GitHub CLI (`gh auth git-credential`). The sandbox
/// can't hide those — git reaches the keychain through a system service, not
/// a file. `entries` are `credential.helper` and `credential.<url>.helper`
/// `(key, value)` pairs in the order git reads them; an empty value resets the
/// helpers read before it under its key (every one, for `credential.helper`).
pub fn keychain_helpers(entries: &[(String, String)]) -> Vec<String> {
    let mut kept: Vec<(&str, &str)> = Vec::new();
    for (key, value) in entries {
        match value.trim() {
            "" if key == "credential.helper" => kept.clear(),
            "" => kept.retain(|(k, _)| k != key),
            v => kept.push((key, v)),
        }
    }
    let mut found: Vec<String> = Vec::new();
    for (_, helper) in kept {
        if is_keychain(helper) && !found.iter().any(|f| f == helper) {
            found.push(helper.to_string());
        }
    }
    found
}

/// Whether a credential helper value names a keychain helper: `osxkeychain`,
/// `git-credential-manager` or a path to one (a path with spaces in it too,
/// so any word may name it), or `!… gh auth git-credential`.
fn is_keychain(helper: &str) -> bool {
    let command = helper.trim_start_matches('!');
    let names: Vec<&str> = command
        .split_whitespace()
        .map(|word| {
            let word = word.trim_matches(['"', '\'']);
            let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
            let base = base.strip_suffix(".exe").unwrap_or(base);
            base.strip_prefix("git-credential-").unwrap_or(base)
        })
        .collect();
    names.iter().any(|n| {
        matches!(
            *n,
            "osxkeychain" | "manager" | "manager-core" | "wincred" | "libsecret" | "gnome-keyring"
        )
    }) || (names.contains(&"gh") && command.contains("git-credential"))
}

/// Whether a credential the sandbox is meant to withhold is still in git's
/// reach. `withheld`: the sandbox is on, here, and credentials aren't allowed;
/// `entries` as for [`keychain_helpers`].
pub fn keychain_check(withheld: bool, entries: &[(String, String)]) -> Check {
    let name = "git credentials";
    if !withheld {
        return check(
            name,
            Status::Skip,
            "not withheld (no sandbox, or credentials allowed)",
        );
    }
    let found = keychain_helpers(entries);
    if found.is_empty() {
        return check(
            name,
            Status::Pass,
            "no keychain credential helper — sandboxed git has no token to ask for",
        );
    }
    check(
        name,
        Status::Warn,
        format!(
            "git can get a token from {} — a keychain the sandbox can't hide. Claude Code's shells start with no credential helper, but an agent can name one in its own command (`git -c credential.helper=…`): protect every branch on the server, or remove the stored credential if you don't need it",
            found.join(", ")
        ),
    )
}

/// [`keychain_check`] for this project, on this machine: the helpers git reads
/// in the repository (its config files only — not a reset in this process's
/// environment, such as the one Claude Code's shells get). Read, never run.
fn git_credentials(project: &Project, root: &Path, layers: &Layers) -> Check {
    let on = layers
        .get("/sandbox/enabled")
        .is_some_and(|(v, _)| v.as_bool() == Some(true));
    let withheld =
        on && !project.claude.sandbox.allow_credentials && crate::claude::sandbox_supported();
    let mut git = Command::new("git");
    git.arg("-C")
        .arg(root)
        .args(["config", "--get-regexp", r"^credential\..*helper$"]);
    for var in ["GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS"] {
        git.env_remove(var);
    }
    let listed = git
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let entries: Vec<(String, String)> = listed
        .lines()
        .map(|l| match l.split_once(' ') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (l.to_string(), String::new()),
        })
        .collect();
    keychain_check(withheld, &entries)
}

/// Claude Code merges the user's settings, the project's, a git-ignored
/// settings.local.json and any managed policy, the most specific winning key by
/// key. Any of them can switch the hooks off or loosen a gate — also with a
/// key ocgen doesn't set — while the generated settings.json looks fine, and
/// the probes alone wouldn't show it.
fn effective(layers: &Layers, expected: &Value) -> Check {
    let name = "effective settings";
    let want_env: Vec<(&String, &Value)> = expected
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .collect();
    let has_hooks = expected
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|h| !h.is_empty());
    if !has_hooks && want_env.is_empty() {
        return check(name, Status::Skip, "no hooks or gate settings to override");
    }
    let mut weaker = Vec::new();
    // The files the weakening settings are in, for how to undo them.
    let mut culprits: Vec<&str> = Vec::new();
    let mut changed = Vec::new();
    if has_hooks {
        if let Some((why, from)) = layers.hooks_off() {
            weaker.push(format!(
                "{why}, so no hook runs and the gates let everything through"
            ));
            culprits.push(from);
        }
    }
    let short = |s: &str| match s.char_indices().nth(40) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    };
    // Every key ocgen sets, then every other key any file sets: a gate switch
    // ocgen leaves out can loosen a gate as well.
    let mut keys: Vec<(&str, Option<String>)> = want_env
        .iter()
        .map(|(k, v)| (k.as_str(), Some(text(v))))
        .collect();
    let mut extra: Vec<&str> = layers
        .0
        .iter()
        .filter_map(|(.., v)| v.get("env").and_then(Value::as_object))
        .flat_map(|env| env.keys().map(String::as_str))
        .filter(|k| !want_env.iter().any(|(w, _)| w == k))
        .collect();
    extra.sort_unstable();
    extra.dedup();
    keys.extend(extra.into_iter().map(|k| (k, None)));
    for (key, want) in keys {
        // `/` and `~` are escaped in a JSON pointer.
        let pointer = format!("/env/{}", key.replace('~', "~0").replace('/', "~1"));
        let (got, from) = match layers.get(&pointer) {
            Some((v, from)) => (Some(text(v)), from),
            None => (None, SETTINGS),
        };
        if got == want {
            continue;
        }
        let weak = weakens(key, want.as_deref(), got.as_deref(), expected);
        let line = match (&got, &want) {
            (Some(g), Some(w)) => format!("{from} sets {key}={} (ocgen: {})", short(g), short(w)),
            (Some(g), None) => format!("{from} sets {key}={} (not set by ocgen)", short(g)),
            (None, Some(w)) => format!("{key} is missing from {from} (ocgen: {})", short(w)),
            (None, None) => continue,
        };
        if weak {
            weaker.push(line);
            culprits.push(from);
        } else if want.is_some() {
            // A key ocgen doesn't set is worth a word only when it loosens a gate.
            changed.push(line);
        }
    }
    if !weaker.is_empty() {
        weaker.extend(changed);
        culprits.sort_unstable();
        culprits.dedup();
        let fixes: Vec<String> = culprits.into_iter().map(undo).collect();
        check(
            name,
            Status::Fail,
            format!("{} — {}", weaker.join("; "), fixes.join("; ")),
        )
    } else if !changed.is_empty() {
        check(name, Status::Warn, changed.join("; "))
    } else {
        check(
            name,
            Status::Pass,
            "no settings.local.json, user or managed setting turns the hooks off or overrides the gate settings",
        )
    }
}

/// Whether `got` in place of ocgen's `want` (`None`: ocgen doesn't set it) for
/// `key` loosens a gate ocgen generates (`ocgen`: its settings.json): a switch
/// turned off, a confidence bar lowered, the objective check dropped, a role
/// exempted that the gate holds, a loop budget that releases the quality gates
/// sooner, the team task gate added where no task gate judges teammates, a
/// documentation site trusted (as the ConfigChange audit judges it), the
/// hooks' sandbox off or protecting less, verify's own probe switch (the
/// gates then decide but record nothing), or a stand-in for gh in /recap's
/// GitHub hook (it would run with the user's gh login, outside the sandbox).
/// A key that no generated gate reads loosens nothing.
///
/// Values are read as the hooks read them — roles and sites in any letter case,
/// a number as either twin reads it, the weaker reading counting: the sh
/// scripts take plain digits only, the Rust hook a u32 (a leading `+` too),
/// and each reads anything else as 0.
fn weakens(key: &str, want: Option<&str>, got: Option<&str>, ocgen: &Value) -> bool {
    let hook = |event: &str| ocgen.pointer(&format!("/hooks/{event}")).is_some();
    let on = |k: &str| ocgen.pointer(&format!("/env/{k}")).and_then(Value::as_str) == Some("1");
    // [the sh scripts' reading, the Rust hook's]; sh's `[ -gt ]` can't hold more
    // than an i64.
    let reads = |v: Option<&str>| {
        let v = v.unwrap_or("");
        let sh = if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
            v.parse::<u64>()
                .ok()
                .filter(|n| i64::try_from(*n).is_ok())
                .unwrap_or(0)
        } else {
            0
        };
        [sh, v.parse::<u32>().map_or(0, u64::from)]
    };
    // Whether the list `got` names an entry ocgen's `want` doesn't.
    let adds = || {
        let ours: Vec<&str> = want.unwrap_or("").split_whitespace().collect();
        got.unwrap_or("")
            .split_whitespace()
            .any(|x| !ours.iter().any(|o| o.eq_ignore_ascii_case(x)))
    };
    match key {
        "TEAM_APPROVAL_GATE" | "TEAM_PLAN_GATE" | "TEAM_RISK_ROUNDS" | "OCGEN_SANDBOX" => {
            want == Some("1") && got != Some("1")
        }
        "OCGEN_SANDBOX_DENY_WRITE" | "OCGEN_SANDBOX_DENY_READ" | "OCGEN_SANDBOX_DENY_ENV" => {
            let have: Vec<&str> = got.unwrap_or_default().split_whitespace().collect();
            want.is_some_and(|w| w.split_whitespace().any(|x| !have.contains(&x)))
        }
        "TEAM_CONFIDENCE_THRESHOLD" | "SUBAGENT_CONFIDENCE_THRESHOLD" => {
            let w = reads(want)[0];
            w > 0 && reads(got).iter().any(|&g| g < w)
        }
        "OCGEN_CHECK_CMD" => {
            want.is_some_and(|w| !w.trim().is_empty()) && got.is_none_or(|g| g.trim().is_empty())
        }
        "SUBAGENT_READONLY_ROLES" => hook("SubagentStop") && adds(),
        "TEAM_READONLY_ROLES" => hook("TeammateIdle") && on("TEAM_RISK_ROUNDS") && adds(),
        "OCGEN_WEBFETCH_DOMAINS" => want.is_some() && adds(),
        // 0 (or unreadable) never releases a gate; with ocgen's 0, any budget does.
        "LOOP_GUARD_MAX_BLOCKS" => {
            let w = reads(want)[0];
            want.is_some() && reads(got).iter().any(|&g| g > 0 && (w == 0 || g < w))
        }
        "TEAM_TASK_GATE" => hook("SubagentStop") && got == Some("1") && want != Some("1"),
        "OCGEN_HOOK_PROBE" => got.is_some(),
        // Replaces gh in /recap's GitHub hook, which runs with the user's gh
        // login outside the sandbox; ocgen never sets it (tests only).
        "OCGEN_RECAP_GH" => got.is_some_and(|g| !g.trim().is_empty()),
        // The reset that keeps git from asking a keychain (claude::GIT_HELPER_RESET).
        "GIT_CONFIG_COUNT" | "GIT_CONFIG_KEY_0" | "GIT_CONFIG_VALUE_0" => {
            want.is_some() && got != want
        }
        _ => false,
    }
}

/// Generated scripts must keep LF line endings. A CRLF checkout (Windows,
/// `core.autocrlf` without a `.gitattributes`) makes sh fail on them, so the
/// hooks break — and a gate that errors can block every call.
fn line_endings(root: &Path, rendered: &HashMap<String, String>) -> Check {
    let name = "line endings";
    let mut scripts: Vec<&str> = rendered
        .keys()
        .map(String::as_str)
        .filter(|r| r.ends_with(".sh"))
        .collect();
    if scripts.is_empty() {
        return check(name, Status::Skip, "no generated scripts");
    }
    scripts.sort_unstable();
    let crlf: Vec<&str> = scripts
        .iter()
        .copied()
        .filter(|r| std::fs::read(root.join(r)).is_ok_and(|b| b.contains(&b'\r')))
        .collect();
    if crlf.is_empty() {
        return check(
            name,
            Status::Pass,
            format!("{} script(s) use LF", scripts.len()),
        );
    }
    check(
        name,
        Status::Fail,
        format!(
            "{} {} CRLF line endings — sh can't run them, so the hooks break. Pin LF in .gitattributes (`*.sh text eol=lf`, `.claude/** text eol=lf`), run `git add --renormalize .`, then `ocgen doctor`",
            crlf.join(", "),
            if crlf.len() == 1 { "has" } else { "have" }
        ),
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

/// A fresh path in the temp dir for one of verify's scratch files or folders.
fn scratch_path(kind: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "ocgen-verify-{}-{}-{kind}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Read all of `r` on a thread, so the child writing it never waits for us.
fn drain<R: Read + Send + 'static>(mut r: R) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// Run `cmd` with `stdin`, up to `limit`: `(exit code, stdout, stderr)`, or why
/// it didn't finish. Both outputs are read while it runs — a pipe nobody reads
/// fills up (about 64 KiB) and stalls the child until the limit. A timeout
/// stops everything the command started, not just the shell.
pub(crate) fn run(
    mut cmd: Command,
    stdin: &str,
    limit: Duration,
) -> Result<(i32, String, String), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a timeout can stop the whole tree.
        cmd.process_group(0);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start it ({e})"))?;
    #[cfg(unix)]
    let _watch = Watch::start(child.id());
    let readers = [
        child.stdout.take().map(drain),
        child.stderr.take().map(drain),
    ];
    if let Some(mut pipe) = child.stdin.take() {
        if !stdin.is_empty() {
            let data = stdin.as_bytes().to_vec();
            // From a thread too: a child that never reads its input can't stall us.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
    }
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > limit => {
                kill_tree(&mut child);
                return Err("timed out".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill_tree(&mut child);
                return Err(e.to_string());
            }
        }
    };
    // A background process it left behind keeps the pipes open: give the
    // output a moment, then stop the stragglers and keep what arrived.
    let settled = |within: Duration| {
        let t = Instant::now();
        while !readers.iter().flatten().all(JoinHandle::is_finished) {
            if t.elapsed() > within {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    };
    if !settled(Duration::from_secs(2)) {
        #[cfg(unix)]
        signal_group(child.id(), "-KILL");
        settled(Duration::from_secs(1));
    }
    let [out, err] = readers.map(|r| {
        r.filter(JoinHandle::is_finished)
            .and_then(|h| h.join().ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    });
    Ok((status.code().unwrap_or(-1), out, err))
}

/// Passes Ctrl-C on to a command [`run`] started. Its process group isn't the
/// terminal's foreground group, so Ctrl-C would stop only verify and leave the
/// command running. This watcher stays in verify's group and hands an
/// interrupt (or a hangup or TERM) on to the command's group. Its stdin is a
/// pipe from verify: if verify goes away any other way, the pipe closes and the
/// watcher stops the command's group. Dropping it ends it quietly.
#[cfg(unix)]
struct Watch(Option<Child>);

#[cfg(unix)]
impl Watch {
    const SCRIPT: &'static str = r#"trap 'kill -s INT -- "-$1" 2>/dev/null; exit 0' INT
trap 'kill -s TERM -- "-$1" 2>/dev/null; exit 0' HUP TERM
read -r line || kill -s TERM -- "-$1" 2>/dev/null
"#;

    fn start(group: u32) -> Self {
        let watcher = Command::new("sh")
            .args(["-c", Self::SCRIPT, "ocgen-verify-watch"])
            .arg(group.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        Watch(watcher.ok())
    }
}

#[cfg(unix)]
impl Drop for Watch {
    fn drop(&mut self) {
        // Killed before its stdin closes, so it never signals the group.
        if let Some(w) = &mut self.0 {
            let _ = w.kill();
            let _ = w.wait();
        }
    }
}

/// Send `signal` (`-TERM`, `-KILL`) to the process group led by `pid`.
#[cfg(unix)]
fn signal_group(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Stop `child` and everything it started: on Unix its process group (TERM, a
/// moment to exit, then KILL), on Windows its process tree.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        signal_group(child.id(), "-TERM");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(1) && matches!(child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(20));
        }
        signal_group(child.id(), "-KILL");
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
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
    let mut c = Command::new(sh);
    c.arg("-c").arg(cmd).current_dir(cwd).envs(env);
    run(c, stdin, limit).ok()
}

/// Run the side-effect-free hook commands with a harmless event and check they
/// exit as expected (a missing script or interpreter shows up as 126/127). A
/// team gate that isn't ocgen's fails unverified; any other hook isn't run.
fn hook_commands(p: &Probe) -> Check {
    let name = "hook commands run";
    let env = &p.env;
    let not_git = std::env::temp_dir();
    let plan_gate = env.get("TEAM_PLAN_GATE").is_some_and(|v| v == "1");
    // The task gate reads the teammate's words from its transcript.
    let transcript = scratch_path("transcript.jsonl");
    let _ = std::fs::write(
        &transcript,
        "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Confidence: 100%\"}]},\"type\":\"assistant\"}\n",
    );
    let mut ran = 0;
    let mut problems = Vec::new();
    // The file that differs → the events not run for it: the team gates, which
    // fail unverified, and the rest, which only aren't run.
    let mut gates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
    const GATES: [&str; 4] = [
        "TaskCreated",
        "TaskCompleted",
        "SubagentStop",
        "TeammateIdle",
    ];
    for (event, group, cmd) in p.generated() {
        // Notification/format/audit hooks have real side effects; skip them here.
        let (payload, expect) = match event.as_str() {
            "SessionStart" => ("{}".to_string(), 0),
            "SubagentStop" => (
                serde_json::json!({ "session_id": "ocgen-verify", "cwd": crate::paths::for_shell(&not_git) }).to_string(),
                0,
            ),
            "TaskCompleted" => (
                serde_json::json!({ "session_id": "ocgen-verify", "task_id": "ocgen-verify", "transcript_path": crate::paths::for_shell(&transcript) }).to_string(),
                0,
            ),
            "TeammateIdle" => (r#"{"session_id":"ocgen-verify","agent_type":"ocgen-verify-probe"}"#.into(), 0),
            "TaskCreated" => {
                let approved = std::fs::read_to_string(p.root.join(".claude/team/plan.md"))
                    .is_ok_and(|p| p.lines().any(|l| l.starts_with("Status: APPROVED")));
                (
                    r#"{"session_id":"ocgen-verify","agent_type":"ocgen-verify-probe"}"#.into(),
                    if plan_gate && !approved { 2 } else { 0 },
                )
            }
            _ => continue,
        };
        if let Some(what) = p.hand_edited(&event, &group, &cmd) {
            let into = if GATES.contains(&event.as_str()) {
                &mut gates
            } else {
                &mut edited
            };
            into.entry(what).or_default().push(event);
            continue;
        }
        ran += 1;
        // An approval belongs to its own team run, so the plan gate may refuse
        // the probe's own task as stale even under an approved plan — but only
        // that refusal is fine: a script that can't run exits 2 too.
        let stale = |err: &str| {
            event == "TaskCreated" && plan_gate && err.starts_with("Plan approval is stale")
        };
        match run_sh(p.sh, &cmd, &payload, env, p.root, Duration::from_secs(20)) {
            None => problems.push(format!("{event}: did not finish")),
            Some((code, _, err)) if code != expect && !(code == 2 && stale(&err)) => {
                problems.push(format!(
                    "{event}: exit {code}, expected {expect} ({})",
                    err.lines().next().unwrap_or("").trim()
                ))
            }
            Some(_) => {}
        }
    }
    let _ = std::fs::remove_file(&transcript);
    let lines = |by_file: BTreeMap<String, Vec<String>>, why: fn(&str) -> String| {
        by_file
            .into_iter()
            .map(|(what, mut events)| {
                events.dedup();
                format!("{}: {}", events.join(", "), why(&what))
            })
            .collect::<Vec<String>>()
    };
    let gates = lines(gates, |what| unverified("gate", what));
    let edited = lines(edited, not_run);
    if !problems.is_empty() || !gates.is_empty() {
        problems.extend(gates);
        problems.extend(edited);
        check(name, Status::Fail, problems.join("; "))
    } else if !edited.is_empty() {
        check(name, Status::Warn, edited.join("; "))
    } else if ran == 0 {
        check(name, Status::Skip, "no side-effect-free hooks to run")
    } else {
        check(
            name,
            Status::Pass,
            format!("{ran} hook command(s) ran as expected"),
        )
    }
}

/// The WebFetch guard must allow only https:// fetches to trusted documentation
/// sites: plain http://, untrusted hosts and look-alikes are blocked. One that
/// isn't ocgen's fails unverified.
fn https_only_fetch(p: &Probe) -> Check {
    let name = "WebFetch guard";
    let Some((event, group, cmd)) = p.find("PreToolUse", "https-only-fetch") else {
        return check(
            name,
            Status::Warn,
            "not generated — every Claude project has one now; run `ocgen doctor` to add it",
        );
    };
    let env = &p.env;
    let run = |url: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "WebFetch",
            "tool_input": { "url": url, "prompt": "verify" }
        })
        .to_string();
        run_sh(p.sh, &cmd, &ev, env, p.root, Duration::from_secs(20)).map(|x| x.0)
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
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Fail, unverified("guard", &what));
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

/// `dir` as one `cd` target, spelled the way the no-op cd hook reads it (and
/// Claude writes it): bare when it can be, else quoted. `None` when no simple
/// spelling carries it.
fn cd_target(dir: &str) -> Option<String> {
    let bare = |c: char| !(c.is_whitespace() || "\"'$`\\;&|<>()*?[]~{}#".contains(c));
    if !dir.is_empty() && dir.chars().all(bare) {
        Some(dir.to_string())
    } else if !dir.contains(['\'', '\r', '\n']) {
        Some(format!("'{dir}'"))
    } else if !dir.contains(['"', '$', '`', '\r', '\n']) {
        Some(format!("\"{dir}\""))
    } else {
        None
    }
}

/// The no-op `cd` hook must drop `cd <project> &&` (keeping the other input
/// fields) and leave a `cd` into any other folder alone — never deciding.
fn noop_cd(p: &Probe) -> Check {
    let name = "no-op cd";
    let Some((event, group, cmd)) = p.find("PreToolUse", "drop-noop-cd") else {
        return check(name, Status::Skip, "hook not enabled");
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    let dir = crate::paths::for_shell(p.root);
    let (Some(here), Some(there)) = (
        cd_target(&dir),
        cd_target(&format!("{dir}/ocgen-verify-elsewhere")),
    ) else {
        return check(
            name,
            Status::Skip,
            "the project path can't be written as a plain or quoted cd target",
        );
    };
    let run = |command: String| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "Bash", "cwd": dir,
            "tool_input": { "command": command, "description": "verify" }
        })
        .to_string();
        run_sh(p.sh, &cmd, &ev, &p.env, p.root, Duration::from_secs(20))
    };
    let elsewhere = run(format!("cd {there} && echo ok"));
    if !matches!(&elsewhere, Some((0, out, _)) if out.trim().is_empty()) {
        return check(
            name,
            Status::Fail,
            format!("touched a cd into another folder ({elsewhere:?}) — run `ocgen doctor`"),
        );
    }
    let Some((0, out, _)) = run(format!("cd {here} && echo ok")) else {
        return check(name, Status::Fail, "the hook failed — run `ocgen doctor`");
    };
    if out.trim().is_empty() {
        return check(
            name,
            Status::Warn,
            "left `cd <project> && …` alone — the sh fallback needs jq (or ocgen on PATH)",
        );
    }
    let v: Value = serde_json::from_str(out.trim()).unwrap_or_default();
    let h = &v["hookSpecificOutput"];
    if h["updatedInput"]["command"] == "echo ok"
        && h["updatedInput"]["description"] == "verify"
        && h.get("permissionDecision").is_none()
    {
        check(
            name,
            Status::Pass,
            "drops `cd <project> &&`; a cd elsewhere is left alone",
        )
    } else {
        check(
            name,
            Status::Fail,
            format!("unexpected output {} — run `ocgen doctor`", out.trim()),
        )
    }
}

/// The /inquire notes view: after a ledger is written, the hook renders its
/// HTML page. Probed on a scratch ledger with the browser switched off.
fn notes_view(p: &Probe) -> Check {
    let name = "/inquire notes view";
    let Some((event, group, cmd)) = p.find("PostToolUse", "inquire-notes") else {
        return check(
            name,
            Status::Skip,
            "/inquire, /recap and reading copies not enabled",
        );
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    let scratch = scratch_path("notes");
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
    let mut env = p.env.clone();
    env.insert("OCGEN_NOTES_OPEN".into(), "0".into());
    let ev = serde_json::json!({
        "session_id": "ocgen-verify", "tool_name": "Write",
        "tool_input": { "file_path": crate::paths::for_shell(&md) }
    })
    .to_string();
    let ran = run_sh(p.sh, &cmd, &ev, &env, p.root, Duration::from_secs(20));
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

/// The language check: a local document written in one of the project's two
/// languages is asked for in the other, and the end of a turn that leaves it
/// behind is held once. Probed on a scratch ledger beside a copy of the
/// project's state, with the browser off; a probe's Stop never holds anything.
fn doc_versions(p: &Probe, project: &crate::render::Project) -> Check {
    let name = "language versions";
    let Some((event, group, cmd)) = p.find("Stop", "inquire-notes") else {
        return check(name, Status::Skip, "one language: nothing to keep in step");
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    let mut quiet = p.env.clone();
    quiet.insert("OCGEN_NOTES_OPEN".into(), "0".into());
    // The write below lands in a scratch copy, outside the project: the hook
    // may keep its record there, so this one run isn't a probe.
    let mut env = quiet.clone();
    env.remove("OCGEN_HOOK_PROBE");
    let stop = serde_json::json!({
        "hook_event_name": "Stop", "session_id": "ocgen-verify", "stop_hook_active": false
    })
    .to_string();
    match run_sh(p.sh, &cmd, &stop, &quiet, p.root, Duration::from_secs(20)) {
        None => return check(name, Status::Fail, "the hook did not finish"),
        Some((code, out, err)) if code != 0 || !out.trim().is_empty() => {
            return check(
                name,
                Status::Fail,
                format!(
                    "the Stop hook exited {code} ({}) or answered a probe — it must hold a turn only for a version owed; run `ocgen doctor`",
                    err.lines().next().unwrap_or(out.lines().next().unwrap_or("")).trim()
                ),
            )
        }
        Some(_) => {}
    }
    let languages = project.doc_languages();
    let Some(second) = languages.get(1) else {
        // One language: only /intent's reading copies are kept in step.
        return if ocgen_hook_ok() {
            check(
                name,
                Status::Pass,
                "each reading copy is checked when a turn ends",
            )
        } else {
            check(
                name,
                Status::Warn,
                format!(
                    "needs ocgen ({}) on PATH — without it nothing asks for reading copies",
                    crate::hooks::PROTOCOL
                ),
            )
        };
    };
    let scratch = scratch_path("versions");
    let state = project.target.state_file();
    let md = scratch.join(".claude/notes/verify.md");
    let made = std::fs::create_dir_all(md.parent().unwrap())
        .and_then(|_| std::fs::create_dir_all(scratch.join(state).parent().unwrap()))
        .and_then(|_| std::fs::copy(p.root.join(state), scratch.join(state)).map(|_| ()))
        .and_then(|_| std::fs::write(&md, "Topic: ocgen verify ledger\n## Q&A log\n"));
    if made.is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
        return check(
            name,
            Status::Warn,
            "could not create a scratch ledger to probe with",
        );
    }
    let wrote = serde_json::json!({
        "hook_event_name": "PostToolUse", "session_id": "ocgen-verify", "tool_name": "Write",
        "tool_input": { "file_path": crate::paths::for_shell(&md) }
    })
    .to_string();
    let ran = run_sh(p.sh, &cmd, &wrote, &env, p.root, Duration::from_secs(20));
    let _ = std::fs::remove_dir_all(&scratch);
    let want = format!("verify.{}.md", crate::notes::versions::suffix(second));
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
        Some((_, out, _)) if out.contains(&want) => check(
            name,
            Status::Pass,
            format!("a document written in one language is asked for in {second} too, and a turn's end checks it"),
        ),
        Some(_) if !ocgen_hook_ok() => check(
            name,
            Status::Warn,
            format!(
                "needs ocgen ({}) on PATH — without it nothing asks for the other language's version",
                crate::hooks::PROTOCOL
            ),
        ),
        Some(_) => check(
            name,
            Status::Fail,
            "the hook ran but didn't ask for the other language's version — run `ocgen doctor`",
        ),
    }
}

/// The /inquire notes word: `note` gets a note for Claude about opening the
/// ledgers, and any other prompt passes untouched. Probed with the browser
/// switched off, in the project itself (it writes nothing then).
fn notes_word(p: &Probe) -> Check {
    let name = "/inquire notes word";
    // Generated only with /inquire (or reading copies): a /recap-only project
    // renders its reports without the word.
    let Some((event, group, cmd)) = p.find("UserPromptSubmit", "inquire-notes") else {
        return check(name, Status::Skip, "/inquire not enabled");
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    let mut env = p.env.clone();
    env.insert("OCGEN_NOTES_OPEN".into(), "0".into());
    let prompt = |prompt: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "hook_event_name": "UserPromptSubmit", "prompt": prompt
        });
        run_sh(
            p.sh,
            &cmd,
            &ev.to_string(),
            &env,
            p.root,
            Duration::from_secs(20),
        )
    };
    let (Some(other), Some(word)) = (prompt("ocgen verify"), prompt(crate::notes::WORDS[0])) else {
        return check(name, Status::Fail, "the hook did not finish");
    };
    if let Some((code, _, err)) = [&other, &word].into_iter().find(|o| o.0 != 0) {
        return check(
            name,
            Status::Fail,
            format!(
                "the hook exited {code} ({}) — it must never stop a prompt; run `ocgen doctor`",
                err.lines().next().unwrap_or("").trim()
            ),
        );
    }
    if !other.1.trim().is_empty() {
        return check(
            name,
            Status::Fail,
            "the hook answered a prompt other than `note` — run `ocgen doctor`",
        );
    }
    if word.1.contains("additionalContext") {
        check(
            name,
            Status::Pass,
            "`note` on its own opens the ledger (or their list) in the browser",
        )
    } else if !ocgen_hook_ok() {
        check(
            name,
            Status::Warn,
            format!(
                "needs ocgen ({}) on PATH — without it `note` reaches Claude as typed",
                crate::hooks::PROTOCOL
            ),
        )
    } else {
        check(
            name,
            Status::Warn,
            "`note` reaches Claude as typed — the ocgen on PATH may predate it; update ocgen",
        )
    }
}

/// /intent's pending intent files and issue drafts @mention every approver the
/// project has now: one drafted before the approvers changed keeps the old list,
/// and an issue filed from it notifies nobody.
fn intent_approvers(project: &Project, root: &Path) -> Check {
    let name = "/intent approvers in drafts";
    let s = &project.claude.intent;
    if !project.claude.workflow.intent {
        return check(name, Status::Skip, "/intent not enabled");
    }
    if s.approvers.is_empty() {
        return check(name, Status::Skip, "no approvers configured");
    }
    let stale = crate::intent::stale(project, root);
    if stale.is_empty() {
        return check(
            name,
            Status::Pass,
            format!("pending intents and drafts name {}", s.approvers.join(", ")),
        );
    }
    let list: Vec<String> = stale
        .iter()
        .take(3)
        .map(|f| format!("{} {} ({})", f.rel, f.gaps.describe(), f.fix(s)))
        .collect();
    let more = match stale.len() {
        n if n > 3 => format!("; {} more", n - 3),
        _ => String::new(),
    };
    check(name, Status::Warn, format!("{}{more}", list.join("; ")))
}

/// The /intent draft review: the word `draft` gets a note for Claude about
/// opening the issue draft, and any other prompt passes untouched; a write to a
/// draft passes silently (the hook remembers it as the session's). Probed with
/// the browser switched off, in the project itself (it writes nothing then).
fn draft_review(p: &Probe) -> Check {
    let name = "/intent draft review";
    let Some((event, group, cmd)) = p.find("UserPromptSubmit", "intent-draft") else {
        return check(name, Status::Skip, "/intent not enabled");
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    // Without the write hooks (after a write, around a Bash call), `draft`
    // can't select the session's draft.
    let mut hooks = Vec::new();
    for event in ["PreToolUse", "PostToolUse"] {
        let Some((event, group, cmd)) = p.find(event, "intent-draft") else {
            return check(
                name,
                Status::Fail,
                format!("no {event} hook to remember the session's draft — run `ocgen doctor`"),
            );
        };
        if let Some(what) = p.hand_edited(&event, &group, &cmd) {
            return check(name, Status::Warn, not_run(&what));
        }
        hooks.push(cmd);
    }
    let (before_cmd, w_cmd) = (hooks.remove(0), hooks.remove(0));
    let mut env = p.env.clone();
    env.insert("OCGEN_NOTES_OPEN".into(), "0".into());
    let run = |cmd: &str, ev: serde_json::Value| {
        run_sh(
            p.sh,
            cmd,
            &ev.to_string(),
            &env,
            p.root,
            Duration::from_secs(20),
        )
    };
    let prompt = |prompt: &str| {
        run(
            &cmd,
            serde_json::json!({
                "session_id": "ocgen-verify", "hook_event_name": "UserPromptSubmit", "prompt": prompt
            }),
        )
    };
    let wrote = run(
        &w_cmd,
        serde_json::json!({
            "session_id": "ocgen-verify", "hook_event_name": "PostToolUse", "tool_name": "Write",
            "tool_input": { "file_path": format!("{}/ocgen-verify.md", crate::notes::draft::DIR) }
        }),
    );
    let bash = |cmd: &str, event: &str| {
        run(
            cmd,
            serde_json::json!({
                "session_id": "ocgen-verify", "hook_event_name": event, "tool_name": "Bash",
                "tool_use_id": "ocgen-verify", "tool_input": { "command": "true" }
            }),
        )
    };
    // The intent files' word: the project's prefix in lowercase, unless taken.
    let intent_word = crate::notes::intents::word(&crate::notes::intents::settings(p.root).prefix);
    let intents = match &intent_word {
        Some(w) => prompt(w).map(Some),
        None => Some(None),
    };
    let (Some(other), Some(word), Some(intents), Some(wrote), Some(before), Some(after)) = (
        prompt("ocgen verify"),
        prompt(crate::notes::draft::WORD),
        intents,
        wrote,
        bash(&before_cmd, "PreToolUse"),
        bash(&w_cmd, "PostToolUse"),
    ) else {
        return check(name, Status::Fail, "the hook did not finish");
    };
    if let Some((code, _, err)) = [
        Some(&other),
        Some(&word),
        intents.as_ref(),
        Some(&wrote),
        Some(&before),
        Some(&after),
    ]
    .into_iter()
    .flatten()
    .find(|o| o.0 != 0)
    {
        return check(
            name,
            Status::Fail,
            format!(
                "the hook exited {code} ({}) — it must never stop a prompt; run `ocgen doctor`",
                err.lines().next().unwrap_or("").trim()
            ),
        );
    }
    if !other.1.trim().is_empty() {
        return check(
            name,
            Status::Fail,
            "the hook answered a prompt other than `draft` — run `ocgen doctor`",
        );
    }
    if [&wrote, &before, &after]
        .iter()
        .any(|o| !o.1.trim().is_empty())
    {
        return check(
            name,
            Status::Fail,
            "the hook answered a write or a Bash call — run `ocgen doctor`",
        );
    }
    let silent = intents
        .as_ref()
        .is_some_and(|o| !o.1.contains("additionalContext"));
    if word.1.contains("additionalContext") && silent {
        check(
            name,
            Status::Warn,
            format!(
                "`{}` reaches Claude as typed — the ocgen on PATH may predate it; update ocgen",
                intent_word.unwrap_or_default()
            ),
        )
    } else if word.1.contains("additionalContext") {
        check(
            name,
            Status::Pass,
            match &intent_word {
                Some(w) => format!(
                    "`draft` on its own opens the issue draft (or their list) in a browser editor, \
                     and `{w}` the intent files"
                ),
                None => {
                    "`draft` on its own opens the issue draft (or their list) in a browser editor"
                        .to_string()
                }
            },
        )
    } else if !ocgen_hook_ok() {
        check(
            name,
            Status::Warn,
            format!(
                "needs ocgen ({}) on PATH — without it `draft` reaches Claude as typed",
                crate::hooks::PROTOCOL
            ),
        )
    } else {
        check(
            name,
            Status::Fail,
            "the hook ran but said nothing about the draft — run `ocgen doctor`",
        )
    }
}

/// /recap's GitHub fetch: a write of its request gets a note for Claude, and any
/// other write passes untouched. Probed in the project itself: in probe mode the
/// hook reads nothing, calls no gh and writes nothing.
fn recap_github(p: &Probe) -> Check {
    let name = "/recap GitHub fetch";
    let Some((event, group, cmd)) = p.find("PostToolUse", "recap-github") else {
        return check(name, Status::Skip, "/recap not enabled");
    };
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Warn, not_run(&what));
    }
    let run = |rel: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "Write",
            "tool_input": { "file_path": crate::paths::for_shell(&p.root.join(rel)) }
        })
        .to_string();
        run_sh(p.sh, &cmd, &ev, &p.env, p.root, Duration::from_secs(20))
    };
    let (Some(other), Some(request)) = (run("src/ocgen-verify.rs"), run(crate::recap::REQUEST))
    else {
        return check(name, Status::Fail, "the hook did not finish");
    };
    if let Some((code, _, err)) = [&other, &request].into_iter().find(|o| o.0 != 0) {
        return check(
            name,
            Status::Fail,
            format!(
                "the hook exited {code} ({}) — it must never fail a write; run `ocgen doctor`",
                err.lines().next().unwrap_or("").trim()
            ),
        );
    }
    if !other.1.trim().is_empty() {
        return check(
            name,
            Status::Fail,
            "the hook answered a write other than /recap's request — run `ocgen doctor`",
        );
    }
    if request.1.contains("additionalContext") {
        check(
            name,
            Status::Pass,
            "/recap's GitHub request is answered with your gh login, outside the sandbox",
        )
    } else if !ocgen_hook_ok() {
        check(
            name,
            Status::Warn,
            format!(
                "needs ocgen ({}) on PATH — without it /recap reports GitHub as not checked",
                crate::hooks::PROTOCOL
            ),
        )
    } else {
        check(
            name,
            Status::Fail,
            "the hook ran but didn't answer /recap's request — run `ocgen doctor`",
        )
    }
}

/// The approval gate must block pushes — including phrasings with flags and via
/// a deploy script — and allow ordinary work. Tested with an empty HOME so a
/// current approval can't hide a broken gate. `off` is why the settings Claude
/// Code merges keep the gate from running at all (and the file that does it).
/// A gate that isn't ocgen's fails unverified.
fn approval_gate(p: &Probe, off: Option<(String, &str)>) -> Check {
    let name = "approval gate blocks git push";
    let Some((event, group, cmd)) = p.find("PreToolUse", "team-approval-gate") else {
        return check(name, Status::Skip, "approval gate not enabled");
    };
    if let Some((why, from)) = off {
        return check(
            name,
            Status::Fail,
            format!(
                "off in Claude Code — {why}, so the gate lets everything through; {}",
                undo(from)
            ),
        );
    }
    if let Some(what) = p.hand_edited(&event, &group, &cmd) {
        return check(name, Status::Fail, unverified("gate", &what));
    }
    let mut env = p.env.clone();
    let empty_home = scratch_path("home");
    let _ = std::fs::remove_dir_all(&empty_home);
    let _ = std::fs::create_dir_all(&empty_home);
    env.insert("HOME".into(), crate::paths::for_shell(&empty_home));
    let run = |c: &str| {
        let ev = serde_json::json!({
            "session_id": "ocgen-verify", "tool_name": "Bash", "tool_input": { "command": c }
        })
        .to_string();
        run_sh(p.sh, &cmd, &ev, &env, p.root, Duration::from_secs(20)).map(|x| x.0)
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
    if p.root.join(".claude/team/execution-approved").exists() {
        return check(
            name,
            Status::Warn,
            "an old .claude/team/execution-approved file is present — it no longer unlocks anything; delete it and use `ocgen approve`",
        );
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match home.and_then(|h| crate::approval::remaining(&h, p.root)) {
        Some(left) => check(
            name,
            Status::Warn,
            format!("gate works, but execution is APPROVED for another {} min (`ocgen approve --revoke` re-locks)", left.div_ceil(60)),
        ),
        None => check(name, Status::Pass, format!("blocks pushes (also with flags), deploys and self-approval; allows normal work ({via})")),
    }
}

/// The git half of the gate: the installed pre-push hook must refuse a push made
/// under Claude Code without an approval, and let the user's own through. It is
/// run the way git runs it, with HOME pointed at an empty folder so no real
/// approval counts and nothing real is touched (git still trusts the
/// repository as it does for verify): with CLAUDECODE set, then without. Only
/// ocgen's own hook is run; `sh` is `None` when no shell starts.
fn pre_push(sh: Option<&Path>, project: &Project, root: &Path) -> Check {
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
        return check(
            name,
            Status::Warn,
            format!(
                "this repo uses core.hooksPath — add `{}` to its pre-push hook",
                crate::render::pre_push_chain(root)
            ),
        );
    }
    let hook = PathBuf::from(common).join("hooks/pre-push");
    match std::fs::read_to_string(&hook) {
        Ok(h) if h.contains("ocgen:pre-push") => {}
        Ok(_) => {
            return check(
                name,
                Status::Warn,
                format!(
                    "another pre-push hook exists — add `{}` to it",
                    crate::render::pre_push_chain(root)
                ),
            )
        }
        Err(_) => return check(name, Status::Warn, "not installed — run `ocgen doctor`"),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let runnable = std::fs::metadata(&hook).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
        if !runnable {
            return check(
                name,
                Status::Fail,
                "installed, but not executable — git skips it, so pushes go through. Run `ocgen doctor`",
            );
        }
    }
    let Some(sh) = sh else {
        return check(
            name,
            Status::Skip,
            "installed, but not run: no shell to run it with",
        );
    };
    // git runs hooks from the top of the work tree.
    let top_dir = git(&["rev-parse", "--show-toplevel"]);
    let top = top_dir
        .as_deref()
        .map_or_else(|| root.to_path_buf(), PathBuf::from);
    let home = scratch_path("home");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::create_dir_all(&home);
    // The scratch HOME hides the real approvals, but also git's global config,
    // and with it a `safe.directory` that trusts this repository (common in
    // containers and CI): carry that trust, as verify's own git calls had it,
    // after any config the environment already passes this way.
    let trust = top_dir.unwrap_or_else(|| crate::paths::for_shell(root));
    let passed = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0);
    let isolate = |cmd: &mut Command| {
        cmd.current_dir(&top)
            .env("HOME", crate::paths::for_shell(&home))
            .env("GIT_CONFIG_COUNT", (passed + 1).to_string())
            .env(format!("GIT_CONFIG_KEY_{passed}"), "safe.directory")
            .env(format!("GIT_CONFIG_VALUE_{passed}"), &trust);
    };
    // A push the way git makes it, under Claude Code or not.
    let push = |claude: bool| {
        let mut cmd = Command::new(sh);
        cmd.arg(crate::paths::for_shell(&hook))
            .args(["origin", "https://ocgen-verify.invalid/repo.git"]);
        isolate(&mut cmd);
        if claude {
            cmd.env("CLAUDECODE", "1");
        } else {
            cmd.env_remove("CLAUDECODE");
        }
        // What git feeds a pre-push hook: one line per ref being pushed.
        let refs = format!(
            "refs/heads/ocgen-verify {} refs/heads/ocgen-verify {}\n",
            "1".repeat(40),
            "0".repeat(40)
        );
        run(cmd, &refs, Duration::from_secs(20))
    };
    let ran = push(true);
    // Only a hook that blocks just Claude Code's push is doing its job.
    let yours = match ran {
        Ok((code, ..)) if code != 0 => Some(push(false)),
        _ => None,
    };
    // The hook lets a push through when its git can't find the repository:
    // with this probe's HOME, not as git runs it. Asked the way the hook asks.
    let unreadable = match ran {
        Ok((0, ..)) => {
            let mut cmd = Command::new(sh);
            cmd.args([
                "-c",
                "git rev-parse --path-format=absolute --git-common-dir",
            ]);
            isolate(&mut cmd);
            match run(cmd, "", Duration::from_secs(20)) {
                Ok((0, out, _)) if !out.trim().is_empty() => None,
                Ok((code, _, err)) => Some(match err.lines().next().map(str::trim) {
                    Some(l) if !l.is_empty() => l.to_string(),
                    _ => format!("git exited {code}"),
                }),
                Err(e) => Some(format!("git {e}")),
            }
        }
        _ => None,
    };
    let _ = std::fs::remove_dir_all(&home);
    if let Some(why) = unreadable {
        return check(
            name,
            Status::Warn,
            format!("installed, but not checked: git couldn't read the repository with verify's scratch HOME ({why}), so the hook let the probe's push through without deciding"),
        );
    }
    if let Some(r) = yours.filter(|r| !matches!(r, Ok((0, ..)))) {
        let why = match r {
            Ok((code, _, err)) => {
                format!("exit {code}: {}", err.lines().next().unwrap_or("").trim())
            }
            Err(e) => e,
        };
        return check(
            name,
            Status::Fail,
            format!("installed, but it fails every push, yours too ({why}) — run `ocgen doctor`"),
        );
    }
    match ran {
        Ok((0, _, _)) => {
            let same = |p: &Path| std::fs::canonicalize(p).ok();
            let nested = same(&top) != same(root);
            check(
                name,
                Status::Fail,
                format!(
                    "installed, but it let a push made under Claude Code through without an approval{} — run `ocgen doctor`",
                    if nested {
                        " (the project isn't at the repository root)"
                    } else {
                        ""
                    }
                ),
            )
        }
        Ok((code, _, _)) => check(
            name,
            Status::Pass,
            format!("installed — it blocked a push made under Claude Code without an approval (exit {code})"),
        ),
        Err(e) => check(name, Status::Fail, format!("installed, but it didn't run: {e}")),
    }
}

/// The statusline ocgen generates must print something. One replaced in a more
/// specific settings file, or edited by hand, is not run.
fn statusline(p: &Probe, layers: &Layers) -> Check {
    let name = "statusline renders";
    let Some(want) = p.expected.get("statusLine") else {
        let why = if p.disk.get("statusLine").is_some() {
            "not one ocgen generates — not run"
        } else {
            "no statusLine configured"
        };
        return check(name, Status::Skip, why);
    };
    if p.disk.get("statusLine") != Some(want) {
        return check(name, Status::Warn, not_run("settings.json"));
    }
    if let Some((_, from)) = layers.get("/statusLine").filter(|(_, f)| *f != SETTINGS) {
        return check(
            name,
            Status::Skip,
            format!("{from} replaces it — verify runs only the statusline ocgen generates"),
        );
    }
    let cmd = want.get("command").and_then(Value::as_str).unwrap_or("");
    if let Some(rel) = p.edited_script(cmd) {
        return check(name, Status::Warn, not_run(&rel));
    }
    let payload = serde_json::json!({
        "model": { "display_name": "ocgen verify" },
        "workspace": {
            "current_dir": crate::paths::for_shell(p.root),
            "project_dir": crate::paths::for_shell(p.root)
        },
        "context_window": { "remaining_percentage": 50 },
    })
    .to_string();
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_PROJECT_DIR".to_string(),
        crate::paths::for_shell(p.root),
    );
    match run_sh(p.sh, cmd, &payload, &env, p.root, Duration::from_secs(10)) {
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

/// Hook scripts and gate-protocol templates in the template override dir (left
/// by an older `ocgen templates init`): ocgen ignores them, so a tweak there
/// silently does nothing. Shown only when there are some.
fn ignored_overrides() -> Option<Check> {
    let ignored = crate::templates::ignored_overrides();
    if ignored.is_empty() {
        return None;
    }
    let dir = crate::templates::override_dir()
        .map(|d| crate::paths::for_shell(&d))
        .unwrap_or_default();
    Some(check(
        "template overrides",
        Status::Warn,
        format!(
            "{} in {dir} ignored — hook scripts and the gate-protocol templates always come from ocgen; delete {}",
            ignored.join(", "),
            if ignored.len() == 1 { "it" } else { "them" }
        ),
    ))
}

/// The CODEOWNERS ocgen is linked to (`ocgen edit intent --codeowners`): still
/// there and text, and no rule of the user's after ocgen's block taking
/// precedence over it. The block's lines are
/// checked with the other files ("files up to date").
fn codeowners(project: &Project, root: &Path) -> Check {
    let name = "CODEOWNERS";
    let s = &project.claude.intent;
    if s.codeowners.is_empty() {
        return check(
            name,
            Status::Skip,
            "not linked (`ocgen edit intent --codeowners <path>` keeps the approvers there)",
        );
    }
    let notes = project.codeowners_report(root);
    if !notes.is_empty() {
        return check(name, Status::Warn, notes.join("; "));
    }
    let what = if s.codeowners_rule().is_some() {
        format!(
            "the approvers review {} — GitHub requires it only with branch protection's \"Require review from Code Owners\", and assigns only owners with explicit write access to the repository (a team must also be visible)",
            match s.codeowners_scope {
                crate::claude::CodeownersScope::All => "every change".to_string(),
                _ => format!("{}/", s.dir.trim_matches('/')),
            }
        )
    } else {
        "no block (no approvers, or scope off)".to_string()
    };
    check(name, Status::Pass, format!("{}: {what}", s.codeowners))
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

/// The first `name` on PATH that runs as a program — on Windows with one of the
/// `PATHEXT` extensions (`claude.exe`, `claude.cmd`), the way a shell finds it.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let exts = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
    } else {
        String::new()
    };
    find_in(&std::env::var_os("PATH")?, name, &exts)
}

/// [`find_on_path`] with PATH and PATHEXT given; empty `exts` means the bare
/// name. Only extensions `Command` can start (batch files via cmd) count.
fn find_in(path: &OsStr, name: &str, exts: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if exts.is_empty() {
        vec![String::new()]
    } else {
        exts.split(';')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.as_str()))
            .collect()
    };
    std::env::split_paths(path)
        .flat_map(|d| {
            exts.iter()
                .map(|e| d.join(format!("{name}{e}")))
                .collect::<Vec<_>>()
        })
        .find(|p| p.is_file())
}

fn claude_validate(project: &Project, root: &Path) -> Check {
    let name = "Claude Code validation";
    let Some(claude) = find_on_path("claude") else {
        return check(name, Status::Skip, "claude CLI not found");
    };
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
        let rel = t.strip_prefix(root).unwrap_or(t).display().to_string();
        let mut cmd = Command::new(&claude);
        // The folder as an argument, never inside a shell string: a plugin
        // folder's name is the repository's to choose.
        cmd.args(["plugin", "validate"])
            .arg(crate::paths::for_shell(t))
            .current_dir(root);
        match run(cmd, "", Duration::from_secs(90)) {
            Ok((0, out, _)) if out.contains("Validation passed") => {}
            Ok((_, out, err)) => failed.push(format!(
                "{rel}: {}",
                format!("{out}{err}")
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
            )),
            Err(e) => failed.push(format!("{rel}: {e}")),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe of `settings` as both what ocgen generates and what is on disk,
    /// with `scripts` written under `root` and counted as ocgen's own.
    fn probe<'a>(
        sh: &'a Path,
        root: &'a Path,
        settings: &'a Value,
        rendered: &'a mut HashMap<String, String>,
        scripts: &[(&str, &str)],
        env: &[(&str, &str)],
    ) -> Probe<'a> {
        for (rel, body) in scripts {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            rendered.insert(rel.to_string(), body.to_string());
        }
        let mut vars: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        vars.insert("CLAUDE_PROJECT_DIR".into(), crate::paths::for_shell(root));
        Probe {
            sh,
            root,
            rendered,
            expected: settings,
            disk: settings,
            env: vars,
        }
    }

    fn pre_tool_use(matcher: &str, script: &str) -> Value {
        serde_json::json!({ "hooks": { "PreToolUse": [ { "matcher": matcher, "hooks": [ {
            "type": "command", "shell": "bash",
            "command": format!("sh \"${{CLAUDE_PROJECT_DIR}}/.claude/hooks/{script}\"")
        } ] } ] } })
    }

    #[test]
    fn output_beyond_a_pipe_buffer_does_not_stall_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let sh = shell(&Value::Null);
        let start = Instant::now();
        let (code, out, err) = run_sh(
            &sh,
            "head -c 300000 /dev/zero; head -c 300000 /dev/zero >&2",
            "",
            &HashMap::new(),
            dir.path(),
            Duration::from_secs(60),
        )
        .expect("finishes");
        assert_eq!((code, out.len(), err.len()), (0, 300000, 300000));
        assert!(start.elapsed() < Duration::from_secs(30));
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_stops_everything_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!(
            "sleep 60 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        let start = Instant::now();
        assert_eq!(
            run(cmd, "", Duration::from_millis(500)),
            Err("timed out".to_string())
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_string();
        let alive = || {
            Command::new("kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let start = Instant::now();
        while alive() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the background sleep {pid} outlived the timeout");
    }

    /// A background process left holding the output pipes doesn't keep verify
    /// waiting: it is stopped, and the output so far is kept.
    #[cfg(unix)]
    #[test]
    fn a_left_behind_process_does_not_hold_up_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let (code, out, _) = run_sh(
            Path::new("sh"),
            "sleep 60 & echo hi",
            "",
            &HashMap::new(),
            dir.path(),
            Duration::from_secs(30),
        )
        .expect("finishes");
        assert_eq!((code, out.trim()), (0, "hi"));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn the_claude_cli_is_found_by_its_windows_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        std::fs::write(dir.path().join("claude.cmd"), "@echo off\n").unwrap();
        assert_eq!(
            find_in(&path, "claude", ".COM;.EXE;.BAT;.CMD;.VBS"),
            Some(dir.path().join("claude.cmd"))
        );
        // A script `Command` can't start doesn't count.
        assert_eq!(find_in(&path, "claude", ".VBS;.JS"), None);
        // Elsewhere, the bare name.
        assert_eq!(find_in(&path, "claude", ""), None);
        std::fs::write(dir.path().join("claude"), "#!/bin/sh\n").unwrap();
        assert_eq!(
            find_in(&path, "claude", ""),
            Some(dir.path().join("claude"))
        );
    }

    #[test]
    fn cd_targets_are_quoted_when_they_must_be() {
        assert_eq!(cd_target("/home/x/proj").as_deref(), Some("/home/x/proj"));
        assert_eq!(
            cd_target("C:/Users/John Smith/proj").as_deref(),
            Some("'C:/Users/John Smith/proj'")
        );
        assert_eq!(
            cd_target("/w/proj (old)").as_deref(),
            Some("'/w/proj (old)'")
        );
        assert_eq!(cd_target("/w/it's").as_deref(), Some("\"/w/it's\""));
        assert_eq!(cd_target("/w/it's $x"), None);
    }

    #[test]
    fn only_gate_settings_reach_a_probe() {
        for key in [
            "TEAM_APPROVAL_GATE",
            "OCGEN_WEBFETCH_DOMAINS",
            "LOOP_GUARD_MAX_BLOCKS",
            "SUBAGENT_CONFIDENCE_THRESHOLD",
            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
        ] {
            assert!(probe_env_key(key), "{key}");
        }
        for key in [
            "PATH",
            "BASH_ENV",
            "ENV",
            "SHELLOPTS",
            "CLAUDE_CODE_GIT_BASH_PATH",
            "OCGEN_NOTES_BROWSER",
            "OCGEN_FORMAT_CMD",
            "OCGEN_CHECK_CMD",
            "OCGEN_RECAP_GH",
        ] {
            assert!(!probe_env_key(key), "{key}");
        }
    }

    /// The env a probe gets is merged like Claude Code merges it: the more
    /// specific file wins, and keys outside the gate settings never pass.
    #[test]
    fn hook_env_merges_settings_files_most_specific_last() {
        let layers = Layers(vec![
            (
                Scope::User,
                "user".into(),
                serde_json::json!({ "env": { "TEAM_PLAN_GATE": "1", "OCGEN_NO_JQ": "1" } }),
            ),
            (
                Scope::Project,
                SETTINGS.into(),
                serde_json::json!({ "env": { "TEAM_PLAN_GATE": "1", "PATH": "/evil" } }),
            ),
            (
                Scope::Local,
                LOCAL.into(),
                serde_json::json!({ "env": { "TEAM_PLAN_GATE": "0" } }),
            ),
        ]);
        let env = layers.hook_env(Path::new("/p"));
        assert_eq!(env["TEAM_PLAN_GATE"], "0");
        assert_eq!(env["OCGEN_NO_JQ"], "1");
        assert!(!env.contains_key("PATH"));
        assert_eq!(env["LOOP_GUARD_MAX_BLOCKS"], "0");
        assert_eq!(env["OCGEN_CHECK_CMD"], "");
        assert_eq!(
            layers.get("/env/TEAM_PLAN_GATE").map(|(_, from)| from),
            Some(LOCAL)
        );
    }

    /// Values are judged the way the hooks read them: a number either twin
    /// can't parse is 0 for it (a bar off, a loop guard that never releases),
    /// roles and sites match in any letter case, and a key only a gate ocgen
    /// doesn't generate reads loosens nothing.
    #[test]
    fn weakening_is_judged_as_the_hooks_read_the_values() {
        let gates = serde_json::json!({
            "hooks": { "SubagentStop": [], "TeammateIdle": [] },
            "env": { "TEAM_RISK_ROUNDS": "1" },
        });
        let w = |key, want, got| weakens(key, want, got, &gates);
        // A bar a twin can't read is off: sh reads plain digits only, the Rust
        // hook a u32 with an optional `+`.
        for got in [" 99", "+99", "5000000000", "99999999999999999999", "x", ""] {
            assert!(
                w("SUBAGENT_CONFIDENCE_THRESHOLD", Some("96"), Some(got)),
                "{got}"
            );
        }
        assert!(w("TEAM_CONFIDENCE_THRESHOLD", Some("96"), None));
        for got in ["99", "096", "96"] {
            assert!(
                !w("SUBAGENT_CONFIDENCE_THRESHOLD", Some("96"), Some(got)),
                "{got}"
            );
        }
        // No bar from ocgen: any is stricter.
        assert!(!w("TEAM_CONFIDENCE_THRESHOLD", None, Some("10")));
        // A switch ocgen doesn't turn on can't be turned off.
        assert!(!w("TEAM_PLAN_GATE", None, Some("0")));
        assert!(w("TEAM_PLAN_GATE", Some("1"), None));
        // Loop budget: lower releases sooner (by either reading); 0 or
        // unreadable never releases; with ocgen's 0, any budget releases.
        for got in ["2", "+2", "1"] {
            assert!(w("LOOP_GUARD_MAX_BLOCKS", Some("3"), Some(got)), "{got}");
        }
        for got in ["0", "3", "+3", "9", "x", "", " 2", "5000000000"] {
            assert!(!w("LOOP_GUARD_MAX_BLOCKS", Some("3"), Some(got)), "{got}");
        }
        assert!(w("LOOP_GUARD_MAX_BLOCKS", Some("0"), Some("5")));
        assert!(!w("LOOP_GUARD_MAX_BLOCKS", None, Some("1")));
        // Read-only lists: a role the gate holds, in any spacing; a role ocgen
        // lists, in any letter case, adds nothing.
        assert!(w("TEAM_READONLY_ROLES", Some("a b"), Some("b\ta  c")));
        assert!(!w("TEAM_READONLY_ROLES", Some("a b"), Some(" b ")));
        assert!(!w(
            "SUBAGENT_READONLY_ROLES",
            Some("Explore Plan"),
            Some("explore PLAN")
        ));
        assert!(w("SUBAGENT_READONLY_ROLES", None, Some("x")));
        // The team task gate leaves in-process teammates to the worker gate.
        assert!(w("TEAM_TASK_GATE", None, Some("1")));
        assert!(!w("TEAM_TASK_GATE", None, Some("0")));
        // A site the guard doesn't trust, in any letter case.
        assert!(w(
            "OCGEN_WEBFETCH_DOMAINS",
            Some("docs.rs"),
            Some("docs.rs evil.example")
        ));
        assert!(!w(
            "OCGEN_WEBFETCH_DOMAINS",
            Some("docs.rs go.dev"),
            Some("GO.DEV")
        ));
        assert!(!w("OCGEN_WEBFETCH_DOMAINS", None, Some("evil.example")));
        // Only verify sets the probe switch.
        assert!(w("OCGEN_HOOK_PROBE", None, Some("")));
        // Nothing sets the stand-in for gh: it would run with the user's login.
        assert!(w("OCGEN_RECAP_GH", None, Some("sh ./x")));
        assert!(!w("OCGEN_RECAP_GH", None, Some(" ")));
        assert!(!w("OCGEN_NO_JQ", None, Some("1")));

        // No worker gate, no risk gate: their lists and switches loosen nothing.
        let none = serde_json::json!({ "hooks": { "TeammateIdle": [] }, "env": {} });
        let w = |key, want, got| weakens(key, want, got, &none);
        assert!(!w("SUBAGENT_READONLY_ROLES", None, Some("x")));
        assert!(!w("TEAM_TASK_GATE", None, Some("1")));
        assert!(!w("TEAM_READONLY_ROLES", None, Some("x")));
    }

    /// The probes still catch a guard that lets traffic through when the guard
    /// is ocgen's own (here: a stand-in counted as generated).
    #[test]
    fn the_webfetch_probe_catches_a_leaky_guard() {
        let sh = shell(&Value::Null);
        let settings = pre_tool_use("WebFetch", "https-only-fetch.sh");
        let env = [("OCGEN_WEBFETCH_DOMAINS", "docs.rs")];

        let dir = tempfile::tempdir().unwrap();
        let mut rendered = HashMap::new();
        let script = [(".claude/hooks/https-only-fetch.sh", "#!/bin/sh\nexit 0\n")];
        let p = probe(&sh, dir.path(), &settings, &mut rendered, &script, &env);
        assert_eq!(https_only_fetch(&p).status, Status::Fail);

        // The old guard dropped backslashes and took the host after the last
        // `@`, so `https://evil\@docs.rs/` passed (it goes to evil).
        let old = "#!/bin/sh\nurl=$(cat | sed -n 's/.*\"url\":\"\\([^\"]*\\)\".*/\\1/p' | tr -d '\\\\')\n\
                   case \"$url\" in https://*) ;; *) exit 2 ;; esac\n\
                   h=${url#https://}; h=${h%%/*}; h=${h##*@}; h=${h%%:*}\n\
                   for d in $OCGEN_WEBFETCH_DOMAINS; do [ \"$h\" = \"$d\" ] && exit 0; done\nexit 2\n";
        let dir = tempfile::tempdir().unwrap();
        let mut rendered = HashMap::new();
        let script = [(".claude/hooks/https-only-fetch.sh", old)];
        let p = probe(&sh, dir.path(), &settings, &mut rendered, &script, &env);
        let c = https_only_fetch(&p);
        assert_eq!(c.status, Status::Fail, "{c:#?}");
        assert!(c.detail.contains("ocgen-verify.invalid\\@"), "{c:#?}");
    }

    /// A gate of ocgen's own that lets pushes through fails (a hand-edited one
    /// isn't run at all: tests/verify_hardening.rs).
    #[test]
    fn the_gate_probe_catches_a_gate_that_lets_pushes_through() {
        let sh = shell(&Value::Null);
        let settings = pre_tool_use(
            "Bash|Write|Edit|MultiEdit|NotebookEdit",
            "team-approval-gate.sh",
        );
        let dir = tempfile::tempdir().unwrap();
        let mut rendered = HashMap::new();
        let open = [(
            ".claude/hooks/team-approval-gate.sh",
            "#!/bin/sh\ncat >/dev/null\nexit 0\n",
        )];
        let p = probe(&sh, dir.path(), &settings, &mut rendered, &open, &[]);
        let c = approval_gate(&p, None);
        assert_eq!(c.status, Status::Fail, "{c:#?}");
        assert!(
            c.detail.contains("not blocked: [git push origin main"),
            "{c:#?}"
        );
    }

    /// A notes hook of ocgen's own that writes no page is never a pass.
    #[test]
    fn the_notes_probe_reports_a_hook_that_renders_nothing() {
        let sh = shell(&Value::Null);
        let settings = serde_json::json!({ "hooks": { "PostToolUse": [ {
            "matcher": "Write|Edit|MultiEdit", "hooks": [ { "type": "command", "shell": "bash",
            "command": "sh \"${CLAUDE_PROJECT_DIR}/.claude/hooks/inquire-notes.sh\"" } ] } ] } });
        let dir = tempfile::tempdir().unwrap();
        let mut rendered = HashMap::new();
        let silent = [(
            ".claude/hooks/inquire-notes.sh",
            "#!/bin/sh\ncat >/dev/null\nexit 0\n",
        )];
        let p = probe(&sh, dir.path(), &settings, &mut rendered, &silent, &[]);
        let c = notes_view(&p);
        assert_ne!(c.status, Status::Pass, "{c:#?}");
        assert_ne!(c.status, Status::Skip, "{c:#?}");
    }

    #[test]
    fn the_noop_cd_probe_catches_a_hook_that_drops_every_cd() {
        let sh = shell(&Value::Null);
        let settings = pre_tool_use("Bash", "drop-noop-cd.sh");
        let dir = tempfile::tempdir().unwrap();
        let mut rendered = HashMap::new();
        let strip_all = "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"updatedInput\":{\"command\":\"echo ok\",\"description\":\"verify\"}}}'\n";
        let script = [(".claude/hooks/drop-noop-cd.sh", strip_all)];
        let p = probe(&sh, dir.path(), &settings, &mut rendered, &script, &[]);
        let c = noop_cd(&p);
        assert_eq!(c.status, Status::Fail, "{c:#?}");
        assert!(c.detail.contains("another folder"), "{c:#?}");
    }
}
