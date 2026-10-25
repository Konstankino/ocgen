//! The `/brief` brief: `.claude/briefs/<slug>.md`, the file `/brief` writes
//! after every answer of its interview and `/deliver`, `/inquire` or
//! `/workstreams` reads instead of interviewing again. Its headings and its
//! `Key: value` lines are English (ocgen parses them: [`parse`]); its text is
//! in whatever language the user writes. The slug is [`super::is_slug`]'s: a
//! file in the folder with any other name gets no page, and is named instead
//! of dropped ([`skipped`]).
//!
//! A brief has no static page. The `inquire-notes` hook shows it after every
//! write in the drafts' editor ([`super::draft`]), served live by the viewer
//! for the briefs folder ([`super::viewer`]), under a coverage strip: one cell
//! per section the target skill needs ([`Target::sections`]), each missing,
//! thin or covered ([`coverage`]), then the Open items the dry run left and the
//! Inferred ones to contest. The word `brief` typed alone in Claude Code opens
//! the session's brief, or the list ([`LIST`]). The lookup here mirrors the
//! drafts' without a `find`: nothing needs one without a CLI command.
//!
//! The parsing is lenient and for display: a carriage return is dropped, an
//! HTML comment skipped, a bullet wrapped over lines read as one, a bullet that
//! is only a placeholder (`—`, `none`, `TBD`) dropped. The skill's first write
//! is often its own template, `<…>` placeholders and all, so those are not
//! text: a section holding nothing else is missing ([`Section::is_empty`]),
//! and a placeholder beside an answer counts for nothing. Thin is a hint
//! colour, never a block: a short complete section is the user's call.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use minijinja::Value;

use super::html::escape;
use super::words::{self, Words};
use super::{browser, draft, is_slug, listing, session, viewer, Shown};

/// Where /brief keeps its briefs, relative to the project root.
pub const DIR: &str = ".claude/briefs";
/// The one-word prompt that opens the brief, or with several, their list.
pub const WORD: &str = "brief";
/// The list's path under the editor's token, and its tabs' topic: not a slug,
/// so no brief can take it.
pub const LIST: &str = "_briefs";
/// Where each session's last-written brief is kept, in the briefs directory
/// (which git-ignores itself): one file per session id, holding a slug.
pub const SESSIONS: &str = session::SESSIONS;
/// What a brief's name may hold, for messages.
pub const NAME_RULE: &str =
    "lowercase letters, digits and hyphens, starting with a letter or digit, at most 80 characters";
/// Under this many non-whitespace characters (characters, not bytes, so no
/// language is penalised) a prose section is thin.
pub const PROSE_CHARS: usize = 40;

/// The section keys: the headings as the skill's template writes them.
pub const GOAL: &str = "Goal";
pub const CONTEXT: &str = "Context";
pub const SCOPE: &str = "Scope";
pub const SUCCESS_CRITERIA: &str = "Success criteria";
pub const CONSTRAINTS: &str = "Constraints";
pub const OUT_OF_SCOPE: &str = "Out of scope";
pub const RISKS: &str = "Risks";
pub const WORKSTREAMS: &str = "Workstreams";
pub const WHAT_I_KNOW: &str = "What I know";
pub const FIRST_QUESTION: &str = "First question";
pub const OPEN: &str = "Open";
pub const INFERRED: &str = "Inferred";

/// The sections every target needs: the strip's cells with no or an unknown
/// target.
pub const CORE: [&str; 5] = [GOAL, CONTEXT, SUCCESS_CRITERIA, CONSTRAINTS, OUT_OF_SCOPE];
const DELIVER_SECTIONS: [&str; 7] = [
    GOAL,
    CONTEXT,
    SCOPE,
    SUCCESS_CRITERIA,
    CONSTRAINTS,
    OUT_OF_SCOPE,
    RISKS,
];
const INQUIRE_SECTIONS: [&str; 7] = [
    GOAL,
    CONTEXT,
    WHAT_I_KNOW,
    FIRST_QUESTION,
    SUCCESS_CRITERIA,
    CONSTRAINTS,
    OUT_OF_SCOPE,
];
const WORKSTREAMS_SECTIONS: [&str; 8] = [
    GOAL,
    CONTEXT,
    SCOPE,
    WORKSTREAMS,
    SUCCESS_CRITERIA,
    CONSTRAINTS,
    OUT_OF_SCOPE,
    RISKS,
];
/// The sections read as lists: covered by their bullets, not their prose.
const LISTS: [&str; 4] = [SUCCESS_CRITERIA, RISKS, WORKSTREAMS, WHAT_I_KNOW];

