//! The generated project's hooks, implemented in Rust (`ocgen hook <name>`).
//!
//! These mirror the POSIX `sh` scripts in `templates/claude/hooks/` exactly — same
//! inputs, messages, exit codes and loop-guard state files — so a project works the
//! same whether a machine runs the binary or falls back to the scripts, and the two
//! can even share one `.claude/loop-guard/` state. Rust adds real JSON parsing and
//! runs anywhere ocgen runs (including native Windows). Parity is enforced by tests
//! that run both implementations against the same fixtures.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::Value;

/// Bump whenever hook behavior or the command-line contract changes. A generated
/// hook command uses the binary only when `ocgen hook --check` prints exactly the
/// protocol the project was generated with; any other ocgen (older or newer)
/// falls back to the project's own scripts, which always match the project.
pub const PROTOCOL: &str = "ocgen-hooks 3";

/// Every hook `ocgen hook <name>` accepts (matching the script names minus `.sh`).
pub const NAMES: [&str; 8] = [
    "subagent-confidence-gate",
    "team-task-completed",
    "team-task-created",
    "team-teammate-idle",
    "team-approval-gate",
    "notify",
    "format",
    "config-audit",
];

/// What a hook tells Claude Code: exit code plus stdout/stderr.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    fn allow() -> Self {
        Self::default()
    }
    fn block(stderr: String) -> Self {
        Self {
            code: 2,
            stdout: String::new(),
            stderr,
        }
    }
}

/// Run hook `name` on `payload` (the event JSON) with `env` (process environment).
pub fn run(name: &str, payload: &str, env: &HashMap<String, String>) -> Outcome {
    let h = Hook::new(payload, env);
    match name {
        "subagent-confidence-gate" => h.subagent_confidence(),
        "team-task-completed" => h.task_completed(),
        "team-task-created" => h.task_created(),
        "team-teammate-idle" => h.teammate_idle(),
        "team-approval-gate" => h.approval_gate(),
        "notify" => h.notify(),
        "format" => h.format(),
        "config-audit" => h.config_audit(),
        other => Outcome {
            code: 1,
            stdout: String::new(),
            stderr: format!("unknown hook '{other}' (known: {})\n", NAMES.join(", ")),
        },
    }
}

struct Hook<'a> {
    raw: &'a str,
    json: Value,
    env: &'a HashMap<String, String>,
}

impl<'a> Hook<'a> {
    fn new(raw: &'a str, env: &'a HashMap<String, String>) -> Self {
        let json = serde_json::from_str(raw).unwrap_or(Value::Null);
        Self { raw, json, env }
    }

    fn env(&self, key: &str) -> &str {
        self.env.get(key).map(String::as_str).unwrap_or("")
    }

    fn project(&self) -> PathBuf {
        match self.env("CLAUDE_PROJECT_DIR") {
            "" => PathBuf::from("."),
            p => PathBuf::from(p),
        }
    }

