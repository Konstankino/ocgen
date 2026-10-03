//! End to end: the guardrails as Claude Code meets them. The other gate tests
//! call `ocgen::hooks::run` or a `.claude/hooks/*.sh` script directly; these run
//! the hook *commands* of a generated project's `.claude/settings.json` verbatim
//! — `bash -c`, the event JSON on stdin, the settings' `env`, `CLAUDE_PROJECT_DIR`
//! — each routed by its matcher, the way Claude Code does.
//!
//! Every scenario runs twice: with this build's `ocgen` on PATH, so the command
//! takes the Rust hooks, and with no `ocgen` on PATH, so it falls back to the
//! scripts. Both must reach the verdict the scenario expects.
//!
//! Unix only: on Windows the same commands run through Git Bash, and
//! `tests/hooks.rs` and `tests/gate_hardening.rs` cover the gates there.
#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ocgen::agent;
use ocgen::approval;
use ocgen::claude::Team;
use ocgen::manifest::Manifest;
use ocgen::paths::for_shell;
use ocgen::render::Project;
use ocgen::target::Target;
use regex::Regex;
use serde_json::{json, Value};
use tempfile::TempDir;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// This build's `ocgen` on PATH: the hook command runs `ocgen hook <name>`.
    Binary,
    /// No `ocgen` on PATH: the hook command runs `.claude/hooks/<name>.sh`.
    Scripts,
}
const MODES: [Mode; 2] = [Mode::Binary, Mode::Scripts];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Verdict {
    Allow,
    /// Exit 2, or a `deny` decision.
    Block,
    /// A `deny` that also stops the agent (`continue: false`).
    Halt,
}
use Verdict::*;

const PASS: &str = "#!/bin/sh\nexit 0\n";
const FAIL: &str = "#!/bin/sh\necho 'test parser::it_works ... FAILED'\nexit 1\n";

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {o:?}");
    o
}

/// A git repository with one commit, by a throwaway identity.
fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "e2e"]);
    git(dir, &["config", "user.email", "e2e@example.invalid"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
}

/// A generated project with every gate on, in its own git repository (which
/// also installs the pre-push hook), and a private HOME for the approvals.
struct Fixture {
    dir: TempDir,
    home: TempDir,
    settings: Value,
    mode: Mode,
}

fn project(mode: Mode) -> Fixture {
    project_with(mode, |_| {})
}

fn project_with(mode: Mode, tune: impl FnOnce(&mut Project)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "e2e".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 95,
        risk_rounds: true,
        approval_gate: true,
    };
    p.claude.workflow.check_cmd = "sh ./check.sh".into();
    p.claude.hooks_extra.config_audit = true;
    tune(&mut p);
    p.scaffold(dir.path(), false).unwrap();
    fs::write(dir.path().join("check.sh"), PASS).unwrap();
    let settings = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    Fixture {
        dir,
        home: tempfile::tempdir().unwrap(),
        settings,
        mode,
    }
}

/// PATH without any `ocgen`; with this build's first in [`Mode::Binary`].
fn path_for(mode: Mode) -> std::ffi::OsString {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_ocgen"));
    let rest = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|d| !d.join("ocgen").exists())
        .collect::<Vec<_>>();
    let dirs = match mode {
        Mode::Binary => std::iter::once(bin.parent().unwrap().to_path_buf())
            .chain(rest)
            .collect(),
        Mode::Scripts => rest,
    };
    std::env::join_paths(dirs).unwrap()
}

/// Whether a variable could steer a gate (the test's own environment mustn't).
fn steers(key: &str) -> bool {
    ["OCGEN_", "TEAM_", "LOOP_GUARD_", "SUBAGENT_", "CLAUDE"]
        .iter()
        .any(|p| key.starts_with(p))
}

impl Fixture {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn shell_root(&self) -> String {
        for_shell(self.root())
    }

