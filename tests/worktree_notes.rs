//! The worktree-notes hook. Notes (/inquire and /recap notes, /brief briefs,
//! /intent drafts) are git-ignored, so a worktree's own never reach the main
//! checkout, and Claude Code counts them as no change: removing the worktree
//! deletes them silently. Before ExitWorktree removes one, the hook stages the
//! notes the main checkout lacks and asks the user; once the removal is done it
//! copies them in, never over a note the main checkout already has.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
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

/// A repository with one commit, a linked worktree of it under
/// `.claude/worktrees/w1`, and a home of its own for the staging area.
struct Repo {
    main: TempDir,
    home: TempDir,
}

impl Repo {
    fn new() -> Self {
        let main = tempdir().unwrap();
        git_ok(main.path(), &["init", "-q"]);
        write(&main.path().join("README.md"), "hi\n");
        write(&main.path().join(".gitignore"), ".claude/\n");
        git_ok(main.path(), &["add", "."]);
        git_ok(main.path(), &["commit", "-qm", "init"]);
        git_ok(
            main.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "worktree-w1",
                ".claude/worktrees/w1",
            ],
        );
        Repo {
            main,
            home: tempdir().unwrap(),
        }
    }

    fn main(&self) -> PathBuf {
        self.main.path().to_path_buf()
    }

    fn wt(&self) -> PathBuf {
        self.main.path().join(".claude/worktrees/w1")
    }

    /// Remove the worktree as Claude Code does after ExitWorktree(remove).
    fn remove_wt(&self) {
        git_ok(
            self.main.path(),
            &["worktree", "remove", "-f", "-f", ".claude/worktrees/w1"],
        );
    }

    /// Run `ocgen hook worktree-notes` on `payload`, from a session whose
    /// project folder is `project`: (exit code, stdout JSON or Null, stderr).
    fn hook(&self, project: &Path, payload: Value, extra: &[(&str, &str)]) -> (i32, Value, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        cmd.args(["hook", "worktree-notes"])
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("CLAUDE_PROJECT_DIR", ocgen::paths::for_shell(project))
            .env_remove("OCGEN_HOOK_PROBE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        // The hook may decide before reading stdin; ignore a BrokenPipe.
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes());
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let json = if stdout.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
        };
        (
            out.status.code().unwrap(),
            json,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn pre(&self, action: &str) -> Value {
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s-1",
            "cwd": ocgen::paths::for_shell(&self.wt()),
            "tool_name": "ExitWorktree",
            "tool_input": { "action": action },
        })
    }

    fn post(&self, action: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s-1",
            "cwd": ocgen::paths::for_shell(&self.main()),
            "tool_name": "ExitWorktree",
            "tool_input": { "action": action },
            "tool_response": {
                "action": action,
                "originalCwd": ocgen::paths::for_shell(&self.main()),
                "worktreePath": ocgen::paths::for_shell(&self.wt()),
                "worktreeBranch": "worktree-w1",
                "message": "ok",
            },
        })
    }

    fn staged(&self) -> Vec<String> {
        let root = self.home.path().join(".claude/ocgen/worktree-notes");
        let mut out = Vec::new();
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    out.push(
                        p.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        walk(&root, &root, &mut out);
        out.sort();
        out
    }
}

fn decision(out: &Value) -> (&str, &str) {
    let h = &out["hookSpecificOutput"];
    (
        h["permissionDecision"].as_str().unwrap_or(""),
        h["permissionDecisionReason"].as_str().unwrap_or(""),
    )
}

