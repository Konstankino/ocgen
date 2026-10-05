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
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

/// Bump whenever hook behavior or the command-line contract changes. A generated
/// hook command uses the binary only when `ocgen hook --check` prints exactly the
/// protocol the project was generated with; any other ocgen (older or newer)
/// falls back to the project's own scripts. Those are written from the scripts
/// embedded in the ocgen that generated the project (hook scripts can't be
/// overridden from the template dir), so they implement the same protocol —
/// unless someone edits the project's copies by hand.
pub const PROTOCOL: &str = "ocgen-hooks 13";

/// Every hook `ocgen hook <name>` accepts (matching the script names minus `.sh`).
pub const NAMES: [&str; 14] = [
    "subagent-confidence-gate",
    "team-task-completed",
    "team-task-created",
    "team-teammate-idle",
    "team-approval-gate",
    "notify",
    "format",
    "config-audit",
    "https-only-fetch",
    "inquire-notes",
    "drop-noop-cd",
    "intent-draft",
    "intent-approvers",
    "recap-github",
];

/// A Bash command that starts with one `cd <target>` and goes on after `&&` or
/// `;` (shared with `drop-noop-cd.sh`, which runs it through jq). The target is
/// one plain word or a simple quoted string — nothing the shell would expand.
pub const NOOP_CD_RE: &str = r#"\A[ \t\r\n]*cd[ \t]+(?:"(?<dq>[^"$`\r\n]*)"|'(?<sq>[^'\r\n]*)'|(?<bare>[^ \t\r\n"'$`\\;&|<>()*?\[\]~{}#][^ \t\r\n"'$`\\;&|<>()*?\[\]{}#]*))[ \t]*(?:&&|;)[ \t\r\n]*(?<rest>[^ \t\r\n;&|][\s\S]*)\z"#;

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