/// The skill a brief is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Deliver,
    Inquire,
    Workstreams,
}

impl Target {
    /// The targets, in the order the skill lists them.
    pub const ALL: [Target; 3] = [Target::Deliver, Target::Inquire, Target::Workstreams];

    /// The target a `Target:` line names: its word, in any case, with or
    /// without a leading `/`. `None` for anything else — an unfilled
    /// `deliver | inquire | workstreams` included.
    pub fn parse(s: &str) -> Option<Target> {
        let t = s.trim();
        let t = t.strip_prefix('/').unwrap_or(t);
        Target::ALL
            .into_iter()
            .find(|target| t.eq_ignore_ascii_case(target.word()))
    }

    /// The target's word: the skill's name without the slash.
    pub fn word(self) -> &'static str {
        match self {
            Target::Deliver => "deliver",
            Target::Inquire => "inquire",
            Target::Workstreams => "workstreams",
        }
    }

    /// The sections a brief for this target needs: the strip's cells, in order.
    pub fn sections(self) -> &'static [&'static str] {
        match self {
            Target::Deliver => &DELIVER_SECTIONS,
            Target::Inquire => &INQUIRE_SECTIONS,
            Target::Workstreams => &WORKSTREAMS_SECTIONS,
        }
    }
}

/// What the `Status:` line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Draft,
    Ready,
    /// Anything else, or no line at all.
    #[default]
    Other,
}

impl Status {
    fn parse(s: &str) -> Status {
        match clean(s).to_ascii_lowercase().as_str() {
            "draft" => Status::Draft,
            "ready" => Status::Ready,
            _ => Status::Other,
        }
    }
}

/// One `## ` section of a brief: its heading as written and its body's lines
/// (carriage returns dropped, HTML comments gone, blank lines at both ends
/// trimmed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    pub heading: String,
    pub lines: Vec<String>,
}

impl Section {
    /// The body as one text.
    pub fn text(&self) -> String {
        self.lines.join("\n").trim().to_string()
    }

    /// The body without the template's `<…>` placeholders: what was written.
    pub fn written(&self) -> String {
        without_placeholders(&self.text())
    }

    /// Whether nothing has been written: every line is blank once the `<…>`
    /// placeholders are gone, or is still one of the template's own
    /// ([`unfilled`]) — `- <criterion-slug>: <what must be true…>`,
    /// `- assumed | verified: <…>`, the Workstreams entry with its fields
    /// unfilled.
    pub fn is_empty(&self) -> bool {
        self.written().lines().all(unfilled)
    }

    /// The non-whitespace characters written (characters, not bytes), the
    /// `<…>` placeholders not counted.
    pub fn prose_chars(&self) -> usize {
        self.written()
            .chars()
            .filter(|c| !c.is_whitespace())
            .count()
    }

    /// The real bullets: each `- `, `* `, `+ ` or numbered item with its
    /// continuation lines joined by a space, fenced code left out, and the
    /// placeholders (`—`, `none`, `TBD`, the template's unfilled lines)
    /// dropped. A half-filled bullet stays as written, its `<…>` included.
    pub fn bullets(&self) -> Vec<String> {
        let mut out = self.bullets_raw();
        out.retain(|b| !placeholder(b));
        out
    }

    /// Every list item, placeholders and all.
    fn bullets_raw(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut open = false;
        let mut fenced = false;
        for line in &self.lines {
            let t = line.trim();
            if is_fence(t) {
                fenced = !fenced;
                open = false;
                continue;
            }
            if fenced {
                continue;
            }
            if t.is_empty() || t.starts_with('#') {
                open = false;
                continue;
            }
            if let Some(b) = bullet(t) {
                out.push(b.trim().to_string());
                open = true;
            } else if open {
                if let Some(last) = out.last_mut() {
                    if !last.is_empty() {
                        last.push(' ');
                    }
                    last.push_str(t);
                }
            }
        }
        out
    }

    /// The section's list items as an Open or Inferred list reads them: only
    /// a bullet that is blank once its `<…>` placeholders are gone, or a
    /// stand-in word, is dropped. An unmade choice such as `worker-kind:
    /// session | subagent` stays: as an open question, or something Claude
    /// assumed, it is real.
    pub fn items(&self) -> Vec<String> {
        let mut out = self.bullets_raw();
        out.retain(|b| !blank_item(b));
        out
    }
}

