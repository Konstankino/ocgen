//! The check-broker hook (`ocgen hook check-broker`): a hook rather than a model
//! runs the project's full check and writes the evidence.
//!
//! A write of `.claude/deliver/<slug>/check-request.json` in the coordinator's
//! checkout asks for one run: `{"id": "<name>", "commit": "<rev>", "overlay":
//! "<optional folder relative to the record folder>"}`. The check runs outside the
//! agent's permissions, so its command never comes from a request: it comes from
//! `OCGEN_FULL_CHECK_CMD`, with `OCGEN_FULL_CHECK_TIMEOUT` as its budget (default
//! [`CHECK_TIMEOUT_MAX`](crate::hooks::CHECK_TIMEOUT_MAX)). It runs in a throwaway
//! worktree at that commit, in the project's folder there, with `CLAUDE_PROJECT_DIR`
//! pointing at that folder. That keeps the checkout's uncommitted changes out of its
//! tree, but two things still reach it: an overlay's files, whose paths are relative
//! to the project and whose name, count and sum go on the record, and whatever a
//! lookup through parent folders finds in the checkout (see the limits below).
//!
//! The record folder keeps `check-<id>.md` (plain `Key: value` lines) and
//! `check-<id>.log` (the whole output, raw bytes). `mirror_note` copies only its own
//! note folders, so `.claude/deliver` isn't mirrored, whatever a file's extension. A
//! run claims its id once, so evidence is never overwritten, and an id holds no
//! hyphen, so `broker-<slug>-<id>` names one run.
//!
//! Unregistered on purpose: this is a prototype unwired from `/deliver`, so it
//! reaches no generated command, and registering it would put it in every project
//! (`tests/scaffold.rs` pins each project's hook scripts). So it has no script in
//! `templates/`, no `components` or `hook_table` entry in `src/render.rs`, and no
//! `PROTOCOL` bump. That makes it the one exception to the mirroring rule at the top
//! of [`crate::hooks`]: no generated hook command calls it, so there is no fallback
//! for an `sh` twin to serve. To try it, add the hook entry by hand (PostToolUse on
//! `Write|Edit|MultiEdit`, `ocgen hook check-broker`, a `timeout` of the budget plus
//! room for cleanup) and set `OCGEN_FULL_CHECK_CMD`.
//!
//! Not in the prototype: the `git apply` route for inline tests, copying what
//! `.worktreeinclude` lists into the worktree, and any Stop or run-state hook.
//!
//! Known limits:
//! - `pid-reuse`: a `started` record whose PID the system has since given to another
//!   process reads as alive, so that folder's requests are refused as
//!   `already running` until that process ends or someone removes the record.
//! - `started-swept-only-by-a-later-request`: nothing else sweeps, so a dead run stays
//!   `started`, with its worktree, until another request comes to its record folder.
//! - `orphaned-check-has-no-budget`: only the hook enforces the budget. A check whose
//!   hook was killed runs to its own end, and holds its record folder until then.
//! - `windows-never-sweeps`: liveness can't be told there, so a dead run stays
//!   `started` for good, and a process a check left behind isn't found.
//! - `log-grows-without-limit`: the log takes all the output, however large.
//! - `checks-see-the-checkout-through-parent-lookups`: a fresh worktree inside the
//!   project reaches the checkout's `node_modules` and `.cargo/config.toml` by walking
//!   up. Moving it outside the project is the open fix.
//! - `ids-side-by-side`: requests with different ids that pass the sweep together run
//!   side by side; only one id's requests are held to one run.
//! - `killed-during-cleanup`: a hook killed while removing the worktree has already
//!   recorded the outcome, so no sweep takes that worktree: it stays until removed by
//!   hand.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::github::write_whole;
use crate::hooks::{cksum, git_line, git_ok, kill_tree};
use crate::notes::brief::is_name;
use crate::notes::session::ignore_self;
use crate::paths::{for_shell, plain};
use crate::sandbox::NOT_APPLIED;

/// The request's name in its record folder.
const REQUEST: &str = "check-request.json";
/// Where this hook's worktrees lie, in the project.
const WORKTREES: &str = ".claude/worktrees";
/// What starts the name of a worktree this hook made. Only such a one is removed.
const BROKER: &str = "broker-";
/// How much of the output's end the note carries, in bytes. Claude Code replaces
/// context above 10,000 characters with a path and a preview, so the whole output
/// stays in the log.
const TAIL: usize = 2048;

