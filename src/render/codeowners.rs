//! ocgen's block in the project's CODEOWNERS: the /intent approvers as required
//! reviewers. ocgen links a CODEOWNERS the project already has (`ocgen edit
//! intent --codeowners`) — it never creates one — keeps a symbolic link to it at
//! `.claude/CODEOWNERS`, and only ever touches the lines between its markers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use regex::Regex;

use super::{plain_rel, Fence, Project};
use crate::claude::{CodeownersScope, CODEOWNERS_LINK, CODEOWNERS_PATHS};
use crate::target::Target;

/// Opens ocgen's block; `# ocgen: end` closes it.
const MARK: &str = "# ocgen: /intent approvers (ocgen edit intent --approver / --codeowners-scope)";
const END: &str = "# ocgen: end";

/// `existing` with ocgen's block holding `rule` — replaced in place, appended
/// when absent (last, so it wins), or removed when `rule` is `None` — or `None`
/// when nothing changes. The user's lines are never touched.
pub(super) fn with_block(existing: &str, rule: Option<&str>) -> Option<String> {
    let block = rule.map(|r| format!("{MARK}\n{r}\n{END}\n"));
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let new = match (lines.iter().position(|l| l.trim_end() == MARK), block) {
        (Some(i), block) => {
            // A lost end marker: the block is the marker line alone.
            let end = lines[i + 1..]
                .iter()
                .position(|l| l.trim_end() == END)
                .map_or(i, |j| i + 1 + j);
            let mut before = lines[..i].concat();
            let after = lines[end + 1..].concat();
            match block {
                Some(b) => format!("{before}{b}{after}"),
                None => {
                    // The blank line ocgen put before its block goes with it.
                    if before.ends_with("\n\n") || before == "\n" {
                        before.pop();
                    }
                    format!("{before}{after}")
                }
            }
        }
        (None, Some(b)) if existing.trim().is_empty() => b,
        (None, Some(b)) => {
            let sep = if existing.ends_with('\n') { "" } else { "\n" };
            format!("{existing}{sep}\n{b}")
        }
        (None, None) => return None,
    };
    (new != existing).then_some(new)
}

/// The project's folder within its git repository (`services/api/`), empty at
/// the top; `None` outside a repository.
pub(super) fn repo_prefix(root: &Path) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(crate::paths::plain(root))
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// `typed` (`.github/CODEOWNERS`) as a CODEOWNERS ocgen may link in the project
/// at `root`: an existing text file where GitHub reads it, inside the project,
/// and not shadowed by one GitHub reads first. Returns the path to store.
pub fn check_link(root: &Path, typed: &str) -> Result<String> {
    let rel = typed.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./").to_string();
    if rel.is_empty() || !plain_rel(Path::new(&rel)) {
        bail!(
            "'{typed}': the CODEOWNERS must be a file inside the project ({})",
            CODEOWNERS_PATHS.join(", ")
        );
    }
    if !CODEOWNERS_PATHS.contains(&rel.as_str()) {
        bail!(
            "{rel}: GitHub reads CODEOWNERS only from {} — link one of those",
            CODEOWNERS_PATHS.join(", ")
        );
    }
    if let Some(prefix) = repo_prefix(root).filter(|p| !p.is_empty()) {
        bail!(
            "this project is in a folder of its repository ({prefix}), and GitHub reads CODEOWNERS only at the repository's top, outside the project — add the approvers there by hand"
        );
    }
    let path = root.join(&rel);
    match fs::symlink_metadata(&path) {
        Err(_) => {
            bail!("{rel} doesn't exist — ocgen links the CODEOWNERS you have; create it first")
        }
        Ok(m) if m.file_type().is_symlink() => {
            bail!("{rel} is a symbolic link — link the real file")
        }
        Ok(m) if !m.is_file() => bail!("{rel} isn't a file"),
        Ok(_) => {}
    }
    if String::from_utf8(fs::read(&path)?).is_err() {
        bail!("{rel} isn't UTF-8 text — ocgen won't edit it");
    }
    Fence::new(root).path(Path::new(&rel))?;
    if let Some(first) = CODEOWNERS_PATHS
        .iter()
        .take_while(|p| **p != rel)
        .find(|p| fs::symlink_metadata(root.join(p)).is_ok())
    {
        bail!("GitHub reads {first} first, so {rel} would be ignored — link {first} instead");
    }
    Ok(rel)
}

/// The CODEOWNERS GitHub would read in `root` (the first of its locations that
/// exists), for a hint.
pub fn found(root: &Path) -> Option<&'static str> {
    CODEOWNERS_PATHS
        .into_iter()
        .find(|p| root.join(p).is_file())
}

/// Remove ocgen's block from `rel` in `root` (a file it was linked to before):
/// only a real text file inside the project is touched.
pub(super) fn remove_block(fence: &Fence, rel: &str) -> Option<(PathBuf, String)> {
    let path = fence.path(Path::new(rel)).ok()?;
    let old = String::from_utf8(fs::read(&path).ok()?).ok()?;
    with_block(&old, None).map(|new| (path, new))
}

