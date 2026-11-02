//! The check-broker hook: a hook rather than a model runs the project's full
//! check, in a throwaway worktree at the requested commit, and writes the
//! evidence. These run the binary on a real repository with `OCGEN_SANDBOX`
//! dropped (a session sets it), so they hold under the Bash sandbox and on CI.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ocgen::paths::for_shell;
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

const SLUG: &str = "s1";
const ID: &str = "c1";
const REQUEST: &str = ".claude/deliver/s1/check-request.json";
const CMD: &str = "OCGEN_FULL_CHECK_CMD";

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

/// Every folder and file under a root, by relative path, with what each file holds.
type Tree = BTreeMap<String, Vec<u8>>;

fn walk(root: &Path, dir: &Path, out: &mut Tree) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        let rel = for_shell(path.strip_prefix(root).unwrap());
        if e.file_type().unwrap().is_dir() {
            walk(root, &path, out);
            out.insert(format!("{rel}/"), Vec::new());
        } else {
            out.insert(rel, fs::read(&path).unwrap_or_default());
        }
    }
}

/// The paths that differ between two trees.
fn changed(before: &Tree, after: &Tree) -> Vec<String> {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .cloned()
        .collect()
}

/// Whether `text` has `word` as a whole word, in any case.
fn says(text: &str, word: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|w| w == word)
}

/// The PostToolUse event of a Write of `file`, from a session in `cwd`.
fn write_event(file: &Path, cwd: &Path) -> String {
    json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s-1",
        "tool_name": "Write",
        "cwd": for_shell(cwd),
        "tool_input": { "file_path": for_shell(file) },
    })
    .to_string()
}

/// A PID that no longer runs: a child that has ended and been reaped.
fn dead_pid() -> u32 {
    let mut child = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// What the hook said.
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    /// The note for Claude (PostToolUse `additionalContext`).
    fn note(&self) -> String {
        let v: Value =
            serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", self.stdout));
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"], "PostToolUse",
            "{v}"
        );
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_else(|| panic!("no note: {v}"))
            .to_string()
    }

    fn assert_silent(&self, what: &str) {
        assert_eq!(
            (self.stdout.as_str(), self.stderr.as_str()),
            ("", ""),
            "{what}"
        );
    }
}

/// A record's `Key: value` lines.
#[derive(Debug)]
struct Record(BTreeMap<String, String>);

impl Record {
    fn get(&self, key: &str) -> &str {
        self.0.get(key).map_or("", String::as_str)
    }
}

/// A repository with one commit, and a folder beside it for a check's markers,
/// so a marker never dirties the repository. `.claude/` isn't git-ignored here:
/// the record folder has to keep itself out of git.
struct Repo {
    _t: TempDir,
    root: PathBuf,
    top: PathBuf,
    project: PathBuf,
    marks: PathBuf,
}

impl Repo {
    fn new() -> Self {
        Self::with_project_at("")
    }