    /// A shell with the environment Claude Code gives hooks and tools here.
    fn bash(&self, script: &str, cwd: &Path, extra: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(script).current_dir(cwd);
        for (k, _) in std::env::vars() {
            if steers(&k) {
                cmd.env_remove(k);
            }
        }
        cmd.env("PATH", path_for(self.mode))
            .env("HOME", self.home.path())
            .env("CLAUDE_PROJECT_DIR", self.shell_root());
        if let Some(env) = self.settings["env"].as_object() {
            for (k, v) in env {
                cmd.env(k, v.as_str().unwrap_or_default());
            }
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }

    /// The hook commands Claude Code runs for `event` (and, for tool events,
    /// whose matcher takes `tool`).
    fn commands(&self, event: &str, tool: Option<&str>) -> Vec<String> {
        let groups = self.settings["hooks"][event].as_array().cloned();
        let mut out = Vec::new();
        for g in groups.unwrap_or_default() {
            let m = g["matcher"].as_str().unwrap_or("");
            if let (Some(tool), false) = (tool, m.is_empty() || m == "*") {
                if !Regex::new(&format!("^(?:{m})$")).unwrap().is_match(tool) {
                    continue;
                }
            }
            for h in g["hooks"].as_array().unwrap() {
                if h["type"] == "command" {
                    out.push(h["command"].as_str().unwrap().to_string());
                }
            }
        }
        out
    }

    /// Fire `event` at every hook registered for it; the strictest verdict wins,
    /// with the first line of what that hook said.
    fn fire(&self, event: &str, payload: Value, extra: &[(&str, &str)]) -> (Verdict, String) {
        let tool = payload["tool_name"].as_str().map(str::to_string);
        let commands = self.commands(event, tool.as_deref());
        assert!(!commands.is_empty(), "no hook for {event} {tool:?}");
        let mut worst = (Allow, String::new());
        for c in commands {
            let mut child = self
                .bash(&c, self.root(), extra)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let _ = child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.to_string().as_bytes());
            let o = child.wait_with_output().unwrap();
            let (stdout, stderr) = (
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr),
            );
            let code = o.status.code().unwrap_or(-1);
            // Anything but 0 or 2 is a hook that broke, and Claude Code would go on.
            assert!(
                code == 0 || code == 2,
                "{:?} {event}: `{c}` exited {code}\n{stderr}",
                self.mode
            );
            let out: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
            let deny = out.pointer("/hookSpecificOutput/permissionDecision")
                == Some(&json!("deny"))
                || out["decision"] == "block";
            let v = match (code == 2 || deny, out["continue"] == json!(false)) {
                (false, _) => Allow,
                (true, false) => Block,
                (true, true) => Halt,
            };
            // The first line of each channel: a released quality gate gives its
            // reason on stderr and the UNRESOLVED warning in systemMessage.
            let said = [
                stderr.trim(),
                out.pointer("/hookSpecificOutput/permissionDecisionReason")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                out["systemMessage"].as_str().unwrap_or(""),
            ]
            .into_iter()
            .filter_map(|s| s.lines().next().filter(|l| !l.is_empty()))
            .collect::<Vec<_>>()
            .join(" | ");
            if rank(v) > rank(worst.0) || (rank(v) == rank(worst.0) && worst.1.is_empty()) {
                worst = (v, said);
            }
        }
        worst
    }

    fn escalations(&self) -> String {
        fs::read_to_string(self.root().join(".claude/loop-guard/escalations.md"))
            .unwrap_or_default()
    }
}

fn rank(v: Verdict) -> u8 {
    match v {
        Allow => 0,
        Block => 1,
        Halt => 2,
    }
}