#[test]
fn removing_a_worktree_asks_before_its_notes_go_and_copies_them_on_approval() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    // Only in the worktree: carried.
    write(&wt.join(".claude/notes/auth-flow.md"), "# Auth\n");
    write(&wt.join(".claude/notes/auth-flow.html"), "<p>Auth</p>\n");
    write(&wt.join(".claude/briefs/retries.md"), "Goal: retries\n");
    write(&wt.join(".claude/briefs/.gitignore"), "*\n");
    write(&wt.join(".claude/intent/drafts/issue-42.md"), "# Issue\n");
    // The same in both: nothing to carry.
    write(&wt.join(".claude/notes/same.md"), "same\n");
    write(&main.join(".claude/notes/same.md"), "same\n");
    // Another version in the main checkout: carried beside it, never over it.
    write(&wt.join(".claude/notes/plan.md"), "worktree\n");
    write(&main.join(".claude/notes/plan.md"), "main\n");
    // Runtime state: never carried.
    write(&wt.join(".claude/notes/recap/state.json"), "{}\n");
    write(&wt.join(".claude/notes/github/request.json"), "{}\n");
    // Not notes.
    write(&wt.join(".claude/loop-guard/s-1/count"), "1\n");

    let (code, out, err) = r.hook(&wt, r.pre("remove"), &[]);
    assert_eq!(code, 0, "{err}");
    let (verdict, reason) = decision(&out);
    assert_eq!(verdict, "ask", "{out:#}");
    for want in [
        ".claude/notes/auth-flow.md",
        ".claude/briefs/retries.md",
        ".claude/intent/drafts/issue-42.md",
        ".claude/notes/plan.md",
    ] {
        assert!(reason.contains(want), "{want}: {reason}");
    }
    for never in ["same.md", "state.json", "request.json", "loop-guard"] {
        assert!(!reason.contains(never), "{never}: {reason}");
    }
    assert!(reason.contains("plan.from-w1.md"), "{reason}");
    // Nothing reaches the main checkout before the approval.
    assert!(!main.join(".claude/notes/auth-flow.md").exists());
    assert!(!r.staged().is_empty());

    // Approved: Claude Code removes the worktree, then the copy lands.
    r.remove_wt();
    let (code, out, err) = r.hook(&main, r.post("remove"), &[]);
    assert_eq!(code, 0, "{err}");
    let note = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains(".claude/notes/auth-flow.md"), "{out:#}");
    let read = |rel: &str| fs::read_to_string(main.join(rel)).unwrap();
    assert_eq!(read(".claude/notes/auth-flow.md"), "# Auth\n");
    assert_eq!(read(".claude/notes/auth-flow.html"), "<p>Auth</p>\n");
    assert_eq!(read(".claude/briefs/retries.md"), "Goal: retries\n");
    assert_eq!(read(".claude/briefs/.gitignore"), "*\n");
    assert_eq!(read(".claude/intent/drafts/issue-42.md"), "# Issue\n");
    assert_eq!(read(".claude/notes/plan.md"), "main\n");
    assert_eq!(read(".claude/notes/plan.from-w1.md"), "worktree\n");
    assert!(!main.join(".claude/notes/recap/state.json").exists());
    assert!(!main.join(".claude/loop-guard").exists());
    assert!(r.staged().is_empty(), "{:?}", r.staged());
}

#[test]
fn nothing_to_ask_when_the_worktree_has_no_notes_of_its_own() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    // No notes at all.
    assert_eq!(
        r.hook(&wt, r.pre("remove"), &[]),
        (0, Value::Null, String::new())
    );
    // Notes the main checkout already has, and runtime state.
    write(&wt.join(".claude/notes/same.md"), "same\n");
    write(&main.join(".claude/notes/same.md"), "same\n");
    write(&wt.join(".claude/notes/recap/state.json"), "{}\n");
    assert_eq!(
        r.hook(&wt, r.pre("remove"), &[]),
        (0, Value::Null, String::new())
    );
    assert!(r.staged().is_empty());
}

#[test]
fn keeping_the_worktree_or_leaving_the_main_checkout_asks_nothing() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    write(&wt.join(".claude/notes/auth-flow.md"), "# Auth\n");
    // `keep` leaves the notes where they are.
    assert_eq!(
        r.hook(&wt, r.pre("keep"), &[]),
        (0, Value::Null, String::new())
    );
    // A session in the main checkout isn't in a worktree.
    let mut p = r.pre("remove");
    p["cwd"] = json!(ocgen::paths::for_shell(&main));
    assert_eq!(r.hook(&main, p, &[]), (0, Value::Null, String::new()));
    // Another tool.
    let mut p = r.pre("remove");
    p["tool_name"] = json!("Bash");
    assert_eq!(r.hook(&wt, p, &[]), (0, Value::Null, String::new()));
    // `ocgen verify`'s probe stages nothing.
    let (_, out, _) = r.hook(&wt, r.pre("remove"), &[("OCGEN_HOOK_PROBE", "1")]);
    assert_eq!(decision(&out).0, "ask");
    assert!(r.staged().is_empty());
}