/// Whether a bullet says nothing even as an Open or Inferred item.
fn blank_item(b: &str) -> bool {
    let s = without_placeholders(b);
    let s = s
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '*' | '_'))
        .trim();
    let t = bullet(s).unwrap_or(s);
    !has_text(t)
        || matches!(
            t.to_ascii_lowercase().as_str(),
            "none" | "none yet" | "nothing" | "nothing yet" | "tbd" | "todo" | "n/a"
        )
}

fn is_fence(t: &str) -> bool {
    t.starts_with("```") || t.starts_with("~~~")
}

/// The text of a list item, if `t` (trimmed) starts one.
fn bullet(t: &str) -> Option<&str> {
    if matches!(t, "-" | "*" | "+") {
        return Some("");
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &t[digits..];
        return rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "));
    }
    None
}

/// Whether a bullet says nothing yet: it is one of the template's unfilled
/// lines ([`unfilled`]: blank, `—`, `<…>`, `<slug>: <…>`, `assumed |
/// verified: <…>`), or a stand-in typed in its place (`none`, `TBD`).
fn placeholder(b: &str) -> bool {
    let s = without_placeholders(b);
    let s = s
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '*' | '_'))
        .trim();
    unfilled(s)
        || matches!(
            s.to_ascii_lowercase().as_str(),
            "none" | "none yet" | "nothing" | "nothing yet" | "tbd" | "todo" | "n/a"
        )
}