/// One run of the hook, as [`crate::hooks`] sets it up.
pub(crate) struct Job<'a> {
    /// The project: the coordinator's checkout, never the event's `cwd`, which
    /// follows a subagent.
    pub(crate) root: PathBuf,
    pub(crate) slug: &'a str,
    /// The declared check (`OCGEN_FULL_CHECK_CMD`).
    pub(crate) cmd: &'a str,
    pub(crate) budget: Duration,
    /// Whether the check goes through the sandbox helper (`OCGEN_SANDBOX=1`).
    pub(crate) sandboxed: bool,
    /// How a hook runs project code: in the sandbox when the project's is on.
    pub(crate) command: &'a dyn Fn(&str, &Path) -> Result<Command, String>,
}

/// The slug of a request: `rel`, relative to the project with forward slashes, is
/// exactly `.claude/deliver/<slug>/check-request.json`.
pub(crate) fn request_slug(rel: &str) -> Option<&str> {
    let slug = rel
        .strip_prefix(".claude/deliver/")?
        .strip_suffix("/check-request.json")?;
    is_name(slug).then_some(slug)
}

/// Take the request in the slug's record folder, run the check if it may run, and
/// say what came of it. Whatever goes wrong is said in the note: this never fails
/// the write.
pub(crate) fn answer(job: &Job) -> String {
    let dir = job.root.join(".claude/deliver").join(job.slug);
    // The hook runs outside the sandbox, so it goes through no link a sandboxed
    // shell may have left: nothing is read or written past one.
    let parts = [".claude", "deliver", job.slug, ".gitignore"];
    let link = link_on_the_way(&job.root, &parts).or_else(|| link_on_the_way(&dir, &[REQUEST]));
    if let Some(link) = link {
        return format!("check-broker: refused: {link} is a symlink, and this hook follows none");
    }
    if let Err(e) = fs::create_dir_all(&dir) {
        let shown = for_shell(&dir);
        return format!("check-broker: refused: can't make {shown}: {e}");
    }
    ignore_self(&dir);
    let (request, id) = match read_request(&dir) {
        Ok(read) => read,
        Err(why) => return format!("check-broker: refused: {why}"),
    };

    let swept = sweep(job, &dir);
    let remark = if swept.unknown.is_empty() {
        String::new()
    } else {
        let ids = swept.unknown.join(", ");
        format!("Not swept: whether {ids} still run can't be told on this platform.")
    };
    // Before anything writes a record: evidence is never overwritten, a live
    // run's least of all. Claiming the id in one step stops two requests for it
    // racing past each other, so a slow loser can't overwrite a finished record.
    let record = dir.join(format!("check-{id}.md"));
    let log = dir.join(format!("check-{id}.log"));
    let claimed = if fs::symlink_metadata(&log).is_ok() {
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
    } else {
        claim(&record)
    };
    if let Err(e) = claimed {
        let shown = for_shell(&record);
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return format!("check-broker {id}: not-run: can't claim {shown}: {e}");
        }
        return format!(
            "check-broker {id}: refused: {shown} is already on record, and evidence is never \
             overwritten. Pick a new id and write the request again."
        );
    }
    let mut run = Run {
        job,
        dir,
        log,
        remark,
        record: Record {
            id,
            command: job.cmd.trim().to_string(),
            hook_pid: std::process::id().to_string(),
            ..Record::default()
        },
    };
    if let Some(live) = swept.live {
        return run.end("refused", format!("already running: {live} is still alive"));
    }
    let commit = request.get("commit").and_then(Value::as_str).unwrap_or("");
    if commit.is_empty() {
        return run.end("refused", "empty `commit`: name the commit to check");
    }
    if job.cmd.trim().is_empty() {
        return run.end("not-run", "no full check declared");
    }
    if !git_ok(&job.root, &["rev-parse", "--git-dir"]) {
        return run.end("not-run", "not a git repository");
    }
    let rev = format!("{commit}^{{commit}}");
    let Some(sha) = git_line(&job.root, &["rev-parse", "--verify", "-q", &rev]) else {
        return run.end("refused", format!("unknown commit {commit:?}"));
    };
    run.record.commit = sha.clone();
    let overlay = match overlay_of(&request, &run.dir) {
        Ok(overlay) => overlay,
        Err(why) => return run.end("refused", why),
    };
    if let Some(overlay) = &overlay {
        run.record.overlay = overlay.name.clone();
        run.record.overlay_files = overlay.files.len().to_string();
    }
    let name = worktree_name(job.slug, &run.record.id);
    if let Some(link) = link_on_the_way(&job.root, &[".claude", "worktrees", &name]) {
        return run.end("refused", format!("{link} is a symlink"));
    }
    let wt = job.root.join(WORKTREES).join(&name);

    // Before anything slow, so a hook killed from here on leaves a trace.
    run.record.status = "started";
    run.record.worktree = for_shell(&wt);
    if let Err(why) = run.save() {
        return format!("check-broker {}: not-run: {why}", run.record.id);
    }
    if let Err(why) = add_worktree(&job.root, &wt, &sha) {
        return run.end("not-run", format!("couldn't make the worktree: {why}"));
    }
    let (status, reason) = run.check_in(&wt, overlay.as_ref());
    // On record before the cleanup, which a hook killed or out of time won't finish.
    run.record.status = status;
    run.record.reason = reason;
    let mut saved = run.save();
    if let Err(why) = remove_worktree(&job.root, &wt) {
        let reason = &mut run.record.reason;
        if !reason.is_empty() {
            reason.push_str("; ");
        }
        reason.push_str(&format!("the worktree wasn't removed: {why}"));
        saved = run.save();
    }
    run.said(saved)
}

