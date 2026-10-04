//! /intent's drafts against the project's current approvers.
//!
//! The approvers are written into the /intent skill when it is generated, so a
//! session that started before they changed keeps drafting with the old list —
//! one wrote "No approvers configured" a day after an approver was added, and
//! an issue filed from it notifies nobody. These checks read the files and the
//! state as they are now, so they hold whatever a session was told: the
//! `intent-approvers` hook after every write, the draft editor and `ocgen draft`
//! when you review, `ocgen edit intent` when the approvers change, and
//! `ocgen verify`.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::claude::IntentSettings;
use crate::render::Project;

/// What the drafts say in place of approvers when none are configured.
pub const NO_APPROVERS: &str = "No approvers configured";

/// How a file falls short of the current approvers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gaps {
    /// Approvers it doesn't @mention, as configured.
    pub missing: Vec<String>,
    /// It still says "No approvers configured".
    pub says_none: bool,
}

impl Gaps {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && !self.says_none
    }

    /// One clause on what's wrong: "doesn't @mention @a, @b and says …".
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.missing.is_empty() {
            parts.push(format!("doesn't @mention {}", self.missing.join(", ")));
        }
        if self.says_none {
            parts.push(format!("says \"{NO_APPROVERS}\""));
        }
        parts.join(" and ")
    }
}

/// Whether `text` @mentions `handle` (`@alice`, `@org/team`) as a whole
/// handle, in any case — not inside an email, a longer name or another team.
pub fn mentions(text: &str, handle: &str) -> bool {
    let name = handle.trim().trim_start_matches('@');
    if name.is_empty() {
        return false;
    }
    Regex::new(&format!(
        r"(?i)(?:^|[^\w])@{}(?:$|[^\w/-])",
        regex::escape(name)
    ))
    .is_ok_and(|re| re.is_match(text))
}

/// How `text` falls short of `approvers` (nothing, with no approvers).
pub fn gaps(text: &str, approvers: &[String]) -> Gaps {
    if approvers.is_empty() {
        return Gaps::default();
    }
    Gaps {
        missing: approvers
            .iter()
            .filter(|a| !mentions(text, a))
            .cloned()
            .collect(),
        says_none: text
            .to_ascii_lowercase()
            .contains(&NO_APPROVERS.to_ascii_lowercase()),
    }
}

/// Where an issue draft is wrapped by hand: the 1-based numbers of the lines
/// that end mid-sentence inside a paragraph, a list item or a quote. GitHub
/// shows every newline in an issue as a line break, so a draft wrapped at 100
/// columns reads as broken lines once filed. A line counts when it ends in `,`
/// or `;`, or the next one goes on in lowercase — not a sentence over another
/// (the template's **Blocks prod?** and **Depends on…** lines), a label or a
/// URL on its own line, a hard break (two spaces, `\`, `<br>`), or code. The
/// draft editor applies the same rule (`tools/milkdown/src/editor.js`).
pub fn hard_wraps(text: &str) -> Vec<usize> {
    use pulldown_cmark::{Event, Options, Parser};
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut lines = Vec::new();
    for (event, range) in Parser::new_ext(text, opts).into_offset_iter() {
        if event != Event::SoftBreak {
            continue;
        }
        let Some(nl) = text[range.start..].find('\n').map(|i| range.start + i) else {
            continue;
        };
        let prev = text[..nl].rsplit('\n').next().unwrap_or("");
        let next = text[nl + 1..].split('\n').next().unwrap_or("");
        if mid_sentence(prev, next) {
            lines.push(text[..nl].matches('\n').count() + 1);
        }
    }
    lines
}

/// Whether the break between source lines `prev` and `next` falls mid-sentence.
fn mid_sentence(prev: &str, next: &str) -> bool {
    let end = prev.trim_end_matches(|c: char| c.is_whitespace() || "*_~`".contains(c));
    if end.ends_with(',') || end.ends_with(';') {
        return true;
    }
    let start = next.trim_start_matches(|c: char| c.is_whitespace() || c == '>');
    let lower = start.to_ascii_lowercase();
    if ["http://", "https://", "www."]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return false;
    }
    start
        .trim_start_matches(|c: char| "*_~`[(\"'“‘".contains(c))
        .chars()
        .next()
        .is_some_and(char::is_lowercase)
}

