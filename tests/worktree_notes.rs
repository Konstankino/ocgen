//! The worktree-notes hook. Notes (/inquire and /recap notes, a /chapter book,
//! /brief briefs, /intent drafts) are git-ignored, so a worktree's own never reach the main
//! checkout, and Claude Code counts them as no change: removing the worktree
//! deletes them silently. Most removals run no hook at all (`:q` in a `claude
//! --worktree` session, an archived desktop session), so the hook copies each
//! note into the main checkout as soon as it is written: after Claude's writes,
//! at the end of each turn, and on the draft editor's save. It replaces its own
//! earlier copy, never a note the main checkout has in another version. Before
//! ExitWorktree removes a worktree, the hook stages what is still missing and
//! asks the user; once the removal is done it copies that in.

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

    /// PostToolUse after Claude's `tool` (Write, Edit, MultiEdit) wrote
    /// `file`, in a session working in `cwd`.
    fn wrote(&self, cwd: &Path, file: &Path, tool: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s-1",
            "cwd": ocgen::paths::for_shell(cwd),
            "tool_name": tool,
            "tool_input": { "file_path": ocgen::paths::for_shell(file) },
        })
    }

    /// Stop: the end of a turn, in a session working in `cwd`.
    fn stop(&self, cwd: &Path) -> Value {
        json!({
            "hook_event_name": "Stop",
            "session_id": "s-1",
            "cwd": ocgen::paths::for_shell(cwd),
            "stop_hook_active": false,
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

// ------------------------------------------------------ as they are written --

#[test]
fn a_note_written_in_a_worktree_reaches_the_main_checkout_at_once_and_follows_its_edits() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    let rel = ".claude/intent/drafts/issue-42.md";
    let read = |rel: &str| fs::read_to_string(main.join(rel)).unwrap();
    write(&wt.join(rel), "v1\n");
    let quiet = (0, Value::Null, String::new());
    assert_eq!(
        r.hook(&wt, r.wrote(&wt, &wt.join(rel), "Write"), &[]),
        quiet
    );
    assert_eq!(read(rel), "v1\n");
    // The folder it lands in keeps itself out of git there too, at once: a
    // `git add -A` in the main checkout never takes the copy.
    assert_eq!(read(".claude/intent/drafts/.gitignore"), "*\n");
    // Edited again: ocgen's own copy follows.
    write(&wt.join(rel), "v2\n");
    assert_eq!(r.hook(&wt, r.wrote(&wt, &wt.join(rel), "Edit"), &[]), quiet);
    assert_eq!(read(rel), "v2\n");
    // Changed in the main checkout since: that version stays, and the
    // worktree's is saved beside it — and follows its edits there.
    write(&main.join(rel), "main\n");
    write(&wt.join(rel), "v3\n");
    r.hook(&wt, r.wrote(&wt, &wt.join(rel), "MultiEdit"), &[]);
    assert_eq!(read(rel), "main\n");
    assert_eq!(read(".claude/intent/drafts/issue-42.from-w1.md"), "v3\n");
    write(&wt.join(rel), "v4\n");
    r.hook(&wt, r.wrote(&wt, &wt.join(rel), "Edit"), &[]);
    assert_eq!(read(rel), "main\n");
    assert_eq!(read(".claude/intent/drafts/issue-42.from-w1.md"), "v4\n");
    // What it copied is recorded in the worktree, out of git, and goes with it.
    assert_eq!(
        fs::read_to_string(wt.join(".claude/worktree-notes/.gitignore")).unwrap(),
        "*\n"
    );
    assert!(!main.join(".claude/worktree-notes").exists());
    // Removed with no hook at all (`:q`, an archived desktop session): the
    // note is already in the main checkout.
    r.remove_wt();
    assert_eq!(read(".claude/intent/drafts/issue-42.from-w1.md"), "v4\n");
}

#[test]
fn a_note_the_main_checkout_already_has_is_never_written_over() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    let read = |rel: &str| fs::read_to_string(main.join(rel)).unwrap();
    write(&main.join(".claude/notes/.gitignore"), "*.html\n");
    write(&main.join(".claude/notes/plan.md"), "main\n");
    write(&wt.join(".claude/notes/plan.md"), "worktree\n");
    r.hook(
        &wt,
        r.wrote(&wt, &wt.join(".claude/notes/plan.md"), "Write"),
        &[],
    );
    assert_eq!(read(".claude/notes/plan.md"), "main\n");
    assert_eq!(read(".claude/notes/plan.from-w1.md"), "worktree\n");
    // The main checkout's own ignore file stays as it is.
    assert_eq!(read(".claude/notes/.gitignore"), "*.html\n");
    // The same text in both is the main checkout's own: nothing to copy, and a
    // later edit goes beside it.
    write(&main.join(".claude/notes/same.md"), "same\n");
    write(&wt.join(".claude/notes/same.md"), "same\n");
    r.hook(
        &wt,
        r.wrote(&wt, &wt.join(".claude/notes/same.md"), "Write"),
        &[],
    );
    assert!(!main.join(".claude/notes/same.from-w1.md").exists());
    write(&wt.join(".claude/notes/same.md"), "changed\n");
    r.hook(
        &wt,
        r.wrote(&wt, &wt.join(".claude/notes/same.md"), "Edit"),
        &[],
    );
    assert_eq!(read(".claude/notes/same.md"), "same\n");
    assert_eq!(read(".claude/notes/same.from-w1.md"), "changed\n");
}

#[test]
fn the_end_of_a_turn_copies_what_other_writes_left() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    // Written by a shell command or the draft editor, not by Claude's tools.
    write(&wt.join(".claude/notes/auth-flow.md"), "# Auth\n");
    write(&wt.join(".claude/briefs/retries.md"), "Goal: retries\n");
    write(&wt.join(".claude/intent/drafts/pr-retries.md"), "# PR\n");
    write(&wt.join(".claude/notes/recap/state.json"), "{}\n");
    write(&wt.join(".claude/loop-guard/s-1/count"), "1\n");
    assert_eq!(
        r.hook(&wt, r.stop(&wt), &[]),
        (0, Value::Null, String::new())
    );
    let read = |rel: &str| fs::read_to_string(main.join(rel)).unwrap();
    assert_eq!(read(".claude/notes/auth-flow.md"), "# Auth\n");
    assert_eq!(read(".claude/briefs/retries.md"), "Goal: retries\n");
    assert_eq!(read(".claude/intent/drafts/pr-retries.md"), "# PR\n");
    // Runtime state stays.
    assert!(!main.join(".claude/notes/recap/state.json").exists());
    assert!(!main.join(".claude/loop-guard").exists());
    // ExitWorktree then has nothing left to ask about.
    assert_eq!(
        r.hook(&wt, r.pre("remove"), &[]),
        (0, Value::Null, String::new())
    );
    assert!(r.staged().is_empty());
}