fn tool_event(sid: &str, tool: &str, input: Value) -> Value {
    json!({"session_id": sid, "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input})
}

fn bash_event(sid: &str, command: &str) -> Value {
    tool_event(sid, "Bash", json!({ "command": command }))
}

/// Collects every scenario that didn't get its verdict, so a failure lists all.
#[derive(Default)]
struct Report(Vec<String>);

impl Report {
    fn expect(&mut self, mode: Mode, what: &str, want: Verdict, got: (Verdict, String)) {
        if got.0 != want {
            self.0.push(format!(
                "[{mode:?}] {what}: want {want:?}, got {:?} ({})",
                got.0, got.1
            ));
        }
    }

    fn assert_clean(self) {
        assert!(self.0.is_empty(), "\n{}\n", self.0.join("\n"));
    }
}

// ------------------------------------------------------------------ the paths --

/// The premise of every other test here: with this build on PATH the generated
/// command takes the binary, without it the scripts.
#[test]
fn each_mode_takes_the_path_it_claims() {
    let f = project(Mode::Binary);
    let gate = f.commands("PreToolUse", Some("Bash")).join("\n");
    assert!(
        gate.contains("ocgen hook --check") && gate.contains(ocgen::hooks::PROTOCOL),
        "{gate}"
    );
    let probe = |mode| {
        let f = project(mode);
        let o = f
            .bash("ocgen hook --check 2>/dev/null", f.root(), &[])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    assert_eq!(probe(Mode::Binary), ocgen::hooks::PROTOCOL);
    assert_eq!(probe(Mode::Scripts), "", "no ocgen on PATH in Scripts mode");
}

// ------------------------------------------------------- the approval gate --

const ROUTINE: &[&str] = &[
    "git status",
    "cargo test",
    "git log --grep=push",
    "cat scripts/deploy.sh",
    "curl -s https://api.github.com/x | cut -d: -f2",
];

const HIGH_IMPACT: &[&str] = &[
    "git push origin main",
    "git -C . push",
    "git \"push\" origin main",
    "g\\it pu\\sh",
    "git -c alias.p=push p",
    "ls && git push",
    "echo $(git push)",
    "git push --force-with-lease origin HEAD:main",
    "terraform -chdir=infra apply -auto-approve",
    "kubectl -n prod delete deployment api",
    "helm upgrade api ./chart",
    "npm publish",
    "pnpm -r publish",
    "cargo publish",
    "docker buildx build --push -t img .",
    "curl -X POST https://api.example.com/x",
    "curl --json '{\"a\":1}' https://api.example.com",
    "curl -sd @p.json https://x",
    "ssh prod-host uptime",
    "rsync -a . host:/srv",
    "aws s3 rm s3://bucket/key",
    "gh pr merge 12 --squash",
    "gh api -X DELETE repos/o/r",
    "sh ./deploy.sh",
    "make deploy",
    "python3 scripts/publish_release.py",
    "echo hi\u{7}",
];

#[test]
fn the_approval_gate_stops_high_impact_commands_from_any_tool() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        for (i, c) in ROUTINE.iter().enumerate() {
            r.expect(
                mode,
                c,
                Allow,
                f.fire("PreToolUse", bash_event(&format!("ok{i}"), c), &[]),
            );
        }
        for (i, c) in HIGH_IMPACT.iter().enumerate() {
            let got = f.fire("PreToolUse", bash_event(&format!("hi{i}"), c), &[]);
            r.expect(mode, &format!("{c:?}"), Block, got);
        }
        for tool in ["mcp__shell__run", "Monitor"] {
            let ev = tool_event(tool, tool, json!({"command": "git push"}));
            r.expect(
                mode,
                &format!("{tool}: git push"),
                Block,
                f.fire("PreToolUse", ev, &[]),
            );
        }
    }
    r.assert_clean();
}

#[test]
fn an_agent_cannot_approve_itself() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        for (i, c) in [
            "ocgen approve",
            "ocgen approve --minutes 600",
            "echo 9999999999 > ~/.claude/ocgen/approvals/x",
            "cd ~/.claude && cd ocgen && touch approvals/k",
            "touch ~/.CLAUDE/OCGEN/APPROVALS/k",
            "touch ~/.claude/./ocgen//approvals/k",
            "d=~/.claude/ocg; touch ${d}en/approvals/k",
        ]
        .iter()
        .enumerate()
        {
            r.expect(
                mode,
                c,
                Block,
                f.fire("PreToolUse", bash_event(&format!("sa{i}"), c), &[]),
            );
        }
        let store = f.home.path().join(".claude/ocgen/approvals/x");
        // The same file, reached from the project (both are temporary folders).
        let around = format!(
            "{}/../{}/.claude/ocgen/approvals/x",
            f.shell_root(),
            f.home.path().file_name().unwrap().to_string_lossy()
        );
        for (i, p) in [for_shell(&store), around].iter().enumerate() {
            let ev = tool_event(
                &format!("w{i}"),
                "Write",
                json!({"file_path": p, "content": "9999999999"}),
            );
            r.expect(
                mode,
                &format!("Write {p}"),
                Block,
                f.fire("PreToolUse", ev, &[]),
            );
        }
    }
    r.assert_clean();
}

#[test]
fn a_human_approval_unlocks_until_it_is_revoked_or_expires() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        let push = |sid: &str| f.fire("PreToolUse", bash_event(sid, "git push"), &[]);
        let marker = approval::grant(f.home.path(), f.root(), 10).unwrap();
        r.expect(mode, "approved: git push", Allow, push("h1"));
        let ev = bash_event("h2", "ocgen approve --minutes 1440");
        r.expect(
            mode,
            "approved: ocgen approve",
            Block,
            f.fire("PreToolUse", ev, &[]),
        );
        approval::revoke(f.home.path(), f.root()).unwrap();
        r.expect(mode, "revoked: git push", Block, push("h3"));
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "1\n").unwrap();
        r.expect(mode, "expired: git push", Block, push("h4"));
        fs::write(&marker, "99999999999\n").unwrap();
        r.expect(mode, "expiry beyond 24h: git push", Block, push("h5"));
    }
    r.assert_clean();
}

