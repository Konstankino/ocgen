//! Git checks about how a generated project's Claude config reaches worktrees.
//!
//! A Claude Code worktree (`claude --worktree`, desktop worktree sessions,
//! `isolation: worktree` subagents) is a fresh checkout of *committed* files, and
//! `.worktreeinclude` copies only *git-ignored* ones into it. So config that is
//! neither — untracked, not ignored — never reaches a worktree, and its sessions
//! start without their settings, hooks and rules. Committing `.claude/` is
//! optional: while nothing under it is committed, ocgen keeps its config out of
//! git in this clone ([`sync_exclude`], `.git/info/exclude`), so the copy picks it
//! up.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// What a worktree must find in its checkout, committed or copied by
/// `.worktreeinclude` (which lists each of these). Agents, commands and skills
/// are absent: Claude Code loads them from the main checkout when a worktree has
/// none, so a copy would only shadow the live ones.
pub const WORKTREE_COPIES: [&str; 9] = [
    "CLAUDE.md",
    ".mcp.json",
    ".claude/settings.json",
    ".claude/rules/*.md",
    ".claude/hooks/*.sh",
    ".claude/output-styles/*.md",
    ".claude/statusline.sh",
    ".claude/.ocgen-state.json",
    ".claude/intent/*.md",
];

/// Kept out of git along with [`WORKTREE_COPIES`], though never copied: the
/// config Claude Code reads from the main checkout, the agents' project memory,
/// and the worktrees themselves.
const ALSO_EXCLUDED: [&str; 5] = [
    ".claude/agents/",
    ".claude/commands/",
    ".claude/skills/",
    ".claude/agent-memory/",
    ".claude/worktrees/",
];

/// Opens ocgen's block in `.git/info/exclude`, followed by the project's folder
/// from the repository's top (`.` at the top), so projects in one repository
/// each keep a block of their own.
const EXCLUDE_MARK: &str = "# ocgen:exclude ";
const EXCLUDE_END: &str = "# ocgen:end";

/// `git -C root <args>`'s stdout, or `None` when git can't run or fails (not a
/// repository).
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(crate::paths::plain(root))
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether anything under `root/.claude/` is tracked (committed or staged).
fn claude_tracked(root: &Path) -> Option<bool> {
    git(root, &["ls-files", "-z", "--", ".claude"]).map(|o| !o.is_empty())
}

/// The config a worktree of the repository at `root` would start without.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gaps {
    /// Files of [`WORKTREE_COPIES`] that are neither tracked nor git-ignored,
    /// relative to `root`.
    pub missing: Vec<String>,
    /// Whether anything under `.claude/` is tracked: the project commits its
    /// config, so ocgen leaves the ignoring to the user.
    pub committed: bool,
}

/// What worktrees of `root`'s repository would miss. `None` when `root` isn't in a
/// git repository (or git can't be run).
pub fn worktree_gaps(root: &Path) -> Option<Gaps> {
    let mut args = vec!["ls-files", "-z", "--others", "--exclude-standard", "--"];
    args.extend(WORKTREE_COPIES);
    let missing = git(root, &args)?
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect();
    Some(Gaps {
        missing,
        committed: claude_tracked(root)?,
    })
}

/// The warning `verify`, `landscape` and `doctor` show for non-empty [`Gaps`].
pub fn gaps_warning(g: &Gaps) -> String {
    const SHOWN: usize = 4;
    let mut list = g
        .missing
        .iter()
        .take(SHOWN)
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if g.missing.len() > SHOWN {
        list.push_str(&format!(" and {} more", g.missing.len() - SHOWN));
    }
    let fix = if g.committed {
        "Commit them, or git-ignore them so `.worktreeinclude` copies them."
    } else {
        "Run `ocgen doctor`: it keeps them out of git in this clone (`.git/info/exclude`), \
so `.worktreeinclude` copies them."
    };
    format!(
        "{} Claude config file(s) are neither committed nor git-ignored, so worktree sessions \
(`claude --worktree`, desktop) start without them — no settings, hooks or rules: {list}. {fix}",
        g.missing.len()
    )
}

/// Outcome of [`sync_exclude`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclude {
    /// ocgen's block was added or refreshed in this file.
    Written(PathBuf),
    /// ocgen's block was removed from this file: `.claude/` is committed now.
    Removed(PathBuf),
    /// The file already says what it should.
    Unchanged,
    /// Not in a git repository.
    NotApplicable,
}