#[test]
fn nothing_is_copied_outside_notes_from_the_main_checkout_or_in_a_probe() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    let quiet = (0, Value::Null, String::new());
    // Not a note.
    write(&wt.join("src/lib.rs"), "fn f() {}\n");
    assert_eq!(
        r.hook(&wt, r.wrote(&wt, &wt.join("src/lib.rs"), "Write"), &[]),
        quiet
    );
    assert!(!main.join("src/lib.rs").exists());
    // Runtime state.
    let request = wt.join(".claude/notes/github/request.json");
    write(&request, "{}\n");
    assert_eq!(r.hook(&wt, r.wrote(&wt, &request, "Write"), &[]), quiet);
    assert!(!main.join(".claude/notes/github").exists());
    // A session in the main checkout has no worktree to copy from.
    let own = main.join(".claude/notes/own.md");
    write(&own, "own\n");
    assert_eq!(r.hook(&main, r.wrote(&main, &own, "Write"), &[]), quiet);
    assert_eq!(r.hook(&main, r.stop(&main), &[]), quiet);
    assert!(!main.join(".claude/worktree-notes").exists());
    // `ocgen verify`'s probe changes nothing.
    let probe = [("OCGEN_HOOK_PROBE", "1")];
    let note = wt.join(".claude/notes/probe.md");
    write(&note, "probe\n");
    assert_eq!(r.hook(&wt, r.wrote(&wt, &note, "Write"), &probe), quiet);
    assert_eq!(r.hook(&wt, r.stop(&wt), &probe), quiet);
    assert!(!main.join(".claude/notes/probe.md").exists());
    assert!(!wt.join(".claude/worktree-notes").exists());
}

#[test]
fn a_project_in_a_subfolder_copies_into_the_same_subfolder_as_it_writes() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    fs::create_dir_all(main.join("svc/api")).unwrap();
    let project = wt.join("svc/api");
    let note = project.join(".claude/notes/db.md");
    write(&note, "db\n");
    r.hook(&project, r.wrote(&project, &note, "Write"), &[]);
    assert_eq!(
        fs::read_to_string(main.join("svc/api/.claude/notes/db.md")).unwrap(),
        "db\n"
    );
    assert!(project.join(".claude/worktree-notes/.gitignore").is_file());
}

/// The draft editor's server, quit when the test ends.
struct Editor(ocgen::notes::viewer::Info);

