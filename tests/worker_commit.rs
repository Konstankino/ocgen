//! The worker-commit hook. A worktree-isolated subagent (the implementer, a
//! /fanout worker) edits files in its own worktree but has no shell to commit
//! them, and Claude Code refuses the coordinator's git commands in another
//! worktree. So the coordinator's merge found nothing, and the user had to commit
//! by hand. When such a subagent stops, the hook commits what it left on its own
//! `worktree-agent-*` branch, so the branch merges. Both implementations (the
//! binary and the script) are run on the same fixtures.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Via {
    Binary,
    Script,
}

/// A repository with one commit and an agent worktree as Claude Code makes one
/// (`.claude/worktrees/agent-abc` on `worktree-agent-abc`).
struct Repo {
    main: TempDir,
}

impl Repo {
    fn new() -> Self {
        let main = tempdir().unwrap();
        let m = main.path();
        git(m, &["init", "-q"]);
        git(m, &["config", "user.name", "t"]);
        git(m, &["config", "user.email", "t@example.com"]);
        // Windows runners check files out with CRLF (core.autocrlf=true); the
        // merged file is compared byte for byte.
        git(m, &["config", "core.autocrlf", "false"]);
        write(&m.join("app.py"), "x = 1\n");
        write(&m.join(".gitignore"), ".claude/\n");
        git(m, &["add", "."]);
        git(m, &["commit", "-qm", "init"]);
        git(
            m,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "worktree-agent-abc",
                ".claude/worktrees/agent-abc",
            ],
        );
        Repo { main }
    }

    fn main(&self) -> PathBuf {
        self.main.path().to_path_buf()
    }

    fn wt(&self) -> PathBuf {
        self.main.path().join(".claude/worktrees/agent-abc")
    }

    fn stop(&self, cwd: &Path) -> Value {
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "s-1",
            "agent_id": "abc",
            "agent_type": "implementer",
            "cwd": ocgen::paths::for_shell(cwd),
            "last_assistant_message": "Done.\nConfidence: 97%",
        })
    }

    /// (exit code, stdout, stderr) of the hook on `payload`, from a session
    /// whose project is the main checkout.
    fn run(&self, via: Via, payload: &Value, extra: &[(&str, &str)]) -> (i32, String, String) {
        self.run_hook("worker-commit", via, payload, extra)
    }

    fn run_hook(
        &self,
        hook: &str,
        via: Via,
        payload: &Value,
        extra: &[(&str, &str)],
    ) -> (i32, String, String) {
        let mut cmd = match via {
            Via::Binary => {
                let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
                c.args(["hook", hook]);
                c
            }
            Via::Script => {
                let mut c = Command::new("sh");
                c.arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join(format!("templates/claude/hooks/{hook}.sh")),
                );
                c
            }
        };
        cmd.env(
            "CLAUDE_PROJECT_DIR",
            ocgen::paths::for_shell(self.main.path()),
        )
        .env_remove("OCGEN_HOOK_PROBE")
        .env_remove("OCGEN_NO_JQ")
        .env_remove("OCGEN_WORKER_COMMIT")
        .env_remove("OCGEN_CHECK_CMD")
        .env_remove("SUBAGENT_CONFIDENCE_THRESHOLD")
        .env_remove("SUBAGENT_READONLY_ROLES")
        .env_remove("LOOP_GUARD_MAX_BLOCKS")
        .env_remove("TEAM_TASK_GATE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes());
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn commits(&self, rev: &str) -> usize {
        git(self.main.path(), &["rev-list", "--count", rev])
            .parse()
            .unwrap()
    }
}

const BOTH: [Via; 2] = [Via::Binary, Via::Script];