/// The repository's local exclude file and `root`'s folder from its top
/// (`svc/api/`, empty at the top).
fn exclude_location(root: &Path) -> Option<(PathBuf, String)> {
    let file = git(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    )?;
    let prefix = git(root, &["rev-parse", "--show-prefix"])?;
    Some((PathBuf::from(file.trim()), prefix.trim().replace('\\', "/")))
}

fn mark(prefix: &str) -> String {
    let key = if prefix.is_empty() { "." } else { prefix };
    format!("{EXCLUDE_MARK}{key}")
}

/// `s` with gitignore's special characters escaped, so a folder name is matched
/// as written.
fn literal(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if matches!(c, '\\' | '*' | '?' | '[') {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

/// ocgen's block for the project in `prefix`: its config, anchored to that folder.
fn exclude_block(prefix: &str) -> String {
    let base = format!("/{}", literal(prefix));
    let mut block = format!(
        "{}\n\
# Claude config kept out of git in this clone only, so .worktreeinclude copies it into\n\
# worktrees. To commit .claude/ instead, delete this block and commit it: ocgen stops\n\
# adding the block once anything under .claude/ is committed.\n",
        mark(prefix)
    );
    for p in WORKTREE_COPIES.iter().chain(&ALSO_EXCLUDED) {
        block.push_str(&format!("{base}{p}\n"));
    }
    block.push_str(EXCLUDE_END);
    block.push('\n');
    block
}

/// `text` without the block that opens with `mark`, its trailing blank lines
/// trimmed. `None` when there's no such block.
fn without_block(text: &str, mark: &str) -> Option<String> {
    if !text.lines().any(|l| l == mark) {
        return None;
    }
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line == mark {
            inside = true;
            if out.ends_with("\n\n") {
                out.pop(); // the blank line that set the block apart
            }
        } else if inside {
            inside = line != EXCLUDE_END;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    let kept = out.trim_end().len();
    out.truncate(kept);
    if !out.is_empty() {
        out.push('\n');
    }
    Some(out)
}

/// Keep `root`'s Claude config out of git in this clone while nothing under its
/// `.claude/` is committed — a block in the repository's `.git/info/exclude`
/// (shared by all its worktrees, never committed), so `.worktreeinclude` copies
/// the config into every new worktree. Once anything under `.claude/` is
/// tracked, the block is removed: the project commits its config. The rest of
/// the file, and other projects' blocks, are left as they are.
pub fn sync_exclude(root: &Path) -> Result<Exclude> {
    let Some((file, prefix)) = exclude_location(root) else {
        return Ok(Exclude::NotApplicable);
    };
    let Some(committed) = claude_tracked(root) else {
        return Ok(Exclude::NotApplicable);
    };
    let existing = match fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let stripped = without_block(&existing, &mark(&prefix));
    let (wanted, outcome) = if committed {
        match stripped {
            Some(s) => (s, Exclude::Removed(file.clone())),
            None => return Ok(Exclude::Unchanged),
        }
    } else {
        let rest = stripped.unwrap_or_else(|| existing.clone());
        let rest = rest.trim_end();
        let sep = if rest.is_empty() { "" } else { "\n\n" };
        (
            format!("{rest}{sep}{}", exclude_block(&prefix)),
            Exclude::Written(file.clone()),
        )
    };
    if wanted == existing {
        return Ok(Exclude::Unchanged);
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&file, wanted).with_context(|| format!("writing {}", file.display()))?;
    Ok(outcome)
}

/// Whether ocgen's block for `root` is in its repository's `.git/info/exclude`.
pub fn excluded_locally(root: &Path) -> bool {
    exclude_location(root)
        .and_then(|(file, prefix)| {
            let text = fs::read_to_string(file).ok()?;
            Some(text.lines().any(|l| l == mark(&prefix)))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_removed_whole_and_only_its_own() {
        let mine = exclude_block("a/");
        let theirs = exclude_block("b/");
        let text = format!("*.log\n\n{mine}\n{theirs}");
        let out = without_block(&text, &mark("a/")).unwrap();
        assert_eq!(out, format!("*.log\n\n{theirs}"));
        assert!(without_block(&out, &mark("a/")).is_none());
    }

    #[test]
    fn the_top_of_the_repository_is_dot() {
        assert!(exclude_block("").starts_with("# ocgen:exclude .\n"));
        assert!(exclude_block("").contains("\n/.claude/settings.json\n"));
        assert!(exclude_block("svc/").contains("\n/svc/.claude/settings.json\n"));
    }

    #[test]
    fn folder_names_are_matched_literally() {
        assert_eq!(literal("a[1]*/b?\\"), "a\\[1]\\*/b\\?\\\\");
    }
}