impl Drop for Editor {
    fn drop(&mut self) {
        ocgen::notes::viewer::quit(&self.0);
    }
}

/// A raw HTTP request to the editor: (status, body).
fn http(info: &ocgen::notes::viewer::Info, method: &str, path: &str, body: &str) -> (u16, String) {
    use std::io::Read as _;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], info.port));
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let origin = format!("http://127.0.0.1:{}", info.port);
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: {origin}\r\nContent-Type: text/plain;charset=UTF-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        info.port,
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    let body = buf.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (code, body.to_string())
}

#[test]
fn a_save_in_the_draft_editor_reaches_the_main_checkout() {
    let r = Repo::new();
    let (main, wt) = (r.main(), r.wt());
    let drafts = wt.join(".claude/intent/drafts");
    write(&drafts.join("issue-42.md"), "# Issue\n");
    let fake = r.home.path().join("fake-browser.sh");
    write(&fake, "exit 0\n");
    let out = Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .arg("draft")
        .current_dir(&wt)
        .env("HOME", r.home.path())
        .env("USERPROFILE", r.home.path())
        .env("OCGEN_NOTES_OPEN", "1")
        .env(
            "OCGEN_NOTES_BROWSER",
            format!("sh '{}'", ocgen::paths::for_shell(&fake)),
        )
        .env("OCGEN_NOTES_BROWSER_WAIT", "1")
        .env("OCGEN_NOTES_GRACE_MS", "5000")
        .env("OCGEN_NOTES_IDLE_SECS", "60")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let editor = Editor(ocgen::notes::viewer::read_info(&drafts).expect("an editor is running"));
    let doc = format!("/{}/issue-42.md", editor.0.token);
    let (code, body) = http(&editor.0, "GET", &doc, "");
    assert_eq!(code, 200, "{body}");
    let rev = serde_json::from_str::<Value>(&body).unwrap()["rev"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, body) = http(
        &editor.0,
        "POST",
        &format!("{doc}?rev={rev}"),
        "# Issue, edited\n",
    );
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        fs::read_to_string(main.join(".claude/intent/drafts/issue-42.md")).unwrap(),
        "# Issue, edited\n"
    );
}

#[test]
fn the_hook_is_wired_on_writes_turns_and_exit_worktree_when_the_project_keeps_notes() {
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
        for event in ["PreToolUse", "PostToolUse", "Stop"] {
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
            (
                "PostToolUse".to_string(),
                "Write|Edit|MultiEdit".to_string()
            ),
            ("Stop".to_string(), String::new()),
        ]
    );
    let w = &mut p.claude.workflow;
    w.inquire = false;
    w.recap = false;
    w.brief = false;
    w.intent = false;
    w.chapter = false;
    assert!(groups(&p).is_empty());
    // A book in .claude/notes is a note too: /chapter alone keeps the hook.
    p.claude.workflow.chapter = true;
    assert_eq!(groups(&p).len(), 4);
}

/// A chapter is plain text in a subfolder of `.claude/notes`, and its sources file
/// one folder deeper: both reach the main checkout like any other note.
#[test]
fn a_book_written_in_a_worktree_reaches_the_main_checkout() {
    let repo = Repo::new();
    let wt = repo.wt();
    write(
        &wt.join(".claude/notes/book/001-orders.txt"),
        "Chapter one.\n",
    );
    write(
        &wt.join(".claude/notes/book/sources/001-orders.txt"),
        "Commit: abc\n",
    );
    let (code, _, err) = repo.hook(&repo.main(), repo.stop(&wt), &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        fs::read_to_string(repo.main().join(".claude/notes/book/001-orders.txt")).unwrap(),
        "Chapter one.\n"
    );
    assert_eq!(
        fs::read_to_string(
            repo.main()
                .join(".claude/notes/book/sources/001-orders.txt")
        )
        .unwrap(),
        "Commit: abc\n"
    );
}

/// Fetched AWS logs aren't notes: they stay in their worktree and go with it, so
/// a dump never lands in the main checkout or holds up the worktree's removal.
#[test]
fn fetched_aws_logs_are_neither_mirrored_nor_asked_about() {
    let repo = Repo::new();
    let wt = repo.wt();
    write(&wt.join(".claude/aws-logs/1-s3/0001-a.gz"), "log\n");
    write(&wt.join(".claude/aws-logs/1-s3/manifest.json"), "{}\n");
    let (code, _, err) = repo.hook(&repo.main(), repo.stop(&wt), &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!repo.main().join(".claude/aws-logs").exists());
    assert!(repo.staged().is_empty());
}