#[test]
fn a_worker_s_uncommitted_changes_are_committed_on_its_branch() {
    for via in BOTH {
        for no_jq in [false, true] {
            let r = Repo::new();
            let wt = r.wt();
            write(&wt.join("app.py"), "x = 2\n");
            write(&wt.join("tests/test_app.py"), "def test(): pass\n");
            // A note is git-ignored: it stays out of the commit.
            write(&wt.join(".claude/notes/n.md"), "n\n");
            let extra: &[(&str, &str)] = if no_jq { &[("OCGEN_NO_JQ", "1")] } else { &[] };
            let (code, out, err) = r.run(via, &r.stop(&wt), extra);
            assert_eq!(code, 0, "{via:?}: {err}");
            let msg: Value =
                serde_json::from_str(&out).unwrap_or_else(|e| panic!("{via:?} {e}: {out}"));
            let say = msg["systemMessage"].as_str().unwrap_or_default();
            assert!(
                say.contains("implementer") && say.contains("worktree-agent-abc"),
                "{via:?}: {say}"
            );
            assert_eq!(r.commits("worktree-agent-abc"), 2, "{via:?}");
            assert_eq!(
                r.commits("HEAD"),
                1,
                "{via:?}: the main branch is untouched"
            );
            assert_eq!(git(&wt, &["status", "--porcelain"]), "", "{via:?}");
            let files = git(&wt, &["show", "--name-only", "--format=", "HEAD"]);
            assert_eq!(files, "app.py\ntests/test_app.py", "{via:?}");
            let subject = git(&wt, &["log", "-1", "--format=%s"]);
            assert_eq!(
                subject, "chore(implementer): the subagent's changes",
                "{via:?}"
            );
            assert!(!git(&wt, &["log", "-1", "--format=%B"]).contains("Co-Authored-By"));
            // The coordinator's merge now takes the change.
            git(
                &r.main(),
                &["merge", "-q", "--no-edit", "worktree-agent-abc"],
            );
            assert_eq!(
                fs::read_to_string(r.main().join("app.py")).unwrap(),
                "x = 2\n"
            );
        }
    }
}

#[test]
fn nothing_is_committed_outside_an_agent_worktree_or_when_clean() {
    for via in BOTH {
        let r = Repo::new();
        let (main, wt) = (r.main(), r.wt());
        // A clean worker: nothing to commit, nothing said.
        assert_eq!(
            r.run(via, &r.stop(&wt), &[]),
            (0, String::new(), String::new()),
            "{via:?}"
        );
        // A subagent in the main checkout (not isolated): the lead's own
        // uncommitted work is never committed.
        write(&main.join("app.py"), "x = 3\n");
        assert_eq!(
            r.run(via, &r.stop(&main), &[]),
            (0, String::new(), String::new()),
            "{via:?}"
        );
        assert_eq!(r.commits("HEAD"), 1);
        // A worktree that isn't an agent's (a `claude --worktree` session's).
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "worktree-issue",
                ".claude/worktrees/issue",
            ],
        );
        let session = main.join(".claude/worktrees/issue");
        write(&session.join("app.py"), "x = 4\n");
        assert_eq!(
            r.run(via, &r.stop(&session), &[]),
            (0, String::new(), String::new()),
            "{via:?}"
        );
        assert_eq!(r.commits("worktree-issue"), 1);
        // `ocgen verify`'s probe changes nothing.
        write(&wt.join("app.py"), "x = 5\n");
        let (code, _, _) = r.run(via, &r.stop(&wt), &[("OCGEN_HOOK_PROBE", "1")]);
        assert_eq!(code, 0);
        assert_eq!(r.commits("worktree-agent-abc"), 1, "{via:?}");
    }
}

#[test]
fn a_commit_that_fails_says_so_and_never_blocks() {
    for via in BOTH {
        let r = Repo::new();
        let wt = r.wt();
        // No identity to commit with.
        git(&r.main(), &["config", "--unset", "user.name"]);
        git(&r.main(), &["config", "--unset", "user.email"]);
        git(&r.main(), &["config", "user.useConfigOnly", "true"]);
        write(&wt.join("app.py"), "x = 2\n");
        let home = tempdir().unwrap();
        let h = home.path().to_str().unwrap();
        let (code, out, _) = r.run(
            via,
            &r.stop(&wt),
            &[
                ("HOME", h),
                ("XDG_CONFIG_HOME", h),
                ("GIT_CONFIG_NOSYSTEM", "1"),
            ],
        );
        assert_eq!(code, 0, "{via:?}");
        let msg: Value =
            serde_json::from_str(&out).unwrap_or_else(|e| panic!("{via:?} {e}: {out}"));
        let say = msg["systemMessage"].as_str().unwrap_or_default();
        assert!(say.contains("could not commit"), "{via:?}: {say}");
        assert_eq!(r.commits("worktree-agent-abc"), 1);
        // Nothing half done: the change is still in the worktree, unstaged or staged.
        assert!(!git(&wt, &["status", "--porcelain"]).is_empty());
    }
}