/// Whether `s` can name a run: a slug with no hyphen, so that `broker-<slug>-<id>`
/// splits at its last hyphen and no two runs share a worktree.
fn is_id(s: &str) -> bool {
    is_name(s) && !s.contains('-')
}

/// Make the record's file, or fail if it is there already (a link included, which
/// this never follows).
fn claim(record: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(record)
        .map(drop)
}

/// The request in `dir` and its id. An error has no id to name a record by, so
/// it goes in the note alone.
fn read_request(dir: &Path) -> Result<(Value, String), String> {
    let text =
        fs::read_to_string(dir.join(REQUEST)).map_err(|e| format!("can't read {REQUEST}: {e}"))?;
    let request: Value =
        serde_json::from_str(&text).map_err(|e| format!("{REQUEST} isn't valid JSON: {e}"))?;
    let id = request.get("id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() {
        return Err(format!("{REQUEST} has no `id`: name this run"));
    }
    if !is_id(id) {
        return Err(format!(
            "`id` {id:?} isn't a safe name: only a-z and 0-9, no hyphens"
        ));
    }
    let id = id.to_string();
    Ok((request, id))
}

/// The first symlink among `parts`, taken as a path from `root`, as shown.
fn link_on_the_way(root: &Path, parts: &[&str]) -> Option<String> {
    let mut at = root.to_path_buf();
    for part in parts {
        at.push(part);
        if is_link(&at) {
            return Some(for_shell(&at));
        }
    }
    None
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

// ----------------------------------------------------------------- the record --

/// What `check-<id>.md` holds.
#[derive(Default)]
struct Record {
    status: &'static str,
    id: String,
    commit: String,
    command: String,
    overlay: String,
    overlay_files: String,
    overlay_sum: String,
    exit: String,
    left_behind: String,
    duration_s: String,
    worktree: String,
    output: String,
    reason: String,
    hook_pid: String,
    check_pid: String,
}

impl Record {
    fn text(&self) -> String {
        let fields = [
            ("Status", self.status),
            ("Id", self.id.as_str()),
            ("Commit", self.commit.as_str()),
            ("Command", self.command.as_str()),
            ("Overlay", self.overlay.as_str()),
            ("Overlay-Files", self.overlay_files.as_str()),
            ("Overlay-Sum", self.overlay_sum.as_str()),
            ("Exit", self.exit.as_str()),
            ("Left-Behind", self.left_behind.as_str()),
            ("Duration-s", self.duration_s.as_str()),
            ("Worktree", self.worktree.as_str()),
            ("Output", self.output.as_str()),
            ("Reason", self.reason.as_str()),
            ("Hook-Pid", self.hook_pid.as_str()),
            ("Check-Pid", self.check_pid.as_str()),
        ];
        fields
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{key}: {}\n", one_line(value)))
            .collect()
    }
}

/// A value as it goes in a `Key: value` line: on that one line, so nothing it
/// holds can forge another (a two-line command, a `Status:` line).
fn one_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

/// The value of a record's first `key:` line, or empty.
fn field<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
        .map_or("", str::trim)
}

