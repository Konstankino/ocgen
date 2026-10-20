//! What each Claude Code session wrote last, per kind of document: the issue
//! drafts ([`super::draft`]), the /inquire ledgers and the intent files
//! ([`super::intents`]). The word that opens a kind's list opens it with that
//! document selected, whichever changed last.
//!
//! The records live in a state directory that git-ignores itself — the drafts'
//! and the ledgers' own folders, or, for the intent files, which are tracked,
//! `.claude/intent/viewer`: one file per session id under [`SESSIONS`], holding
//! a document's name ([`remember`]): the one a write names, or the one Claude
//! Code's file watcher saw change (a Bash command's, whatever the command).

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Where, in the state directory, each session's last-written document is kept.
pub const SESSIONS: &str = ".sessions";
/// How long a session's record is kept after its last write.
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Where, in [`SESSIONS`], ocgen before hooks protocol 15 kept the documents'
/// state around each Bash call: removed when found.
const BEFORE: &str = ".before";

/// A session id fit to name a file: letters, digits, `-` and `_`.
pub(super) fn plain_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
    let _ = fs::remove_dir_all(at.join(BEFORE));
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