/// The worker gate's settings, with the gate committing for the worker.
const GATED: [(&str, &str); 3] = [
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("LOOP_GUARD_MAX_BLOCKS", "1"),
    ("OCGEN_WORKER_COMMIT", "1"),
];

fn said(via: Via, out: &str) -> String {
    let msg: Value = serde_json::from_str(out).unwrap_or_else(|e| panic!("{via:?} {e}: {out}"));
    msg["systemMessage"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// With the worker gate on, the gate commits the worker's changes itself, after
/// its verdict, in one process: a separate hook beside it would race it. A
/// worker the loop guard releases gets a second, empty commit that says so, so
/// its branch shows the work was never verified.
#[test]
fn the_gate_commits_a_worker_and_marks_one_it_releases() {
    for via in BOTH {
        let r = Repo::new();
        let wt = r.wt();
        write(&wt.join("app.py"), "x = 2\n");
        let mut stop = r.stop(&wt);
        stop["last_assistant_message"] = json!("Done.\nConfidence: 40%");
        let (code, out, err) = r.run_hook("subagent-confidence-gate", via, &stop, &GATED);
        assert_eq!(code, 0, "{via:?}: released: {err}");
        let say = said(via, &out);
        assert!(
            say.contains("UNRESOLVED") && say.contains("worktree-agent-abc"),
            "{via:?}: {say}"
        );
        assert_eq!(r.commits("worktree-agent-abc"), 3, "{via:?}");
        assert_eq!(git(&wt, &["status", "--porcelain"]), "", "{via:?}");
        let log = git(&wt, &["log", "-2", "--format=%s"]);
        assert_eq!(
            log,
            "chore(implementer): UNRESOLVED, released by the loop guard\n\
             chore(implementer): the subagent's changes",
            "{via:?}"
        );
        let body = git(&wt, &["log", "-1", "--format=%b"]);
        assert!(
            body.contains("worker abc") && body.contains("confidence 40% < 96%"),
            "{via:?}: {body}"
        );
        assert_eq!(git(&wt, &["show", "--name-only", "--format=", "HEAD"]), "");
        assert_eq!(
            git(&wt, &["show", "--name-only", "--format=", "HEAD~1"]),
            "app.py"
        );
    }
}

/// A worker that passes is committed as before; one sent back is committed too
/// (as the separate hook did at every stop), with no mark; without the setting
/// the gate commits nothing; `ocgen verify`'s probe changes nothing.
#[test]
fn the_gate_commits_like_the_hook_it_replaces() {
    for via in BOTH {
        let r = Repo::new();
        let wt = r.wt();
        let mut stop = r.stop(&wt);
        write(&wt.join("app.py"), "x = 2\n");
        let (code, out, err) = r.run_hook("subagent-confidence-gate", via, &stop, &GATED);
        assert_eq!(code, 0, "{via:?}: {err}");
        assert!(said(via, &out).contains("committed implementer's changes on worktree-agent-abc"));
        assert_eq!(r.commits("worktree-agent-abc"), 2, "{via:?}");
        // Sent back: committed, not marked.
        write(&wt.join("app.py"), "x = 3\n");
        stop["agent_id"] = json!("abd");
        stop["last_assistant_message"] = json!("Done.\nConfidence: 40%");
        let back = [GATED[0], ("LOOP_GUARD_MAX_BLOCKS", "3"), GATED[2]];
        let (code, _, err) = r.run_hook("subagent-confidence-gate", via, &stop, &back);
        assert_eq!(code, 2, "{via:?}: {err}");
        assert!(err.contains("below the required 96%"), "{via:?}: {err}");
        assert_eq!(r.commits("worktree-agent-abc"), 3, "{via:?}");
        assert_eq!(
            git(&wt, &["log", "-1", "--format=%s"]),
            "chore(implementer): the subagent's changes"
        );
        // The setting off: the gate leaves committing to the separate hook.
        write(&wt.join("app.py"), "x = 4\n");
        stop["last_assistant_message"] = json!("Done.\nConfidence: 97%");
        let (code, out, _) = r.run_hook("subagent-confidence-gate", via, &stop, &GATED[..2]);
        assert_eq!((code, out.as_str()), (0, ""), "{via:?}");
        assert_eq!(r.commits("worktree-agent-abc"), 3, "{via:?}");
        // The probe.
        let probe = [GATED[0], GATED[1], GATED[2], ("OCGEN_HOOK_PROBE", "1")];
        r.run_hook("subagent-confidence-gate", via, &stop, &probe);
        assert_eq!(r.commits("worktree-agent-abc"), 3, "{via:?}");
    }
}

/// Where the worker gate is on, it commits for isolated workers: the
/// worker-commit hook isn't wired beside it (its script stays, the gate runs
/// it), and the gate is told so.
#[test]
fn the_gate_takes_over_the_commit_when_both_are_on() {
    let m = ocgen::manifest::Manifest::load().unwrap();
    let mut p = ocgen::render::Project::from_manifest(&m, "English");
    p.target = ocgen::target::Target::ClaudeCode;
    p.project_name = "wc".into();
    p.providers.clear();
    p.agents = ocgen::agent::claude_default_pipeline("English").unwrap();
    p.claude.workflow.fanout = true;
    p.claude.workflow.subagent_confidence = 90;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let s: Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let stop = s["hooks"]["SubagentStop"].to_string();
    assert!(stop.contains("subagent-confidence-gate"), "{stop}");
    assert!(!stop.contains("worker-commit"), "{stop}");
    assert_eq!(s["env"]["OCGEN_WORKER_COMMIT"], "1", "{}", s["env"]);
    assert!(dir.path().join(".claude/hooks/worker-commit.sh").is_file());
}

#[test]
fn the_hook_is_wired_on_subagent_stop_when_workers_are_isolated() {
    assert!(ocgen::hooks::NAMES.contains(&"worker-commit"));
    assert!(!ocgen::hooks::FAIL_CLOSED.contains(&"worker-commit"));
    let m = ocgen::manifest::Manifest::load().unwrap();
    let mut p = ocgen::render::Project::from_manifest(&m, "English");
    p.target = ocgen::target::Target::ClaudeCode;
    p.project_name = "wc".into();
    p.providers.clear();
    p.agents = ocgen::agent::claude_default_pipeline("English").unwrap();
    // Without the worker gate, which otherwise commits for them.
    p.claude.workflow.subagent_confidence = 0;
    p.claude.workflow.check_cmd.clear();
    let wired = |p: &ocgen::render::Project| -> bool {
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        let s: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let mut found = false;
        for g in s["hooks"]["SubagentStop"].as_array().into_iter().flatten() {
            for h in g["hooks"].as_array().into_iter().flatten() {
                let cmd = h["command"].as_str().unwrap_or_default();
                if cmd.contains("worker-commit") {
                    assert!(cmd.contains(ocgen::hooks::PROTOCOL), "{cmd}");
                    assert!(h.get("onFailure").is_none(), "{h}");
                    found = true;
                }
            }
        }
        assert_eq!(
            dir.path().join(".claude/hooks/worker-commit.sh").is_file(),
            found
        );
        found
    };
    for a in p.agents.iter_mut() {
        if a.role == "implementer" {
            a.isolation = "worktree".into();
        }
    }
    assert!(wired(&p));
    for a in p.agents.iter_mut() {
        a.isolation.clear();
    }
    p.claude.workflow.fanout = true;
    assert!(wired(&p), "/fanout spawns isolated workers");
    p.claude.workflow.fanout = false;
    assert!(!wired(&p));
}