// ------------------------------------------------------------------- the sweep --

/// What the sweep of a record folder found.
#[derive(Default)]
struct Swept {
    /// A run still alive: its hook runs, or its check, which outlives a killed
    /// hook in a process group of its own.
    live: Option<String>,
    /// Runs nothing here can tell the state of (Windows).
    unknown: Vec<String>,
}

/// A `started` record whose hook and check are both gone becomes `interrupted`,
/// and its worktree goes.
fn sweep(job: &Job, dir: &Path) -> Swept {
    let mut swept = Swept::default();
    let Ok(entries) = fs::read_dir(dir) else {
        return swept;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let id = name
            .to_str()
            .and_then(|n| n.strip_prefix("check-")?.strip_suffix(".md"))
            .filter(|id| is_id(id));
        let Some(id) = id else {
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if field(&text, "Status") != "started" {
            continue;
        }
        let hook = alive(field(&text, "Hook-Pid"));
        let check = alive(field(&text, "Check-Pid"));
        match (hook, check) {
            (Some(false), Some(false)) => interrupt(job, dir, id, &text),
            (None, _) | (_, None) => swept.unknown.push(id.to_string()),
            _ => swept.live = Some(id.to_string()),
        }
    }
    swept
}

/// Mark a dead run. Its worktree's path comes from the slug and the id, never
/// from the record's `Worktree:` line, so a garbled line can't cost anyone a
/// folder.
fn interrupt(job: &Job, dir: &Path, id: &str, text: &str) {
    let wt = job.root.join(WORKTREES).join(worktree_name(job.slug, id));
    let mut why = "the hook and its check both ended before the run finished".to_string();
    if let Err(e) = remove_worktree(&job.root, &wt) {
        why.push_str(&format!("; the worktree wasn't removed: {e}"));
    }
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("Status:") {
            out.push_str("Status: interrupted\n");
        } else if !line.starts_with("Reason:") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!("Reason: {}\n", one_line(&why)));
    let _ = write_whole(dir, &format!("check-{id}.md"), &out);
}

/// Whether process `pid` runs (a field of a record: nothing usable in it is no
/// process). EPERM means it runs, as another user's.
#[cfg(unix)]
fn alive(pid: &str) -> Option<bool> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let Some(pid) = pid.parse::<i32>().ok().filter(|p| *p > 0) else {
        return Some(false);
    };
    // SAFETY: signal 0 delivers nothing; it only asks whether the process is there.
    if unsafe { kill(pid, 0) } == 0 {
        return Some(true);
    }
    let error = std::io::Error::last_os_error();
    Some(error.kind() == std::io::ErrorKind::PermissionDenied)
}

/// Windows has no `kill(pid, 0)` here: the state of a run can't be told, so
/// nothing is swept, and the note says so.
#[cfg(not(unix))]
fn alive(_pid: &str) -> Option<bool> {
    None
}

// ---------------------------------------------------------------- the worktree --

fn worktree_name(slug: &str, id: &str) -> String {
    format!("{BROKER}{slug}-{id}")
}

/// `git -C root <args>`; on failure, what git said.
fn git_run(root: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(plain(root))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// A detached checkout of `sha`. Git's hooks are off: `git worktree add` runs
/// `post-checkout`, a throwaway checkout needs none, and this process runs outside
/// the sandbox.
fn add_worktree(root: &Path, wt: &Path, sha: &str) -> Result<(), String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let hooks = format!("core.hooksPath={null}");
    let shown = for_shell(wt);
    git_run(
        root,
        &["-c", &hooks, "worktree", "add", "--detach", &shown, sha],
    )
}

