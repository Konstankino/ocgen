//! Git checks about how a generated project is committed.
//!
//! A Claude Code worktree (`claude --worktree`, desktop worktree sessions) is a
//! fresh checkout of *tracked* files, so a project that git-ignores its `.claude/`
//! config starts every worktree session without its settings, hooks and rules.

use std::path::Path;
use std::process::Command;

/// Whether `.claude/settings.json` under `root` is git-ignored. `None` when `root`
/// is not in a git repository or git can't be run.
pub fn claude_config_ignored(root: &Path) -> Option<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", ".claude/settings.json"])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None, // 128: not a repository (or another git error)
    }
}

/// The warning shown by `landscape` and `doctor` when [`claude_config_ignored`] is true.
pub const IGNORED_CONFIG_WARNING: &str =
    "`.claude/` is git-ignored, so `claude --worktree` sessions \
start without its settings, hooks (approval gate, confidence gates, loop guard) and rules — only \
the `.worktreeinclude` copies carry them. Commit `.claude/` and ignore just `settings.local.json`, \
`worktrees/`, `notes/` and `loop-guard/`.";
