//! What each Claude Code session wrote last, per kind of document: the issue
//! drafts ([`super::draft`]), the /inquire ledgers and the intent files
//! ([`super::intents`]). The word that opens a kind's list opens it with that
//! document selected, whichever changed last.
//!
//! The records live in a state directory that git-ignores itself — the drafts'
//! and the ledgers' own folders, or, for the intent files, which are tracked,
//! `.claude/intent/viewer`: one file per session id under [`SESSIONS`], holding
//! a document's name. A write is remembered by its path ([`remember`]); a Bash
//! call by what it changed ([`before_bash`], [`after_bash`]), whatever the
//! command.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where, in the state directory, each session's last-written document is kept.
pub const SESSIONS: &str = ".sessions";
/// How long a session's record is kept after its last write.
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Where, in [`SESSIONS`], the documents' state is kept from just before a Bash
/// call to just after it: one file per tool call.
pub const BEFORE: &str = ".before";
/// How long the state kept for a call that never finished is kept.
const BEFORE_TTL: Duration = Duration::from_secs(24 * 3600);

/// A session id fit to name a file: letters, digits, `-` and `_`.
pub(super) fn plain_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A document's name: its file name without `.md`.
fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The state directory's records are working state: it git-ignores itself.
pub fn ignore_self(state: &Path) {
    let ignore = state.join(".gitignore");
    if !ignore.exists() {
        let _ = fs::write(ignore, "*\n");
    }
}

/// Remember, in `state`, that `session` wrote document `name` last, and drop
/// records older than a month. False when nothing was recorded: an id or a name
/// (`is_name`) that can't name a file, or a write that failed.
pub fn remember(state: &Path, session: &str, name: &str, is_name: fn(&str) -> bool) -> bool {
    if !plain_id(session) || !is_name(name) {
        return false;
    }
    let at = state.join(SESSIONS);
    if fs::create_dir_all(&at).is_err() {
        return false;
    }
    ignore_self(state);
    prune(&at, SESSION_TTL);
    let now = SystemTime::now();
    let file = at.join(session);
    if super::write_if_changed(&file, name.as_bytes()).is_err() {
        return false;
    }
    // An unchanged record isn't rewritten: keep it fresh all the same.
    let _ = fs::File::options()
        .write(true)
        .open(&file)
        .and_then(|f| f.set_modified(now));
    true
}

/// Remove the files in `at` last changed longer than `ttl` ago.
fn prune(at: &Path, ttl: Duration) {
    let now = SystemTime::now();
    for e in fs::read_dir(at).into_iter().flatten().flatten() {
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > ttl);
        if stale {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Each of `docs`' state — size, time and text — by name.
fn states(docs: &[PathBuf]) -> BTreeMap<String, String> {
    docs.iter()
        .map(|p| {
            let m = fs::metadata(p).ok();
            let len = m.as_ref().map_or(0, |m| m.len());
            let t = m
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            let rev = fs::read(p).map(|b| super::rev(&b)).unwrap_or_default();
            (stem(p), format!("{len}:{t}:{rev}"))
        })
        .collect()
}

/// Before Bash call `call` (its tool-use id): keep the state of `docs` (the
/// documents as their list orders them) in `state` until [`after_bash`], and
/// drop states kept for calls that never finished (a refused command) a day
/// ago. False when nothing was kept: a call id that can't name a file, or a
/// write that failed. The caller checks the documents' folder exists.
pub fn before_bash(state: &Path, call: &str, docs: &[PathBuf]) -> bool {
    if !plain_id(call) {
        return false;
    }
    let at = state.join(SESSIONS).join(BEFORE);
    if fs::create_dir_all(&at).is_err() {
        return false;
    }
    ignore_self(state);
    prune(&at, BEFORE_TTL);
    let body: String = states(docs)
        .iter()
        .map(|(name, state)| format!("{name}\t{state}\n"))
        .collect();
    super::write_if_changed(&at.join(call), body.as_bytes()).is_ok()
}

/// After Bash call `call`: the documents of `docs` (in their list's order) it
/// added or changed — by size, time or text, against the state [`before_bash`]
/// kept — and of those the first in that order becomes `session`'s
/// ([`remember`]). Returns that document's name.
pub fn after_bash(
    state: &Path,
    call: &str,
    session: &str,
    docs: &[PathBuf],
    is_name: fn(&str) -> bool,
) -> Option<String> {
    if !plain_id(call) {
        return None;
    }
    let kept = state.join(SESSIONS).join(BEFORE).join(call);
    let before = fs::read_to_string(&kept).ok()?;
    let _ = fs::remove_file(&kept);
    let before: BTreeMap<&str, &str> = before.lines().filter_map(|l| l.split_once('\t')).collect();
    let now = states(docs);
    let name = docs
        .iter()
        .map(|p| stem(p))
        .find(|n| before.get(n.as_str()).copied() != now.get(n).map(String::as_str))?;
    remember(state, session, &name, is_name).then_some(name)
}

/// The document `session` wrote last, while `<docs>/<name>.md` still exists.
pub fn recall(
    state: &Path,
    docs: &Path,
    session: &str,
    is_name: fn(&str) -> bool,
) -> Option<String> {
    if !plain_id(session) {
        return None;
    }
    let name = fs::read_to_string(state.join(SESSIONS).join(session)).ok()?;
    let name = name.trim();
    (is_name(name) && docs.join(format!("{name}.md")).is_file()).then(|| name.to_string())
}