/// Remove a worktree this hook made, on that exact path; anything not under
/// `.claude/worktrees/broker-` is refused. `git worktree remove` clears its own
/// entry. A prune drops every entry whose folder is missing, one on an unmounted
/// disk included, so it runs only for a folder already gone whose entry is still
/// there.
fn remove_worktree(root: &Path, wt: &Path) -> Result<(), String> {
    let folder = root.join(WORKTREES);
    let name = wt.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let shown = for_shell(wt);
    if wt.parent() != Some(folder.as_path()) || !name.starts_with(BROKER) {
        return Err(format!("{shown} isn't under {WORKTREES}/{BROKER}"));
    }
    if fs::symlink_metadata(wt).is_ok() {
        return git_run(root, &["worktree", "remove", "--force", &shown]);
    }
    if registered(root, wt) {
        return git_run(root, &["worktree", "prune"]);
    }
    Ok(())
}

/// Whether git still lists a worktree at `wt`, whose folder is gone. Git lists the
/// real path it made the worktree at, which the project's own path may not be.
fn registered(root: &Path, wt: &Path) -> bool {
    let (Ok(real), Some(name)) = (root.canonicalize(), wt.file_name()) else {
        return false;
    };
    let real = plain(&real).join(WORKTREES).join(name);
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(plain(root))
        .args(["worktree", "list", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    let listed = String::from_utf8_lossy(&out.stdout);
    listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|path| Path::new(path) == real)
}

// ----------------------------------------------------------------- the overlay --

/// Files a request adds to the checkout: a folder in the record folder, whose paths
/// are relative to the project.
struct Overlay {
    /// What the request calls it.
    name: String,
    from: PathBuf,
    /// Its files, relative to it, sorted.
    files: Vec<PathBuf>,
}

/// The overlay `request` names, or why it can't be used: no key or `null` is no
/// overlay, but an empty value or folder is refused, since an unfilled template
/// must not read as a pass. The hook runs outside the sandbox, so it stays in the
/// record folder and follows no link.
fn overlay_of(request: &Value, records: &Path) -> Result<Option<Overlay>, String> {
    let value = match request.get("overlay") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Err("`overlay` is empty".to_string()),
        Some(Value::String(s)) => s,
        Some(other) => return Err(format!("`overlay` {other} isn't a folder's name")),
    };
    let rel = Path::new(value);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!(
            "overlay {value:?} must be a plain path in the record folder"
        ));
    }
    let mut from = records.to_path_buf();
    for part in rel.components() {
        from.push(part);
        if is_link(&from) {
            return Err(format!("overlay {value:?} goes through a symlink"));
        }
    }
    if !from.is_dir() {
        return Err(format!(
            "overlay {value:?} isn't a folder in the record folder"
        ));
    }
    let mut files = Vec::new();
    overlay_walk(&from, &from, &mut files)?;
    if files.is_empty() {
        return Err(format!("overlay {value:?} holds no files"));
    }
    files.sort();
    Ok(Some(Overlay {
        name: value.clone(),
        from,
        files,
    }))
}

fn overlay_walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("can't read the overlay: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("can't read the overlay: {e}"))?;
        let kind = entry
            .file_type()
            .map_err(|e| format!("can't read the overlay: {e}"))?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if kind.is_dir() {
            overlay_walk(root, &path, files)?;
        } else if kind.is_file() {
            files.push(rel);
        } else {
            let shown = for_shell(&rel);
            return Err(format!(
                "the overlay's {shown} is a symlink or isn't a file"
            ));
        }
    }
    Ok(())
}

/// What in `dir` already stands where the new path `rel` would go: the path
/// itself, or a file or a link on the way to it.
fn taken(dir: &Path, rel: &Path) -> Option<PathBuf> {
    let last = rel.components().count();
    let mut at = dir.to_path_buf();
    let mut shown = PathBuf::new();
    for (i, part) in rel.components().enumerate() {
        at.push(part);
        shown.push(part);
        let meta = fs::symlink_metadata(&at).ok()?;
        if i + 1 == last || !meta.is_dir() {
            return Some(shown);
        }
    }
    None
}