#[test]
fn repeated_gated_attempts_halt_the_agent() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        for (n, want) in [(1, Block), (2, Block), (3, Halt)] {
            let mut ev = bash_event("loop", "git push");
            ev["agent_type"] = json!("implementer");
            r.expect(
                mode,
                &format!("attempt {n}"),
                want,
                f.fire("PreToolUse", ev, &[]),
            );
        }
        assert!(
            f.escalations()
                .contains("[approval-gate] implementer: halted"),
            "{mode:?}: {}",
            f.escalations()
        );
    }
    r.assert_clean();
}

// ----------------------------------------------------------- the WebFetch guard --

#[test]
fn webfetch_reaches_only_trusted_docs_over_https() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        for (i, (url, want)) in [
            ("https://docs.rs/regex", Allow),
            ("https://developer.mozilla.org/en-US/", Allow),
            ("http://docs.rs/regex", Block),
            ("https://evil.com/", Block),
            ("https://docs.rs.evil.com/", Block),
            ("https://evildocs.rs/", Block),
            ("https://docs.rs@evil.com/", Block),
            ("https://evil.com\\@docs.rs/", Block),
            ("https://docs%2ers/", Block),
            ("ftp://docs.rs/", Block),
            ("file:///etc/passwd", Block),
        ]
        .into_iter()
        .enumerate()
        {
            let ev = tool_event(
                &format!("wf{i}"),
                "WebFetch",
                json!({"url": url, "prompt": "x"}),
            );
            r.expect(mode, url, want, f.fire("PreToolUse", ev, &[]));
        }
    }
    r.assert_clean();
}

// ------------------------------------------------------------- the worker gate --

#[test]
fn a_worker_that_changed_files_needs_a_stated_confidence_and_a_passing_check() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        let wt = tempfile::tempdir().unwrap();
        repo(wt.path());
        fs::write(wt.path().join("check.sh"), PASS).unwrap();
        commit(wt.path(), "base");
        let cwd = for_shell(wt.path());
        // SubagentStart records the tree; the worker edits; SubagentStop judges.
        // Each event carries its own hook_event_name, as Claude Code sends it —
        // without it the gate can't tell a start from a finish.
        let start = |id: &str, role: &str| {
            json!({"session_id": "s", "hook_event_name": "SubagentStart",
                   "agent_id": id, "agent_type": role, "cwd": cwd})
        };
        let run = |id: &str, role: &str, edit: bool, said: &str| {
            f.fire("SubagentStart", start(id, role), &[]);
            if edit {
                fs::write(wt.path().join(format!("{id}.rs")), "// change\n").unwrap();
            }
            let mut stop = start(id, role);
            stop["hook_event_name"] = json!("SubagentStop");
            stop["last_assistant_message"] = json!(said);
            f.fire("SubagentStop", stop, &[])
        };
        r.expect(
            mode,
            "read-only explorer",
            Allow,
            run("e1", "explorer", false, "Found it."),
        );
        r.expect(
            mode,
            "inline confidence",
            Block,
            run("i1", "implementer", true, "Done. Confidence: 99%"),
        );
        r.expect(
            mode,
            "80% < 96%",
            Block,
            run("i2", "implementer", true, "Done.\nConfidence: 80%"),
        );
        r.expect(
            mode,
            "97%, check passes",
            Allow,
            run("i3", "implementer", true, "Done.\nConfidence: 97%"),
        );
        fs::write(wt.path().join("check.sh"), FAIL).unwrap();
        r.expect(
            mode,
            "100%, check fails",
            Block,
            run("i4", "implementer", true, "Done.\nConfidence: 100%"),
        );
        // A worker that can't get past the check is released after the budget,
        // marked UNRESOLVED and escalated, rather than looping forever.
        f.fire("SubagentStart", start("i5", "implementer"), &[]);
        fs::write(wt.path().join("i5.rs"), "// change\n").unwrap();
        let mut stop = start("i5", "implementer");
        stop["hook_event_name"] = json!("SubagentStop");
        stop["last_assistant_message"] = json!("Still failing.\nConfidence: 70%");
        for (n, want) in [(1, Block), (2, Block), (3, Allow)] {
            let got = f.fire("SubagentStop", stop.clone(), &[]);
            if n == 3 && !got.1.contains("UNRESOLVED") {
                r.0.push(format!("[{mode:?}] released without UNRESOLVED: {}", got.1));
            }
            r.expect(mode, &format!("stuck worker, stop {n}"), want, got);
        }
        assert!(
            f.escalations().contains("[subagent-confidence] i5"),
            "{mode:?}: {}",
            f.escalations()
        );
    }
    r.assert_clean();
}