/// `file` relative to `root`, with forward slashes — as given, else both
/// resolved (a linked or short-named path, a Windows verbatim one).
fn relative_to(file: &Path, root: &Path) -> Option<String> {
    let rel = match file.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => {
            let (f, r) = (file.canonicalize().ok()?, root.canonicalize().ok()?);
            f.strip_prefix(r).ok()?.to_path_buf()
        }
    };
    Some(rel.to_string_lossy().replace('\\', "/"))
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
        "https-only-fetch" => h.https_only_fetch(),
        "inquire-notes" => h.inquire_notes(),
        "drop-noop-cd" => h.drop_noop_cd(),
        "intent-draft" => h.intent_draft(),
        "intent-approvers" => h.intent_approvers(),
        "recap-github" => h.recap_github(),
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

    /// The event's team: its `team_name` or, when it has none (Claude Code marks
    /// the field deprecated), the team Claude Code makes for its session — the
    /// one every in-process teammate's meta names. Empty without either.
    fn team(&self) -> String {
        match self.field("team_name") {
            t if !t.is_empty() => t,
            _ => match self.field("session_id") {
                s if s.is_empty() => s,
                s => implicit_team(&s),
            },
        }
    }

    /// Whether this run is `ocgen verify`'s probe: decide, but change nothing on
    /// disk (no plan stamp, no marker or baseline bookkeeping).
    fn probe(&self) -> bool {
        self.env("OCGEN_HOOK_PROBE") == "1"
    }

    /// A number setting as the scripts read it (`case *[!0-9]*`): plain digits,
    /// else `default`.
    fn env_num<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        let v = self.env(key);
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
            return default;
        }
        v.parse().unwrap_or(default)
    }

    /// How long a check may run, in seconds (`OCGEN_CHECK_TIMEOUT`, default 300).
    fn check_timeout(&self) -> u64 {
        self.env_num("OCGEN_CHECK_TIMEOUT", 300)
    }

    fn guard(&self, gate: &str, subject: &str) -> Option<Guard> {
        Guard::new(self, gate, subject)
    }

    // ---------------------------------------------------------------- gates --

    /// SubagentStart records the tree a worker starts from; SubagentStop gates a
    /// worker whose tree changed since (or, with no record, any dirty tree).
    /// Read-only roles (`SUBAGENT_READONLY_ROLES`) are never gated — an
    /// in-process teammate stops as a subagent named after itself, so its role
    /// is read from its transcript's `.meta.json` (`customAgentType`).
    fn subagent_confidence(&self) -> Outcome {
        let thr: u32 = self.env_num("SUBAGENT_CONFIDENCE_THRESHOLD", 0);
        let check = self.env("OCGEN_CHECK_CMD");
        if thr == 0 && check.is_empty() {
            return Outcome::allow();
        }
        let agent = safe(&self.field("agent_id"));
        let baseline = (!agent.is_empty())
            .then(|| self.project().join(".claude/worker-baseline").join(&agent));
        let forget = || {
            if let (Some(b), false) = (&baseline, self.probe()) {
                let _ = fs::remove_file(b);
            }
        };
        let readonly = self.env("SUBAGENT_READONLY_ROLES");
        if listed(readonly, &self.field("agent_type"))
            || listed(readonly, &self.meta_field("customAgentType"))
        {
            forget();
            return Outcome::allow();
        }
        // With the team task gate on, TaskCompleted judges a teammate per task;
        // its turn ends aren't a worker finishing.
        if self.env("TEAM_TASK_GATE") == "1" && self.meta_field("taskKind") == "in_process_teammate"
        {
            forget();
            return Outcome::allow();
        }
        // The hook input's `cwd` follows the subagent (its own worktree when
        // isolated), unlike CLAUDE_PROJECT_DIR.
        let cwd = match self.field("cwd") {
            c if !c.is_empty() => PathBuf::from(c),
            _ => self.project(),
        };
        if self.field("hook_event_name") == "SubagentStart" {
            if let (Some(b), Some(state), false) = (&baseline, tree_state(&cwd), self.probe()) {
                record_baseline(b, &state);
            }
            return Outcome::allow();
        }
        let out = self.worker_gate(thr, check, &cwd, baseline.as_deref());
        // A worker that may finish is done: forget where it started.
        if out.code == 0 {
            forget();
        }
        out
    }

    /// A string field of the `.meta.json` beside the stopping agent's transcript
    /// (`agent_transcript_path`): its role (`customAgentType`), whether it is a
    /// teammate (`taskKind`). Empty when there is none.
    fn meta_field(&self, key: &str) -> String {
        let tp = self.field("agent_transcript_path");
        let Some(stem) = tp.strip_suffix(".jsonl") else {
            return String::new();
        };
        fs::read_to_string(format!("{stem}.meta.json"))
            .ok()
            .and_then(|m| serde_json::from_str::<Value>(&m).ok())
            .and_then(|v| v.get(key)?.as_str().map(String::from))
            .unwrap_or_default()
    }

    fn worker_gate(&self, thr: u32, check: &str, cwd: &Path, baseline: Option<&Path>) -> Outcome {
        if !worker_changed(cwd, baseline) {
            return Outcome::allow();
        }
        // Only the worker's own final message counts — not the rest of the event
        // (running background tasks, crons).
        let score = stated_confidence(&self.field("last_assistant_message"));
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
        let limit = self.check_timeout();
        if let Some((rc, tail)) = self.failed_check(check, cwd, Duration::from_secs(limit)) {
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
                         >= {thr}% confident, and end your final message with a line 'Confidence: NN%'.\n"
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

    /// TaskCompleted carries only the lead-written task (subject, description) —
    /// never scored. The teammate's own words are the last assistant message in
    /// its transcript; failing that, a marker file keyed by team and task, removed
    /// once the task passes.
    fn task_completed(&self) -> Outcome {
        let thr: u32 = self.env_num("TEAM_CONFIDENCE_THRESHOLD", 0);
        let base = self.env("OCGEN_CHECK_CMD");
        if thr == 0 && base.is_empty() {
            return Outcome::allow();
        }
        let tid = Regex::new(r#"(?i)"(task_?id|id)"[[:space:]]*:[[:space:]]*"([^"]+)""#)
            .unwrap()
            .captures(self.raw)
            .map(|c| c[2].to_string())
            .unwrap_or_default();
        let team = match safe(&self.team()) {
            t if t.is_empty() => "team".to_string(),
            t => t,
        };
        let marker_rel = format!(
            ".claude/team/confidence/{team}/{}.txt",
            if tid.is_empty() {
                "<task-id>".to_string()
            } else {
                safe(&tid)
            }
        );
        let marker = self.project().join(&marker_rel);
        let mut score = None;
        if thr > 0 {
            score = stated_confidence(&self.teammate_words());
            if score.is_none() && !tid.is_empty() {
                score = fs::read_to_string(&marker)
                    .ok()
                    .and_then(|m| marker_confidence(&m));
            }
        }
        let check = task_check(base, &self.field("task_description"));
        let subject = if tid.is_empty() { "task" } else { tid.as_str() };
        let guard = self.guard("task-confidence", subject);
        let unresolved = |subject: &str, why: &str| {
            format!(
                "UNRESOLVED: task {subject} completed by the loop guard ({why}). Needs review; see .claude/loop-guard/escalations.md"
            )
        };
        // 1. The objective check, in the shared project directory.
        if let Some((rc, tail)) = self.locked_check(&check) {
            let msg = format!(
                "Check failed: `{check}` (exit {rc}). The task isn't complete until it passes — a\n\
                 stated confidence doesn't override a failing check. Teammates share this directory,\n\
                 so the failure may come from another teammate's unfinished edits: if it isn't yours,\n\
                 say so instead of editing their files. Last output:\n{tail}"
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
                        "No confidence found for this completion. End your final message with a line\n\
                         'Confidence: NN%' (>= {thr}%), or write it to {marker_rel}.\n"
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
        if !self.probe() && !tid.is_empty() {
            let _ = fs::remove_file(&marker);
        }
        if let Some(g) = guard {
            g.reset();
        }
        Outcome::allow()
    }

    /// The text of the teammate's last assistant message. An in-process teammate
    /// shares the lead's session, so `transcript_path` is the lead's; its own
    /// transcript sits beside it (see [`sidechain`]), found under the event's
    /// team (see [`Hook::team`]). A teammate in its own session writes
    /// `transcript_path` itself. The lead's transcript never speaks for a
    /// teammate: not when the event names the lead session's implicit team
    /// (`session-<first 8 of its id>`), nor when any teammates are filed beside
    /// it.
    fn teammate_words(&self) -> String {
        let tp = self.field("transcript_path");
        if tp.is_empty() {
            return String::new();
        }
        let named = self.field("team_name");
        let side = sidechain(&tp, &self.field("teammate_name"), &self.team());
        let lead =
            side.lead || (!named.is_empty() && named == implicit_team(&self.field("session_id")));
        let file = match side.own {
            Some(f) => f,
            None if lead => return String::new(),
            None => PathBuf::from(&tp),
        };
        // Claude Code appends transcript lines on a timer, so the final message
        // may land just after this hook starts.
        let settle: u64 = self.env_num("OCGEN_TRANSCRIPT_SETTLE_MS", TRANSCRIPT_SETTLE_MS);
        std::thread::sleep(Duration::from_millis(settle));
        last_assistant_text(&file)
    }

    /// Run `check` in `dir` for at most `budget` — in the sandbox when the
    /// project's is on. `None` when there is no check or it passed; otherwise its
    /// exit code — or `timeout after <OCGEN_CHECK_TIMEOUT>s` once the check and
    /// every process it started are killed, or `not run` when the sandbox it
    /// needs isn't there — and the last lines of its output (or why not run).
    fn failed_check(&self, check: &str, dir: &Path, budget: Duration) -> Option<(String, String)> {
        if check.is_empty() {
            return None;
        }
        let mut cmd = match self.project_command(check, dir) {
            Ok(cmd) => cmd,
            Err(why) => return Some(("not run".into(), format!("{why}\n"))),
        };
        let limit = self.check_timeout();
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
            // Its own process group (the sandbox's front end leading it), so a
            // timeout takes its children with it.
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            let mut child = cmd
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(out)
                .stderr(err)
                .spawn()
                .ok()?;
            let start = Instant::now();
            loop {
                match child.try_wait().ok()? {
                    Some(status) => {
                        return Some(
                            status
                                .code()
                                .map_or("signal".to_string(), |c| c.to_string()),
                        )
                    }
                    None if start.elapsed() >= budget => {
                        kill_tree(&mut child);
                        return Some(format!("timeout after {limit}s"));
                    }
                    None => std::thread::sleep(Duration::from_millis(20)),
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

    /// The team check runs in the shared project directory, one teammate at a
    /// time: a `mkdir` lock under `.claude/team/`, taken over once it is older
    /// than the timeout plus a minute (or, never stamped, after a few seconds).
    /// Git never sees it. Waiting for it counts against the same timeout as the
    /// check.
    fn locked_check(&self, check: &str) -> Option<(String, String)> {
        if check.is_empty() {
            return None;
        }
        let limit = self.check_timeout();
        let start = Instant::now();
        let lock = self.project().join(".claude/team/check.lock");
        let mut held = false;
        let mut unstamped: Option<Instant> = None;
        if lock.parent().is_some_and(|p| fs::create_dir_all(p).is_ok()) {
            loop {
                match fs::create_dir(&lock) {
                    Ok(()) => {
                        held = true;
                        // Runtime state, not a change to anyone's tree.
                        let _ = fs::write(lock.join(".gitignore"), "*\n");
                        let _ = fs::write(lock.join("at"), format!("{}\n", epoch()));
                        break;
                    }
                    // Can't lock here (a read-only tree?): run unlocked.
                    Err(_) if !lock.is_dir() => break,
                    Err(_) => {
                        let stale = match read_num64(&lock.join("at")) {
                            Some(at) => {
                                unstamped = None;
                                epoch().saturating_sub(at) > limit + 60
                            }
                            // Its holder stamps it at once, so it died first.
                            None => {
                                unstamped.get_or_insert_with(Instant::now).elapsed()
                                    >= Duration::from_secs(UNSTAMPED_LOCK_SECS)
                            }
                        };
                        if stale {
                            let _ = fs::remove_dir_all(&lock);
                            unstamped = None;
                            continue;
                        }
                        if start.elapsed().as_secs() >= limit {
                            return Some((
                                format!("timeout after {limit}s"),
                                "Another teammate's check held .claude/team/check.lock the whole time.\n"
                                    .into(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        let left = Duration::from_secs(limit).saturating_sub(start.elapsed());
        let out = self.failed_check(check, &self.project(), left);
        if held {
            let _ = fs::remove_dir_all(&lock);
        }
        out
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
        let verdict = match fs::read_to_string(&plan) {
            Ok(body) => self.plan_verdict(&plan, &body),
            Err(_) => Err(PLAN_NOT_APPROVED),
        };
        let msg = match verdict {
            Ok(()) => {
                if let Some(g) = guard {
                    g.reset();
                }
                return Outcome::allow();
            }
            Err(msg) => msg,
        };
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
        Outcome::block(msg.into())
    }

    /// Whether `plan.md` (`body`) is approved for this plan and this team run.
    /// The first event to see a fresh `Status: APPROVED` line stamps it with the
    /// run and a checksum of the plan without its Status lines (see
    /// [`plan_sum`]); a later run, or any edit to the plan, leaves the approval
    /// stale until the line is written afresh — closing a risk or ticking a task
    /// is progress, not an edit. `/team-plan` writes the approving session
    /// (`session=<id>`), so a fresh line from another session is stale too.
    ///
    /// The run is the event's team ([`Hook::team`]) — the lead's own events carry
    /// none, so for them it is the team Claude Code makes for the lead's session,
    /// `session-<first 8 of its id>`. An event also belongs to that session's
    /// team (an in-process teammate shares the lead's session).
    fn plan_verdict(&self, path: &Path, body: &str) -> Result<(), &'static str> {
        let mut lines = split_lines(body);
        let Some(n) = lines
            .iter()
            .rposition(|l| l.starts_with("Status: APPROVED"))
        else {
            return Err(PLAN_NOT_APPROVED);
        };
        let sum = plan_sum(&lines);
        let sid = self.field("session_id");
        let own = match sid.as_str() {
            "" => String::new(),
            s => safe(&implicit_team(s)),
        };
        let run = match safe(&self.team()) {
            t if t.is_empty() => "none".to_string(),
            t => t,
        };
        let ours = |r: &str| r == run || (!own.is_empty() && r == own);
        let stamp = Regex::new(r"\[ocgen: run=([^ \]]+) plan=([0-9]+-[0-9]+)\]").unwrap();
        if let Some(c) = stamp.captures_iter(&lines[n]).last() {
            return if !ours(&c[1]) {
                Err(PLAN_OTHER_RUN)
            } else if c[2] != sum {
                Err(PLAN_CHANGED)
            } else {
                Ok(())
            };
        }
        let by = Regex::new(r"session=([A-Za-z0-9_-]+)")
            .unwrap()
            .captures_iter(&lines[n])
            .last()
            .map(|c| c[1].to_string());
        if by.is_some_and(|by| !ours(&implicit_team(&by))) {
            return Err(PLAN_OTHER_SESSION);
        }
        if !self.probe() {
            let line = lines[n].trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b');
            lines[n] = format!("{line} [ocgen: run={run} plan={sum}]");
            let out: String = lines.iter().map(|l| format!("{l}\n")).collect();
            // Replaced whole, as the sh twin does, so a concurrent reader never
            // sees half a plan.
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(format!(".ocgen-{}", std::process::id()));
            if fs::write(&tmp, out).is_err() || fs::rename(&tmp, path).is_err() {
                let _ = fs::remove_file(&tmp);
            }
        }
        Ok(())
    }

    /// TeammateIdle holds a teammate while a risk IT owns is pending. The teammate
    /// is its `teammate_name` (its role, `agent_type`, only when the event has no
    /// name), and risk owners name teammates. A role-named owner — from an older
    /// /team-plan, when several teammates may share the role — holds no named
    /// teammate, but is reported rather than passing unseen. An in-process
    /// teammate's role is in its transcript's meta (see [`sidechain`]), filed
    /// under the event's team ([`Hook::team`]).
    fn teammate_idle(&self) -> Outcome {
        if self.env("TEAM_RISK_ROUNDS") != "1" {
            return Outcome::allow();
        }
        let name = self.field("teammate_name");
        let role = match sidechain(&self.field("transcript_path"), &name, &self.team()).role {
            r if r.is_empty() => self.field("agent_type"),
            r => r,
        };
        let who = if name.is_empty() {
            role.clone()
        } else {
            name.clone()
        };
        if who.is_empty() {
            return release(
                "Risk gate: this TeammateIdle event names no teammate (no teammate_name or agent_type), so its pending risks in .claude/team/plan.md were not checked.",
            );
        }
        if listed(self.env("TEAM_READONLY_ROLES"), &role) {
            return Outcome::allow();
        }
        let Ok(plan) = fs::read_to_string(self.project().join(".claude/team/plan.md")) else {
            return Outcome::allow();
        };
        let pending = Regex::new(r"(?i)mitigation:[[:space:]]*pending").unwrap();
        let owner = |id: &str| {
            Regex::new(&format!(
                r"(?i)owner:[[:space:]]*{}([^a-z0-9_-]|$)",
                regex::escape(id)
            ))
            .unwrap()
        };
        let mine = owner(&who);
        let owned: Vec<&str> = plan
            .lines()
            .filter(|l| pending.is_match(l) && mine.is_match(l))
            .collect();
        let guard = self.guard("risk-idle", &who);
        if owned.is_empty() {
            if let Some(g) = guard {
                g.reset();
            }
            if !name.is_empty() && !role.is_empty() && !role.eq_ignore_ascii_case(&name) {
                let by_role = owner(&role);
                let n = plan
                    .lines()
                    .filter(|l| pending.is_match(l) && by_role.is_match(l))
                    .count();
                if n > 0 {
                    return release(&format!(
                        "Risk gate: {n} pending risk(s) in .claude/team/plan.md name the role {role} as Owner, not a teammate, so they hold no one. Write Owner: <teammate-name>."
                    ));
                }
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
                    "UNRESOLVED: {who} could not close its pending risks after {n} holds; released by the loop guard. See .claude/loop-guard/escalations.md"
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
        // the approval store. (Neither message names the store or how to write
        // it: this text goes to the agent.)
        let self_approval = || {
            self.deny(
                "Blocked: only a HUMAN may approve execution, from a terminal outside the agent\n\
                 (ocgen approve). No tool call may approve or touch the approval store.\n"
                    .into(),
                "tried to approve itself",
            )
        };
        // Any tool with a shell command — Bash, Monitor, PowerShell, an MCP tool.
        let cmd = text("command");
        if !cmd.is_empty() {
            if crate::risk::self_approval(&cmd) {
                return self_approval();
            }
            let gated = crate::risk::has_control(&cmd) || crate::risk::high_impact(&cmd).is_some();
            if gated && !self.approved() {
                return self.deny(
                    "BLOCKED by the execution-approval gate: this is a high-impact external\n\
                     action (ssh / cloud mutation / git push|merge / deploy / publish / etc.).\n\
                     A human must review, then approve from their own terminal (it expires by itself):\n    \
                     ocgen approve\n\
                     No agent may approve, and no tool call may touch the approval store.\n"
                        .into(),
                    "gated command without approval",
                );
            }
        }
        // Any tool's target file or folder, compared in canonical form. `approv`
        // also covers the names a case-insensitive disk takes for `approvals`
        // (the Windows short name `APPROV~1`, a long s for the s).
        let cwd = self.field("cwd");
        let store = ["file_path", "notebook_path", "path"]
            .iter()
            .any(|k| canonical_path(&text(k), &cwd).contains("ocgen/approv"));
        if store {
            return self_approval();
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

    /// PostToolUse (Edit|Write): run the formatter in the project directory — or,
    /// for a file in another worktree of its repository (a worktree-isolated
    /// worker's own), in the project's folder there — in the sandbox when the
    /// project's is on.
    fn format(&self) -> Outcome {
        let cmd = self.env("OCGEN_FORMAT_CMD").trim();
        if cmd.is_empty() {
            return Outcome::allow();
        }
        let root = self.edit_root();
        let mut run = match self.project_command(cmd, &root) {
            Ok(run) => run,
            Err(why) => {
                return Outcome {
                    code: 0,
                    stdout: String::new(),
                    stderr: format!("format hook: '{cmd}' not run: {why}\n"),
                }
            }
        };
        let ok = run.current_dir(&root).output().map(|o| o.status.success());
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

    /// Where to format after an edit: the project directory, unless the edited
    /// file is in another worktree of the project's repository (a
    /// worktree-isolated worker's) — then the project's folder in that worktree.
    /// A file in an unrelated repository is not formatted there.
    fn edit_root(&self) -> PathBuf {
        let project = self.project();
        let file = self
            .json
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let drive = file.as_bytes().get(1) == Some(&b':');
        let file = match Path::new(file) {
            p if file.is_empty() || p.is_absolute() || drive => p.to_path_buf(),
            p => project.join(p),
        };
        let git = |dir: &Path, what: &str| {
            let o = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["rev-parse", what])
                .stdin(Stdio::null())
                .output()
                .ok()?;
            let out = String::from_utf8_lossy(&o.stdout)
                .trim_end_matches(['\n', '\r'])
                .to_string();
            o.status.success().then_some(out)
        };
        // The repository a folder belongs to, shared by all its worktrees.
        let common =
            |dir: &Path| git(dir, "--git-common-dir").and_then(|c| dir.join(c).canonicalize().ok());
        let Some(dir) = file.parent().filter(|_| !file.as_os_str().is_empty()) else {
            return project;
        };
        let Some(top) = git(dir, "--show-toplevel").filter(|t| !t.is_empty()) else {
            return project;
        };
        if git(&project, "--show-toplevel").as_deref() == Some(top.as_str())
            || common(dir).is_none()
            || common(dir) != common(&project)
        {
            return project;
        }
        let prefix = git(&project, "--show-prefix").unwrap_or_default();
        let inside = Path::new(&top).join(prefix.trim_end_matches('/'));
        if !prefix.is_empty() && inside.is_dir() {
            inside
        } else {
            PathBuf::from(top)
        }
    }

    /// PreToolUse (WebFetch): the WebFetch guard — https:// only, and only to a
    /// trusted documentation site (`OCGEN_WEBFETCH_DOMAINS`, space-separated; a
    /// leading `*.` trusts every subdomain, not the bare domain). An empty or unset
    /// list trusts nothing; an unreadable URL is blocked (fail closed).
    fn https_only_fetch(&self) -> Outcome {
        let url = self
            .json
            .pointer("/tool_input/url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start();
        let lower = url.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("https://") else {
            return Outcome::block(if lower.starts_with("http://") {
                let rest = url.split_once("://").map_or(url, |(_, r)| r);
                format!("Blocked: plain http:// is not allowed for WebFetch in this project — fetch https://{rest} instead, or skip the page if it has no HTTPS version.\n")
            } else if lower.is_empty() {
                "Blocked: could not read the WebFetch URL, so it can't be checked against the trusted documentation sites.\n".to_string()
            } else {
                "Blocked: only https:// URLs may be fetched in this project.\n".to_string()
            });
        };
        // The host part (up to path/query/fragment) must be a plain host name with
        // an optional port. Anything else — a user part, a backslash (a path
        // separator to WHATWG parsers, so `evil.com\@docs.rs` goes to evil.com),
        // %-escapes, whitespace, non-ASCII — could be read differently by the
        // fetcher than here, so it is blocked rather than parsed.
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (host, port) = authority.split_once(':').unwrap_or((authority, "1"));
        let plain = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
            && !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit());
        if !plain {
            return Outcome::block(
                "Blocked: the URL's host must be a plain host name (letters, digits, dots, hyphens) with an optional port — no user@ part, backslash, %-escape or spaces. Fetch https://<host>/<path> instead, or skip it.\n".to_string(),
            );
        }
        let host = host.strip_suffix('.').unwrap_or(host);
        let list = self.env("OCGEN_WEBFETCH_DOMAINS");
        if list.trim().is_empty() {
            return Outcome::block(format!(
                "Blocked: no documentation sites are trusted in this project, so WebFetch is off. Ask the user to trust one with `ocgen edit docs --trust {host}`, or skip it.\n"
            ));
        }
        let trusted = list.split_whitespace().any(|d| {
            let d = d.to_ascii_lowercase();
            match d.strip_prefix('*') {
                Some(suffix) if suffix.starts_with('.') => {
                    host.ends_with(suffix) && host != &suffix[1..]
                }
                _ => host == d,
            }
        });
        if trusted {
            Outcome::allow()
        } else {
            Outcome::block(format!(
                "Blocked: {host} is not a trusted documentation site for this project (trusted: {list}). Ask the user to trust it with `ocgen edit docs --trust {host}`, or skip it.\n"
            ))
        }
    }

    /// PreToolUse (Bash): drop a leading `cd` into the folder Claude is already
    /// in. It changes nothing, but after a `cd` Claude Code can't tell where
    /// relative paths point, so under `blockReadsOutsideWorkingDirectories` every
    /// such command asks. Claude Code only skips a `cd` spelled exactly like its
    /// working directory; this also matches a trailing `/` and the resolved path,
    /// and on Windows (or under Git Bash) `C:\` vs `C:/` vs `/c/` and case. The
    /// shorter command goes back as `updatedInput` and is permission-checked as
    /// usual — never decided here.
    fn drop_noop_cd(&self) -> Outcome {
        let Some(input) = self.json.get("tool_input").filter(|v| v.is_object()) else {
            return Outcome::allow();
        };
        let cmd = input.get("command").and_then(Value::as_str).unwrap_or("");
        let re = regex::Regex::new(NOOP_CD_RE).expect("NOOP_CD_RE compiles");
        let Some(m) = re.captures(cmd) else {
            return Outcome::allow();
        };
        let target = ["dq", "sq", "bare"]
            .iter()
            .find_map(|g| m.name(g))
            .map_or("", |t| t.as_str());
        let cwd = match self.field("cwd") {
            c if !c.is_empty() => c.to_string(),
            _ => self.env("CLAUDE_PROJECT_DIR").to_string(),
        };
        // Windows path forms only where they mean Windows paths: on a Unix
        // filesystem `/w/App` is not `/w/app`, and `/x/…` is a folder, not a drive.
        let windows =
            cfg!(windows) || !self.env("MSYSTEM").is_empty() || self.env("OS") == "Windows_NT";
        if !same_folder(target, &cwd, windows) {
            return Outcome::allow();
        }
        let mut up = input.clone();
        up["command"] = Value::String(m["rest"].to_string());
        let out = serde_json::json!({
            "hookSpecificOutput": { "hookEventName": "PreToolUse", "updatedInput": up }
        });
        Outcome {
            code: 0,
            stdout: format!("{out}\n"),
            stderr: String::new(),
        }
    }

    /// PostToolUse (Write|Edit|MultiEdit): when an /inquire ledger was written,
    /// render its HTML view and show it (refresh an open tab, else open one).
    /// Never blocks — the ledger is already written; problems are one stderr line.
    fn inquire_notes(&self) -> Outcome {
        use crate::notes;
        let path = self
            .json
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let note = |msg: String| Outcome {
            code: 0,
            stdout: String::new(),
            stderr: format!("inquire-notes: {msg}\n"),
        };
        // An /intent reading copy: rendered and opened as a file.
        if notes::intent_view_target(path).is_some() {
            let md = match Path::new(path) {
                p if p.is_absolute() => p.to_path_buf(),
                p => self.project().join(p),
            };
            if let Err(e) = notes::render_file(&md) {
                return note(format!("could not render the HTML view: {e:#}"));
            }
            return match notes::show_intent_view(&md, self.env, false) {
                Ok(_) => Outcome::allow(),
                Err(e) => note(format!("could not show the HTML view: {e:#}")),
            };
        }
        match notes::ledger_target(path) {
            notes::Target::NotLedger => return Outcome::allow(),
            notes::Target::BadSlug => {
                return note(format!(
                    "{path} has no HTML view — name ledgers with a lowercase-hyphen slug (e.g. request-flow.md)"
                ))
            }
            notes::Target::Ledger { .. } => {}
        }
        let md = match Path::new(path) {
            p if p.is_absolute() => p.to_path_buf(),
            p => self.project().join(p),
        };
        if let Err(e) = notes::render_file(&md) {
            return note(format!("could not render the HTML view: {e:#}"));
        }
        match notes::show(&md, self.env, false) {
            Ok(_) => Outcome::allow(),
            Err(e) => note(format!("could not show the HTML view: {e:#}")),
        }
    }

    /// UserPromptSubmit: the word `draft`, sent on its own, opens the /intent
    /// issue draft in the browser editor — or, with several, their list, with
    /// the one this session wrote last selected — from here, outside the Bash
    /// sandbox, where a local server can start, and tells Claude what happened.
    /// Any other prompt passes untouched.
    ///
    /// PostToolUse (Write|Edit|MultiEdit): a write to a draft makes it this
    /// session's ([`crate::notes::draft::remember`]), silently. Never blocks.
    fn intent_draft(&self) -> Outcome {
        use crate::notes::draft;
        if self.field("hook_event_name") == "PostToolUse" {
            self.remember_draft();
            return Outcome::allow();
        }
        if !draft::is_prompt(&self.field("prompt")) {
            return Outcome::allow();
        }
        let dir = self.project().join(draft::DIR);
        let all = draft::drafts(&dir);
        let note = match all.as_slice() {
            [] => format!(
                "The user typed `draft` to open the /intent issue draft in their browser, but there is \
                 no issue draft in {}/ yet, so nothing was opened. /intent and /review-intent write \
                 one when they draft a GitHub issue.",
                draft::DIR
            ),
            [md] => self.open_draft(md),
            _ => self.open_draft_list(&dir, all.len()),
        };
        let out = serde_json::json!({
            "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": note }
        });
        Outcome {
            code: 0,
            stdout: format!("{out}\n"),
            stderr: String::new(),
        }
    }

    /// After a write: remember the draft written, if it is one of this project's.
    /// A probe changes nothing.
    fn remember_draft(&self) {
        use crate::notes::draft;
        let path = self
            .json
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if path.is_empty() || self.probe() {
            return;
        }
        let root = self.project();
        let file = match Path::new(path) {
            p if p.is_absolute() => p.to_path_buf(),
            p => root.join(p),
        };
        let Some(rel) = relative_to(&file, &root) else {
            return;
        };
        let Some(slug) = draft::draft_target(&format!("/{rel}")) else {
            return;
        };
        if rel == format!("{}/{slug}.md", draft::DIR) && file.is_file() {
            draft::remember(&root.join(draft::DIR), &self.field("session_id"), &slug);
        }
    }

    /// The approvers warning for draft `md`, if it falls short of them.
    fn draft_gaps_warning(md: &Path) -> String {
        use crate::notes::draft;
        let gaps = draft::gaps_of(md);
        if gaps.is_empty() {
            return String::new();
        }
        format!(
            " Also tell them: the draft {} — the project's approvers are now {}; GitHub \
             notifies only the people an issue @mentions (the page shows the same warning).",
            gaps.describe(),
            draft::approvers_for(md).join(", ")
        )
    }

    /// The one draft there is, opened in the editor: what to tell Claude.
    fn open_draft(&self, md: &Path) -> String {
        use crate::notes::{draft, Shown};
        let rel = format!(
            "{}/{}",
            draft::DIR,
            md.file_name().unwrap_or_default().to_string_lossy()
        );
        let terminal = "`ocgen draft` in a terminal opens it in their browser";
        let note = match draft::show(md, self.env, true) {
            Ok(Shown::Off) => format!(
                "The user typed `draft`, but opening a browser is switched off here \
                 (OCGEN_NOTES_OPEN=0). Tell them the issue draft is {rel} and that {terminal}."
            ),
            Ok(Shown::Reloaded(_) | Shown::Pending) => format!(
                "The user typed `draft`: the issue draft {rel} is already open in a tab of \
                 their browser. Say so in one line and wait. Re-read the file before you use \
                 the draft again — they may have changed it there."
            ),
            Ok(_) => format!(
                "The user typed `draft`: ocgen opened the issue draft {rel} in their browser, \
                 where they can edit the raw Markdown and save it to that file, preview it the \
                 way GitHub shows it, and copy it. Say so in one line and wait (if they meant \
                 something else by `draft`, answer that instead). Re-read the file before you \
                 use the draft again — they may have changed it."
            ),
            Err(e) => format!(
                "The user typed `draft`, but ocgen could not open the issue draft {rel} in the \
                 browser ({e:#}). Tell them so, and that {terminal}."
            ),
        };
        format!("{note}{}", Self::draft_gaps_warning(md))
    }

    /// Several drafts: their list, opened with this session's draft selected —
    /// what to tell Claude.
    fn open_draft_list(&self, dir: &Path, count: usize) -> String {
        use crate::notes::{draft, Shown};
        let mine = draft::session_draft(dir, &self.field("session_id"));
        let rel = mine.as_ref().map(|s| format!("{}/{s}.md", draft::DIR));
        let reread = "Re-read whichever draft they name before you use it again — they may have \
                      changed it there.";
        let note = match draft::show_list(dir, mine.as_deref(), self.env, true) {
            Ok(Shown::Off) => format!(
                "The user typed `draft`, but opening a browser is switched off here \
                 (OCGEN_NOTES_OPEN=0). Tell them that `ocgen draft` in a terminal opens the list \
                 of the {count} issue drafts in {}/ to pick from, and `ocgen draft <name>` opens \
                 one{}.",
                draft::DIR,
                rel.as_ref()
                    .map(|r| format!("; the one this session last wrote is {r}"))
                    .unwrap_or_default()
            ),
            Ok(Shown::Reloaded(_) | Shown::Pending) => format!(
                "The user typed `draft`: the list of {count} issue drafts is already open in a tab \
                 of their browser, and ocgen moved it {}. Say so in one line and wait: they pick \
                 the draft there. {reread}",
                match &rel {
                    Some(r) => format!("to {r}, the draft this session last wrote"),
                    None => "to its first page (this session hasn't written a draft)".to_string(),
                }
            ),
            Ok(_) => format!(
                "The user typed `draft`: ocgen opened the list of {count} issue drafts in their \
                 browser ({} a page, newest first), {}. They pick the one to open there, in the \
                 browser editor. Say so in one line and wait (if they meant something else by \
                 `draft`, answer that instead). {reread}",
                draft::PER_PAGE,
                match &rel {
                    Some(r) => format!("with {r} — the draft this session last wrote — selected"),
                    None => "with none selected (this session hasn't written a draft)".to_string(),
                }
            ),
            Err(e) => format!(
                "The user typed `draft`, but ocgen could not open the list of issue drafts in the \
                 browser ({e:#}). Tell them so, and that `ocgen draft` in a terminal lists them and \
                 `ocgen draft <name>` opens one."
            ),
        };
        let warn = mine
            .map(|s| Self::draft_gaps_warning(&dir.join(format!("{s}.md"))))
            .unwrap_or_default();
        format!("{note}{warn}")
    }

    /// PostToolUse (Write|Edit|MultiEdit): after a write to a pending /intent file
    /// or an issue draft, check it against the approvers in the project's state
    /// now — not the list the session's /intent skill was generated with — and
    /// tell Claude what is missing, so it fixes the file before anyone files it.
    /// A draft is also checked for lines wrapped by hand ([`intent::hard_wraps`]):
    /// GitHub shows each newline in an issue as a line break. Never fails a write.
    fn intent_approvers(&self) -> Outcome {
        use crate::intent;
        let path = self
            .json
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if path.is_empty() {
            return Outcome::allow();
        }
        let root = self.project();
        let Ok(project) = crate::render::Project::load_state(&root) else {
            return Outcome::allow();
        };
        let s = &project.claude.intent;
        if !project.claude.workflow.intent {
            return Outcome::allow();
        }
        let file = match Path::new(path) {
            p if p.is_absolute() => p.to_path_buf(),
            p => root.join(p),
        };
        let Some(rel) = relative_to(&file, &root) else {
            return Outcome::allow();
        };
        let Some(kind) = intent::kind(s, &rel) else {
            return Outcome::allow();
        };
        let mut reasons = Vec::new();
        if let Some(gaps) = (!s.approvers.is_empty())
            .then(|| intent::check_file(&file, &s.approvers))
            .flatten()
        {
            let fix = match kind {
                intent::Kind::Intent => {
                    "Name each one in its Approvers: line and give each a pending line in its Sign-off"
                }
                intent::Kind::Draft => {
                    "Name them in the \"Needs from\" heading with one pending sign-off line each \
                     (`- [ ] @handle — pending`)"
                }
            };
            reasons.push(format!(
                "ocgen: {rel} {} — but this project's approvers are now {} (`ocgen edit intent`; \
                 the list can change while /intent runs, so trust this one over your \
                 instructions). {fix}, and drop \"{}\": their @mentions are how GitHub notifies \
                 them. Fix the file now.",
                gaps.describe(),
                s.approvers.join(", "),
                intent::NO_APPROVERS
            ));
        }
        if kind == intent::Kind::Draft {
            let wraps = std::fs::read_to_string(&file)
                .map(|t| intent::hard_wraps(&t))
                .unwrap_or_default();
            if !wraps.is_empty() {
                let at: Vec<String> = wraps.iter().map(usize::to_string).collect();
                reasons.push(format!(
                    "ocgen: {rel} has lines wrapped by hand mid-sentence (lines {}): GitHub shows \
                     every newline in an issue as a line break, so they read as broken lines \
                     there. Write each paragraph and each list item on one line, and keep a break \
                     only where the reader should see one (like the **Blocks prod?** and \
                     **Depends on…** lines). Fix the file now.",
                    at.join(", ")
                ));
            }
        }
        if reasons.is_empty() {
            return Outcome::allow();
        }
        let reason = reasons.join("\n\n");
        let out = serde_json::json!({ "decision": "block", "reason": reason });
        Outcome {
            code: 0,
            stdout: format!("{out}\n"),
            stderr: String::new(),
        }
    }

    /// PostToolUse (Write|Edit|MultiEdit): when /recap writes its GitHub request,
    /// look up what's new on the issues and the branches' PRs it names and write
    /// it to `.claude/notes/recap/github.json`, then point Claude there. A hook
    /// runs outside the Bash sandbox, so this one fixed, read-only query uses the
    /// user's own gh login, which the sandbox withholds from Claude's shell. Any
    /// other write passes untouched. Never fails a write.
    fn recap_github(&self) -> Outcome {
        let path = self
            .json
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if path.is_empty() {
            return Outcome::allow();
        }
        let root = self.project();
        let file = match Path::new(path) {
            p if p.is_absolute() => p.to_path_buf(),
            p => root.join(p),
        };
        if relative_to(&file, &root).as_deref() != Some(crate::recap::REQUEST) {
            return Outcome::allow();
        }
        let gh = crate::recap::Gh {
            program: self.env("OCGEN_RECAP_GH").trim().to_string(),
            limit: Duration::from_secs(self.env_num("OCGEN_RECAP_GH_TIMEOUT", 20)),
        };
        let note = crate::recap::answer(&root, &gh, self.probe());
        let out = serde_json::json!({
            "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": note }
        });
        Outcome {
            code: 0,
            stdout: format!("{out}\n"),
            stderr: String::new(),
        }
    }

    /// ConfigChange: log every settings/skills change made during a session to
    /// `.claude/audit/config-changes.log` (self-git-ignored). While a safety gate
    /// is on, a change that would weaken it is also blocked — Claude Code then
    /// keeps the settings it loaded (see [`Hook::weakened_gate`]).
    fn config_audit(&self) -> Outcome {
        let source = self.field("source");
        let file = self.field("file_path");
        let weakened = self.weakened_gate(&source, &file);
        let dir = self.project().join(".claude/audit");
        if fs::create_dir_all(&dir).is_ok() {
            let ignore = dir.join(".gitignore");
            if !ignore.exists() {
                let _ = fs::write(&ignore, "*\n");
            }
            let or = |v: &str, d: &str| {
                if v.is_empty() {
                    d.to_string()
                } else {
                    v.to_string()
                }
            };
            let mut line = format!(
                "{} {} {}",
                crate::clock::iso_stamp(),
                or(&source, "unknown"),
                or(&file, "-")
            );
            if let Some(what) = weakened {
                line.push_str(&format!(" blocked: {what}"));
            }
            line.push('\n');
            use std::io::Write as _;
            if let Ok(mut f) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("config-changes.log"))
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        match weakened {
            Some(what) => Outcome::block(format!(
                "Blocked: this settings change would weaken a safety gate ({what}), so it is not applied to this session.\n\
                 If a human made it on purpose, restart Claude Code to load it. An agent may not change the gates.\n"
            )),
            None => Outcome::allow(),
        }
    }

    /// What a settings change would weaken, judged from the changed file's new
    /// text against the gate settings in force (this hook's environment). Only
    /// while the approval gate or the WebFetch guard is on; never for policy
    /// settings (Claude Code can't block those) or skills. Weakening means:
    /// `disableAllHooks`, the approval gate switched off, a WebFetch site added,
    /// or — in the project's own ocgen-generated `settings.json` — the gate's
    /// env, its hooks or this audit hook removed, or the file deleted. Values are
    /// read from the text (every `"KEY": value` anywhere in it), the same way the
    /// script reads them without jq — so a determined rewrite (a gate's hook
    /// narrowed with a matcher, or its command changed) isn't caught; the sandbox
    /// and the permission rules are the boundary there.
    fn weakened_gate(&self, source: &str, file: &str) -> Option<&'static str> {
        if matches!(source, "policy_settings" | "skills") || file.is_empty() {
            return None;
        }
        let approval = self.env("TEAM_APPROVAL_GATE") == "1";
        let domains = self.env.get("OCGEN_WEBFETCH_DOMAINS");
        if !approval && domains.is_none() {
            return None;
        }
        let path = if file.starts_with('/') || file.as_bytes().get(1) == Some(&b':') {
            PathBuf::from(file)
        } else {
            self.project().join(file)
        };
        // The project's own settings.json, written by ocgen with the gate env.
        let ours = source == "project_settings"
            && self
                .project()
                .join(".claude/hooks/config-audit.sh")
                .is_file();
        let Ok(text) = fs::read_to_string(&path) else {
            return ours.then_some("deletes the project settings");
        };
        let text = text.replace(['\n', '\r'], " ");
        let values = |key: &str| -> Vec<String> {
            Regex::new(&format!(
                r#""{key}"[[:space:]]*:[[:space:]]*("([^"\\]|\\.)*"|[^,}}[:space:]]+)"#
            ))
            .unwrap()
            .captures_iter(&text)
            .map(|c| {
                let v = &c[1];
                v.strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(v)
                    .to_string()
            })
            .collect()
        };
        if Regex::new(r#""disableAllHooks"[[:space:]]*:[[:space:]]*true"#)
            .unwrap()
            .is_match(&text)
        {
            return Some("turns off every hook");
        }
        if approval {
            let set = values("TEAM_APPROVAL_GATE");
            if set.iter().any(|v| v != "1") || (ours && set.is_empty()) {
                return Some("turns off the approval gate");
            }
            if ours && !text.contains("team-approval-gate") {
                return Some("removes the approval gate hook");
            }
        }
        if let Some(cur) = domains {
            let trusted: Vec<String> = cur
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect();
            let added = values("OCGEN_WEBFETCH_DOMAINS").iter().any(|v| {
                v.split_whitespace()
                    .any(|d| !trusted.contains(&d.to_ascii_lowercase()))
            });
            if added {
                return Some("trusts another WebFetch site");
            }
            if ours && !text.contains("https-only-fetch") {
                return Some("removes the WebFetch guard");
            }
        }
        if ours && !text.contains("config-audit") {
            return Some("removes this audit hook");
        }
        None
    }
}

/// A shell command, the way the hook scripts run it (`sh -c` everywhere: Claude
/// Code runs hooks through bash, including Git Bash on Windows).
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", cmd]);
    c
}

// ---------------------------------------------------------------- sandbox --
//
// Claude Code runs hooks outside its sandbox, so with the project's sandbox on
// (`OCGEN_SANDBOX=1`) the project code a hook starts itself — the check, the
// formatter — runs in an OS sandbox of its own: no writes to the paths in
// `OCGEN_SANDBOX_DENY_WRITE` (nor to the withheld credentials), no reads of
// `OCGEN_SANDBOX_DENY_READ`, and `OCGEN_SANDBOX_DENY_ENV` unset. Everything else
// stays as the user has it, so it is narrower than Claude Code's sandbox (no
// network limit, writes elsewhere allowed). Seatbelt on macOS; bubblewrap on
// Linux, where a missing `bwrap` means the command doesn't run at all — never
// unsandboxed. Native Windows has no sandbox: commands run as before.

/// Where macOS keeps Seatbelt's command-line front end.
const SEATBELT: &str = "/usr/bin/sandbox-exec";

/// Why a sandboxed command can't run on Linux (also printed by the scripts).
pub const NO_BWRAP: &str = "the project's sandbox is on, but bubblewrap (bwrap) isn't installed, and ocgen never runs this command unsandboxed — install bubblewrap (e.g. sudo apt install bubblewrap); Claude Code's own sandbox needs it too";

/// Why a sandboxed command can't run on macOS (also printed by the scripts).
pub const NO_SEATBELT: &str = "the project's sandbox is on, but /usr/bin/sandbox-exec is missing, and ocgen never runs this command unsandboxed";

/// What a hook's sandbox is made from: the env's deny lists (sandbox path
/// syntax: `~/` the home folder, `/` absolute, anything else relative), the home
/// folder, the project, and the folder the command runs in. Relative entries
/// name a path in the project and the same path in that folder.
pub struct SandboxSpec<'a> {
    pub deny_write: &'a str,
    pub deny_read: &'a str,
    pub home: &'a str,
    pub project: &'a Path,
    pub dir: &'a Path,
}

impl SandboxSpec<'_> {
    /// The project and the command's folder, resolved.
    fn bases(&self) -> (String, String) {
        (
            real_path(&self.project.to_string_lossy()),
            real_path(&self.dir.to_string_lossy()),
        )
    }

    /// The paths `entries` name, each once (as `sb_paths` in the scripts):
    /// `~/x` in the home folder, `/x` as is, a relative one in `base` — with
    /// `relative_only`, only the relative ones. On Linux an entry in `.claude/`
    /// stands for `.claude` itself: bubblewrap can only protect what exists, and
    /// a whole read-only `.claude` also keeps a missing settings file from being
    /// created.
    fn paths(&self, entries: &str, base: &str, relative_only: bool, linux: bool) -> Vec<String> {
        let mut out = Vec::new();
        for e in entries.split_ascii_whitespace() {
            let raw = if e == "~" || e.starts_with("~/") {
                if relative_only || self.home.is_empty() {
                    continue;
                }
                format!("{}{}", self.home, &e[1..])
            } else if e.starts_with('/') {
                if relative_only {
                    continue;
                }
                e.to_string()
            } else {
                let mut rel = e.strip_prefix("./").unwrap_or(e);
                if linux && rel.starts_with(".claude/") {
                    rel = ".claude";
                }
                format!("{base}/{rel}")
            };
            push_new(&mut out, real_path(&raw));
        }
        out
    }

    /// The paths to write-protect (every entry, in the project), the ones to
    /// write-protect in the command's own folder (relative entries, when it
    /// isn't the project), and the ones to hide (both).
    fn targets(&self, linux: bool) -> (Vec<String>, Vec<String>, Vec<String>) {
        let (p, d) = self.bases();
        let write = self.paths(self.deny_write, &p, false, linux);
        let mut inner = Vec::new();
        let mut read = self.paths(self.deny_read, &p, false, linux);
        if d != p {
            inner = self.paths(self.deny_write, &d, true, linux);
            for r in self.paths(self.deny_read, &d, true, linux) {
                push_new(&mut read, r);
            }
        }
        (write, inner, read)
    }
}

/// The Seatbelt profile (`sandbox-exec -p`) for a command: writes denied to the
/// write-protected paths and to the withheld credentials, reads denied to the
/// credentials, and no folder above any of them renamed or removed (which would
/// move a protected path out of the way).
pub fn seatbelt_profile(spec: &SandboxSpec) -> String {
    let (write, inner, read) = spec.targets(false);
    let mut all = Vec::new();
    for t in write.iter().chain(&inner).chain(&read) {
        push_new(&mut all, t.clone());
    }
    let clause = |op: &str, filter: &str, paths: &[String]| {
        if paths.is_empty() {
            return String::new();
        }
        let list: String = paths
            .iter()
            .map(|p| {
                format!(
                    " ({filter} \"{}\")",
                    p.replace('\\', "\\\\").replace('"', "\\\"")
                )
            })
            .collect();
        format!("(deny {op}{list})")
    };
    format!(
        "(version 1)(allow default){}{}{}",
        clause("file-write*", "subpath", &all),
        clause("file-write-unlink", "literal", &ancestors(&all)),
        clause("file-read*", "subpath", &read)
    )
}

/// The bubblewrap options for a command, in order: the whole system as it is;
/// every folder above a protected path bound onto itself (a mount point can't be
/// renamed away); the write-protected paths that exist read-only — then, when
/// the command's folder lies inside one (a worker's worktree in `.claude/`), that
/// folder writable again, and its own protected paths read-only; last, an empty
/// folder over each withheld credential folder and /dev/null over each file.
pub fn bwrap_args(spec: &SandboxSpec) -> Vec<String> {
    let (write, inner, read) = spec.targets(true);
    let (p, d) = spec.bases();
    let exists = |t: &str| Path::new(t).exists();
    let mut args: Vec<String> = ["--dev-bind", "/", "/"].map(String::from).to_vec();
    let mut all = write.clone();
    all.extend(inner.iter().cloned());
    all.extend(read.iter().cloned());
    for a in ancestors(&all) {
        if Path::new(&a).is_dir() {
            args.extend(["--bind".into(), a.clone(), a]);
        }
    }
    let mut reopen = false;
    for t in write.iter().filter(|t| exists(t)) {
        args.extend(["--ro-bind".into(), t.clone(), t.clone()]);
        reopen |= d != p && format!("{d}/").starts_with(&format!("{t}/"));
    }
    if reopen {
        args.extend(["--bind".into(), d.clone(), d]);
    }
    for t in inner.iter().filter(|t| exists(t)) {
        args.extend(["--ro-bind".into(), t.clone(), t.clone()]);
    }
    for r in read {
        if Path::new(&r).is_dir() {
            args.extend(["--tmpfs".into(), r]);
        } else if exists(&r) {
            args.extend(["--ro-bind".into(), "/dev/null".into(), r]);
        }
    }
    args
}

/// Every folder above each path (shallowest first, once), but not `/`.
fn ancestors(paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for t in paths {
        let parts: Vec<&str> = t.split('/').collect();
        let mut p = String::new();
        for c in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
            p.push('/');
            p.push_str(c);
            push_new(&mut out, p.clone());
        }
    }
    out
}

fn push_new(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// A path as the kernel sees it: its longest existing part with every symlink
/// resolved, then the rest as written (as `sb_real` in the scripts computes it).
/// Only Unix sandboxes; elsewhere the path stays as written.
fn real_path(path: &str) -> String {
    if !cfg!(unix) {
        return path.to_string();
    }
    let mut full = PathBuf::from(path);
    if full.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            full = cwd.join(full);
        }
    }
    let mut rest = Vec::new();
    let mut cur = full.as_path();
    loop {
        if let Ok(mut real) = fs::canonicalize(cur) {
            for r in rest.iter().rev() {
                real.push(r);
            }
            return real.to_string_lossy().into_owned();
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return full.to_string_lossy().into_owned(),
        }
    }
}

/// An environment variable's name (what `unset` takes).
fn env_name(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| !b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A program on `path` (a `PATH` value), as `command -v` finds it.
fn on_path(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':').filter(|d| !d.is_empty()).find_map(|d| {
        let p = Path::new(d).join(name);
        #[cfg(unix)]
        let runnable = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let runnable = p.is_file();
        runnable.then_some(p)
    })
}

impl Hook<'_> {
    /// How a hook runs project code (`cmd`, through `sh -c`, in `dir`): in the
    /// sandbox when the project's is on and the system has one, else as is.
    /// `Err` says why it can't run at all: the sandbox is on, but its tool isn't
    /// there.
    fn project_command(&self, cmd: &str, dir: &Path) -> Result<Command, &'static str> {
        if self.env("OCGEN_SANDBOX") != "1" || !cfg!(any(target_os = "macos", target_os = "linux"))
        {
            return Ok(shell(cmd));
        }
        let project = self.project();
        let spec = SandboxSpec {
            deny_write: self.env("OCGEN_SANDBOX_DENY_WRITE"),
            deny_read: self.env("OCGEN_SANDBOX_DENY_READ"),
            home: self.env("HOME"),
            project: &project,
            dir,
        };
        let mut c = if cfg!(target_os = "macos") {
            if !Path::new(SEATBELT).is_file() {
                return Err(NO_SEATBELT);
            }
            let mut c = Command::new(SEATBELT);
            c.arg("-p").arg(seatbelt_profile(&spec));
            c
        } else {
            let bwrap = on_path("bwrap", self.env("PATH")).ok_or(NO_BWRAP)?;
            let mut c = Command::new(bwrap);
            c.args(bwrap_args(&spec));
            c
        };
        c.args(["/bin/sh", "-c", cmd]);
        for v in self
            .env("OCGEN_SANDBOX_DENY_ENV")
            .split_ascii_whitespace()
            .filter(|v| env_name(v))
        {
            c.env_remove(v);
        }
        Ok(c)
    }
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

/// `tr -c 'A-Za-z0-9._-' '_' | cut -c1-80` in the C locale the scripts run in:
/// byte by byte.
fn safe(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                b as char
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

impl Guard {
    fn new(h: &Hook, gate: &str, subject: &str) -> Option<Self> {
        let max: u32 = h.env_num("LOOP_GUARD_MAX_BLOCKS", 0);
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

/// A file tool's path, canonical for matching (the gate script's `canon`): `\` as
/// `/`, made absolute against `cwd` when relative, `//` squeezed, `.` and `..`
/// resolved lexically, ASCII-lowercased (APFS and NTFS ignore case; on a
/// case-sensitive disk this only blocks more). Always starts with `/`; empty for
/// an empty path.
fn canonical_path(path: &str, cwd: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let p = path.replace('\\', "/");
    let absolute = p.starts_with('/') || p.starts_with('~') || p.as_bytes().get(1) == Some(&b':');
    let full = if absolute || cwd.is_empty() {
        p
    } else {
        format!("{}/{p}", cwd.replace('\\', "/"))
    };
    let mut out: Vec<&str> = Vec::new();
    for seg in full.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/")).to_ascii_lowercase()
}

fn read_num64(p: &Path) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// Seconds since the Unix epoch (`date +%s`).
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether `role` is in the space-separated `list` (any case).
fn listed(list: &str, role: &str) -> bool {
    !role.is_empty()
        && list
            .split_whitespace()
            .any(|r| r.eq_ignore_ascii_case(role))
}

// ------------------------------------------------------------- confidence --

/// The confidence an agent stated: the last line that starts with `Confidence`
/// and a percentage (`Confidence: 97%`, `- **Confidence** — 97%`, `_Confidence_:
/// 97%`), 0–100 — only markup and punctuation around the word. Read
/// byte by byte, as the sh twins do in the C locale, so a non-ASCII separator
/// counts the same in both. Prose (`low confidence in 3 cases`) never matches.
fn stated_confidence(text: &str) -> Option<u32> {
    let re = regex::bytes::Regex::new(
        r"(?mi-u)^[^0-9A-Za-z\n]*confidence[^0-9A-Za-z\n]{0,8}([0-9]{1,3})[ \t]*%",
    )
    .unwrap();
    let last = re.captures_iter(text.as_bytes()).last()?;
    let n: u32 = std::str::from_utf8(&last[1]).ok()?.parse().ok()?;
    (n <= 100).then_some(n)
}

/// A confidence marker file's score: its first integer, if it has at most three
/// digits and is at most 100.
fn marker_confidence(body: &str) -> Option<u32> {
    let digits: String = body
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    let n: u32 = digits.parse().ok().filter(|_| digits.len() <= 3)?;
    (n <= 100).then_some(n)
}

/// How long a team check lock may stay unstamped (no `at`) before it is taken over.
const UNSTAMPED_LOCK_SECS: u64 = 3;

/// How long the task gate waits for Claude Code to write a teammate's final
/// message before reading its transcript (Claude Code flushes every 100 ms).
const TRANSCRIPT_SETTLE_MS: u64 = 300;

/// `session-<first 8 bytes of the session id>`: the team Claude Code makes for a
/// lead session that names none.
fn implicit_team(session: &str) -> String {
    let head: Vec<u8> = session.bytes().take(8).collect();
    format!("session-{}", String::from_utf8_lossy(&head))
}

/// What the lead's `subagents/` folder holds for an in-process teammate.
#[derive(Default)]
struct Sidechain {
    /// Its own transcript (the newest, when it was spawned more than once).
    own: Option<PathBuf>,
    /// Its role (`customAgentType`), from that transcript's meta.
    role: String,
    /// Whether this folder holds teammates' transcripts (of its team or any
    /// other: only a lead has teammates) — so the transcript beside it is the
    /// lead's.
    lead: bool,
}

/// An in-process teammate's own transcript and role. Claude Code 2.1.277 files
/// its transcript beside the lead's (`<transcript>/subagents/`, or a folder
/// below) as `agent-<agent id>.jsonl`, with an `agent-<agent id>.meta.json`
/// that names the teammate (`name`), its team (`teamName`) and its role
/// (`customAgentType`). A plain subagent's meta names no team.
fn sidechain(tp: &str, mate: &str, team: &str) -> Sidechain {
    let mut out = Sidechain::default();
    if tp.is_empty() || mate.is_empty() || team.is_empty() {
        return out;
    }
    let sub = PathBuf::from(tp.strip_suffix(".jsonl").unwrap_or(tp)).join("subagents");
    let sorted = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    // The shell's glob order: `subagents/*.meta.json`, then `subagents/*/*.meta.json`.
    let top = sorted(&sub);
    let metas = top
        .iter()
        .filter(|p| p.is_file())
        .cloned()
        .chain(top.iter().filter(|p| p.is_dir()).flat_map(|d| sorted(d)))
        .filter(|p| p.to_string_lossy().ends_with(".meta.json") && p.is_file());
    let mut newest: Option<std::time::SystemTime> = None;
    for meta in metas {
        let Some(v) = fs::read_to_string(&meta)
            .ok()
            .and_then(|m| serde_json::from_str::<Value>(&m).ok())
        else {
            continue;
        };
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let of = text("teamName");
        if of.is_empty() {
            continue;
        }
        out.lead = true;
        if of != team || text("name") != mate {
            continue;
        }
        let m = meta.to_string_lossy();
        let file = PathBuf::from(format!("{}.jsonl", &m[..m.len() - ".meta.json".len()]));
        match fs::metadata(&file).and_then(|md| md.modified()) {
            Ok(at) if newest.is_none_or(|n| at > n) => {
                newest = Some(at);
                out.own = Some(file);
                out.role = text("customAgentType");
            }
            Ok(_) => {}
            Err(_) if out.role.is_empty() => out.role = text("customAgentType"),
            Err(_) => {}
        }
    }
    out
}

/// The text of the last assistant message in a Claude Code transcript (JSONL,
/// one content block per line): its text blocks, joined by newlines.
fn last_assistant_text(path: &Path) -> String {
    let Ok(body) = fs::read(path) else {
        return String::new();
    };
    let mut last = String::new();
    for line in String::from_utf8_lossy(&body).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let texts: Vec<&str> = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        if !texts.is_empty() {
            last = texts.join("\n");
        }
    }
    last
}

// -------------------------------------------------------------- the checks --

/// The check for one team task. A task may narrow the project check with a
/// `Check:` line in its description — the project check plus plain arguments
/// (`Check: cargo test -p parser`). The check runs outside the agent's
/// permissions, so a line that is anything else is ignored: the project check runs.
fn task_check(base: &str, desc: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    let line = desc.lines().find_map(|l| {
        l.trim_start_matches([' ', '\t'])
            .strip_prefix("Check:")
            .map(|c| {
                c.trim_start_matches([' ', '\t'])
                    .trim_end_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\x0b')
            })
    });
    let plain = |rest: &str| {
        rest.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=,@+% -".contains(c))
    };
    match line {
        Some(c)
            if c.strip_prefix(base)
                .and_then(|r| r.strip_prefix(' '))
                .is_some_and(plain) =>
        {
            c.to_string()
        }
        _ => base.to_string(),
    }
}

/// Kill a timed-out check and everything it started: its process group on Unix
/// (it was spawned as the group's leader), its process tree on Windows.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: a plain signal to the process group we created.
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ------------------------------------------------------------ worker state --

/// Where a work tree stands: its top level, HEAD, one checksum (`cksum`, as the
/// sh twin computes it) of everything uncommitted, and whether there is any.
struct TreeState {
    top: String,
    head: String,
    sum: String,
    dirty: bool,
}

/// The state of the work tree at `dir`; `None` outside a git work tree. The sum
/// covers, in this order: `git status --porcelain --untracked-files=all`, the
/// staged and the unstaged diff (both work before the first commit), and the
/// blob ids of the untracked, not ignored files (`git ls-files -o
/// --exclude-standard`, regular files it can open only, hashed by `git
/// hash-object --stdin-paths` at the top level) — so editing a file git doesn't
/// track yet changes it too.
fn tree_state(dir: &Path) -> Option<TreeState> {
    // In the C locale, as the sh twin runs it: `git diff` translates some lines.
    let git_in = |at: &Path, args: &[&str]| {
        let mut c = Command::new("git");
        c.arg("-C").arg(at).args(args).env("LC_ALL", "C");
        c
    };
    let git = |args: &[&str]| git_in(dir, args).stdin(Stdio::null()).output().ok();
    let line = |o: Option<std::process::Output>| {
        o.map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim_end_matches(['\n', '\r'])
                .to_string()
        })
        .unwrap_or_default()
    };
    let top = git(&["rev-parse", "--show-toplevel"]).filter(|o| o.status.success());
    top.as_ref()?;
    let top = line(top);
    let head = line(git(&["rev-parse", "--verify", "-q", "HEAD"]));
    let out = |args: &[&str]| git(args).map(|o| o.stdout).unwrap_or_default();
    let status = out(&[
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=all",
    ]);
    let dirty = !status.is_empty();
    let mut all = status;
    for staged in [true, false] {
        let mut args = vec!["--no-optional-locks", "diff"];
        if staged {
            args.push("--cached");
        }
        args.extend(["--no-ext-diff", "--no-color"]);
        all.extend(out(&args));
    }
    // The untracked files' content, one path per line as the script passes them
    // (a name with a newline is split, the same way, into names that aren't
    // files), keeping regular files it can open: `hash-object` stops at the
    // first path it can't read (a nested repository, a dangling link, a file
    // nobody may read).
    let root = Path::new(&top);
    let listed = git_in(
        root,
        &[
            "--no-optional-locks",
            "ls-files",
            "-o",
            "--exclude-standard",
            "-z",
        ],
    )
    .stdin(Stdio::null())
    .output()
    .map(|o| o.stdout)
    .unwrap_or_default();
    let mut paths: Vec<u8> = Vec::new();
    for name in listed.split(|&b| b == 0 || b == b'\n') {
        let file = root.join(path_of(name));
        if !name.is_empty() && file.is_file() && fs::File::open(&file).is_ok() {
            paths.extend_from_slice(name);
            paths.push(b'\n');
        }
    }
    if !paths.is_empty() {
        let hashed = git_in(root, &["hash-object", "--no-filters", "--stdin-paths"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut child| {
                let mut input = child.stdin.take()?;
                // Fed from another thread: git writes as it reads, and a full
                // pipe on either side would otherwise wait on the other.
                let feed = std::thread::spawn(move || {
                    use std::io::Write as _;
                    let _ = input.write_all(&paths);
                });
                let o = child.wait_with_output().ok();
                let _ = feed.join();
                o
            });
        all.extend(hashed.map(|o| o.stdout).unwrap_or_default());
    }
    Some(TreeState {
        top,
        head,
        sum: cksum(&all),
        dirty,
    })
}

/// A path git printed, as raw bytes (on Windows git prints UTF-8).
fn path_of(name: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(std::ffi::OsStr::from_bytes(name))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(name).into_owned())
    }
}

/// Record the tree a worker starts from, in a self-ignored folder (so the record
/// itself never shows up in `git status`).
fn record_baseline(path: &Path, s: &TreeState) {
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = fs::write(&ignore, "*\n");
    }
    let _ = fs::write(path, format!("{}\n{}\n{}\n", s.top, s.head, s.sum));
}

/// Whether a worker changed its tree since `baseline` was recorded. In the same
/// tree: HEAD, status or diff moved. In another one (an isolated worktree made
/// after the record): uncommitted changes, or a HEAD past the starting commit.
/// Without a record, any uncommitted change counts.
fn worker_changed(cwd: &Path, baseline: Option<&Path>) -> bool {
    let recorded = baseline.and_then(|b| fs::read_to_string(b).ok());
    let now = tree_state(cwd);
    match (recorded, now) {
        (Some(r), Some(n)) => {
            let mut l = r.lines();
            let (top, head, sum) = (l.next(), l.next(), l.next());
            if top == Some(n.top.as_str()) {
                sum != Some(n.sum.as_str()) || head != Some(n.head.as_str())
            } else {
                n.dirty || head != Some(n.head.as_str())
            }
        }
        (_, n) => n.is_some_and(|n| n.dirty),
    }
}

/// POSIX `cksum`: the CRC-32 (polynomial 0x04C11DB7, length appended) and the
/// byte count, as `cksum` prints them.
fn cksum(data: &[u8]) -> String {
    fn step(crc: u32, b: u8) -> u32 {
        let mut c = crc ^ (u32::from(b) << 24);
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            };
        }
        c
    }
    let mut crc = data.iter().fold(0, |c, &b| step(c, b));
    let mut len = data.len() as u64;
    while len > 0 {
        crc = step(crc, (len & 0xff) as u8);
        len >>= 8;
    }
    format!("{} {}", !crc, data.len())
}

// ----------------------------------------------------------- plan approval --

const PLAN_NOT_APPROVED: &str =
    "Plan not approved yet. Run /team-plan and get the plan APPROVED in\n\
                                 .claude/team/plan.md before creating execution tasks.\n";
const PLAN_OTHER_RUN: &str =
    "Plan approval is stale: .claude/team/plan.md was approved for another team run.\n\
     Run /team-plan for this goal and get it approved again before creating execution tasks.\n";
const PLAN_CHANGED: &str =
    "Plan approval is stale: .claude/team/plan.md changed after it was approved.\n\
     Get the changed plan approved, then replace its Status line with a fresh 'Status: APPROVED'.\n";
const PLAN_OTHER_SESSION: &str =
    "Plan approval is stale: .claude/team/plan.md was approved in another session.\n\
     Run /team-plan in this session and get it approved again before creating execution tasks.\n";

/// A file's lines as `awk` reads them: split on `\n`, a final newline ending
/// the last line rather than starting an empty one.
fn split_lines(body: &str) -> Vec<String> {
    let mut lines: Vec<String> = body.split('\n').map(String::from).collect();
    if body.ends_with('\n') || body.is_empty() {
        lines.pop();
    }
    lines
}

/// The plan's checksum, as `<crc>-<bytes>`, without its Status lines and without
/// the state the team changes as it works: a task's checkbox (`- [x]` reads as
/// `- [ ]`) and a risk's mitigation (its line from `Mitigation:` on, which the
/// risk gate has the owner close). The script's `grep -v '^Status:' | sed … |
/// cksum`.
fn plan_sum(lines: &[String]) -> String {
    let checkbox =
        Regex::new(r"^([[:space:]]*(([-*+]|[0-9]+[.)])[[:space:]]+)?)\[[xX ]\]").unwrap();
    let mitigation = Regex::new(r"[Mm][Ii][Tt][Ii][Gg][Aa][Tt][Ii][Oo][Nn]:.*").unwrap();
    let body: String = lines
        .iter()
        .filter(|l| !l.starts_with("Status:"))
        .map(|l| {
            let l = checkbox.replace(l, "${1}[ ]");
            format!("{}\n", mitigation.replace(&l, "Mitigation:"))
        })
        .collect();
    cksum(body.as_bytes()).replace(' ', "-")
}

// ------------------------------------------------------------ drop-noop-cd --

/// A folder path for comparing: no trailing `/` (but `/` stays). On Windows also
/// `/` separators, MSYS `/c/…` as `c:/…` (`c:/` stays) and lowercase.
fn norm_folder(p: &str, windows: bool) -> String {
    let mut s = if windows {
        p.replace('\\', "/")
    } else {
        p.to_string()
    };
    let b = s.as_bytes();
    if windows
        && b.len() >= 2
        && b[0] == b'/'
        && b[1].is_ascii_alphabetic()
        && (b.len() == 2 || b[2] == b'/')
    {
        s = format!(
            "{}:/{}",
            b[1] as char,
            s[2.min(s.len())..].trim_start_matches('/')
        );
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    let b = s.as_bytes();
    let drive = windows && b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if drive && s.len() == 2 {
        s.push('/');
    }
    if drive {
        s.make_ascii_lowercase();
    }
    s
}

/// Whether `cd target` from `cwd` stays put: `.`/`./`, or an absolute path that
/// is the same folder by text or, failing that, once both are resolved.
fn same_folder(target: &str, cwd: &str, windows: bool) -> bool {
    if target == "." || target == "./" {
        return true;
    }
    if target.contains("\\\\") || cwd.is_empty() {
        return false;
    }
    let (t, c) = (norm_folder(target, windows), norm_folder(cwd, windows));
    let b = t.as_bytes();
    let absolute = t.starts_with('/') || (windows && b.len() >= 3 && b[1] == b':' && b[2] == b'/');
    if !absolute {
        return false;
    }
    if t == c {
        return true;
    }
    let real = |p: &str| {
        std::fs::canonicalize(p).ok().map(|r| {
            let r = if windows {
                crate::paths::for_shell(&r)
            } else {
                crate::paths::plain(&r).to_string_lossy().into_owned()
            };
            norm_folder(&r, windows)
        })
    };
    matches!((real(target), real(cwd)), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `~` inside a path is literal (the shell expands only a leading one), and
    /// Windows short names have one: `C:\\Users\\RUNNER~1\\…` on CI runners.
    #[test]
    fn noop_cd_takes_a_tilde_inside_a_path_but_not_a_leading_one() {
        let env: HashMap<String, String> = HashMap::new();
        let run = |cmd: &str, cwd: &str| {
            let payload = serde_json::json!({
                "tool_name": "Bash", "tool_input": { "command": cmd }, "cwd": cwd
            })
            .to_string();
            run("drop-noop-cd", &payload, &env).stdout
        };
        let out = run("cd /p/RUNNER~1/proj && ls", "/p/RUNNER~1/proj");
        assert!(out.contains(r#""command":"ls""#), "{out:?}");
        for cmd in ["cd ~ && ls", "cd ~/proj && ls", "cd ~me/proj && ls"] {
            assert_eq!(run(cmd, "/p/RUNNER~1/proj"), "", "{cmd}");
        }
    }

    #[test]
    fn stated_confidence_reads_only_a_line_that_states_it() {
        for (text, want) in [
            ("Confidence: 97%", Some(97)),
            ("done\n**Confidence** — 97%", Some(97)),
            ("- Confidence ≈ 097 %", Some(97)),
            ("Confidence: 97%\nConfidence: 80%", Some(80)),
            ("Confidence: 60% (target 96%)", Some(60)),
            ("low confidence in 3 edge cases", None),
            ("done. Confidence: 97%", None),
            ("Confidence: 9/10", None),
            ("Confidence: 150%", None),
            ("Confidence: 1000%", None),
            ("Confidence:\n97%", None),
            ("python eval.py --min-confidence 0.8", None),
            ("_Confidence_: 97%", Some(97)),
            ("__Confidence__ — 97%", Some(97)),
            ("min_confidence: 97%", None),
            ("confidence_threshold: 97%", None),
        ] {
            assert_eq!(stated_confidence(text), want, "{text:?}");
        }
    }

    #[test]
    fn an_adversary_report_states_no_confidence_unless_it_ends_with_one() {
        // A finding's own confidence sits mid-line, so it is never the report's.
        let report = "Verdict: REWORK\n\
            Findings:\n\
            - [High] src/unpack.rs:88 — no canonical path — `../../.bashrc` — writes outside — canonicalize — Verified\n\
            - [Low] src/cli.rs:120 — full path in errors — bad `--path` — leaks layout — relative path — Inferred (Confidence: 70%)\n\
            Challenged claims:\n\
            - implementer: «Confidence: 97%, all tests pass» — refuted — `cargo test unpack` fails\n\
            Not checked: Windows";
        assert_eq!(stated_confidence(report), None);
        assert_eq!(
            stated_confidence(&format!("{report}\nConfidence: 90%")),
            Some(90)
        );
    }

    #[test]
    fn marker_confidence_takes_the_first_integer() {
        assert_eq!(marker_confidence("97"), Some(97));
        assert_eq!(marker_confidence("Confidence: 60% (target 96%)"), Some(60));
        assert_eq!(marker_confidence("97% verified 2026-10-02"), Some(97));
        assert_eq!(marker_confidence("9720261002"), None);
        assert_eq!(marker_confidence("none"), None);
    }

    #[test]
    fn task_check_only_narrows_the_project_check() {
        let base = "cargo test";
        for (desc, want) in [
            ("Check: cargo test -p parser", "cargo test -p parser"),
            (
                "Fix it.\n  Check:  cargo test --test api_v2  \n",
                "cargo test --test api_v2",
            ),
            ("Check: cargo test", "cargo test"),
            ("Check: cargo test; rm -rf ~", "cargo test"),
            ("Check: cargo test $(id)", "cargo test"),
            ("Check: cargo testx", "cargo test"),
            ("Check: rm -rf build", "cargo test"),
            ("no line", "cargo test"),
        ] {
            assert_eq!(task_check(base, desc), want, "{desc:?}");
        }
        assert_eq!(task_check("", "Check: rm -rf build"), "");
    }

    #[test]
    fn cksum_matches_posix() {
        assert_eq!(cksum(b""), "4294967295 0");
        assert_eq!(cksum(b"a\n"), "2418082923 2");
    }

    /// Paths that exist nowhere, so the builders' output is the same on every OS.
    const NOWHERE: &str = "/ocgen-sandbox-test-nowhere";

    fn spec<'a>(
        write: &'a str,
        read: &'a str,
        home: &'a str,
        p: &'a Path,
        d: &'a Path,
    ) -> SandboxSpec<'a> {
        SandboxSpec {
            deny_write: write,
            deny_read: read,
            home,
            project: p,
            dir: d,
        }
    }

    #[test]
    fn sandbox_entries_name_home_absolute_and_project_paths() {
        let home = format!("{NOWHERE}/home");
        let p = PathBuf::from(format!("{NOWHERE}/proj"));
        let d = PathBuf::from(format!("{NOWHERE}/proj/.claude/worktrees/w1"));
        let entries = format!("~/.claude ./.claude/hooks .git/hooks {NOWHERE}/abs ~/.claude");
        let s = spec(&entries, "~/.ssh", &home, &p, &d);
        let (write, inner, read) = s.targets(false);
        let at = |s: &str| format!("{NOWHERE}/{s}");
        assert_eq!(
            write,
            [
                at("home/.claude"),
                at("proj/.claude/hooks"),
                at("proj/.git/hooks"),
                at("abs")
            ],
            "each once, in order"
        );
        // The command's own folder gets the relative entries only.
        assert_eq!(
            inner,
            [
                at("proj/.claude/worktrees/w1/.claude/hooks"),
                at("proj/.claude/worktrees/w1/.git/hooks")
            ]
        );
        assert_eq!(read, [at("home/.ssh")]);
        // On Linux an entry in .claude/ stands for .claude itself.
        let (write, _, _) = s.targets(true);
        assert_eq!(write[1], at("proj/.claude"));
        // No home: no `~/` entries. The project itself: no inner list.
        let s = spec("~/.claude ./.mcp.json", "~/.ssh", "", &p, &p);
        assert_eq!(
            s.targets(false),
            (vec![at("proj/.mcp.json")], vec![], vec![])
        );
    }

    #[test]
    fn the_seatbelt_profile_denies_writes_reads_and_renames() {
        let home = format!("{NOWHERE}/h\"q");
        let p = PathBuf::from(format!("{NOWHERE}/p"));
        let s = spec("~/.claude ./.git/hooks", "~/.ssh", &home, &p, &p);
        let n = NOWHERE;
        assert_eq!(
            seatbelt_profile(&s),
            format!(
                "(version 1)(allow default)\
                 (deny file-write* (subpath \"{n}/h\\\"q/.claude\") (subpath \"{n}/p/.git/hooks\") (subpath \"{n}/h\\\"q/.ssh\"))\
                 (deny file-write-unlink (literal \"{n}\") (literal \"{n}/h\\\"q\") (literal \"{n}/p\") (literal \"{n}/p/.git\"))\
                 (deny file-read* (subpath \"{n}/h\\\"q/.ssh\"))"
            )
        );
        // Nothing to protect: just the default.
        assert_eq!(
            seatbelt_profile(&spec("", "", &home, &p, &p)),
            "(version 1)(allow default)"
        );
    }

    #[test]
    fn bwrap_leaves_out_what_does_not_exist() {
        let p = PathBuf::from(format!("{NOWHERE}/p"));
        let s = spec("~/.claude ./.claude/x", "~/.ssh", NOWHERE, &p, &p);
        assert_eq!(bwrap_args(&s), ["--dev-bind", "/", "/"]);
    }

    #[cfg(unix)]
    #[test]
    fn bwrap_protects_what_exists_and_reopens_a_worker_folder_inside_it() {
        let t = tempfile::tempdir().unwrap();
        let real = |rel: &str| format!("{}/{rel}", real_path(&t.path().to_string_lossy()));
        for dir in [
            "home/.claude/ocgen",
            "home/.ssh",
            "p/.claude/worktrees/w1/.claude",
            "p/.git/hooks",
        ] {
            fs::create_dir_all(t.path().join(dir)).unwrap();
        }
        fs::write(t.path().join("home/.npmrc"), "").unwrap();
        let home = t.path().join("home").to_string_lossy().into_owned();
        let p = t.path().join("p");
        let d = p.join(".claude/worktrees/w1");
        let s = spec(
            "~/.claude ./.claude/settings.json ./.git/hooks ./.mcp.json",
            "~/.ssh ~/.npmrc ~/.aws",
            &home,
            &p,
            &d,
        );
        let args = bwrap_args(&s);
        let pos = |a: &str, b: &str| {
            args.windows(2)
                .rposition(|w| w[0] == a && w[1] == b)
                .unwrap_or_else(|| panic!("no {a} {b} in {args:?}"))
        };
        assert_eq!(args[..3], ["--dev-bind", "/", "/"]);
        // Folders above protected paths can't be renamed away.
        let above = pos("--bind", &real("p/.claude/worktrees"));
        // The protected paths that exist: .claude whole on Linux.
        let store = pos("--ro-bind", &real("home/.claude"));
        let dot_claude = pos("--ro-bind", &real("p/.claude"));
        pos("--ro-bind", &real("p/.git/hooks"));
        assert!(!args.iter().any(|a| a.ends_with(".mcp.json")), "{args:?}");
        // The worker's folder writable again, after, then its own .claude.
        let reopen = pos("--bind", &real("p/.claude/worktrees/w1"));
        let own = pos("--ro-bind", &real("p/.claude/worktrees/w1/.claude"));
        assert!(
            above < store && dot_claude < reopen && reopen < own,
            "{args:?}"
        );
        // Credentials: an empty folder, /dev/null over a file, nothing for one
        // that isn't there.
        pos("--tmpfs", &real("home/.ssh"));
        assert!(args
            .windows(3)
            .any(|w| w == ["--ro-bind", "/dev/null", &real("home/.npmrc")]));
        assert!(!args.iter().any(|a| a.ends_with(".aws")));
    }

    #[cfg(unix)]
    #[test]
    fn real_path_resolves_symlinks_and_keeps_a_missing_tail() {
        let t = tempfile::tempdir().unwrap();
        let base = real_path(&t.path().to_string_lossy());
        fs::create_dir_all(t.path().join("dotfiles/claude")).unwrap();
        std::os::unix::fs::symlink(t.path().join("dotfiles/claude"), t.path().join(".claude"))
            .unwrap();
        let link = format!("{}/.claude/ocgen/approvals", t.path().display());
        assert_eq!(
            real_path(&link),
            format!("{base}/dotfiles/claude/ocgen/approvals")
        );
        assert_eq!(
            real_path(&format!("{}//x/", t.path().display())),
            format!("{base}/x")
        );
    }

    #[test]
    fn sandbox_helpers() {
        assert!(env_name("GH_TOKEN") && env_name("_x1"));
        assert!(!env_name("") && !env_name("1A") && !env_name("A-B") && !env_name("A B"));
        assert_eq!(
            ancestors(&["/a/b/c".into(), "/a/d".into(), "/e".into()]),
            ["/a", "/a/b"]
        );
        assert_eq!(on_path("ocgen-no-such-tool", "/nowhere:"), None);
    }
}