impl Overlay {
    /// Copy the files under `dir`, the check's folder. A sum over each path and what
    /// was copied there, so the record can name the tree the check saw.
    fn copy_to(&self, dir: &Path) -> Result<String, String> {
        let mut summed = Vec::new();
        for rel in &self.files {
            let shown = for_shell(rel);
            let to = dir.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{shown}: {e}"))?;
            }
            fs::copy(self.from.join(rel), &to).map_err(|e| format!("{shown}: {e}"))?;
            let body = fs::read(&to).map_err(|e| format!("{shown}: {e}"))?;
            summed.extend_from_slice(format!("{shown}\0{}\0", body.len()).as_bytes());
            summed.extend_from_slice(&body);
        }
        Ok(cksum(&summed))
    }
}

// --------------------------------------------------------------------- the run --

/// A request that got as far as having a record of its own.
struct Run<'a> {
    job: &'a Job<'a>,
    dir: PathBuf,
    log: PathBuf,
    /// Said at the end of the note, whatever comes of the run.
    remark: String,
    record: Record,
}

impl Run<'_> {
    fn save(&self) -> Result<(), String> {
        let name = format!("check-{}.md", self.record.id);
        write_whole(&self.dir, &name, &self.record.text())
            .map_err(|e| format!("can't write {name}: {e}"))
    }

    /// The final record, then the note.
    fn end(mut self, status: &'static str, reason: impl Into<String>) -> String {
        self.record.status = status;
        self.record.reason = reason.into();
        let saved = self.save();
        self.said(saved)
    }

    /// The note, with what stopped the record being written, if anything did.
    fn said(&self, saved: Result<(), String>) -> String {
        let mut note = self.note();
        if let Err(why) = saved {
            note.push('\n');
            note.push_str(&why);
        }
        note
    }

    /// What Claude reads: what came of it, where the full output is, and the
    /// output's end, bounded. The whole of it stays in the log.
    fn note(&self) -> String {
        let r = &self.record;
        let (id, status, reason) = (&r.id, r.status, &r.reason);
        let mut note = if r.output.is_empty() {
            let record = for_shell(&self.dir.join(format!("check-{id}.md")));
            format!("check-broker {id}: {status}: {reason}; record: {record}")
        } else {
            let (exit, secs, output) = (&r.exit, &r.duration_s, &r.output);
            let mut ran =
                format!("check-broker {id}: {status}, exit {exit}, {secs}s; full output: {output}");
            if !reason.is_empty() {
                ran.push('\n');
                ran.push_str(reason);
            }
            if !r.left_behind.is_empty() {
                ran.push_str(
                    "\nProcesses left behind were killed; the status is what the exit code says.",
                );
            }
            ran
        };
        if !self.remark.is_empty() {
            note.push('\n');
            note.push_str(&self.remark);
        }
        let end = tail(&self.log);
        if !end.is_empty() {
            note.push_str("\n\n");
            note.push_str(&end);
        }
        note
    }

    /// All that happens in the worktree, in the project's folder there: the overlay,
    /// then the check, into the log. What came of it: the status, and why where that
    /// needs saying.
    fn check_in(&mut self, wt: &Path, overlay: Option<&Overlay>) -> (&'static str, String) {
        // A project in a subfolder of its repository runs in the matching one, and
        // never at the top for want of knowing: that would check the wrong folder.
        let Some(prefix) = git_line(&self.job.root, &["rev-parse", "--show-prefix"]) else {
            let why = "git can't say where the project lies in its repository".to_string();
            return ("not-run", why);
        };
        let dir = match prefix.as_str() {
            "" => wt.to_path_buf(),
            prefix => wt.join(prefix.trim_end_matches('/')),
        };
        if !dir.is_dir() {
            return ("not-run", format!("{prefix} isn't in this commit"));
        }
        if let Some(overlay) = overlay {
            if let Some(path) = overlay.files.iter().find_map(|rel| taken(&dir, rel)) {
                let shown = for_shell(&path);
                let why = format!("the overlay's {shown} already exists at this commit");
                return ("refused", why);
            }
            match overlay.copy_to(&dir) {
                Ok(sum) => self.record.overlay_sum = sum,
                Err(why) => return ("not-run", format!("couldn't copy the overlay: {why}")),
            }
            // A check that ran on files the record can't name would be worse than none.
            if let Err(why) = self.save() {
                return ("not-run", why);
            }
        }
        let mut cmd = match (self.job.command)(self.job.cmd, dir.as_path()) {
            Ok(cmd) => cmd,
            Err(why) => return ("not-run", why),
        };
        // The whole check sees the requested commit, however it finds the project.
        // This changes only what the check reads: the sandbox's policy is already
        // compiled, from the real project, and the helper never reads this variable.
        cmd.env("CLAUDE_PROJECT_DIR", for_shell(&dir));
        let log = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.log);
        let (out, err) = match log.and_then(|out| out.try_clone().map(|err| (out, err))) {
            Ok(files) => files,
            Err(e) => return ("not-run", format!("couldn't open the log: {e}")),
        };
        // Its own process group (the sandbox's front end leading it), so a
        // timeout takes its children with it.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let start = Instant::now();
        let spawned = cmd
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                let _ = fs::remove_file(&self.log);
                return ("not-run", format!("couldn't start the check: {e}"));
            }
        };
        // Right away: killing the hook doesn't kill the check's group, so the
        // sweep has to know this PID too.
        self.record.output = for_shell(&self.log);
        self.record.check_pid = child.id().to_string();
        let _ = self.save();
        let (status, exit, why) = loop {
            match child.try_wait() {
                Ok(Some(done)) => {
                    // Whatever the leader left running must not outlive the worktree.
                    // A forgotten server is common and doesn't flip the result.
                    if group_lives(child.id()) {
                        kill_tree(&mut child);
                        self.record.left_behind = "yes".to_string();
                    }
                    break judged(done.code(), self.job.sandboxed);
                }
                Ok(None) if start.elapsed() >= self.job.budget => {
                    kill_tree(&mut child);
                    let limit = self.job.budget.as_secs();
                    let why = format!("over its budget of {limit}s, so it was killed");
                    break ("timeout", "killed".to_string(), why);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    kill_tree(&mut child);
                    let why = format!("lost track of the check: {e}");
                    break ("not-run", "unknown".to_string(), why);
                }
            }
        };
        self.record.exit = exit;
        self.record.duration_s = format!("{:.1}", start.elapsed().as_secs_f64());
        (status, why)
    }
}