// ------------------------------------------------------------- the team gates --

#[test]
fn team_work_needs_an_approved_plan_a_confident_teammate_and_closed_risks() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        let plan = f.root().join(".claude/team/plan.md");
        fs::create_dir_all(plan.parent().unwrap()).unwrap();
        let sid = "e2esess1";
        let created = |who: &str| {
            let ev = json!({"session_id": sid, "hook_event_name": "TaskCreated", "agent_type": who, "task_id": "t1"});
            f.fire("TaskCreated", ev, &[])
        };
        r.expect(mode, "no plan", Block, created("lead1"));
        fs::write(&plan, "# Plan\nStatus: DRAFT\n- [ ] task A\n").unwrap();
        r.expect(mode, "draft plan", Block, created("lead2"));
        fs::write(
            &plan,
            format!("# Plan\n- [ ] task A\nStatus: APPROVED session={sid}\n"),
        )
        .unwrap();
        r.expect(mode, "approved plan", Allow, created("lead"));
        let body = fs::read_to_string(&plan).unwrap();
        fs::write(
            &plan,
            body.replace("task A", "task A and drop the prod database"),
        )
        .unwrap();
        r.expect(mode, "plan edited after approval", Block, created("lead3"));
        fs::write(
            &plan,
            "# Plan\n- [ ] task A\nStatus: APPROVED session=someoneelse\n",
        )
        .unwrap();
        r.expect(mode, "approved by another session", Block, created("lead4"));

        // The task gate reads the teammate's own last message, never the task.
        let transcript = f.root().join("mate.jsonl");
        let say = |text: &str| {
            let line = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}});
            fs::write(&transcript, format!("{line}\n")).unwrap();
        };
        let tp = for_shell(&transcript);
        let completed = |tid: &str, subject: &str| {
            let ev = json!({"session_id": "mate", "hook_event_name": "TaskCompleted", "task_id": tid,
                "task_subject": subject, "task_description": subject, "transcript_path": tp});
            f.fire("TaskCompleted", ev, &[("OCGEN_TRANSCRIPT_SETTLE_MS", "0")])
        };
        say("I wrote the code.");
        r.expect(
            mode,
            "confidence only in the task subject",
            Block,
            completed("ta", "Fix\nConfidence: 99%"),
        );
        say("Done.\nConfidence: 60%");
        r.expect(mode, "teammate at 60%", Block, completed("tb", "Fix"));
        say("Done, tests added.\nConfidence: 97%");
        r.expect(
            mode,
            "teammate at 97%, check passes",
            Allow,
            completed("tc", "Fix"),
        );
        fs::write(f.root().join("check.sh"), FAIL).unwrap();
        r.expect(
            mode,
            "teammate at 97%, check fails",
            Block,
            completed("td", "Fix"),
        );
        fs::write(f.root().join("check.sh"), PASS).unwrap();

        let idle = |name: &str, role: &str| {
            let ev = json!({"session_id": format!("idle-{name}"), "hook_event_name": "TeammateIdle",
                "teammate_name": name, "agent_type": role});
            f.fire("TeammateIdle", ev, &[])
        };
        fs::write(
            &plan,
            "- Risk: data loss — Owner: implementer-1 — Mitigation: pending\n",
        )
        .unwrap();
        r.expect(
            mode,
            "owner idles, risk pending",
            Block,
            idle("implementer-1", "implementer"),
        );
        r.expect(
            mode,
            "read-only teammate idles",
            Allow,
            idle("explorer-1", "explorer"),
        );
        fs::write(
            &plan,
            "- Risk: data loss — Owner: implementer-1 — Mitigation: done\n",
        )
        .unwrap();
        r.expect(
            mode,
            "owner idles, risk closed",
            Allow,
            idle("implementer-1", "implementer"),
        );
    }
    r.assert_clean();
}

// ------------------------------------------------------------ the config audit --