/// What the link at `.claude/CODEOWNERS` points at: `../<rel>`.
fn link_target(rel: &str) -> PathBuf {
    let up = Path::new("..");
    rel.split('/')
        .fold(up.to_path_buf(), |p, part| p.join(part))
}

/// Whether `t` is a link ocgen made (`../` and one of GitHub's locations).
fn ours(t: &Path) -> bool {
    CODEOWNERS_PATHS.iter().any(|p| t == link_target(p))
}

impl Project {
    /// The CODEOWNERS ocgen is linked to and the rule its block holds there
    /// (`None`: the block goes). `None` when nothing is linked.
    pub(super) fn codeowners_block(&self) -> Option<(String, Option<String>)> {
        let s = &self.claude.intent;
        if s.codeowners.is_empty() || self.target != Target::ClaudeCode {
            return None;
        }
        let on = self.claude.workflow.intent && self.claude.output.project;
        Some((
            s.codeowners.clone(),
            on.then(|| s.codeowners_rule()).flatten(),
        ))
    }

    /// The linked CODEOWNERS: (path, relative path, current text, new text when
    /// it changes). `None` when nothing is linked, or the file is left alone (gone,
    /// a link, not text, or kept by the user) — [`Project::codeowners_report`]
    /// says why.
    pub(super) fn codeowners_change(
        &self,
        fence: &Fence,
        keep: &BTreeSet<String>,
    ) -> Option<(PathBuf, String, String, Option<String>)> {
        let (rel, rule) = self.codeowners_block()?;
        if keep.contains(&rel) {
            return None;
        }
        let path = fence.path(Path::new(&rel)).ok()?;
        let old = String::from_utf8(fs::read(&path).ok()?).ok()?;
        let new = with_block(&old, rule.as_deref());
        Some((path, rel, old, new))
    }

    /// Make `.claude/CODEOWNERS` the link to the linked CODEOWNERS, or remove
    /// ocgen's link when nothing is linked. Something else there is left alone.
    pub(super) fn sync_codeowners_link(&self, target: &Path) {
        let link = target.join(CODEOWNERS_LINK);
        let want = self
            .codeowners_block()
            .filter(|_| self.claude.workflow.intent && self.claude.output.project)
            .map(|(rel, _)| link_target(&rel));
        let current = fs::symlink_metadata(&link)
            .ok()
            .map(|m| (m.file_type().is_symlink(), fs::read_link(&link).ok()));
        match (current, want) {
            (Some((true, Some(t))), Some(w)) if t == w => {}
            (Some((true, Some(t))), w) if ours(&t) => {
                let _ = fs::remove_file(&link);
                if let Some(w) = w {
                    make_link(&w, &link);
                }
            }
            (None, Some(w)) if Fence::new(target).path(Path::new(CODEOWNERS_LINK)).is_ok() => {
                make_link(&w, &link);
            }
            _ => {} // nothing to do, or not ocgen's: left alone (reported)
        }
    }