#[test]
fn a_denied_removal_copies_nothing() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    write(&wt.join(".claude/notes/auth-flow.md"), "# Auth\n");
    let (_, out, _) = r.hook(&wt, r.pre("remove"), &[]);
    assert_eq!(decision(&out).0, "ask");
    // Denied: Claude Code runs no PostToolUse. The next time, the hook stages
    // afresh from what the worktree holds then.
    fs::remove_file(wt.join(".claude/notes/auth-flow.md")).unwrap();
    write(&wt.join(".claude/notes/other.md"), "other\n");
    let (_, out, _) = r.hook(&wt, r.pre("remove"), &[]);
    assert!(decision(&out).1.contains("other.md"));
    assert!(!decision(&out).1.contains("auth-flow.md"));
    r.remove_wt();
    r.hook(&main, r.post("remove"), &[]);
    assert!(main.join(".claude/notes/other.md").is_file());
    assert!(!main.join(".claude/notes/auth-flow.md").exists());
    // A PostToolUse with nothing staged does nothing.
    assert_eq!(
        r.hook(&main, r.post("remove"), &[]),
        (0, Value::Null, String::new())
    );
}

#[test]
fn a_project_in_a_subfolder_carries_into_the_same_subfolder() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    fs::create_dir_all(main.join("svc/api")).unwrap();
    fs::create_dir_all(wt.join("svc/api")).unwrap();
    write(&wt.join("svc/api/.claude/notes/db.md"), "db\n");
    let mut p = r.pre("remove");
    p["cwd"] = json!(ocgen::paths::for_shell(&wt.join("svc/api")));
    let (_, out, _) = r.hook(&main.join("svc/api"), p, &[]);
    assert_eq!(decision(&out).0, "ask", "{out:#}");
    r.remove_wt();
    r.hook(&main.join("svc/api"), r.post("remove"), &[]);
    assert_eq!(
        fs::read_to_string(main.join("svc/api/.claude/notes/db.md")).unwrap(),
        "db\n"
    );
}

#[test]
fn the_hook_is_wired_on_exit_worktree_when_the_project_keeps_notes() {
    assert!(ocgen::hooks::NAMES.contains(&"worktree-notes"));
    assert!(!ocgen::hooks::FAIL_CLOSED.contains(&"worktree-notes"));
    let m = ocgen::manifest::Manifest::load().unwrap();
    let mut p = ocgen::render::Project::from_manifest(&m, "English");
    p.target = ocgen::target::Target::ClaudeCode;
    p.project_name = "wn".into();
    p.providers.clear();
    p.agents = ocgen::agent::claude_default_pipeline("English").unwrap();
    let groups = |p: &ocgen::render::Project| -> Vec<(String, String)> {
        let dir = tempdir().unwrap();
        p.scaffold(dir.path(), false).unwrap();
        let s: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let mut out = Vec::new();
        for event in ["PreToolUse", "PostToolUse"] {
            for g in s["hooks"][event].as_array().into_iter().flatten() {
                for h in g["hooks"].as_array().into_iter().flatten() {
                    let cmd = h["command"].as_str().unwrap_or_default();
                    if cmd.contains("worktree-notes") {
                        assert!(cmd.contains(ocgen::hooks::PROTOCOL), "{cmd}");
                        assert!(h.get("onFailure").is_none(), "{h}");
                        out.push((
                            event.to_string(),
                            g["matcher"].as_str().unwrap_or_default().to_string(),
                        ));
                    }
                }
            }
        }
        assert_eq!(
            dir.path().join(".claude/hooks/worktree-notes.sh").is_file(),
            !out.is_empty()
        );
        out
    };
    assert_eq!(
        groups(&p),
        [
            ("PreToolUse".to_string(), "ExitWorktree".to_string()),
            ("PostToolUse".to_string(), "ExitWorktree".to_string()),
        ]
    );
    let w = &mut p.claude.workflow;
    w.inquire = false;
    w.recap = false;
    w.brief = false;
    w.intent = false;
    assert!(groups(&p).is_empty());
}
