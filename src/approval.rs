//! The execution-approval store. An approval is a small file OUTSIDE the project
//! (`~/.claude/ocgen/approvals/<project-key>`) holding the UNIX time it expires,
//! written by a human with `ocgen approve`. Keeping it out of the project — and
//! time-limited — means an agent can't create it as a side effect of normal work,
//! and a forgotten approval re-locks by itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The repository root a project (or any worktree of it) belongs to, so every
/// worktree shares one approval. Falls back to the resolved project path. Returned
/// in the forward-slash form a shell produces (`C:/Users/x` on Windows, as from
/// `git rev-parse` or `pwd -W`), so the hook scripts compute the same key.
fn root_of(project: &Path) -> String {
    let common = Command::new("git")
        .arg("-C")
        .arg(crate::paths::plain(project))
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()));
    match common.as_deref().and_then(Path::parent) {
        Some(p) if !p.as_os_str().is_empty() => crate::paths::for_shell(p),
        _ => crate::paths::for_shell(
            &fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf()),
        ),
    }
}

/// A file-name-safe key for the project: its root path with every byte outside
/// `A-Za-z0-9._-` replaced by `_` (last 200 bytes). The gate script computes the
/// same key with `tr -c 'A-Za-z0-9._-' '_' | tail -c 200`.
pub fn project_key(project: &Path) -> String {
    let root = root_of(project);
    let bytes: Vec<u8> = root
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                b
            } else {
                b'_'
            }
        })
        .collect();
    let tail = &bytes[bytes.len().saturating_sub(200)..];
    String::from_utf8_lossy(tail).into_owned()
}

/// Where this project's approval lives under `home`.
pub fn marker_path(home: &Path, project: &Path) -> PathBuf {
    home.join(".claude/ocgen/approvals")
        .join(project_key(project))
}

/// Approve high-impact actions for `minutes`. Returns the marker path.
pub fn grant(home: &Path, project: &Path, minutes: u64) -> Result<PathBuf> {
    let marker = marker_path(home, project);
    if let Some(dir) = marker.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(&marker, format!("{}\n", now() + minutes * 60))
        .with_context(|| format!("writing {}", marker.display()))?;
    Ok(marker)
}

/// Re-lock now. Returns whether there was an approval to remove.
pub fn revoke(home: &Path, project: &Path) -> Result<bool> {
    let marker = marker_path(home, project);
    if marker.exists() {
        fs::remove_file(&marker).with_context(|| format!("removing {}", marker.display()))?;
        return Ok(true);
    }
    Ok(false)
}

/// Seconds of approval left, or `None` when locked (no approval, or expired).
pub fn remaining(home: &Path, project: &Path) -> Option<u64> {
    let text = fs::read_to_string(marker_path(home, project)).ok()?;
    let expiry: u64 = text
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    expiry.checked_sub(now()).filter(|left| *left > 0)
}