    /// A top-level string field of the event.
    fn field(&self, key: &str) -> String {
        self.json
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    /// Stated confidences (`Confidence: 97%`) anywhere in the event, in order.
    fn confidences(&self) -> Vec<u32> {
        let re = Regex::new(r"(?i)confidence[^0-9]{0,4}([0-9]{1,3})").unwrap();
        re.captures_iter(self.raw)
            .filter_map(|c| c[1].parse().ok())
            .collect()
    }

    fn guard(&self, gate: &str, subject: &str) -> Option<Guard> {
        Guard::new(self, gate, subject)
    }

    // ---------------------------------------------------------------- gates --

    fn subagent_confidence(&self) -> Outcome {
        let thr: u32 = self
            .env("SUBAGENT_CONFIDENCE_THRESHOLD")
            .parse()
            .unwrap_or(0);
        let check = self.env("OCGEN_CHECK_CMD");
        if thr == 0 && check.is_empty() {
            return Outcome::allow();
        }
        let cwd = match self.field("cwd") {
            c if !c.is_empty() => PathBuf::from(c),
            _ => self.project(),
        };
        // Only a worker that changed files is gated; read-only workers pass.
        let dirty = Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args(["status", "--porcelain"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);
        if !dirty {
            return Outcome::allow();
        }
        let score = self.confidences().last().copied();
        let worker = [self.field("agent_id"), self.field("agent_type")]
            .into_iter()
            .find(|w| !w.is_empty())
            .unwrap_or_else(|| "subagent".into());
        let guard = self.guard("subagent-confidence", &worker);
        let unresolved = |subject: &str, why: &str| {
            format!(
                "UNRESOLVED: worker {subject} stopped by the loop guard ({why}). Review its changes; see .claude/loop-guard/escalations.md"
            )
        };
        // 1. The objective check, in the worker's own directory.
        if let Some((rc, tail)) = self.failed_check(check, &cwd) {
            let msg = format!(
                "Check failed: `{check}` (exit {rc}). Fix it before finishing — a stated confidence\n\
                 doesn't override a failing check. Last output:\n{tail}"
            );
            return quality_block(
                guard,
                msg,
                format!("check `{check}` failing (exit {rc})"),
                None,
                unresolved,
            );
        }
        // 2. The stated confidence.
        if thr > 0 {
            match score {
                None => {
                    let msg = format!(
                        "You changed files but did not state your confidence. Finish only when you are\n\
                         >= {thr}% confident, and state 'Confidence: NN%' in your final message.\n"
                    );
                    return quality_block(
                        guard,
                        msg,
                        format!("confidence not stated (bar {thr}%)"),
                        None,
                        unresolved,
                    );
                }
                Some(s) if s < thr => {
                    let msg = format!(
                        "Confidence {s}% is below the required {thr}%. Keep working (or re-scope)\n\
                         until you are >= {thr}% confident, then state 'Confidence: NN%' and finish.\n"
                    );
                    return quality_block(
                        guard,
                        msg,
                        format!("confidence {s}% < {thr}%"),
                        Some(s),
                        unresolved,
                    );
                }
                Some(_) => {}
            }
        }
        if let Some(g) = guard {
            g.reset();
        }
        Outcome::allow()
    }

    fn task_completed(&self) -> Outcome {
        let thr: u32 = self.env("TEAM_CONFIDENCE_THRESHOLD").parse().unwrap_or(0);
        let check = self.env("OCGEN_CHECK_CMD");
        if thr == 0 && check.is_empty() {
            return Outcome::allow();
        }
        let tid = Regex::new(r#"(?i)"(task_?id|id)"[[:space:]]*:[[:space:]]*"([^"]+)""#)
            .unwrap()
            .captures(self.raw)
            .map(|c| c[2].to_string())
            .unwrap_or_default();
        let mut score = self.confidences().first().copied();
        if score.is_none() && !tid.is_empty() {
            let marker = self
                .project()
                .join(format!(".claude/team/confidence/{tid}.txt"));
            score = fs::read_to_string(marker)
                .ok()
                .map(|s| s.chars().filter(char::is_ascii_digit).collect::<String>())
                .and_then(|d| d.parse().ok());
        }
        let subject = if tid.is_empty() { "task" } else { tid.as_str() };
        let guard = self.guard("task-confidence", subject);
        let unresolved = |subject: &str, why: &str| {
            format!(
                "UNRESOLVED: task {subject} completed by the loop guard ({why}). Needs review; see .claude/loop-guard/escalations.md"
            )
        };
        // 1. The objective check, in the project directory.
        if let Some((rc, tail)) = self.failed_check(check, &self.project()) {
            let msg = format!(
                "Check failed: `{check}` (exit {rc}). The task isn't complete until it passes — a\n\
                 stated confidence doesn't override a failing check. Last output:\n{tail}"
            );
            return quality_block(
                guard,
                msg,
                format!("check `{check}` failing (exit {rc})"),
                None,
                unresolved,
            );
        }
        // 2. The stated confidence.
        if thr > 0 {
            match score {
                None => {
                    let msg = format!(
                        "No confidence found for this completion. State 'Confidence: NN%' in the completion\n\
                         (or write .claude/team/confidence/<task-id>.txt); it must be >= {thr}%.\n"
                    );
                    return quality_block(
                        guard,
                        msg,
                        format!("confidence not stated (bar {thr}%)"),
                        None,
                        unresolved,
                    );
                }
                Some(s) if s < thr => {
                    let msg = format!(
                        "Confidence {s}% is below the required {thr}%.\n\
                         Keep working (or re-scope) until you are >= {thr}% confident, then re-complete.\n"
                    );
                    return quality_block(
                        guard,
                        msg,
                        format!("confidence {s}% < {thr}%"),
                        Some(s),
                        unresolved,
                    );
                }
                Some(_) => {}
            }
        }
        if let Some(g) = guard {
            g.reset();
        }
        Outcome::allow()
    }

    /// Run the project's check command in `dir`. `None` when there is no check or
    /// it passed; otherwise its exit code and the last lines of its output. A
    /// check that outlives `OCGEN_CHECK_TIMEOUT` seconds (default 300) is killed
    /// and counts as failed.
    fn failed_check(&self, check: &str, dir: &Path) -> Option<(String, String)> {
        if check.is_empty() {
            return None;
        }
        let limit = self.env("OCGEN_CHECK_TIMEOUT").parse().unwrap_or(300u64);
        let log = std::env::temp_dir().join(format!(
            "ocgen-check-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let rc = (|| {
            let out = fs::File::create(&log).ok()?;
            let err = out.try_clone().ok()?;
            let mut child = shell(check)
                .current_dir(dir)
                .stdin(std::process::Stdio::null())
                .stdout(out)
                .stderr(err)
                .spawn()
                .ok()?;
            let start = std::time::Instant::now();
            loop {
                match child.try_wait().ok()? {
                    Some(status) => {
                        return Some(
                            status
                                .code()
                                .map_or("signal".to_string(), |c| c.to_string()),
                        )
                    }
                    None if start.elapsed().as_secs() >= limit => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Some(format!("timeout after {limit}s"));
                    }
                    None => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        })()
        .unwrap_or_else(|| "could not run".to_string());
        let output = fs::read_to_string(&log).unwrap_or_default();
        let _ = fs::remove_file(&log);
        if rc == "0" {
            return None;
        }
        let lines: Vec<&str> = output.lines().collect();
        let tail = lines[lines.len().saturating_sub(15)..].join("\n");
        Some((rc, format!("{tail}\n")))
    }

    fn task_created(&self) -> Outcome {
        if self.env("TEAM_PLAN_GATE") != "1" {
            return Outcome::allow();
        }
        let who = [self.field("agent_type"), self.field("agent_id")]
            .into_iter()
            .find(|w| !w.is_empty())
            .unwrap_or_else(|| "lead".into());
        let guard = self.guard("plan-gate", &who);
        let plan = self.project().join(".claude/team/plan.md");
        let approved = fs::read_to_string(&plan)
            .map(|p| p.lines().any(|l| l.starts_with("Status: APPROVED")))
            .unwrap_or(false);
        if approved {
            if let Some(g) = guard {
                g.reset();
            }
            return Outcome::allow();
        }
        // Never released: an unapproved plan must not start work.
        if let Some(g) = &guard {
            if g.bump() >= g.max {
                g.escalate("kept creating tasks before the plan was approved");
                return Outcome::block(
                    "STOP retrying: the plan is still not approved. Tell the lead that\n\
                     .claude/team/plan.md needs 'Status: APPROVED' from the user, then go idle.\n"
                        .into(),
                );
            }
        }
        Outcome::block(
            "Plan not approved yet. Run /team-plan and get the plan APPROVED in\n\
             .claude/team/plan.md before creating execution tasks.\n"
                .into(),
        )
    }

    fn teammate_idle(&self) -> Outcome {
        if self.env("TEAM_RISK_ROUNDS") != "1" {
            return Outcome::allow();
        }
        let role = self.field("agent_type");
        if role.is_empty() {
            return Outcome::allow(); // can't identify the teammate → don't livelock it
        }
        let readonly = self
            .env("TEAM_READONLY_ROLES")
            .split_whitespace()
            .any(|r| r.eq_ignore_ascii_case(&role));
        if readonly {
            return Outcome::allow();
        }
        let Ok(plan) = fs::read_to_string(self.project().join(".claude/team/plan.md")) else {
            return Outcome::allow();
        };
        let pending = Regex::new(r"(?i)mitigation:[[:space:]]*pending").unwrap();
        let owner = Regex::new(&format!(
            r"(?i)owner:[[:space:]]*{}([^a-z0-9_-]|$)",
            regex::escape(&role)
        ))
        .unwrap();
        let owned: Vec<&str> = plan
            .lines()
            .filter(|l| pending.is_match(l) && owner.is_match(l))
            .collect();
        let guard = self.guard("risk-idle", &role);
        if owned.is_empty() {
            if let Some(g) = guard {
                g.reset();
            }
            return Outcome::allow();
        }
        if let Some(g) = &guard {
            let n = g.bump();
            if n >= g.max {
                // Let it idle rather than pressure it into faking a closure.
                let first: Vec<&str> = owned.iter().take(3).copied().collect();
                g.escalate(&format!("held {n}x; still pending: {}", first.join(" ")));
                return release(&format!(
                    "UNRESOLVED: {role} could not close its pending risks after {n} holds; released by the loop guard. See .claude/loop-guard/escalations.md"
                ));
            }
        }
        Outcome::block(
            "You still own risks marked 'Mitigation: pending' in .claude/team/plan.md.\n\
             Close each one you own (mark it done or accepted) before going idle.\n\
             If you truly can't mitigate one, say why — never mark it done to get past this.\n"
                .into(),
        )
    }

    fn approval_gate(&self) -> Outcome {
        if self.env("TEAM_APPROVAL_GATE") != "1" {
            return Outcome::allow();
        }
        let input = self.json.get("tool_input");
        let text = |key: &str| {
            input
                .and_then(|i| i.get(key))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        // Only a human may approve: no tool call may run `ocgen approve` or touch
        // the approval store.
        let self_approval = || {
            self.deny(
                "Blocked: only a HUMAN may approve execution, from a terminal outside the agent\n\
                 (ocgen approve). No tool call may approve or touch the approval store.\n"
                    .into(),
                "tried to approve itself",
            )
        };
        if self.field("tool_name") == "Bash" {
            let cmd = crate::risk::normalize(&text("command"));
            // (`ocgen/?approvals`: a Windows `ocgen\approvals` loses its backslash
            // in normalization.)
            if Regex::new(r"ocgen[[:space:]]+approve|ocgen/?approvals")
                .unwrap()
                .is_match(&cmd)
            {
                return self_approval();
            }
            if crate::risk::high_impact(&cmd).is_some() && !self.approved() {
                let marker = crate::approval::marker_path(&self.home(), &self.project());
                return self.deny(
                    format!(
                        "BLOCKED by the execution-approval gate: this is a high-impact external\n\
                         action (ssh / cloud mutation / git push|merge / deploy / publish / etc.).\n\
                         A human must review, then approve from their own terminal (it expires by itself):\n    \
                         ocgen approve\n\
                         Without ocgen: echo $(( $(date +%s) + 1800 )) > \"{}\"\n\
                         No agent may approve.\n",
                        marker.display()
                    ),
                    "gated command without approval",
                );
            }
        } else {
            let path = match text("file_path") {
                p if !p.is_empty() => p,
                _ => text("notebook_path"),
            };
            // Windows paths use backslashes.
            if path.replace('\\', "/").contains("ocgen/approvals") {
                return self_approval();
            }
        }
        Outcome::allow()
    }

    fn home(&self) -> PathBuf {
        match self.env("HOME") {
            "" => PathBuf::from(self.env("USERPROFILE")),
            h => PathBuf::from(h),
        }
    }

    /// Whether a human approval for this project is currently in force.
    fn approved(&self) -> bool {
        crate::approval::remaining(&self.home(), &self.project()).is_some()
    }

    /// Block a gated call; once the loop-guard budget is spent, deny AND halt the
    /// agent (`continue: false`) so it stops retrying. Never allows the call.
    fn deny(&self, stderr: String, what: &str) -> Outcome {
        let who = match self.field("agent_type") {
            w if !w.is_empty() => w,
            _ => "main".into(),
        };
        if let Some(g) = self.guard("approval-gate", &who) {
            if g.bump() >= g.max {
                g.escalate(&format!(
                    "halted after repeated high-impact attempts: {what}"
                ));
                let msg = format!(
                    "Halted by the loop guard: repeated attempts at a gated high-impact action ({what}). A human must review and approve from their own terminal; see .claude/loop-guard/escalations.md"
                );
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": msg,
                    },
                    "continue": false,
                    "stopReason": msg,
                });
                return Outcome {
                    code: 0,
                    stdout: format!("{out}\n"),
                    stderr,
                };
            }
        }
        Outcome::block(stderr)
    }

    // --------------------------------------------------------- quality of life --

    fn notify(&self) -> Outcome {
        let msg = match self.field("message") {
            m if !m.is_empty() => m,
            _ => {
                let ev = self.field("hook_event_name");
                let ev = if ev.is_empty() {
                    "needs your attention".to_string()
                } else {
                    ev
                };
                format!("Claude Code: {ev}")
            }
        };
        let msg: String = msg.replace(['"', '\\'], "").chars().take(200).collect();
        let shown = if cfg!(target_os = "macos") {
            Command::new("osascript")
                .arg("-e")
                .arg(format!(
                    "display notification \"{msg}\" with title \"Claude Code\""
                ))
                .output()
                .is_ok()
        } else if cfg!(target_os = "linux") {
            Command::new("notify-send")
                .args(["Claude Code", &msg])
                .output()
                .is_ok()
        } else {
            false
        };
        if !shown {
            eprint!("\x07");
        }
        Outcome::allow()
    }

    fn format(&self) -> Outcome {
        let cmd = self.env("OCGEN_FORMAT_CMD").trim();
        if cmd.is_empty() {
            return Outcome::allow();
        }
        let ok = shell(cmd)
            .current_dir(self.project())
            .output()
            .map(|o| o.status.success());
        if ok.unwrap_or(false) {
            Outcome::allow()
        } else {
            // Never blocks: a failing formatter is reported, not fatal.
            Outcome {
                code: 0,
                stdout: String::new(),
                stderr: format!("format hook: '{cmd}' failed (ignored)\n"),
            }
        }
    }

    fn config_audit(&self) -> Outcome {
        let dir = self.project().join(".claude/audit");
        if fs::create_dir_all(&dir).is_err() {
            return Outcome::allow();
        }
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            let _ = fs::write(&ignore, "*\n");
        }
        let or = |v: String, d: &str| if v.is_empty() { d.to_string() } else { v };
        let line = format!(
            "{} {} {}\n",
            crate::clock::iso_stamp(),
            or(self.field("source"), "unknown"),
            or(self.field("file_path"), "-")
        );
        use std::io::Write as _;
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("config-changes.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
        Outcome::allow()
    }
}

/// A shell command, the way the hook scripts run it (`sh -c` everywhere: Claude
/// Code runs hooks through bash, including Git Bash on Windows).
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", cmd]);
    c
}

/// A quality gate's block: count it, and once the budget is spent (or a stated
/// confidence stops rising) release with an UNRESOLVED message instead. `reason`
/// says what failed; `stall_score` enables the stopped-rising detection.
fn quality_block(
    guard: Option<Guard>,
    msg: String,
    reason: String,
    stall_score: Option<u32>,
    unresolved: impl Fn(&str, &str) -> String,
) -> Outcome {
    let Some(g) = guard else {
        return Outcome::block(msg);
    };
    let n = g.bump();
    let stalled = g.stalled(stall_score);
    if n >= g.max || stalled {
        let why = if stalled {
            format!("confidence stopped rising ({n} blocks)")
        } else {
            format!("blocked {n}x")
        };
        let detail = format!("{why}; {reason}");
        g.escalate(&detail);
        let mut out = release(&unresolved(&g.subject, &detail));
        out.stderr = msg;
        return out;
    }
    Outcome::block(format!(
        "{msg}Loop guard: block {n} of {}. If a different approach won't get you there,\n\
         say what blocks you instead of retrying the same thing.\n",
        g.max
    ))
}

/// Quality gate gives way: warn the user (systemMessage) and allow.
fn release(msg: &str) -> Outcome {
    let clean: String = msg.replace(['"', '\\', '\n'], " ");
    Outcome {
        code: 0,
        stdout: format!("{}\n", serde_json::json!({ "systemMessage": clean })),
        stderr: String::new(),
    }
}

// ------------------------------------------------------------- loop guard --

/// The loop guard's per-subject counter, in the same files the `loop-guard.sh`
/// library uses: `.claude/loop-guard/<session>/<gate>-<subject>.{count,score,escalated}`.
struct Guard {
    max: u32,
    dir: PathBuf,
    key: PathBuf,
    gate: String,
    subject: String,
}

/// `tr -c 'A-Za-z0-9._-' '_' | cut -c1-80`.
fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

impl Guard {
    fn new(h: &Hook, gate: &str, subject: &str) -> Option<Self> {
        let max: u32 = h.env("LOOP_GUARD_MAX_BLOCKS").parse().unwrap_or(0);
        if max == 0 {
            return None;
        }
        let dir = h.project().join(".claude/loop-guard");
        let sid = match safe(&h.field("session_id")) {
            s if s.is_empty() => "nosession".to_string(),
            s => s,
        };
        let subject = match safe(subject) {
            s if s.is_empty() => "unknown".to_string(),
            s => s,
        };
        let key = dir.join(&sid).join(format!("{gate}-{subject}"));
        Some(Self {
            max,
            dir,
            key,
            gate: gate.into(),
            subject,
        })
    }