/// What a project file is to /intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An intent file: `<dir>/<PREFIX>-<n>-<slug>.md` or `<dir>/<n>-<slug>.md`.
    Intent,
    /// An issue draft: `.claude/intent/drafts/<name>.md`.
    Draft,
}

/// The kind of project-relative path `rel` (either slash), if it is one.
pub fn kind(s: &IntentSettings, rel: &str) -> Option<Kind> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    if crate::notes::draft::draft_target(&format!("/{rel}")).is_some()
        && rel.starts_with(crate::notes::draft::DIR)
    {
        return Some(Kind::Draft);
    }
    let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    if !dir.eq_ignore_ascii_case(s.dir.trim_matches('/')) {
        return None;
    }
    let named = Regex::new(&format!(
        r"(?i)^(?:{}-)?\d+-.+\.md$",
        regex::escape(&s.prefix)
    ))
    .is_ok_and(|re| re.is_match(file));
    named.then_some(Kind::Intent)
}

/// Whether an intent file is still pending — `Status: Proposed`, or no status
/// yet. A decided one (Accepted, Rejected…) is history and keeps its approvers.
pub fn pending(text: &str) -> bool {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("Status:"))
        .map(str::trim)
        .is_none_or(|s| s.to_ascii_lowercase().starts_with("proposed"))
}

/// The gaps of file `path` (of `kind`) against `approvers`; `None` when it is
/// fine, decided, or unreadable.
pub fn check_file(path: &Path, kind: Kind, approvers: &[String]) -> Option<Gaps> {
    let text = fs::read_to_string(path).ok()?;
    if kind == Kind::Intent && !pending(&text) {
        return None;
    }
    Some(gaps(&text, approvers)).filter(|g| !g.is_empty())
}

/// A file that falls short of the current approvers.
#[derive(Debug, Clone)]
pub struct Stale {
    /// Project-relative, forward slashes.
    pub rel: String,
    pub kind: Kind,
    pub gaps: Gaps,
}

impl Stale {
    /// What to do about it.
    pub fn fix(&self, s: &IntentSettings) -> String {
        match self.kind {
            Kind::Intent => {
                let file = self.rel.rsplit('/').next().unwrap_or(&self.rel);
                let id = Regex::new(&format!(r"(?i)^((?:{}-)?\d+)", regex::escape(&s.prefix)))
                    .ok()
                    .and_then(|re| re.captures(file).map(|c| c[1].to_string()))
                    .unwrap_or_else(|| file.to_string());
                format!("resume it with `/intent {id}`")
            }
            Kind::Draft => {
                let name = self.rel.rsplit('/').next().unwrap_or(&self.rel);
                format!(
                    "fix it with `ocgen draft {}` (or resume its intent)",
                    name.trim_end_matches(".md")
                )
            }
        }
    }
}

/// The pending intent files and the issue drafts of project `root` that fall
/// short of its current approvers, intent files first, each by name.
pub fn stale(project: &Project, root: &Path) -> Vec<Stale> {
    let s = &project.claude.intent;
    if !project.claude.workflow.intent || s.approvers.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (dir, kind) in [
        (root.join(s.dir.trim_matches('/')), Kind::Intent),
        (root.join(crate::notes::draft::DIR), Kind::Draft),
    ] {
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .collect();
        files.sort();
        for path in files {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let rel = match kind {
                Kind::Intent => format!("{}/{name}", s.dir.trim_matches('/')),
                Kind::Draft => format!("{}/{name}", crate::notes::draft::DIR),
            };
            if self::kind(s, &rel) != Some(kind) {
                continue;
            }
            if let Some(gaps) = check_file(&path, kind, &s.approvers) {
                out.push(Stale { rel, kind, gaps });
            }
        }
    }
    out
}