/// `text` without the template's `<…>` placeholders: every `<` … `>` with no
/// other angle bracket and no blank line between them, so a placeholder
/// wrapped over two lines is still one, and `a < b` here with `c > d` a
/// paragraph later is prose. `Option<T>` loses its `<T>`, which costs a
/// criterion nothing and a prose section three characters.
fn without_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(['<', '>']) {
            Some(j)
                if after[j..].starts_with('>')
                    && !after[..j].lines().skip(1).any(|l| l.trim().is_empty()) =>
            {
                rest = &after[j + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether a line, its `<…>` placeholders already gone, is still as the
/// template wrote it: nothing with a letter or digit beyond a list marker,
/// punctuation, `Key:` labels and the template's unmade choices
/// ([`unmade_choice`]). Field by field (the board's ` — `): a field is
/// unfilled when it has no letter or digit, or is a label — one word, or an
/// unmade choice such as `assumed | verified` — whose value is blank or an
/// unmade choice (`Worker: session | subagent`). Prose with a colon (`The goal
/// is: …`) is a label of several words, so it is written; so is a value that
/// uses `|` in words (`session | subagent for the pins piece?`).
fn unfilled(line: &str) -> bool {
    let t = line.trim();
    let t = bullet(t).unwrap_or(t);
    t.split('—').all(|field| match field.split_once(':') {
        Some((label, value)) => {
            let label = label.trim();
            let key = !label.contains(char::is_whitespace) || unmade_choice(label);
            key && (!has_text(value) || unmade_choice(value))
        }
        None => !has_text(field),
    })
}

/// Whether `s` is one of the template's choices with none made — `session |
/// subagent`, `none |` (its `<slug>` already gone), the `assumed | verified`
/// label: alternatives on `|`, each one bare word of letters, digits and
/// hyphens, or nothing. `|` is the brief's own idiom for alternatives, so a
/// choice made in words (`exits 0 | 1 as the README says`, `` `cargo test
/// 2>&1 | tail -20` ``) is text, as is anything with a space, a backtick or
/// punctuation in an alternative.
fn unmade_choice(s: &str) -> bool {
    s.contains('|')
        && s.split('|')
            .all(|alt| alt.trim().chars().all(|c| c.is_alphanumeric() || c == '-'))
}

/// Whether `s` has a letter or a digit, in any script.
fn has_text(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

/// `v` trimmed, without the backticks a value may be written in.
fn clean(v: &str) -> String {
    v.trim().trim_matches('`').trim().to_string()
}

/// Whether heading `h` is section `key`: the key, in any case, as its first
/// words — `Success criteria (deliver)` is `Success criteria`, `Scoped` isn't
/// `Scope`.
pub fn heading_is(h: &str, key: &str) -> bool {
    let h = h.trim();
    h.len() >= key.len()
        && h.is_char_boundary(key.len())
        && h[..key.len()].eq_ignore_ascii_case(key)
        && !h[key.len()..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
}

/// A brief as written: its header and its sections, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brief {
    /// The `# ` heading's text, without its `Brief:` key: the slug, usually.
    pub title: String,
    /// The target the `Target:` line names, when it names one.
    pub target: Option<Target>,
    /// The `Target:` line's value as written.
    pub target_raw: String,
    pub status: Status,
    /// The `Status:` line's value as written.
    pub status_raw: String,
    /// The `Updated:` line's date (before its middle dot, if it has one).
    pub updated: String,
    /// The `Commit:` part of the `Updated:` line, or a `Commit:` line's value.
    pub commit: String,
    /// The `Hand-off:` line's value (also `Handoff:`, `Hand off:`).
    pub handoff: String,
    pub sections: Vec<Section>,
}

impl Brief {
    /// The section with key `key`, by its heading's first words in any case:
    /// `## Success criteria (deliver)` is the Success criteria.
    pub fn section(&self, key: &str) -> Option<&Section> {
        self.sections.iter().find(|s| heading_is(&s.heading, key))
    }

    /// What the dry run still asks: the Open section's items.
    pub fn open(&self) -> Vec<String> {
        self.section(OPEN).map(Section::items).unwrap_or_default()
    }

    /// What Claude filled in without asking: the Inferred section's items.
    pub fn inferred(&self) -> Vec<String> {
        self.section(INFERRED)
            .map(Section::items)
            .unwrap_or_default()
    }

    /// The one command to type next: the `Hand-off:` line once it is written
    /// (the template's own `/<target> .claude/briefs/<slug>.md` still holds a
    /// placeholder), else `/<target> .claude/briefs/<slug>.md`, `/deliver`
    /// with no target.
    pub fn handoff_command(&self, slug: &str) -> String {
        if !self.handoff.is_empty() && !self.handoff.contains('<') {
            return self.handoff.clone();
        }
        let target = self.target.map_or("deliver", Target::word);
        format!("/{target} {DIR}/{slug}.md")
    }
}

/// `h` without a leading `key` in any case.
fn strip_key<'a>(h: &'a str, key: &str) -> Option<&'a str> {
    (h.len() >= key.len()
        && h.is_char_boundary(key.len())
        && h[..key.len()].eq_ignore_ascii_case(key))
    .then(|| h[key.len()..].trim())
}

/// Parse brief text `md`, leniently (see the module docs). The header is every
/// `Key: value` line before the first `## `; a line that is none is ignored.
pub fn parse(md: &str) -> Brief {
    // A Windows editor may save a byte-order mark; it is not the title's.
    let md = md.strip_prefix('\u{feff}').unwrap_or(md);
    let mut b = Brief::default();
    let mut comment = false;
    let mut fenced = false;
    let mut current: Option<Section> = None;
    for raw in md.lines() {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if comment {
            comment = !t.contains("-->");
            continue;
        }
        if t.starts_with("<!--") {
            comment = !t.contains("-->");
            continue;
        }
        if is_fence(t) {
            fenced = !fenced;
        } else if !fenced {
            if let Some(h) = t.strip_prefix("## ") {
                if let Some(done) = current.take() {
                    b.sections.push(finish(done));
                }
                current = Some(Section {
                    heading: h.trim().to_string(),
                    lines: Vec::new(),
                });
                continue;
            }
        }
        match current.as_mut() {
            Some(s) => s.lines.push(line.to_string()),
            None if fenced => {}
            None => header_line(&mut b, t),
        }
    }
    if let Some(done) = current.take() {
        b.sections.push(finish(done));
    }
    b
}

/// A section with the blank lines at both ends of its body dropped.
fn finish(mut s: Section) -> Section {
    while s.lines.first().is_some_and(|l| l.trim().is_empty()) {
        s.lines.remove(0);
    }
    while s.lines.last().is_some_and(|l| l.trim().is_empty()) {
        s.lines.pop();
    }
    s
}

/// Read one header line (trimmed) into `b`.
fn header_line(b: &mut Brief, t: &str) {
    if let Some(h) = t.strip_prefix("# ") {
        let h = h.trim();
        b.title = strip_key(h, "Brief:").unwrap_or(h).to_string();
        return;
    }
    let Some((k, v)) = t.split_once(':') else {
        return;
    };
    let v = v.trim();
    match k.trim().to_ascii_lowercase().as_str() {
        "target" => {
            b.target_raw = v.to_string();
            b.target = Target::parse(v);
        }
        "status" => {
            b.status_raw = v.to_string();
            b.status = Status::parse(v);
        }
        "updated" => match v.split_once('·') {
            Some((date, rest)) => {
                b.updated = date.trim().to_string();
                if let Some(c) = strip_key(rest.trim(), "Commit:") {
                    if b.commit.is_empty() {
                        b.commit = clean(c);
                    }
                }
            }
            None => b.updated = v.to_string(),
        },
        "commit" => b.commit = clean(v),
        "hand-off" | "handoff" | "hand off" => b.handoff = clean(v),
        _ => {}
    }
}

// ------------------------------------------------------------- coverage --

/// How far a section is filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Absent, or nothing written: blank, or still the template's own lines.
    Missing,
    /// There, but under the threshold: a hint, never a block.
    Thin,
    Covered,
}

impl Fill {
    /// The state's English word: the strip's class names and the words' keys.
    pub fn word(self) -> &'static str {
        match self {
            Fill::Missing => "missing",
            Fill::Thin => "thin",
            Fill::Covered => "covered",
        }
    }
}