    fn file(&self, ext: &str) -> PathBuf {
        let mut p = self.key.clone().into_os_string();
        p.push(ext);
        PathBuf::from(p)
    }

    fn prepare(&self) -> bool {
        let Some(parent) = self.key.parent() else {
            return false;
        };
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
        let ignore = self.dir.join(".gitignore");
        if !ignore.exists() {
            let _ = fs::write(&ignore, "*\n");
        }
        true
    }

    /// Increment and return this subject's consecutive block count.
    fn bump(&self) -> u32 {
        if !self.prepare() {
            return 0;
        }
        let n = read_num(&self.file(".count")).unwrap_or(0) + 1;
        let _ = fs::write(self.file(".count"), format!("{n}\n"));
        n
    }

    /// True when a stated confidence did not rise since the last block (records it).
    fn stalled(&self, score: Option<u32>) -> bool {
        let Some(s) = score else {
            return false;
        };
        let prev = read_num(&self.file(".score"));
        let _ = fs::write(self.file(".score"), format!("{s}\n"));
        prev.is_some_and(|p| s <= p)
    }

    /// The gate passed: the next attempt gets a fresh budget.
    fn reset(&self) {
        for ext in [".count", ".score", ".escalated"] {
            let _ = fs::remove_file(self.file(ext));
        }
    }

    /// Log once per episode to `escalations.md`.
    fn escalate(&self, detail: &str) {
        if !self.prepare() || self.file(".escalated").exists() {
            return;
        }
        let _ = fs::write(self.file(".escalated"), "");
        let line = format!(
            "- {} [{}] {}: {}\n",
            crate::clock::iso_stamp(),
            self.gate,
            self.subject,
            detail.replace('\n', " ")
        );
        use std::io::Write as _;
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("escalations.md"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn read_num(p: &Path) -> Option<u32> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}