#[test]
fn a_settings_change_that_weakens_a_gate_is_refused() {
    let mut r = Report::default();
    for mode in MODES {
        let f = project(mode);
        let change = |source: &str, file: &Path| {
            let ev = json!({"session_id": "cfg", "hook_event_name": "ConfigChange",
                "source": source, "file_path": for_shell(file)});
            f.fire("ConfigChange", ev, &[])
        };
        let local = f.root().join(".claude/settings.local.json");
        for (body, want) in [
            (r#"{"disableAllHooks": true}"#, Block),
            (r#"{"env": {"TEAM_APPROVAL_GATE": "0"}}"#, Block),
            (
                r#"{"env": {"OCGEN_WEBFETCH_DOMAINS": "docs.rs evil.com"}}"#,
                Block,
            ),
            (r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#, Allow),
        ] {
            fs::write(&local, body).unwrap();
            r.expect(mode, body, want, change("local_settings", &local));
        }
        let ours = f.root().join(".claude/settings.json");
        let mut s = f.settings.clone();
        s["hooks"]["PreToolUse"]
            .as_array_mut()
            .unwrap()
            .retain(|g| !g.to_string().contains("team-approval-gate"));
        fs::write(&ours, serde_json::to_string_pretty(&s).unwrap()).unwrap();
        r.expect(
            mode,
            "approval gate hook removed",
            Block,
            change("project_settings", &ours),
        );
    }
    r.assert_clean();
}

// ------------------------------------------------------------ the layers behind --

/// The approval gate matches text, so a push can be phrased around it. Whatever
/// it lets through must still not land: under Claude Code, without an approval,
/// the git pre-push hook refuses it. A human's own push is never affected.
#[test]
fn a_push_the_text_gate_misses_still_does_not_land() {
    for mode in MODES {
        let f = project(mode);
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        fs::write(f.root().join("ship.sh"), "git push -q origin main\n").unwrap();
        commit(f.root(), "generated");
        git(
            f.root(),
            &["remote", "add", "origin", &for_shell(remote.path())],
        );
        let landed = || {
            let o = git(remote.path(), &["for-each-ref", "refs/heads"]);
            !o.stdout.is_empty()
        };
        for (i, c) in [
            "x=pu; git ${x}sh -q origin main",
            "eval \"$(printf 'git pu%s -q origin main' sh)\"",
            "sh ./ship.sh",
        ]
        .iter()
        .enumerate()
        {
            if f.fire("PreToolUse", bash_event(&format!("p{i}"), c), &[]).0 != Allow {
                continue; // stopped at the gate, as Claude Code would stop it
            }
            let o = f
                .bash(c, f.root(), &[("CLAUDECODE", "1")])
                .output()
                .unwrap();
            assert!(!o.status.success(), "{mode:?} `{c}` pushed: {o:?}");
            assert!(
                String::from_utf8_lossy(&o.stderr).contains("execution-approval backstop"),
                "{mode:?} `{c}`: {o:?}"
            );
            assert!(!landed(), "{mode:?} `{c}` reached the remote");
        }
        let o = f
            .bash("git push -q origin main", f.root(), &[])
            .output()
            .unwrap();
        assert!(o.status.success(), "{mode:?} a human push: {o:?}");
        assert!(landed(), "{mode:?} a human push lands");
    }
}

/// The sandbox can't hide an OS keychain, which git asks through a credential
/// helper. With credentials withheld, the generated settings start the agent's
/// shell with no helper, so a plain `git push` has no token to send.
#[test]
fn a_shell_with_credentials_withheld_has_no_git_credential_helper() {
    use std::os::unix::fs::PermissionsExt as _;
    let asks_keychain = |f: &Fixture| {
        // Stands in for osxkeychain: hands a token to whoever asks.
        let helper = f.home.path().join("keychain");
        fs::write(
            &helper,
            "#!/bin/sh\n[ \"$1\" = get ] && echo username=u && echo password=from-keychain\n",
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            f.home.path().join(".gitconfig"),
            format!("[credential]\n\thelper = {}\n", for_shell(&helper)),
        )
        .unwrap();
        let o = f
            .bash(
                "printf 'protocol=https\\nhost=example.invalid\\n\\n' | git credential fill",
                f.root(),
                &[("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_TERMINAL_PROMPT", "0")],
            )
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).contains("password=from-keychain")
    };
    assert!(
        asks_keychain(&project(Mode::Scripts)),
        "without the sandbox, git asks the helper (the test's premise)"
    );
    let sandboxed = project_with(Mode::Scripts, |p| p.claude.sandbox.enabled = true);
    assert!(
        !asks_keychain(&sandboxed),
        "credentials withheld, yet git asked the keychain"
    );
}