    /// What to tell the user about the linked CODEOWNERS: a file left alone, a
    /// link that couldn't be made, and the user's rules after ocgen's block that
    /// take precedence over it.
    pub fn codeowners_report(&self, target: &Path) -> Vec<String> {
        let Some((rel, rule)) = self.codeowners_block() else {
            return Vec::new();
        };
        let relink = "re-link it with `ocgen edit intent --codeowners <path>`, or unlink it with `--codeowners off`";
        let path = target.join(&rel);
        let mut notes = Vec::new();
        match fs::symlink_metadata(&path) {
            Err(_) => {
                return vec![format!(
                    "CODEOWNERS: {rel} is gone, and ocgen never creates one — {relink}"
                )]
            }
            Ok(m) if m.file_type().is_symlink() => {
                return vec![format!(
                    "CODEOWNERS: {rel} is now a symbolic link, so ocgen left it alone — {relink}"
                )]
            }
            Ok(_) => {}
        }
        let Some(text) = fs::read(&path).ok().and_then(|b| String::from_utf8(b).ok()) else {
            return vec![format!(
                "CODEOWNERS: {rel} isn't UTF-8 text, so ocgen left it alone — {relink}"
            )];
        };
        let link = target.join(CODEOWNERS_LINK);
        match fs::symlink_metadata(&link) {
            Ok(m) if !m.file_type().is_symlink() => notes.push(format!(
                "CODEOWNERS: {CODEOWNERS_LINK} is a file of yours, so ocgen keeps no link there"
            )),
            Ok(_) if fs::read_link(&link).is_ok_and(|t| t != link_target(&rel) && !ours(&t)) => {
                notes.push(format!(
                    "CODEOWNERS: {CODEOWNERS_LINK} is a link of yours, so ocgen left it alone"
                ))
            }
            Err(_) if self.claude.workflow.intent && self.claude.output.project => {
                notes.push(format!(
                    "CODEOWNERS: couldn't make the link {CODEOWNERS_LINK} (symbolic links aren't available here) — the block in {rel} is kept up to date all the same"
                ))
            }
            _ => {}
        }
        if rule.is_some() {
            let sample = match self.claude.intent.codeowners_scope {
                CodeownersScope::Intents => Some(format!(
                    "{}/{}-x.md",
                    self.claude.intent.dir.trim_matches('/'),
                    self.claude.intent.first_id()
                )),
                _ => None,
            };
            let later = rules_after_block(&text, sample.as_deref());
            if !later.is_empty() {
                notes.push(format!(
                    "CODEOWNERS: these rules after ocgen's block in {rel} take precedence over it (the last match wins): {} — move them above the block if the approvers should still review those files",
                    later.iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        notes
    }
}

fn make_link(to: &Path, link: &Path) {
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(to, link);
    #[cfg(windows)]
    let _ = std::os::windows::fs::symlink_file(to, link);
}

/// The rules after ocgen's block that match `sample` (any rule, without one):
/// CODEOWNERS takes the last matching pattern.
fn rules_after_block(text: &str, sample: Option<&str>) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == MARK) else {
        return Vec::new();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_end() == END)
        .map_or(start, |j| start + 1 + j);
    lines[end + 1..]
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| {
            let pattern = l.split_whitespace().next().unwrap_or("");
            sample.is_none_or(|s| matches(pattern, s))
        })
        .map(String::from)
        .collect()
}

/// Whether a CODEOWNERS `pattern` (gitignore-style: `*`, `**`, `?`; a leading or
/// inner `/` anchors it at the top; a trailing `/` means a folder) matches `path`.
fn matches(pattern: &str, path: &str) -> bool {
    let folder = pattern.ends_with('/');
    let core = pattern.trim_start_matches('/').trim_end_matches('/');
    if core.is_empty() {
        return false;
    }
    let anchored = pattern.starts_with('/') || core.contains('/');
    let mut re = String::new();
    let mut chars = core.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    re.push_str("(?:.*/)?"); // `**/x`: x in any folder, the top too
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    // `docs/*` matches the files in docs/, not deeper (GitHub's docs say so); a
    // plain name matches that file or everything in that folder.
    let wild_last = core
        .rsplit('/')
        .next()
        .is_some_and(|s| s.contains(['*', '?']));
    let head = if anchored { "^" } else { "^(?:.*/)?" };
    let tail = if folder {
        "/.*$"
    } else if wild_last {
        "$"
    } else {
        "(?:/.*)?$"
    };
    Regex::new(&format!("{head}{re}{tail}")).is_ok_and(|r| r.is_match(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINE: &str = "* @owner\n";

    #[test]
    fn the_block_is_appended_updated_in_place_and_removed() {
        let added = with_block(MINE, Some("/docs/adr/ @a")).unwrap();
        assert_eq!(added, format!("{MINE}\n{MARK}\n/docs/adr/ @a\n{END}\n"));
        assert_eq!(with_block(&added, Some("/docs/adr/ @a")), None);
        // In place: the user's lines around it stay where they are.
        let around = format!("{added}/x/ @team\n");
        let updated = with_block(&around, Some("/docs/adr/ @a @b")).unwrap();
        assert_eq!(
            updated,
            format!("{MINE}\n{MARK}\n/docs/adr/ @a @b\n{END}\n/x/ @team\n")
        );
        assert_eq!(with_block(&added, None).unwrap(), MINE);
        assert_eq!(
            with_block(&updated, None).unwrap(),
            format!("{MINE}/x/ @team\n")
        );
        assert_eq!(with_block(MINE, None), None);
        // Without a trailing newline, or empty.
        assert_eq!(
            with_block("* @owner", Some("* @a")).unwrap(),
            format!("* @owner\n\n{MARK}\n* @a\n{END}\n")
        );
        assert_eq!(
            with_block("", Some("* @a")).unwrap(),
            format!("{MARK}\n* @a\n{END}\n")
        );
    }

    #[test]
    fn codeowners_patterns_match_like_github() {
        let p = "docs/adr/ADR-0001-x.md";
        for yes in [
            "*",
            "**",
            "*.md",
            "docs/",
            "/docs/",
            "/docs/adr/",
            "docs/adr",
            "/docs/**",
            "adr/",
            "docs/adr/*",
            "/docs/adr/ADR-*",
            "**/adr/",
        ] {
            assert!(matches(yes, p), "{yes}");
        }
        for no in [
            "/adr/",
            "/src/",
            "*.rs",
            "/docs/adr/*.txt",
            "docs/adr/x/",
            "/README.md",
            "/docs/*",
        ] {
            assert!(!matches(no, p), "{no}");
        }
    }

    #[test]
    fn only_later_matching_rules_are_reported() {
        let text = format!(
            "/docs/ @before\n{MARK}\n/docs/adr/ @a\n{END}\n# note\n/docs/ @after\n/src/ @dev\n"
        );
        assert_eq!(
            rules_after_block(&text, Some("docs/adr/ADR-0001-x.md")),
            ["/docs/ @after"]
        );
        assert_eq!(
            rules_after_block(&text, None),
            ["/docs/ @after", "/src/ @dev"]
        );
    }
}