    /// The project at `sub` in the repository (empty: its top level).
    fn with_project_at(sub: &str) -> Self {
        let t = tempdir().unwrap();
        let root = ocgen::paths::plain(&t.path().canonicalize().unwrap());
        let (top, marks) = (root.join("repo"), root.join("marks"));
        let project = if sub.is_empty() {
            top.clone()
        } else {
            top.join(sub)
        };
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&marks).unwrap();
        git(&top, &["init", "-q"]);
        git(&top, &["config", "user.name", "t"]);
        git(&top, &["config", "user.email", "t@example.com"]);
        // Windows runners check files out with CRLF (core.autocrlf=true); the
        // checks here compare what they read.
        git(&top, &["config", "core.autocrlf", "false"]);
        write(&project.join("app.txt"), "committed\n");
        git(&top, &["add", "."]);
        git(&top, &["commit", "-qm", "init"]);
        Repo {
            _t: t,
            root,
            top,
            project,
            marks,
        }
    }

    fn head(&self) -> String {
        git(&self.top, &["rev-parse", "HEAD"])
    }

    fn records(&self) -> PathBuf {
        self.project.join(".claude/deliver").join(SLUG)
    }

    fn worktree(&self, id: &str) -> PathBuf {
        self.project
            .join(".claude/worktrees")
            .join(format!("broker-{SLUG}-{id}"))
    }

    fn mark(&self, name: &str) -> PathBuf {
        self.marks.join(name)
    }

    fn marked(&self, name: &str) -> bool {
        self.mark(name).exists()
    }

    /// A check that leaves the marker `name`, then does `then`.
    fn marking(&self, name: &str, then: &str) -> String {
        let mark = for_shell(&self.mark(name));
        format!("echo ran >> '{mark}'; {then}")
    }

    fn request(&self, body: &Value) {
        write(&self.project.join(REQUEST), &body.to_string());
    }

    /// A good request: this id, at HEAD.
    fn ask(&self) {
        self.request(&json!({ "id": ID, "commit": self.head() }));
    }

    fn ask_with_overlay(&self, overlay: &str) {
        let commit = self.head();
        self.request(&json!({ "id": ID, "commit": commit, "overlay": overlay }));
    }

    /// The hook's command, from a session whose project is this one and from a
    /// folder outside the repository.
    fn hook_command(&self, extra: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        cmd.args(["hook", "check-broker"])
            .current_dir(&self.marks)
            .env("CLAUDE_PROJECT_DIR", for_shell(&self.project))
            .env_remove("OCGEN_HOOK_PROBE")
            .env_remove("OCGEN_SANDBOX")
            .env_remove(CMD)
            .env_remove("OCGEN_FULL_CHECK_TIMEOUT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }

    /// The hook on `payload`. It never fails the write: anything but exit 0 fails
    /// here.
    fn hook(&self, payload: &str, extra: &[(&str, &str)]) -> Out {
        let mut child = self.hook_command(extra).spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
        Out {
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            stderr,
        }
    }

    /// The hook on a Write of `file`, from a session in `cwd`.
    fn wrote_from(&self, file: &Path, cwd: &Path, extra: &[(&str, &str)]) -> Out {
        self.hook(&write_event(file, cwd), extra)
    }

    /// The hook on the Write of the request.
    fn fire(&self, extra: &[(&str, &str)]) -> Out {
        self.wrote_from(&self.project.join(REQUEST), &self.project, extra)
    }

    /// The hook on the Write of the request, left running.
    #[cfg(unix)]
    fn start(&self, extra: &[(&str, &str)]) -> std::process::Child {
        let mut cmd = self.hook_command(extra);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let event = write_event(&self.project.join(REQUEST), &self.project);
        let _ = child.stdin.take().unwrap().write_all(event.as_bytes());
        child
    }

    /// A `git` for a hook's `PATH`: `arms` (of a `case "$*"`), else the real one.
    /// The `PATH` value to give the hook; the test's own git stays the real one.
    #[cfg(unix)]
    fn shim_git(&self, arms: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let found = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8_lossy(&found.stdout).trim().to_string();
        let dir = self.marks.join("shim");
        let script = dir.join("git");
        let body = format!("#!/bin/sh\ncase \"$*\" in\n{arms}\nesac\nexec '{real}' \"$@\"\n");
        write(&script, &body);
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        format!("{}:{}", for_shell(&dir), std::env::var("PATH").unwrap())
    }

    fn record(&self, id: &str) -> Record {
        let path = self.records().join(format!("check-{id}.md"));
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        Record(
            text.lines()
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .collect(),
        )
    }

    fn log(&self, id: &str) -> Vec<u8> {
        fs::read(self.records().join(format!("check-{id}.log"))).unwrap_or_default()
    }

    fn log_text(&self, id: &str) -> String {
        String::from_utf8_lossy(&self.log(id)).into_owned()
    }

    fn tree(&self) -> Tree {
        let mut out = Tree::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    /// The tree without the record folder and git's own state: what a refused
    /// request has to leave alone.
    fn outside_the_records(&self) -> Tree {
        self.tree()
            .into_iter()
            .filter(|(k, _)| {
                !k.starts_with("repo/.claude/deliver/s1/") && !k.starts_with("repo/.git/")
            })
            .collect()
    }

    /// Only the main checkout is left: on disk, and in git's own list (which
    /// keeps a worktree whose folder went until it is pruned).
    fn assert_no_worktree(&self, what: &str) {
        let list = git(&self.top, &["worktree", "list", "--porcelain"]);
        let listed = list.lines().filter(|l| l.starts_with("worktree ")).count();
        assert_eq!(listed, 1, "{what}: {list}");
        let left: Vec<_> = fs::read_dir(self.project.join(".claude/worktrees"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(left.is_empty(), "{what}: {left:?}");
    }

    /// Fire a request the hook has to refuse: on the record with a reason,
    /// nothing run, no worktree. The record and the note.
    fn assert_refused(&self, what: &str) -> (Record, String) {
        let check = self.marking("ran", "exit 0");
        let note = self.fire(&[(CMD, check.as_str())]).note();
        let rec = self.record(ID);
        assert_eq!(rec.get("Status"), "refused", "{what}: {rec:?}");
        assert!(!rec.get("Reason").is_empty(), "{what}: {rec:?}");
        assert!(
            note.starts_with("check-broker c1: refused"),
            "{what}: {note}"
        );
        assert!(!self.marked("ran"), "{what}: the check ran");
        self.assert_no_worktree(what);
        (rec, note)
    }

    /// Drop the record of `ID`: an id is never reused, and each case in a loop
    /// wants a refusal of its own.
    fn forget_record(&self) {
        fs::remove_file(self.records().join(format!("check-{ID}.md"))).unwrap();
    }

    /// An earlier run as a hook that died leaves it: a `started` record and a
    /// real linked worktree.
    #[cfg(unix)]
    fn seed_started(&self, id: &str, hook: u32, check: Option<u32>) -> PathBuf {
        let wt = self.worktree(id);
        let shown = for_shell(&wt);
        git(
            &self.top,
            &["worktree", "add", "-q", "--detach", &shown, "HEAD"],
        );
        let head = self.head();
        let mut text = format!(
            "Status: started\nId: {id}\nCommit: {head}\nCommand: true\n\
             Worktree: {shown}\nHook-Pid: {hook}\n"
        );
        if let Some(check) = check {
            text.push_str(&format!("Check-Pid: {check}\n"));
        }
        write(&self.records().join(format!("check-{id}.md")), &text);
        wt
    }
}

// ---------------------------------------------------------------- trigger --

#[test]
fn any_other_write_passes_silently_and_writes_nothing() {
    let r = Repo::new();
    let body = json!({ "id": ID, "commit": "HEAD" }).to_string();
    let check = r.marking("ran", "exit 0");
    let env = [(CMD, check.as_str())];
    for rel in [
        "app.txt",
        ".claude/notes/github/request.json",
        ".claude/deliver/check-request.json",
        ".claude/deliver/s1/other.json",
        ".claude/deliver/s1/check-request.json.bak",
        ".claude/deliver/s1/nested/check-request.json",
        "x/.claude/deliver/s1/check-request.json",
        // Slugs that can't name a record folder.
        ".claude/deliver/Bad-Slug/check-request.json",
        ".claude/deliver/bad_slug/check-request.json",
        ".claude/deliver/bad slug/check-request.json",
        ".claude/deliver/-leading/check-request.json",
    ] {
        let file = r.project.join(rel);
        write(&file, &body);
        let before = r.tree();
        r.wrote_from(&file, &r.project, &env).assert_silent(rel);
        assert_eq!(changed(&before, &r.tree()), Vec::<String>::new(), "{rel}");
    }

    // `..` where the slug goes: a request lies where it resolves to.
    write(&r.project.join(".claude/check-request.json"), &body);
    let dotted = r.project.join(".claude/deliver/../check-request.json");
    let before = r.tree();
    r.wrote_from(&dotted, &r.project, &env).assert_silent("..");
    assert_eq!(changed(&before, &r.tree()), Vec::<String>::new(), "..");

    // A subagent's worktree: only the coordinator's checkout has requests.
    git(
        &r.top,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree-agent-abc",
            ".claude/worktrees/agent-abc",
        ],
    );
    let wt = r.project.join(".claude/worktrees/agent-abc");
    let file = wt.join(REQUEST);
    write(&file, &body);
    let before = r.tree();
    r.wrote_from(&file, &wt, &env).assert_silent("a worktree");
    assert_eq!(changed(&before, &r.tree()), Vec::<String>::new());

    // No path to judge at all.
    for payload in [
        "",
        "not json",
        "{}",
        r#"{"tool_input":{}}"#,
        r#"{"tool_input":{"file_path":""}}"#,
    ] {
        let before = r.tree();
        r.hook(payload, &env).assert_silent(payload);
        assert_eq!(
            changed(&before, &r.tree()),
            Vec::<String>::new(),
            "{payload}"
        );
    }
}

// ----------------------------------------------------------------- results --

#[test]
fn a_green_check_passes_with_its_whole_output_on_file() {
    let r = Repo::new();
    r.ask();
    let check = "echo green-output; pwd";
    let note = r.fire(&[(CMD, check)]).note();

    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert_eq!(rec.get("Id"), ID, "{rec:?}");
    assert_eq!(rec.get("Commit"), r.head(), "{rec:?}");
    assert_eq!(rec.get("Command"), check, "{rec:?}");
    assert_eq!(rec.get("Exit"), "0", "{rec:?}");
    assert!(rec.get("Duration-s").parse::<f64>().is_ok(), "{rec:?}");
    assert_eq!(rec.get("Worktree"), for_shell(&r.worktree(ID)), "{rec:?}");
    assert!(
        rec.get("Output")
            .ends_with(".claude/deliver/s1/check-c1.log"),
        "{rec:?}"
    );
    for pid in ["Hook-Pid", "Check-Pid"] {
        assert!(rec.get(pid).parse::<u32>().is_ok(), "{pid}: {rec:?}");
    }
    assert_eq!(rec.get("Left-Behind"), "", "{rec:?}");
    assert_eq!(rec.get("Overlay"), "", "{rec:?}");

    let log = r.log_text(ID);
    assert!(log.contains("green-output"), "{log}");
    assert!(
        log.contains("broker-s1-c1"),
        "it ran in the worktree: {log}"
    );

    assert!(
        note.starts_with("check-broker c1: passed, exit 0, "),
        "{note}"
    );
    assert!(
        note.contains("full output: ") && note.contains(".claude/deliver/s1/check-c1.log"),
        "{note}"
    );
    assert!(!r.worktree(ID).exists());
    r.assert_no_worktree("a green check");
}

#[test]
fn a_failing_check_is_failed_with_its_exit_code() {
    let r = Repo::new();
    r.ask();
    let note = r.fire(&[(CMD, "echo two-tests-failed; exit 3")]).note();
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "failed", "{rec:?}");
    assert_eq!(rec.get("Exit"), "3", "{rec:?}");
    assert!(
        note.starts_with("check-broker c1: failed, exit 3, "),
        "{note}"
    );
    assert!(note.contains("two-tests-failed"), "{note}");
    assert!(r.log_text(ID).contains("two-tests-failed"));
    r.assert_no_worktree("a failing check");
}

/// Claude Code replaces context above 10,000 characters with a path and a
/// preview, so the note carries a tail and the log carries everything.
#[test]
fn every_line_goes_to_the_log_and_the_note_stays_bounded() {
    let r = Repo::new();
    r.ask();
    let check = "i=1; while [ $i -le 5000 ]; do echo line-$i; i=$((i+1)); done";
    let note = r.fire(&[(CMD, check)]).note();
    let log = r.log_text(ID);
    let want: Vec<String> = (1..=5000).map(|i| format!("line-{i}")).collect();
    assert!(
        log.lines().eq(want.iter().map(String::as_str)),
        "{} lines in the log",
        log.lines().count()
    );
    assert!(note.contains("line-5000"), "{note}");
    assert!(!note.contains("line-1\n"), "{note}");
    assert!(note.len() < 4096, "{} bytes", note.len());
}

#[test]
fn output_that_isn_t_utf8_still_reaches_the_note() {
    let r = Repo::new();
    r.ask();
    let check = r"printf 'first-words\n\377\376 binary\nlast-words\n'; exit 1";
    let note = r.fire(&[(CMD, check)]).note();
    assert!(
        note.contains("first-words") && note.contains("last-words"),
        "{note}"
    );
    assert_eq!(r.record(ID).get("Status"), "failed");
    assert!(
        r.log(ID).windows(2).any(|w| w == b"\xff\xfe"),
        "the log keeps the raw bytes"
    );
}

// ---------------------------------------------------------------- refusals --

/// A request with no id fit to name a record: the note says it's refused and
/// what's wrong, nothing runs, and no record is written, there or anywhere else.
/// The note.
fn refused_without_an_id(body: &str, words: &[&str]) -> String {
    let r = Repo::new();
    write(&r.project.join(REQUEST), body);
    let check = r.marking("ran", "exit 0");
    let before = r.outside_the_records();
    let note = r.fire(&[(CMD, check.as_str())]).note();
    assert!(says(&note, "refused"), "{body}: {note}");
    assert!(words.iter().any(|w| says(&note, w)), "{body}: {note}");
    assert!(!r.marked("ran"), "{body}: the check ran");
    r.assert_no_worktree(body);
    assert_eq!(
        changed(&before, &r.outside_the_records()),
        Vec::<String>::new(),
        "{body}"
    );
    let records: Vec<String> = fs::read_dir(r.records())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("check-") && name != "check-request.json")
        .collect();
    assert_eq!(records, Vec::<String>::new(), "{body}");
    note
}

/// "json" alone would prove nothing: every such note names `check-request.json`.
#[test]
fn bad_json_or_a_missing_id_is_refused() {
    let note = refused_without_an_id("{not json", &["refused"]);
    assert!(note.to_lowercase().contains("valid json"), "{note}");
    refused_without_an_id("[]", &["id"]);
    refused_without_an_id(r#"{"commit":"HEAD"}"#, &["id"]);
    refused_without_an_id(r#"{"id":"","commit":"HEAD"}"#, &["id"]);
}

/// An id names files and a folder outside the sandbox, and a line of the record.
#[test]
fn an_unsafe_id_is_refused() {
    for id in [
        "..",
        "../../evil",
        "a/b",
        "a\\b",
        ".hidden",
        "-flag",
        "a b",
        "a\nStatus: passed",
    ] {
        let body = json!({ "id": id, "commit": "HEAD" }).to_string();
        refused_without_an_id(&body, &["id"]);
    }
}

/// `broker-<slug>-<id>` splits at its last hyphen only if no id holds one, so no
/// two runs can share a worktree.
#[test]
fn a_hyphenated_id_is_refused_by_note() {
    let body = json!({ "id": "b-c", "commit": "HEAD" }).to_string();
    refused_without_an_id(&body, &["hyphens"]);
}

#[test]
fn an_empty_or_unknown_commit_is_refused_on_the_record() {
    for (body, word) in [
        (json!({ "id": ID }), "commit"),
        (json!({ "id": ID, "commit": "" }), "commit"),
        (json!({ "id": ID, "commit": "no-such-rev" }), "no-such-rev"),
        (json!({ "id": ID, "commit": "--all" }), "--all"),
    ] {
        let r = Repo::new();
        r.request(&body);
        let (rec, note) = r.assert_refused(&body.to_string());
        let reason = rec.get("Reason");
        assert!(
            says(reason, "commit") || says(reason, word),
            "{body}: {rec:?}"
        );
        assert!(says(&note, "commit") || says(&note, word), "{body}: {note}");
    }
}

#[test]
fn no_declared_check_is_not_run_and_says_so() {
    // A blank command is no command: `sh -c " "` exits 0, a pass with no check.
    for declared in [None, Some(""), Some("  ")] {
        let r = Repo::new();
        r.ask();
        let env: Vec<(&str, &str)> = declared.map(|v| (CMD, v)).into_iter().collect();
        let note = r.fire(&env).note();
        let rec = r.record(ID);
        assert_eq!(rec.get("Status"), "not-run", "{declared:?}: {rec:?}");
        assert!(
            rec.get("Reason").contains("no full check declared"),
            "{declared:?}: {rec:?}"
        );
        assert!(
            note.starts_with("check-broker c1: not-run"),
            "{declared:?}: {note}"
        );
        assert!(
            note.contains("no full check declared"),
            "{declared:?}: {note}"
        );
        r.assert_no_worktree("no declared check");
    }
}

/// The check runs outside the agent's permissions, so the command never comes
/// from a request: not instead of the declared one, not in place of a missing one.
#[test]
fn a_command_in_the_request_is_ignored() {
    let r = Repo::new();
    let smuggled = r.marking("smuggled", "exit 0");
    let body = |id: &str| {
        json!({
            "id": id,
            "commit": "HEAD",
            "command": smuggled,
            "cmd": smuggled,
            "check": smuggled,
            "OCGEN_FULL_CHECK_CMD": smuggled,
        })
    };
    r.request(&body(ID));
    let declared = r.marking("declared", "exit 0");
    r.fire(&[(CMD, declared.as_str())]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert_eq!(rec.get("Command"), declared, "{rec:?}");
    assert!(r.marked("declared"));
    assert!(!r.marked("smuggled"));

    r.request(&body("c2"));
    r.fire(&[]);
    assert_eq!(r.record("c2").get("Status"), "not-run");
    assert!(!r.marked("smuggled"));
}

// ---------------------------------------------------------------- overlays --

#[test]
fn an_overlay_s_new_file_reaches_the_check_and_nowhere_else() {
    let r = Repo::new();
    let carried = r.records().join("overlay-1/tests/extra.txt");
    write(&carried, "from the overlay\n");
    r.ask_with_overlay("overlay-1");
    r.fire(&[(CMD, "cat tests/extra.txt")]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert!(r.log_text(ID).contains("from the overlay"));
    assert!(!r.project.join("tests/extra.txt").exists());
    assert!(carried.is_file(), "copied, not moved");
    r.assert_no_worktree("an overlay");

    // "Passed at <sha>" is that commit plus these files, and the record names them.
    assert_eq!(rec.get("Overlay"), "overlay-1", "{rec:?}");
    assert_eq!(rec.get("Overlay-Files"), "1", "{rec:?}");
    let sum = rec.get("Overlay-Sum");
    assert!(!sum.is_empty(), "{rec:?}");
    // The sum follows what a file holds, and where it lands.
    write(&carried, "other content\n");
    r.request(&json!({ "id": "c2", "commit": "HEAD", "overlay": "overlay-1" }));
    r.fire(&[(CMD, "exit 0")]);
    let held = r.record("c2");
    assert_ne!(held.get("Overlay-Sum"), sum, "{held:?}");
    write(&r.records().join("overlay-2/moved.txt"), "other content\n");
    r.request(&json!({ "id": "c3", "commit": "HEAD", "overlay": "overlay-2" }));
    r.fire(&[(CMD, "exit 0")]);
    let moved = r.record("c3");
    assert_ne!(
        moved.get("Overlay-Sum"),
        held.get("Overlay-Sum"),
        "{moved:?}"
    );
}

/// Overlay paths are relative to the project: a check that discovers its tests in
/// its own folder sees them, and it is there that a path collides.
#[test]
fn an_overlay_lands_in_the_project_s_folder_of_the_checkout() {
    let r = Repo::with_project_at("sub/proj");
    let carried = r.records().join("overlay-1/tests/extra.txt");
    write(&carried, "from the overlay\n");
    r.ask_with_overlay("overlay-1");
    r.fire(&[(CMD, "cat tests/extra.txt")]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert!(r.log_text(ID).contains("from the overlay"));

    let r = Repo::with_project_at("sub/proj");
    write(&r.records().join("overlay-1/app.txt"), "overwritten\n");
    r.ask_with_overlay("overlay-1");
    let (rec, _) = r.assert_refused("a collision in the project's folder");
    assert!(rec.get("Reason").contains("app.txt"), "{rec:?}");
}

/// An unfilled template value or a folder with nothing in it must not read as a
/// pass with an overlay; `null` or no key at all is no overlay.
#[test]
fn an_empty_overlay_is_refused() {
    let r = Repo::new();
    fs::create_dir_all(r.records().join("empty/nested")).unwrap();
    for overlay in ["", "empty"] {
        r.ask_with_overlay(overlay);
        r.assert_refused(overlay);
        r.forget_record();
    }
    r.request(&json!({ "id": ID, "commit": "HEAD", "overlay": null }));
    r.fire(&[(CMD, "exit 0")]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert_eq!(rec.get("Overlay"), "", "{rec:?}");
}

/// A committed file on the way is taken too: the path can't be made.
#[test]
fn an_overlay_path_through_a_committed_file_is_refused() {
    let r = Repo::new();
    write(&r.records().join("overlay-1/app.txt/inner.txt"), "x\n");
    r.ask_with_overlay("overlay-1");
    let (rec, _) = r.assert_refused("a file on the way");
    assert!(rec.get("Reason").contains("app.txt"), "{rec:?}");
}

/// A committed symlink on the way would have the copy, which runs outside the
/// sandbox, write wherever the link points.
#[cfg(unix)]
#[test]
fn an_overlay_path_through_a_committed_symlink_is_refused() {
    let r = Repo::new();
    let outside = r.marks.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.project.join("linked")).unwrap();
    git(&r.top, &["add", "linked"]);
    git(&r.top, &["commit", "-qm", "a link"]);
    write(&r.records().join("overlay-1/linked/leak.txt"), "x\n");
    r.ask_with_overlay("overlay-1");
    let (rec, _) = r.assert_refused("a symlink on the way");
    assert!(rec.get("Reason").contains("linked"), "{rec:?}");
    assert!(!outside.join("leak.txt").exists(), "written through it");
}

#[test]
fn an_overlay_that_collides_is_refused_by_path_and_runs_nothing() {
    let r = Repo::new();
    write(&r.records().join("overlay-1/new.txt"), "new\n");
    write(&r.records().join("overlay-1/app.txt"), "overwritten\n");
    r.ask_with_overlay("overlay-1");
    let (rec, note) = r.assert_refused("a collision");
    assert!(rec.get("Reason").contains("app.txt"), "{rec:?}");
    assert!(note.contains("app.txt"), "{note}");
    assert_eq!(
        fs::read_to_string(r.project.join("app.txt")).unwrap(),
        "committed\n"
    );
}

#[test]
fn an_overlay_outside_the_record_folder_is_refused() {
    let r = Repo::new();
    write(&r.project.join(".claude/deliver/other/new.txt"), "x\n");
    write(&r.records().join("sub/keep.txt"), "x\n");
    write(&r.records().join("overlay-1/new.txt"), "x\n");
    write(&r.marks.join("outside/new.txt"), "x\n");
    for overlay in [
        "../other".to_string(),
        "sub/../../other".to_string(),
        // `..` is refused even where it ends up inside.
        "sub/../overlay-1".to_string(),
        for_shell(&r.marks.join("outside")),
        // An absolute value is refused even where it names the record folder.
        for_shell(&r.records().join("overlay-1")),
    ] {
        r.ask_with_overlay(&overlay);
        r.assert_refused(&overlay);
        r.forget_record();
    }
}

/// A check run without the files it was asked to see could pass for the wrong
/// reason.
#[test]
fn an_overlay_folder_that_isn_t_there_is_refused() {
    let r = Repo::new();
    write(&r.records().join("a-file"), "x\n");
    for overlay in ["no-such-folder", "a-file"] {
        r.ask_with_overlay(overlay);
        let (rec, _) = r.assert_refused(overlay);
        assert!(rec.get("Reason").contains(overlay), "{rec:?}");
        r.forget_record();
    }
}

/// The hook runs outside the sandbox, so it never follows a link out of the
/// record folder and into a check.
#[cfg(unix)]
#[test]
fn a_symlink_in_or_as_an_overlay_is_refused() {
    use std::os::unix::fs::symlink;
    let r = Repo::new();
    let outside = r.marks.join("outside");
    write(&outside.join("secret.txt"), "secret\n");
    let with_file = r.records().join("with-file");
    write(&with_file.join("new.txt"), "new\n");
    symlink(outside.join("secret.txt"), with_file.join("leak.txt")).unwrap();
    let with_folder = r.records().join("with-folder");
    fs::create_dir_all(with_folder.join("a")).unwrap();
    symlink(&outside, with_folder.join("a/leak")).unwrap();
    symlink(&outside, r.records().join("is-a-link")).unwrap();
    for overlay in ["with-file", "with-folder", "is-a-link"] {
        r.ask_with_overlay(overlay);
        r.assert_refused(overlay);
        r.forget_record();
    }
}

// ----------------------------------------------------- what the check sees --

#[test]
fn the_check_runs_at_the_requested_commit_not_the_working_tree() {
    let r = Repo::new();
    let version = r.project.join("version.txt");
    write(&version, "first-commit\n");
    git(&r.top, &["add", "."]);
    git(&r.top, &["commit", "-qm", "first"]);
    let first = r.head();
    write(&version, "second-commit\n");
    git(&r.top, &["add", "."]);
    git(&r.top, &["commit", "-qm", "second"]);

    r.request(&json!({ "id": ID, "commit": "HEAD~1" }));
    r.fire(&[(CMD, "cat version.txt")]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert_eq!(rec.get("Commit"), first, "the full SHA: {rec:?}");
    let log = r.log_text(ID);
    assert!(log.contains("first-commit"), "{log}");
    assert!(!log.contains("second-commit"), "{log}");
}

#[test]
fn uncommitted_changes_in_the_checkout_don_t_reach_the_check() {
    let r = Repo::new();
    write(&r.project.join("app.txt"), "dirty\n");
    write(&r.project.join("loose.txt"), "untracked\n");
    write(&r.project.join("staged.txt"), "staged\n");
    git(&r.top, &["add", "staged.txt"]);
    let before = git(&r.top, &["status", "--porcelain"]);

    r.ask();
    let check = "cat app.txt; test ! -e loose.txt && test ! -e staged.txt";
    r.fire(&[(CMD, check)]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    let log = r.log_text(ID);
    assert!(log.contains("committed") && !log.contains("dirty"), "{log}");

    // And the user's work is where it was.
    assert_eq!(git(&r.top, &["status", "--porcelain"]), before);
    assert_eq!(
        fs::read_to_string(r.project.join("app.txt")).unwrap(),
        "dirty\n"
    );
}

/// A check that finds the project through `$CLAUDE_PROJECT_DIR` sees the requested
/// commit too, in a project at the repository's top or in a subfolder of it.
#[test]
fn claude_project_dir_is_the_check_s_own_checkout() {
    let check = r#"cat "$CLAUDE_PROJECT_DIR/app.txt"; echo "$CLAUDE_PROJECT_DIR""#;
    for (sub, end) in [("", "broker-s1-c1"), ("sub/proj", "broker-s1-c1/sub/proj")] {
        let r = Repo::with_project_at(sub);
        write(&r.project.join("app.txt"), "dirty\n");
        r.ask();
        r.fire(&[(CMD, check)]);
        let log = r.log_text(ID);
        let rec = r.record(ID);
        assert_eq!(rec.get("Status"), "passed", "{sub}: {rec:?}: {log}");
        assert!(log.contains("committed") && !log.contains("dirty"), "{log}");
        assert!(log.trim_end().ends_with(end), "{sub}: {log}");
    }
}

#[test]
fn a_project_in_a_subfolder_runs_in_the_matching_subfolder() {
    let r = Repo::with_project_at("sub/proj");
    r.ask();
    r.fire(&[(CMD, "cat app.txt; pwd")]);
    let log = r.log_text(ID);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}: {log}");
    assert!(log.contains("broker-s1-c1/sub/proj"), "{log}");
    r.assert_no_worktree("a project in a subfolder");
    assert_eq!(
        git(&r.top, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
}

/// `git worktree add` runs `post-checkout`, and this process runs outside the
/// sandbox; a throwaway checkout needs no hooks.
#[test]
fn the_repository_s_git_hooks_stay_out_of_the_throwaway_checkout() {
    let r = Repo::new();
    let hooks = r.marks.join("git-hooks");
    let script = hooks.join("post-checkout");
    let mark = for_shell(&r.mark("post-checkout"));
    write(&script, &format!("#!/bin/sh\necho ran >> '{mark}'\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(&r.top, &["config", "core.hooksPath", &for_shell(&hooks)]);

    // The fixture bites: a plain `git worktree add` runs the hook.
    let control = for_shell(&r.marks.join("control"));
    git(
        &r.top,
        &["worktree", "add", "-q", "--detach", &control, "HEAD"],
    );
    assert!(r.marked("post-checkout"), "the control didn't run the hook");
    git(&r.top, &["worktree", "remove", "--force", &control]);
    fs::remove_file(r.mark("post-checkout")).unwrap();

    r.ask();
    r.fire(&[(CMD, "exit 0")]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert!(!r.marked("post-checkout"), "the hook ran");
    r.assert_no_worktree("hooks disabled");
}

// ------------------------------------------------------------ earlier runs --

#[cfg(unix)]
#[test]
fn a_dead_earlier_run_becomes_interrupted_and_loses_its_worktree() {
    let r = Repo::new();
    let both = r.seed_started("old", dead_pid(), Some(dead_pid()));
    // Killed before it started the check: its record has no check PID yet.
    let early = r.seed_started("early", dead_pid(), None);
    // A finished run isn't a running one.
    let done = r.records().join("check-done.md");
    let finished = "Status: passed\nId: done\nExit: 0\n";
    write(&done, finished);

    r.ask();
    let check = r.marking("ran", "exit 0");
    r.fire(&[(CMD, check.as_str())]);

    for (id, wt) in [("old", &both), ("early", &early)] {
        let rec = r.record(id);
        assert_eq!(rec.get("Status"), "interrupted", "{id}: {rec:?}");
        assert!(!wt.exists(), "{id}: its worktree stayed");
    }
    assert_eq!(fs::read_to_string(&done).unwrap(), finished);
    r.assert_no_worktree("after the sweep");
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert!(r.marked("ran"), "the new request ran");
}

#[cfg(unix)]
#[test]
fn a_live_earlier_run_refuses_the_request() {
    let me = std::process::id();
    // Alive through its hook; through its check, which outlives a killed hook;
    // and through a process of another user's, where `kill` answers EPERM (PID
    // 1, which a root test run sees as plainly alive).
    for (hook, check) in [(me, dead_pid()), (dead_pid(), me), (1, dead_pid())] {
        let what = format!("hook {hook}, check {check}");
        let r = Repo::new();
        let wt = r.seed_started("old", hook, Some(check));
        let old = r.records().join("check-old.md");
        let was = fs::read(&old).unwrap();

        r.ask();
        let run = r.marking("ran", "exit 0");
        let note = r.fire(&[(CMD, run.as_str())]).note();

        let rec = r.record(ID);
        assert_eq!(rec.get("Status"), "refused", "{what}: {rec:?}");
        assert!(
            rec.get("Reason").contains("already running"),
            "{what}: {rec:?}"
        );
        assert!(note.contains("already running"), "{what}: {note}");
        assert!(!r.marked("ran"), "{what}: a second copy ran");
        assert!(!r.worktree(ID).exists(), "{what}");
        assert!(wt.is_dir(), "{what}: the live run lost its worktree");
        assert_eq!(fs::read(&old).unwrap(), was, "{what}: its record changed");
    }
}

/// The `Worktree:` line is only a record: the sweep derives the path from the
/// slug and the id, so a garbled line can't cost anyone a folder.
#[cfg(unix)]
#[test]
fn a_garbled_worktree_line_leaves_that_folder_alone() {
    let r = Repo::new();
    git(
        &r.top,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree-agent-abc",
            ".claude/worktrees/agent-abc",
        ],
    );
    let agent = r.project.join(".claude/worktrees/agent-abc");
    let folder = r.marks.join("a-folder");
    write(&folder.join("work.txt"), "work\n");
    let garbled = [("old", &agent), ("older", &folder)];
    for (id, path) in garbled {
        let wt = r.seed_started(id, dead_pid(), None);
        let record = r.records().join(format!("check-{id}.md"));
        let text = fs::read_to_string(&record).unwrap();
        let text = text.replace(&for_shell(&wt), &for_shell(path));
        fs::write(&record, text).unwrap();
    }

    r.ask();
    r.fire(&[(CMD, "exit 0")]);

    for (id, path) in garbled {
        assert_eq!(r.record(id).get("Status"), "interrupted", "{id}");
        assert!(path.is_dir(), "{id}: {path:?} went");
        assert!(!r.worktree(id).exists(), "{id}: its own worktree stayed");
    }
    assert!(agent.join("app.txt").is_file());
    assert!(folder.join("work.txt").is_file());
    let list = git(&r.top, &["worktree", "list", "--porcelain"]);
    assert!(list.contains("agent-abc"), "{list}");
}

/// A dead run whose folder is already gone still has its entry in git's list, and
/// the sweep clears that too.
#[cfg(unix)]
#[test]
fn a_dead_run_whose_folder_is_gone_leaves_the_worktree_list_clean() {
    let r = Repo::new();
    let wt = r.seed_started("old", dead_pid(), None);
    fs::remove_dir_all(&wt).unwrap();
    r.ask();
    r.fire(&[(CMD, "exit 0")]);
    assert_eq!(r.record("old").get("Status"), "interrupted");
    assert_eq!(r.record(ID).get("Status"), "passed");
    r.assert_no_worktree("a dead run with no folder");
}

/// A plain `git worktree prune` drops the entry of every worktree whose folder is
/// missing, one on an unmounted disk included, so a run never prunes on its own.
#[test]
fn a_run_leaves_another_worktree_with_a_missing_folder_registered() {
    let r = Repo::new();
    git(
        &r.top,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree-agent-abc",
            ".claude/worktrees/agent-abc",
        ],
    );
    fs::remove_dir_all(r.project.join(".claude/worktrees/agent-abc")).unwrap();
    r.ask();
    r.fire(&[(CMD, "exit 0")]);
    assert_eq!(r.record(ID).get("Status"), "passed");
    let list = git(&r.top, &["worktree", "list", "--porcelain"]);
    assert!(list.contains("agent-abc"), "{list}");
}

/// Finished evidence is never overwritten: a request with an id already on
/// record gets a note, and the record and the log stay as they are.
#[test]
fn an_id_already_on_record_is_refused_by_note_and_keeps_its_evidence() {
    let r = Repo::new();
    r.ask();
    r.fire(&[(CMD, "echo first-run")]);
    let record = r.records().join("check-c1.md");
    let (record_was, log_was) = (fs::read(&record).unwrap(), r.log(ID));
    assert!(!log_was.is_empty());

    let check = r.marking("ran", "exit 1");
    let note = r.fire(&[(CMD, check.as_str())]).note();
    assert!(note.starts_with("check-broker c1: refused"), "{note}");
    assert!(note.contains("new id"), "{note}");
    assert!(!r.marked("ran"), "the check ran again");
    assert_eq!(fs::read(&record).unwrap(), record_was);
    assert_eq!(r.log(ID), log_was);
    r.assert_no_worktree("a reused id");
}

/// A live run keeps its record and its worktree when a request reuses its id; a
/// dead one is swept first, and the request is refused all the same.
#[cfg(unix)]
#[test]
fn reusing_the_id_of_an_earlier_run_is_refused_by_note_after_the_sweep() {
    let r = Repo::new();
    let wt = r.seed_started(ID, std::process::id(), None);
    let record = r.records().join("check-c1.md");
    let was = fs::read(&record).unwrap();
    r.ask();
    let check = r.marking("ran", "exit 0");
    let note = r.fire(&[(CMD, check.as_str())]).note();
    assert!(note.starts_with("check-broker c1: refused"), "{note}");
    assert!(note.contains("new id"), "{note}");
    assert!(!r.marked("ran"), "a second copy ran");
    assert_eq!(fs::read(&record).unwrap(), was);
    assert!(wt.is_dir(), "the live run lost its worktree");

    let r = Repo::new();
    let wt = r.seed_started(ID, dead_pid(), None);
    r.ask();
    let check = r.marking("ran", "exit 0");
    let note = r.fire(&[(CMD, check.as_str())]).note();
    assert!(note.starts_with("check-broker c1: refused"), "{note}");
    assert!(note.contains("new id"), "{note}");
    assert!(!r.marked("ran"), "the check ran under a used id");
    assert_eq!(r.record(ID).get("Status"), "interrupted");
    assert!(!wt.exists(), "the sweep didn't run first");
    r.assert_no_worktree("a dead run's id");
}

// ------------------------------------------------------------------ budget --

/// Whether process `pid` still runs a few seconds on: gone, or a zombie (killed,
/// not yet reaped by whoever inherited it), is not running. `ps` is what can tell
/// a zombie, but a sandboxed session can't start it: there a failed `kill -0`
/// means gone.
#[cfg(unix)]
fn still_runs(pid: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let probe = format!("kill -0 {pid}");
    loop {
        let gone = match Command::new("ps").args(["-o", "stat=", "-p", pid]).output() {
            Ok(out) => {
                let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
                stat.is_empty() || stat.starts_with('Z')
            }
            Err(_) => !Command::new("sh")
                .args(["-c", probe.as_str()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
        };
        if gone {
            return false;
        }
        if std::time::Instant::now() >= deadline {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Wait for `done`, up to a deadline only a stuck run reaches.
#[cfg(unix)]
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "no {what}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Run `script` in `sh`: whether it succeeded.
#[cfg(unix)]
fn sh(script: &str) -> bool {
    Command::new("sh")
        .args(["-c", script])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A test's forgotten server is common good-faith behaviour, so it doesn't flip
/// the result; but it doesn't run on under the worktree's removal either.
#[cfg(unix)]
#[test]
fn what_a_check_leaves_running_is_killed_and_recorded() {
    let r = Repo::new();
    r.ask();
    let child = for_shell(&r.mark("child.pid"));
    let check = format!("sleep 30 & echo $! > '{child}'; exit 0");
    let note = r.fire(&[(CMD, check.as_str())]).note();
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert_eq!(rec.get("Exit"), "0", "{rec:?}");
    assert_eq!(rec.get("Left-Behind"), "yes", "{rec:?}");
    assert!(note.contains("left behind"), "{note}");
    r.assert_no_worktree("a check that left a process");
    let pid = fs::read_to_string(r.mark("child.pid")).unwrap();
    let alive = still_runs(pid.trim());
    if alive {
        let _ = Command::new("kill").args(["-9", pid.trim()]).output();
    }
    assert!(!alive, "the child still runs");
}

/// The hook may be killed, or hit its `timeout`, during the cleanup: by then the
/// outcome is on record. `git worktree remove` is held open so the record can be
/// read mid-cleanup.
#[cfg(unix)]
#[test]
fn the_outcome_is_on_record_before_the_worktree_goes() {
    let r = Repo::new();
    r.ask();
    let holding = for_shell(&r.mark("removing.pid"));
    let arms = format!("*'worktree remove'*) echo $$ > '{holding}'; exec sleep 30 ;;");
    let path = r.shim_git(&arms);
    let mut hook = r.start(&[(CMD, "exit 3"), ("PATH", path.as_str())]);
    wait_until("removal of the worktree", || r.marked("removing.pid"));
    let rec = r.record(ID);
    hook.kill().unwrap();
    hook.wait().unwrap();
    let pid = fs::read_to_string(r.mark("removing.pid")).unwrap();
    sh(&format!("kill -9 {}", pid.trim()));
    assert_eq!(rec.get("Status"), "failed", "{rec:?}");
    assert_eq!(rec.get("Exit"), "3", "{rec:?}");
    assert!(rec.get("Duration-s").parse::<f64>().is_ok(), "{rec:?}");
}

/// Running at the repository's top instead would check the wrong folder.
#[cfg(unix)]
#[test]
fn a_project_folder_git_can_t_name_is_not_run() {
    let r = Repo::with_project_at("sub/proj");
    r.ask();
    let path = r.shim_git("*--show-prefix*) exit 1 ;;");
    let check = r.marking("ran", "exit 0");
    r.fire(&[(CMD, check.as_str()), ("PATH", path.as_str())]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "not-run", "{rec:?}");
    assert!(!r.marked("ran"), "the check ran");
    r.assert_no_worktree("no prefix");
}

/// The whole life of a killed hook: its check outlives it in a process group of
/// its own and holds the folder, until that group dies and the next request sweeps.
#[cfg(unix)]
#[test]
fn a_killed_hook_s_check_holds_the_folder_until_its_group_dies() {
    let r = Repo::new();
    r.request(&json!({ "id": "old", "commit": "HEAD" }));
    let mut hook = r.start(&[(CMD, "sleep 60")]);
    let record = r.records().join("check-old.md");
    let check_pid = || {
        let text = fs::read_to_string(&record).unwrap_or_default();
        let pid = text.lines().find_map(|l| l.strip_prefix("Check-Pid: "));
        pid.unwrap_or("").to_string()
    };
    wait_until("Check-Pid", || !check_pid().is_empty());
    let pid = check_pid();
    hook.kill().unwrap();
    hook.wait().unwrap();

    r.ask();
    let check = r.marking("ran", "exit 0");
    let note = r.fire(&[(CMD, check.as_str())]).note();
    assert!(note.contains("already running"), "{note}");
    assert!(!r.marked("ran"), "a second copy ran");
    assert_eq!(r.record("old").get("Status"), "started");

    assert!(sh(&format!("kill -s KILL -- -{pid}")), "the group is gone");
    wait_until("end of the check", || !sh(&format!("kill -0 {pid}")));
    r.request(&json!({ "id": "c2", "commit": "HEAD" }));
    r.fire(&[(CMD, "exit 0")]);
    assert_eq!(r.record("old").get("Status"), "interrupted");
    assert_eq!(r.record("c2").get("Status"), "passed");
    r.assert_no_worktree("after the sweep");
}

/// A slow loser must not overwrite a finished record: the id goes to whoever
/// claims it first, and the check runs once.
#[cfg(unix)]
#[test]
fn two_requests_for_one_id_run_it_once() {
    let r = Repo::new();
    r.ask();
    let check = r.marking("ran", "sleep 1; exit 0");
    let mut hooks: Vec<_> = (0..4).map(|_| r.start(&[(CMD, check.as_str())])).collect();
    for hook in &mut hooks {
        assert!(hook.wait().unwrap().success());
    }
    let ran = fs::read_to_string(r.mark("ran")).unwrap();
    assert_eq!(ran.lines().count(), 1, "{ran}");
    assert_eq!(r.record(ID).get("Status"), "passed");
    r.assert_no_worktree("one id, four requests");
}

#[cfg(unix)]
#[test]
fn a_check_over_its_budget_is_a_timeout_and_takes_its_children_with_it() {
    let r = Repo::new();
    r.ask();
    let (leader, child) = (
        for_shell(&r.mark("leader.pid")),
        for_shell(&r.mark("child.pid")),
    );
    let check = format!(
        "echo before-the-hang; echo $$ > '{leader}'; sleep 30 & echo $! > '{child}'; sleep 30"
    );
    let started = std::time::Instant::now();
    let note = r
        .fire(&[(CMD, check.as_str()), ("OCGEN_FULL_CHECK_TIMEOUT", "1")])
        .note();
    assert!(started.elapsed().as_secs() < 20, "it waited for the check");

    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "timeout", "{rec:?}");
    assert!(note.starts_with("check-broker c1: timeout"), "{note}");
    assert!(r.log_text(ID).contains("before-the-hang"));
    r.assert_no_worktree("a timeout");
    for name in ["leader.pid", "child.pid"] {
        let pid = fs::read_to_string(r.mark(name)).unwrap();
        let alive = still_runs(pid.trim());
        if alive {
            let _ = Command::new("kill").args(["-9", pid.trim()]).output();
        }
        assert!(!alive, "{name} still runs");
    }
}

/// `OCGEN_CHECK_TIMEOUT` is the gates' budget, not this hook's.
#[cfg(unix)]
#[test]
fn the_gates_check_timeout_doesn_t_limit_the_broker() {
    let r = Repo::new();
    r.ask();
    r.fire(&[
        (CMD, "sleep 2; echo survived"),
        ("OCGEN_CHECK_TIMEOUT", "1"),
    ]);
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "passed", "{rec:?}");
    assert!(r.log_text(ID).contains("survived"));
}

// --------------------------------------------------------- what's recorded --

/// Every value stays on one line, so a command can't forge a `Status:` line.
#[test]
fn a_two_line_command_runs_and_can_t_forge_a_record_line() {
    let r = Repo::new();
    r.ask();
    let check = "echo first-line\nStatus: passed\nexit 3";
    let note = r.fire(&[(CMD, check)]).note();
    assert!(
        note.starts_with("check-broker c1: failed, exit 3, "),
        "{note}"
    );
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "failed", "{rec:?}");
    assert_eq!(rec.get("Exit"), "3", "{rec:?}");
    assert_eq!(
        rec.get("Command"),
        r"echo first-line\nStatus: passed\nexit 3"
    );
    let text = fs::read_to_string(r.records().join("check-c1.md")).unwrap();
    let statuses = text.lines().filter(|l| l.starts_with("Status:")).count();
    assert_eq!(statuses, 1, "{text}");
    assert!(r.log_text(ID).contains("first-line"));
}

/// 125 is the sandbox helper's "couldn't apply" only where the helper ran: with
/// no sandbox it is the check's own exit code.
#[test]
fn a_check_exiting_125_without_the_sandbox_is_plain_failed() {
    let r = Repo::new();
    r.ask();
    let note = r.fire(&[(CMD, "exit 125")]).note();
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "failed", "{rec:?}");
    assert_eq!(rec.get("Exit"), "125", "{rec:?}");
    assert!(
        note.starts_with("check-broker c1: failed, exit 125, "),
        "{note}"
    );
}

/// `.git` is moved aside rather than deleted (Windows keeps its objects
/// read-only), and git stops looking at the temp root, wherever that lies.
#[test]
fn outside_any_git_repository_the_check_is_not_run() {
    let r = Repo::new();
    fs::rename(r.top.join(".git"), r.marks.join("moved-git")).unwrap();
    r.request(&json!({ "id": ID, "commit": "HEAD" }));
    let check = r.marking("ran", "exit 0");
    let ceiling = for_shell(&r.root);
    let note = r
        .fire(&[
            (CMD, check.as_str()),
            ("GIT_CEILING_DIRECTORIES", ceiling.as_str()),
        ])
        .note();
    let rec = r.record(ID);
    assert_eq!(rec.get("Status"), "not-run", "{rec:?}");
    assert!(
        rec.get("Reason").contains("not a git repository"),
        "{rec:?}"
    );
    assert!(note.starts_with("check-broker c1: not-run"), "{note}");
    assert!(!r.marked("ran"), "the check ran");
    assert!(!r.project.join(".claude/worktrees").exists());
}

/// The hook runs outside the sandbox, so it reads and writes nothing through a
/// link on the way to the record folder.
#[cfg(unix)]
#[test]
fn a_symlink_on_the_way_to_the_record_folder_is_refused() {
    for rel in [".claude/deliver/s1", ".claude/deliver", ".claude"] {
        let r = Repo::new();
        let elsewhere = r.marks.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let link = r.project.join(rel);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        r.ask();
        let check = r.marking("ran", "exit 0");
        let before = r.tree();
        let note = r.fire(&[(CMD, check.as_str())]).note();
        assert!(says(&note, "refused"), "{rel}: {note}");
        assert!(says(&note, "symlink"), "{rel}: {note}");
        assert_eq!(changed(&before, &r.tree()), Vec::<String>::new(), "{rel}");
    }
}

// ------------------------------------------------------------------- state --

/// A worker gate counts untracked files that aren't ignored, so no run may leave
/// one behind — a refused request's file included.
#[test]
fn the_record_folder_keeps_itself_out_of_git_status() {
    let good = json!({ "id": ID, "commit": "HEAD" }).to_string();
    for (body, env) in [
        (good.as_str(), vec![(CMD, "exit 0")]),
        (good.as_str(), vec![(CMD, "exit 1")]),
        (good.as_str(), vec![]),
        ("{not json", vec![(CMD, "exit 0")]),
    ] {
        let r = Repo::new();
        write(&r.project.join(REQUEST), body);
        r.fire(&env);
        assert_eq!(
            git(&r.top, &["status", "--porcelain", "--untracked-files=all"]),
            "",
            "{body} {env:?}"
        );
    }
}

/// `ocgen verify`'s probe: not the check, not the sweep, not a record.
#[test]
fn the_verify_probe_changes_nothing() {
    let r = Repo::new();
    r.ask();
    write(
        &r.records().join("check-old.md"),
        &format!("Status: started\nId: old\nHook-Pid: {}\n", dead_pid()),
    );
    let check = r.marking("ran", "exit 0");
    let before = r.tree();
    r.fire(&[(CMD, check.as_str()), ("OCGEN_HOOK_PROBE", "1")])
        .assert_silent("the probe");
    assert_eq!(changed(&before, &r.tree()), Vec::<String>::new());
}