/// Whether anything is left in the process group of a check whose leader, `pid`, has
/// ended. EPERM means something runs there, as another user's.
#[cfg(unix)]
fn group_lives(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 delivers nothing; it only asks whether the group is there.
    if unsafe { kill(-pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
}

/// Windows has no process group to ask here, and a tree loses its root with the
/// leader: nothing left behind is found.
#[cfg(not(unix))]
fn group_lives(_pid: u32) -> bool {
    false
}

/// How a check that ended is recorded: its status, its exit and why. 125 is the
/// sandbox helper's own "couldn't apply" ([`NOT_APPLIED`]), so it means the check
/// never ran only where the helper ran; anywhere else it is the check's own code.
fn judged(code: Option<i32>, sandboxed: bool) -> (&'static str, String, String) {
    match code {
        Some(0) => ("passed", "0".to_string(), String::new()),
        Some(NOT_APPLIED) if sandboxed => {
            let why = "the sandbox couldn't be applied, so the check never ran".to_string();
            ("not-run", NOT_APPLIED.to_string(), why)
        }
        Some(code) => ("failed", code.to_string(), String::new()),
        None => ("failed", "signal".to_string(), String::new()),
    }
}

/// The end of the log, at most [`TAIL`] bytes, even if something still writes to it,
/// and read lossily, so output that isn't UTF-8 can't blank it. Where the start was
/// cut, whole lines only.
fn tail(log: &Path) -> String {
    let Ok(mut file) = fs::File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let from = len.saturating_sub(TAIL as u64);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err()
        || file.take(TAIL as u64).read_to_end(&mut bytes).is_err()
    {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut end: &str = &text;
    // A bad byte grows into a replacement character: bound it again.
    if end.len() > TAIL {
        let mut at = end.len() - TAIL;
        while !end.is_char_boundary(at) {
            at += 1;
        }
        end = &end[at..];
    }
    if from > 0 || end.len() < text.len() {
        if let Some((_, rest)) = end.split_once('\n').filter(|(_, rest)| !rest.is_empty()) {
            end = rest;
        }
    }
    end.trim_end().to_string()
}