/// One cell of the coverage strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub key: &'static str,
    pub fill: Fill,
}

/// How many cells are in each state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub covered: usize,
    pub thin: usize,
    pub missing: usize,
}

/// What the strip shows for a brief: its target's sections, each filled or
/// not, and how many items its Open and Inferred sections hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub target: Option<Target>,
    pub cells: Vec<Cell>,
    pub open: usize,
    pub inferred: usize,
}

impl Coverage {
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for cell in &self.cells {
            match cell.fill {
                Fill::Covered => c.covered += 1,
                Fill::Thin => c.thin += 1,
                Fill::Missing => c.missing += 1,
            }
        }
        c
    }

    /// Whether every section is covered and the dry run left nothing open.
    pub fn complete(&self) -> bool {
        self.open == 0 && self.cells.iter().all(|c| c.fill == Fill::Covered)
    }
}

/// The coverage of `b`: its target's sections ([`Target::sections`], or
/// [`CORE`] with none). A section is Missing when absent or nothing is written
/// in it ([`Section::is_empty`]: the template's `<…>` placeholders are not
/// text). A list section (Success criteria, Risks, Workstreams, What I know)
/// is Thin under one real bullet — Success criteria also when no bullet is
/// `<slug>: text`, Workstreams when an entry lacks a `Workstream:` or a
/// `Files:` value — and a prose section under [`PROSE_CHARS`] non-whitespace
/// characters written.
pub fn coverage(b: &Brief) -> Coverage {
    let keys: &[&'static str] = b.target.map_or(&CORE, Target::sections);
    let cells = keys
        .iter()
        .map(|key| Cell {
            key,
            fill: fill(b.section(key), key),
        })
        .collect();
    Coverage {
        target: b.target,
        cells,
        open: b.open().len(),
        inferred: b.inferred().len(),
    }
}

fn fill(s: Option<&Section>, key: &str) -> Fill {
    let Some(s) = s else {
        return Fill::Missing;
    };
    if s.is_empty() {
        return Fill::Missing;
    }
    if !LISTS.contains(&key) {
        return if s.prose_chars() < PROSE_CHARS {
            Fill::Thin
        } else {
            Fill::Covered
        };
    }
    let bullets = s.bullets();
    let good = match key {
        _ if bullets.is_empty() => false,
        SUCCESS_CRITERIA => bullets.iter().any(|b| has_slug(b)),
        WORKSTREAMS => bullets.iter().all(|b| is_workstream(b)),
        _ => true,
    };
    if good {
        Fill::Covered
    } else {
        Fill::Thin
    }
}

/// Whether a bullet has the `<slug>: text` form, the slug perhaps in backticks
/// or bold, the text written (not the template's `<…>`).
fn has_slug(b: &str) -> bool {
    without_placeholders(b)
        .split_once(':')
        .is_some_and(|(k, text)| {
            is_slug(
                k.trim()
                    .trim_matches(|c: char| matches!(c, '`' | '*' | '_'))
                    .trim(),
            ) && has_text(text)
        })
}

/// Whether a Workstreams entry has the board's shape, as far as the strip
/// cares: a `Workstream:` and a `Files:` field, each with a value written
/// (the template's `<slug>` and `<paths>` are none).
fn is_workstream(b: &str) -> bool {
    let b = without_placeholders(b);
    ["Workstream:", "Files:"].iter().all(|key| {
        b.split_once(key)
            .is_some_and(|(_, rest)| has_text(rest.split('—').next().unwrap_or("")))
    })
}

// --------------------------------------------------------------- lookup --

/// Whether `s` can name a brief (`<s>.md`): a slug ([`is_slug`]).
pub fn is_name(s: &str) -> bool {
    is_slug(s)
}

/// Whether a prompt is the one word that opens the brief (any case, nothing else).
pub fn is_prompt(prompt: &str) -> bool {
    prompt.trim().eq_ignore_ascii_case(WORD)
}

/// The slug of a brief — `.claude/briefs/<slug>.md`, any path form — or `None`
/// for anything else: a workstream's `.claude/workstreams/<slug>/brief.md`, a
/// draft, a file in the folder whose name is no slug.
pub fn brief_target(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let mut parts = p.rsplit('/');
    let (file, briefs, claude) = (parts.next()?, parts.next()?, parts.next()?);
    if (briefs, claude) != ("briefs", ".claude") {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    is_slug(stem).then(|| stem.to_string())
}

/// Whether `dir` is a project's `.claude/briefs` directory, by its components
/// (on Unix a `\` is part of a name, not a separator).
pub fn is_briefs_dir(dir: &Path) -> bool {
    let abs = super::absolute(dir);
    let mut names = abs.components().rev().map(|c| match c {
        std::path::Component::Normal(n) => n.to_str(),
        _ => None,
    });
    names.next() == Some(Some("briefs")) && names.next() == Some(Some(".claude")) && dir.is_dir()
}

/// The briefs directory of the project containing `start`.
pub fn briefs_dir(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok()?;
    start
        .ancestors()
        .map(|a| a.join(DIR))
        .find(|d| d.is_dir())
        .map(|d| crate::paths::plain(&d))
}

/// The briefs in `dir`, most recently changed first.
pub fn briefs(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".md"))
                .is_some_and(is_name)
        })
        .map(|p| {
            let t = fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The `.md` files in `dir` whose names are no slug ([`is_name`]), sorted.
/// ocgen can't show them, so the list and the hook name them instead of
/// dropping them.
pub fn skipped(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.strip_suffix(".md").is_some_and(|s| !is_name(s)))
        .collect();
    names.sort();
    names
}

/// Remember that `session` wrote brief `slug` last (see [`SESSIONS`]), and
/// drop records older than a month. False when nothing was recorded: an id or
/// slug that can't name a file, or a write that failed.
pub fn remember(dir: &Path, session: &str, slug: &str) -> bool {
    session::remember(dir, session, slug, is_name)
}

/// The brief `session` wrote last, while it still exists.
pub fn session_brief(dir: &Path, session: &str) -> Option<String> {
    session::recall(dir, dir, session, is_name)
}

// ----------------------------------------------------------------- show --

/// Open brief `md` in the browser editor: tell an open tab, or open one.
/// `explicit` is the user's word `brief`: it opens a tab unless one already
/// shows the brief. Without a viewer to run (no ocgen binary, a sandbox) an
/// explicit call fails with the reason, and the hook's — after a write — stays
/// silent with `Shown::Off`: a brief has no file page to fall back on, the next
/// write tries again, and a closed tab is reopened then, as a ledger's is.
pub fn show(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    let Some(slug) = brief_target(&super::absolute(md).to_string_lossy()) else {
        bail!(
            "{} is not a brief in {DIR}/ (a brief is named by a slug: {NAME_RULE})",
            md.display()
        );
    };
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let dir = md.parent().unwrap_or(Path::new("."));
    session::ignore_self(dir);
    let Some(info) = editor(dir, env, explicit)? else {
        return Ok(Shown::Off);
    };
    let action = viewer::reload(&info, &slug, explicit);
    draft::shown(action, || info.url(&slug), env)
}

/// Open the list of the briefs in `dir` — on the page holding `selected`, which
/// is marked — in a tab that shows the list already, or a new one. Without a
/// viewer to run, as [`show`].
pub fn show_list(
    dir: &Path,
    selected: Option<&str>,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Shown> {
    if !is_briefs_dir(dir) {
        bail!("{} is not a {DIR} directory", dir.display());
    }
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let selected = selected.filter(|s| is_name(s));
    session::ignore_self(dir);
    let Some(info) = editor(dir, env, explicit)? else {
        return Ok(Shown::Off);
    };
    let action = viewer::reload_list_of(&info, LIST, selected, explicit);
    draft::shown(action, || info.list_url_of(LIST, selected), env)
}

/// The editor for briefs directory `dir`, started if need be: `None` when none
/// could be and the call isn't the user's.
fn editor(
    dir: &Path,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Option<viewer::Info>> {
    match viewer::ensure(dir, env) {
        Some(info) => Ok(Some(info)),
        None if explicit => bail!(
            "could not start the local editor (it needs the ocgen binary and a free loopback port)"
        ),
        None => Ok(None),
    }
}

// ----------------------------------------------------------------- page --

/// The editor page for brief text `md` (revision `rev`, at `source`, which is
/// `.claude/briefs/<slug>.md`), whose one script carries `nonce`; its own words
/// in `language`. It is the drafts' editor in its brief mode
/// ([`draft::editor_page`]): the coverage strip ([`strip_html`]) above the
/// column, Copy brief and Copy hand-off beside Copy formatted, and none of what
/// only an issue has (the approvers and line-break banners). The page asks for
/// the strip again after every edit (`POST coverage`, [`super::viewer`]).
pub fn page(md: &str, source: &str, rev: &str, nonce: &str, language: &str) -> Result<String> {
    let w = words::for_language(language);
    let name = source.rsplit('/').next().unwrap_or(source).to_string();
    let slug = name.trim_end_matches(".md").to_string();
    // Escaped here: minijinja's own escaping also turns `/` into `&#x2f;`.
    let safe = |s: &str| Value::from_safe_string(escape(s));
    let ctx = minijinja::context! {
        brief => true,
        w => words_value(w),
        name => safe(&name),
        slug => safe(&slug),
        source => safe(source),
        rev => safe(rev),
        nonce => safe(nonce),
        text => safe(md),
        // A brief is a file, not an issue: a newline is a space, as GitHub
        // shows a `.md` file in a repository.
        preview => Value::from_safe_string(draft::render_safe(md, false)),
        // Ours, built to be inlined (nothing in it ends the script).
        editor => Value::from_safe_string(draft::EDITOR_JS.to_string()),
        list => LIST,
        strip => Value::from_safe_string(strip_html(md, &slug, w)),
        // What only a draft has, inert here: the template's draft-only lines
        // aren't rendered, and the script's two checks are stubs.
        gaps => false,
        missing => "",
        says_none => false,
        wraps => 0,
        approvers => "",
    };
    draft::editor_page(ctx)
}

/// The brief-only markup of the page for brief text `md` (slug `slug`), in
/// the words `w`: one `div.brief-head`, whose `data-handoff` is the command to
/// type next ([`Brief::handoff_command`]) and whose `data-target` and
/// `data-status` say what the header names; the target and status badges; the
/// coverage strip, an `ol.coverage` with one `li.cell` per section the target
/// needs, its class `covered`, `thin` or `missing` and its `title` and
/// `aria-label` naming the section and its state; what the dry run still asks
/// (`details.brief-open`, open, or the sentence that nothing is left); and
/// what was inferred without asking (`details.brief-inferred`). Every text is
/// escaped; the section names are shown in the page's language.
pub fn strip_html(md: &str, slug: &str, w: &Words) -> String {
    let b = parse(md);
    let c = coverage(&b);
    let bw = &w.brief;
    let status = match b.status {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Other => "",
    };
    let mut out = format!(
        "<div class=\"brief-head\" data-handoff=\"{}\" data-status=\"{status}\" data-target=\"{}\">\n",
        escape(&b.handoff_command(slug)),
        b.target.map_or("", Target::word)
    );
    // The badges: the target, and the status when the header has one.
    out.push_str("<div class=\"brief-badges\">");
    match b.target {
        Some(t) => out.push_str(&format!(
            "<span class=\"badge target\">{} /{}</span>",
            escape(bw.target),
            t.word()
        )),
        None => out.push_str(&format!(
            "<span class=\"badge target\">{}</span>",
            escape(bw.no_target)
        )),
    }
    let status_label = match b.status {
        Status::Draft => Some(bw.status_draft.to_string()),
        Status::Ready => Some(bw.status_ready.to_string()),
        Status::Other => (!b.status_raw.is_empty()).then(|| b.status_raw.clone()),
    };
    if let Some(label) = status_label {
        out.push_str(&format!(
            "<span class=\"badge status{}\">{}</span>",
            if b.status == Status::Ready {
                " ready"
            } else {
                ""
            },
            escape(&label)
        ));
    }
    out.push_str("</div>\n");
    // The strip: one cell per section the target needs.
    out.push_str(&format!(
        "<ol class=\"coverage\" aria-label=\"{}\">",
        escape(bw.coverage)
    ));
    for cell in &c.cells {
        let state = match cell.fill {
            Fill::Missing => bw.missing,
            Fill::Thin => bw.thin,
            Fill::Covered => bw.covered,
        };
        let name = bw.section(cell.key);
        let label = escape(&format!("{name}: {state}"));
        out.push_str(&format!(
            "<li class=\"cell {}\" title=\"{label}\" aria-label=\"{label}\">{}</li>",
            cell.fill.word(),
            escape(&name)
        ));
    }
    out.push_str("</ol>\n");
    // What the dry run still asks, open: it is what the user reads first.
    let open = b.open();
    out.push_str(&format!(
        "<details class=\"brief-open\" open><summary>{} ({})</summary>",
        escape(bw.open),
        open.len()
    ));
    if open.is_empty() {
        out.push_str(&format!("<p>{}</p>", escape(bw.open_none)));
    } else {
        out.push_str("<ul>");
        for item in &open {
            out.push_str(&format!("<li>{}</li>", escape(item)));
        }
        out.push_str("</ul>");
    }
    out.push_str("</details>\n");
    // What was inferred without asking, folded: contested by slug in the chat.
    let inferred = b.inferred();
    out.push_str(&format!(
        "<details class=\"brief-inferred\"><summary>{} ({})</summary>",
        escape(bw.inferred),
        inferred.len()
    ));
    if !inferred.is_empty() {
        out.push_str("<ul>");
        for item in &inferred {
            out.push_str(&format!("<li>{}</li>", escape(item)));
        }
        out.push_str("</ul>");
    }
    out.push_str("</details>\n</div>\n");
    out
}

/// What a brief is about, for the list: its Goal's first line of text (the
/// template's `<…>` placeholders not counted), at most 120 characters.
pub fn summary(b: &Brief) -> String {
    let Some(goal) = b.section(GOAL) else {
        return String::new();
    };
    let written = goal.written();
    let Some(line) = written.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return String::new();
    };
    match line.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", line[..i].trim_end()),
        None => line.to_string(),
    }
}

/// The list page of briefs directory `dir`: page `page` (out of range: the
/// nearest), or with none, the page holding `selected`, which is marked. Its
/// links lead from `root`, the editor's root as seen from the page (`./`, or
/// `../` under [`LIST`]); its one script carries `nonce`. A row shows the
/// brief's Goal, its target, its status and when its header says it was
/// updated; the files whose names are no slug are named below the header.
pub fn list_page(
    dir: &Path,
    page: Option<usize>,
    selected: Option<&str>,
    root: &str,
    nonce: &str,
) -> Result<String> {
    let language = super::answer_language(dir);
    let w = words::for_language(&language);
    let all = briefs(dir);
    let row = |_: &Path, text: &str| {
        let b = parse(text);
        let mut tags = vec![b
            .target
            .map_or_else(|| w.brief.no_target.to_string(), |t| t.word().to_string())];
        match b.status {
            Status::Draft => tags.push(w.brief.status_draft.to_string()),
            Status::Ready => tags.push(w.brief.status_ready.to_string()),
            Status::Other if !b.status_raw.is_empty() => tags.push(b.status_raw.clone()),
            Status::Other => {}
        }
        if !b.updated.is_empty() {
            tags.push(format!("{} {}", w.docs.updated, b.updated));
        }
        listing::Row {
            summary: summary(&b),
            tags,
            page: None,
        }
    };
    listing::page(
        &listing::List {
            kind: listing::Kind::Briefs,
            docs: &all,
            row: &row,
            source: DIR,
            list: LIST,
            skipped: &skipped(dir),
            skipped_label: w.brief.skipped,
            language: &language,
        },
        page,
        selected,
        root,
        nonce,
    )
}

/// The brief page's words: the draft editor's own (Save, Source, Preview,
/// Focus, the palettes, the banners, the keys) under the brief's title, editor
/// name, footer, offline banner, list link and Copy buttons.
pub fn words_value(w: &'static Words) -> Value {
    let (d, b) = (&w.draft, &w.brief);
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("lang", Value::from(w.lang));
    for (k, v) in [
        ("title", b.title),
        ("editor", b.editor),
        ("all", b.all_briefs),
        ("footer", b.footer),
        ("copy_markdown", b.copy_brief),
        ("copy_handoff", b.copy_handoff),
        ("copy_formatted", d.copy_formatted),
        ("source", d.source),
        ("preview", d.preview),
        ("focus", d.focus),
        ("focus_title", d.focus_title),
        ("palette", d.palette),
        ("black", d.black),
        ("white", d.white),
        ("link_prompt", d.link_prompt),
        ("source_only", d.source_only),
        ("save", d.save),
        ("saved", d.saved),
        ("unsaved", d.unsaved),
        ("saving", d.saving),
        ("save_failed", d.save_failed),
        ("copied", d.copied),
        ("copy_failed", d.copy_failed),
        ("changed", d.changed),
        ("conflict", d.conflict),
        ("load_disk", d.load_disk),
        ("keep_mine", d.keep_mine),
        ("overwrite", d.overwrite),
        ("updated", d.updated),
    ] {
        m.insert(k, Value::from(v));
    }
    // Ours, not the brief's: they mark up the word or the keys.
    m.insert("offline", Value::from_safe_string(b.offline.to_string()));
    m.insert("keys", Value::from_safe_string(d.keys.to_string()));
    Value::from(m)
}
