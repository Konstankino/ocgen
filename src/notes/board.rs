//! The `/workstreams` board's page. The board, `.claude/workstreams/board.md`,
//! is the manager session's record of a goal split into workstreams: a
//! `Status:` line, one `- Workstream: <slug> — Key: value — …` entry per
//! workstream and the `Differs` and `Log` sections, in English, in the shape the
//! `workstreams-merge-gate` hook parses. ocgen renders `board.html` beside it
//! after every write (the `inquire-notes` hook) and never writes the Markdown.
//! The page opens with the broad picture, for a tab kept beside the CLI: the
//! progress (a strip and counts), the next steps read off the board the way the
//! skill goes ([`next_steps`]), and the agents — a card per worker with the
//! workstream and goal it is on, its state and gate, its report's facts and the
//! issues its report, its board line, the differs and the loop guard raise
//! ([`agents`]); a report a worker posted in its worktree shows at once, before
//! the manager collects it ([`posted_report`]). Then the board as written: the
//! header fields, a table of the workstreams — with links to each one's brief
//! and report where they exist, a state track and a link to what each waits
//! for — and the other sections, the log as a timeline. `ocgen notes open
//! workstreams` shows it again. The board is kept in one language only
//! ([`Kind::Board`]), so no page asks for another.
//!
//! The page is live the way a ledger's is: the viewer ([`super::viewer`]) runs
//! for the board's directory, serves the page fresh from the Markdown, watches
//! the board, the briefs and reports it links, the posted reports and the
//! escalations ([`rev`]) — a change the hook never saw reloads the tab too —
//! and serves the briefs and reports read-only beside it. The page's script
//! swaps an update in without a reload, lets a workstream be selected (its
//! agents, its row, its log lines; kept in the address), folds every section
//! and card, and highlights what moved since the tab last showed the board.
//! Without a viewer to run, the static `board.html` opens once as a file.
//!
//! The parsing here is lenient and for display: a value that isn't known yet is
//! `—`, an entry wrapped over lines is read as one until the next `- `, a
//! carriage return is dropped. The hook's own parser decides what merges.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use regex::Regex;

use super::html::escape;
use super::intents::{self, Content, Zen, ZenKind};
use super::versions::{Kind, Switch};
use super::{browser, draft, is_slug, viewer, Shown};

/// The board's file name, in [`Kind::Board`]'s directory.
pub const FILE: &str = "board.md";
/// The word that opens the board: `ocgen notes open workstreams`.
pub const WORD: &str = "workstreams";
/// The board's name in the viewer: its page is `board.html`, its tabs' topic.
pub const TOPIC: &str = "board";
/// The files of a workstream's folder the page links and the viewer serves.
pub const FILES: [&str; 2] = ["brief.md", "report.md"];
/// The table's fixed columns: the entry key each one shows.
const COLUMNS: [&str; 7] = [
    "Workstream",
    "Worker",
    "Branch",
    "State",
    "Gate",
    "Tip",
    "After",
];
/// How much of a full commit SHA the table shows.
const SHORT_SHA: usize = 12;
/// A workstream's way through the board, in order: the states the skill uses.
const WAY: [&str; 5] = ["planned", "dispatched", "reported", "checked", "integrated"];
/// The confidence a worker must state, when the project sets no bar.
const DEFAULT_BAR: u8 = 95;
/// Where the loop guard writes what it halted, relative to the project root.
const ESCALATIONS: &str = ".claude/loop-guard/escalations.md";
/// The most of a brief's goal the agents panel shows.
const GOAL_CHARS: usize = 240;
/// Where a worker posts its report, relative to its worktree (the skill's
/// report block), before the manager collects it to the board's folder.
const POSTED: &str = ".claude/scratch/workstream-report.md";
/// Where the worktrees live, relative to the project root.
const WORKTREES: &str = ".claude/worktrees";

/// One workstream entry: its `Key: value` pairs, in the order written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub fields: Vec<(String, String)>,
}

impl Entry {
    /// The value of `key` (any case), if the entry has it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// A board split for its page.
#[derive(Debug, Clone, Default)]
pub struct Board {
    /// Its `# ` title, without the marker.
    pub title: Option<String>,
    /// The `Key: value` lines at the top (`Status:`, `Goal:`, `Base:`, …), and
    /// the first `Status:` line found anywhere when the top has none.
    pub fields: Vec<(String, String)>,
    /// The workstream entries, wherever they are.
    pub entries: Vec<Entry>,
    /// The `## Differs` items, as `(slug, what)`: a file a branch touched
    /// outside its brief, for the manager to accept or reject.
    pub differs: Vec<(String, String)>,
    /// The rest in order: Markdown, with the table where the first entry was.
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Markdown(String),
    Table,
}

impl Board {
    /// The `Status:` line's value.
    pub fn status(&self) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == "Status")
            .map(|(_, v)| v.as_str())
    }
}

/// A header line: `Key: value`.
fn header_re() -> Regex {
    Regex::new(r"^([A-Za-z][A-Za-z0-9 _-]{0,30}):\s*(.*)$").unwrap()
}

/// The start of a list item: `- `, with any whitespace after the dash.
fn item(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('-')?;
    (rest.starts_with(' ') || rest.starts_with('\t')).then(|| rest.trim_start())
}

/// Whether a list item is a workstream entry.
fn is_entry(text: &str) -> bool {
    text.get(.."Workstream:".len())
        .is_some_and(|head| head.eq_ignore_ascii_case("Workstream:"))
}

/// The `Key: value` pairs of an entry's text: a key is a word followed by `:`
/// at the start or after ` — ` (an en dash counts too), its value runs to the
/// next one. `Branch: — — Worktree: —` reads as `—` for both: a `—` value
/// never swallows the next key.
fn fields_of(text: &str) -> Vec<(String, String)> {
    let re = Regex::new(r"(?:^|\s+[—–]\s+)([A-Za-z][A-Za-z0-9_-]*):").unwrap();
    let marks: Vec<(usize, usize, String)> = re
        .captures_iter(text)
        .map(|c| {
            let m = c.get(0).unwrap();
            (m.start(), m.end(), c[1].to_string())
        })
        .collect();
    marks
        .iter()
        .enumerate()
        .map(|(i, (_, end, key))| {
            let until = marks.get(i + 1).map_or(text.len(), |m| m.0);
            (key.clone(), text[*end..until].trim().to_string())
        })
        .collect()
}

/// Split board text `md` for its page.
pub fn parse(md: &str) -> Board {
    let lines: Vec<&str> = md.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut b = Board::default();
    let mut i = 0;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    if let Some(t) = lines.get(i).and_then(|l| l.trim().strip_prefix("# ")) {
        b.title = Some(t.trim().to_string());
        i += 1;
    }
    let header = header_re();
    while let Some(c) = lines.get(i).and_then(|l| header.captures(l.trim())) {
        b.fields
            .push((c[1].trim().to_string(), c[2].trim().to_string()));
        i += 1;
    }
    let mut chunk: Vec<&str> = Vec::new();
    let mut entry: Option<(Vec<String>, bool)> = None;
    let mut seen_table = false;
    let mut status_at: Option<usize> = None;
    let rest = &lines[i..];
    // A board whose `Status:` line isn't at the top: the first one counts, as
    // for the hook, and it is shown with the fields rather than in the text.
    if b.status().is_none() {
        status_at = rest.iter().position(|l| l.starts_with("Status:"));
        if let Some(at) = status_at {
            b.fields.insert(
                0,
                (
                    "Status".to_string(),
                    rest[at]["Status:".len()..].trim().to_string(),
                ),
            );
        }
    }
    let flush_entry = |entry: &mut Option<(Vec<String>, bool)>,
                       chunk: &mut Vec<&str>,
                       b: &mut Board,
                       seen_table: &mut bool| {
        let Some((lines, is_ws)) = entry.take() else {
            return;
        };
        if is_ws {
            let text = lines.join(" ");
            b.entries.push(Entry {
                fields: fields_of(&text),
            });
            if !*seen_table {
                *seen_table = true;
                if !chunk.is_empty() {
                    b.segments.push(Segment::Markdown(chunk.join("\n")));
                    chunk.clear();
                }
                b.segments.push(Segment::Table);
            }
        }
    };
    let mut in_differs = false;
    for (n, line) in rest.iter().enumerate() {
        if status_at == Some(n) {
            continue;
        }
        if let Some(text) = item(line) {
            flush_entry(&mut entry, &mut chunk, &mut b, &mut seen_table);
            if is_entry(text) {
                entry = Some((vec![text.to_string()], true));
            } else {
                // Any other list item stays Markdown, wrapped lines included.
                if in_differs {
                    if let Some((who, what)) = text.split_once(':') {
                        b.differs
                            .push((who.trim().to_string(), what.trim().to_string()));
                    }
                }
                entry = Some((vec![line.to_string()], false));
                chunk.push(line);
            }
        } else if line.starts_with('#') || line.trim().is_empty() {
            flush_entry(&mut entry, &mut chunk, &mut b, &mut seen_table);
            if line.starts_with('#') {
                in_differs = line
                    .trim_start_matches('#')
                    .trim()
                    .eq_ignore_ascii_case("Differs");
            }
            chunk.push(line);
        } else if let Some((e, is_ws)) = entry.as_mut() {
            e.push(line.trim().to_string());
            if !*is_ws {
                chunk.push(line);
            }
        } else {
            chunk.push(line);
        }
    }
    flush_entry(&mut entry, &mut chunk, &mut b, &mut seen_table);
    if !chunk.is_empty() {
        b.segments.push(Segment::Markdown(chunk.join("\n")));
    }
    b
}

/// Whether `topic` (an `ocgen notes open` argument) asks for the board: the
/// word, or the board's path in any form.
pub fn wanted(topic: &str) -> bool {
    let t = topic.trim();
    t.eq_ignore_ascii_case(WORD)
        || t.replace('\\', "/")
            .ends_with(&format!("{}/{FILE}", Kind::Board.dir()))
}

/// Whether `dir` is a project's `.claude/workstreams` directory, by its
/// components — the one place the viewer serves a board from.
pub fn is_board_dir(dir: &Path) -> bool {
    let abs = super::absolute(dir);
    let mut names = abs.components().rev().map(|c| match c {
        std::path::Component::Normal(n) => n.to_str(),
        _ => None,
    });
    names.next() == Some(Some("workstreams"))
        && names.next() == Some(Some(".claude"))
        && dir.is_dir()
}

/// The board of the project containing `start`, if there is one.
pub fn find(start: &Path) -> Option<PathBuf> {
    super::absolute(start)
        .ancestors()
        .map(|a| a.join(Kind::Board.dir()).join(FILE))
        .find(|p| p.is_file())
        .map(|p| crate::paths::plain(&p))
}

// ----------------------------------------------------------------- page --

/// An inline icon, by name: a small stroke drawing the page can colour and
/// animate (`ico-working` turns, `ico-tick` draws itself, `ico-warn` nods).
fn icon(name: &str) -> String {
    let path = match name {
        "session" => {
            r#"<rect x="2" y="3" width="12" height="10" rx="2"/><path d="M5 7l2 1.5L5 10M8.5 10h3"/>"#
        }
        "subagent" => {
            r#"<circle cx="4" cy="4" r="1.6"/><circle cx="4" cy="12" r="1.6"/><circle cx="12" cy="6" r="1.6"/><path d="M4 5.6v4.8M4 10c3 0 5-1 8-2.4"/>"#
        }
        "worker" => {
            r#"<circle cx="8" cy="5.5" r="2.5"/><path d="M3 14c0-3 2.2-4.5 5-4.5s5 1.5 5 4.5"/>"#
        }
        "unassigned" => r#"<circle cx="8" cy="8" r="5.5" stroke-dasharray="2.5 2"/>"#,
        "working" => r#"<circle cx="8" cy="8" r="5.5" stroke-dasharray="22 13"/>"#,
        "clock" => r#"<circle cx="8" cy="8" r="5.5"/><path d="M8 5v3.2l2 1.2"/>"#,
        "tick" => r#"<path d="M3.5 8.5l3 3 6-7" pathLength="1"/>"#,
        "warn" => r#"<path d="M8 2.5l6 11H2z"/><path d="M8 6.5v3M8 11.6v.4"/>"#,
        _ => "",
    };
    format!(
        r#"<svg class="ico ico-{name}" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#
    )
}

/// A class or data-attribute token from a board value: its first word, in
/// lowercase letters, digits and hyphens; empty for an unknown value.
fn token(value: &str) -> String {
    value
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether a value is unknown (`—`, or nothing).
fn unknown(value: &str) -> bool {
    matches!(value.trim(), "" | "—" | "–" | "-")
}

/// A value as a cell: a commit SHA shortened with the whole on hover, a path
/// under the project `root` made relative to it (the whole on hover), a path or
/// a ref in `<code>`, anything else as text; `—` when unknown.
fn value_html(value: &str, root: Option<&Path>) -> String {
    let v = value.trim();
    if unknown(v) {
        return "—".to_string();
    }
    if let Some(rel) = root.and_then(|r| relative(v, r)) {
        return format!(r#"<code title="{}">{}</code>"#, escape(v), escape(&rel));
    }
    let hex = v.len() >= 7 && v.bytes().all(|b| b.is_ascii_hexdigit());
    if hex && v.len() > SHORT_SHA {
        return format!(
            r#"<code title="{}">{}</code>"#,
            escape(v),
            escape(&v[..SHORT_SHA])
        );
    }
    let path = (v.contains('/') || v.contains('\\'))
        && !v.contains(char::is_whitespace)
        && !v.contains('<');
    if hex || path {
        return format!("<code>{}</code>", escape(v));
    }
    escape(v)
}

/// `value` as a path relative to `root` when it is under it (either separator),
/// else `None`.
fn relative(value: &str, root: &Path) -> Option<String> {
    let root = crate::paths::plain(root)
        .to_string_lossy()
        .replace('\\', "/");
    let root = root.trim_end_matches('/');
    let v = value.replace('\\', "/");
    let rest = v.strip_prefix(root)?;
    let rel = rest.strip_prefix('/')?;
    (!rel.is_empty()).then(|| rel.to_string())
}

/// A state or gate cell: the word, classed so the page can colour it; a
/// merged gate draws its tick.
fn badge(kind: &str, value: &str) -> String {
    let v = value.trim();
    if unknown(v) {
        return "—".to_string();
    }
    let t = token(v);
    let ico = if kind == "gate" && t == "merged" {
        icon("tick")
    } else {
        String::new()
    };
    format!(
        r#"<span class="{kind} {kind}-{t}">{}{ico}</span>"#,
        escape(v)
    )
}

/// The workstream cell: the slug, and links to its brief and report when they
/// are beside the board (`<slug>/brief.md`, `<slug>/report.md`).
fn slug_html(slug: &str, dir: Option<&Path>) -> String {
    let s = slug.trim();
    if unknown(s) {
        return "—".to_string();
    }
    let mut out = if is_slug(s) {
        format!(r##"<a class="pick" href="#ws-{s}">{s}</a>"##)
    } else {
        format!("<b>{}</b>", escape(s))
    };
    if let Some(dir) = dir.filter(|_| is_slug(s)) {
        let links: Vec<String> = ["brief", "report"]
            .iter()
            .filter(|f| dir.join(s).join(format!("{f}.md")).is_file())
            .map(|f| format!(r#"<a href="{s}/{f}.md">{f}</a>"#))
            .collect();
        if !links.is_empty() {
            out.push_str(&format!(
                r#" <span class="files">{}</span>"#,
                links.join(" · ")
            ));
        }
    }
    out
}

/// Where state `state` is on the way ([`WAY`]), if it is on it.
fn step(state: &str) -> Option<usize> {
    let t = token(state);
    WAY.iter().position(|s| *s == t)
}

/// A row's state track: five steps, the ones reached filled, the current one
/// marked; nothing for a state that isn't on the way.
fn track(state: &str) -> String {
    let Some(at) = step(state) else {
        return String::new();
    };
    let mut out = String::from(r#"<ol class="track" aria-hidden="true">"#);
    for (i, _) in WAY.iter().enumerate() {
        let class = match i.cmp(&at) {
            std::cmp::Ordering::Less => r#" class="done""#,
            std::cmp::Ordering::Equal => r#" class="done now""#,
            std::cmp::Ordering::Greater => "",
        };
        out.push_str(&format!(r#"<li{class} style="--i:{i}"></li>"#));
    }
    out.push_str("</ol>");
    out
}

/// The summary above the table: a strip with one segment per workstream in
/// board order, coloured by state, and the counts — each state on the way
/// that occurs, in order, then any other, then the branches ready to merge,
/// then the issues the agents panel raises.
fn summary(entries: &[Entry], issues: usize) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let get = |e: &Entry, k: &str| e.get(k).unwrap_or("").trim().to_string();
    let integrated = entries
        .iter()
        .filter(|e| token(&get(e, "State")) == "integrated")
        .count();
    let mut out = format!(
        "<section class=\"sec\" data-sec=\"progress\"><details class=\"pane\" data-pane=\"progress\" open><summary><h2>Progress</h2> <span class=\"count\">{integrated} of {} integrated · {}%</span></summary>\n",
        entries.len(),
        integrated * 100 / entries.len()
    );
    out.push_str(&format!(
        r#"<div class="board-summary"><div class="board-strip" role="img" aria-label="{integrated} of {} integrated">"#,
        entries.len()
    ));
    for e in entries {
        let state = get(e, "State");
        let slug = get(e, "Workstream");
        out.push_str(&format!(
            r#"<span class="seg state-{}" data-slug="{}" title="{} — {}"></span>"#,
            token(&state),
            if is_slug(&slug) { slug.as_str() } else { "" },
            escape(if unknown(&slug) { "—" } else { &slug }),
            escape(if unknown(&state) { "—" } else { &state })
        ));
    }
    out.push_str(r#"</div><ul class="board-stats">"#);
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in entries {
        let t = token(&get(e, "State"));
        if t.is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(s, _)| *s == t) {
            Some((_, n)) => *n += 1,
            None => counts.push((t, 1)),
        }
    }
    counts.sort_by_key(|(s, _)| step(s).unwrap_or(WAY.len()));
    for (state, n) in counts {
        out.push_str(&format!(
            r#"<li class="state-{state}"><b>{n}</b> {}</li>"#,
            escape(&state)
        ));
    }
    let ready = entries
        .iter()
        .filter(|e| token(&get(e, "Gate")) == "ready")
        .count();
    if ready > 0 {
        out.push_str(&format!(
            r#"<li class="gate-ready"><b>{ready}</b> ready to merge</li>"#
        ));
    }
    if issues > 0 {
        out.push_str(&format!(
            r#"<li class="issues"><b>{issues}</b> {}</li>"#,
            if issues == 1 { "issue" } else { "issues" }
        ));
    }
    out.push_str("</ul></div>\n</details></section>\n");
    out
}

/// The table of `entries`; `dir` is the board's directory, for the links, and
/// `root` the project's, for the paths.
fn table(entries: &[Entry], dir: Option<&Path>, root: Option<&Path>) -> String {
    let slugs: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.get("Workstream"))
        .map(str::trim)
        .filter(|s| is_slug(s))
        .collect();
    let mut out = String::from(r#"<table class="board">"#);
    out.push_str("\n<thead><tr>");
    for c in COLUMNS {
        out.push_str(&format!("<th>{c}</th>"));
    }
    out.push_str("<th>More</th></tr></thead>\n<tbody>\n");
    for e in entries {
        let get = |k: &str| e.get(k).unwrap_or("");
        let slug = get("Workstream").trim();
        // A row names itself, so a dependency on it can link to it.
        let id = if is_slug(slug) {
            format!(r#" id="ws-{slug}""#)
        } else {
            String::new()
        };
        // Named to the view transition, so a row glides rather than jumps
        // when the page changes around it.
        let named = if is_slug(slug) {
            format!(r#" style="view-transition-name: row-{slug}""#)
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<tr{id} data-slug="{}" data-state="{}" data-gate="{}"{named}>"#,
            if is_slug(slug) { slug } else { "" },
            token(get("State")),
            token(get("Gate"))
        ));
        out.push_str(&format!(
            r#"<td class="slug">{}</td>"#,
            slug_html(slug, dir)
        ));
        out.push_str(&format!("<td>{}</td>", {
            let w = get("Worker").trim();
            if unknown(w) {
                "—".to_string()
            } else {
                escape(w)
            }
        }));
        out.push_str(&format!("<td>{}</td>", value_html(get("Branch"), root)));
        out.push_str(&format!(
            "<td>{}{}</td>",
            badge("state", get("State")),
            track(get("State"))
        ));
        out.push_str(&format!("<td>{}</td>", badge("gate", get("Gate"))));
        out.push_str(&format!("<td>{}</td>", value_html(get("Tip"), root)));
        out.push_str(&format!("<td>{}</td>", {
            let a = get("After").trim();
            if unknown(a) {
                "—".to_string()
            } else if slugs.contains(&a) {
                format!(r##"<a class="after" href="#ws-{a}">{a}</a>"##)
            } else {
                escape(a)
            }
        }));
        let more: Vec<String> = e
            .fields
            .iter()
            .filter(|(k, _)| !COLUMNS.iter().any(|c| c.eq_ignore_ascii_case(k)))
            .map(|(k, v)| {
                format!(
                    r#"<span class="pair"><b>{}:</b> {}</span>"#,
                    escape(k),
                    value_html(v, root)
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<td class="more">{}</td></tr>{}"#,
            more.join(" · "),
            "\n"
        ));
    }
    out.push_str("</tbody></table>");
    out
}

/// The workstream file `slug/<name>` beside the board in `dir`, when the page
/// would link it: a regular file, one of [`FILES`], in a slug's folder.
pub fn linked_file(dir: &Path, slug: &str, name: &str) -> Option<PathBuf> {
    if !is_slug(slug) || !FILES.contains(&name) {
        return None;
    }
    let p = dir.join(slug).join(name);
    fs::symlink_metadata(&p)
        .is_ok_and(|m| m.file_type().is_file())
        .then_some(p)
}

/// A revision of the board in `dir` as its live page shows it: the text, the
/// briefs and reports it links (their text: the agents panel reads them) and
/// the loop guard's escalations. Empty when there is no board.
pub fn rev(dir: &Path) -> String {
    let Ok(text) = fs::read_to_string(dir.join(FILE)) else {
        return String::new();
    };
    let mut sig = super::rev(text.as_bytes());
    for e in parse(&text).entries {
        let Some(slug) = e.get("Workstream") else {
            continue;
        };
        for f in FILES {
            if let Some(bytes) = linked_file(dir, slug.trim(), f).and_then(|p| fs::read(p).ok()) {
                sig.push_str(&format!(";{slug}/{f}:{}", super::rev(&bytes)));
            }
        }
    }
    let root = project_root(dir);
    for e in parse(&text).entries {
        if let Some(posted) = posted_report(root, &e) {
            sig.push_str(&format!(";posted:{}", super::rev(posted.as_bytes())));
        }
    }
    if let Some(bytes) = root.and_then(|r| fs::read(r.join(ESCALATIONS)).ok()) {
        sig.push_str(&format!(";escalations:{}", super::rev(&bytes)));
    }
    super::rev(sig.as_bytes())
}

/// The project root of a board directory (`<root>/.claude/workstreams`).
fn project_root(dir: &Path) -> Option<&Path> {
    dir.parent()?.parent()
}

// --------------------------------------------------------------- agents --

/// A worker's report, `<slug>/report.md`: its `Key: value` lines, as the
/// worker wrote them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub fields: Vec<(String, String)>,
}

impl Report {
    /// The value of `key` (any case), if the report has it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// A report's text as its `Key: value` lines.
pub fn parse_report(text: &str) -> Report {
    let re = header_re();
    let fields = text
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter_map(|l| {
            let c = re.captures(l)?;
            Some((c[1].trim().to_string(), c[2].trim().to_string()))
        })
        .collect();
    Report { fields }
}

/// The report of workstream `slug` beside the board in `dir`, if it has one.
pub fn report(dir: &Path, slug: &str) -> Option<Report> {
    let text = fs::read_to_string(linked_file(dir, slug, "report.md")?).ok()?;
    Some(parse_report(&text))
}

/// The report `entry`'s worker posted in its worktree, not yet collected: at
/// the board's `Worktree:` path, else in any worktree under the project's
/// `.claude/worktrees` — whichever names the workstream. Its text.
pub fn posted_report(root: Option<&Path>, entry: &Entry) -> Option<String> {
    let slug = entry.get("Workstream")?.trim();
    if !is_slug(slug) {
        return None;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(w) = entry
        .get("Worktree")
        .map(str::trim)
        .filter(|w| !unknown(w) && !w.eq_ignore_ascii_case("none"))
    {
        let p = Path::new(w);
        let p = match (p.is_absolute(), root) {
            (false, Some(r)) => r.join(p),
            _ => p.to_path_buf(),
        };
        candidates.push(p.join(POSTED));
    }
    if let Some(r) = root {
        let mut found: Vec<PathBuf> = fs::read_dir(r.join(WORKTREES))
            .into_iter()
            .flatten()
            .flatten()
            .map(|d| d.path().join(POSTED))
            .collect();
        found.sort();
        candidates.extend(found);
    }
    candidates.into_iter().find_map(|c| {
        let text = fs::read_to_string(&c).ok()?;
        let names = text.lines().any(|l| {
            l.trim_end_matches('\r')
                .trim()
                .strip_prefix("Workstream:")
                .is_some_and(|v| v.trim() == slug)
        });
        names.then_some(text)
    })
}

/// The goal of workstream `slug`'s brief: the text under `## Goal`, as one
/// line of at most `GOAL_CHARS` characters.
pub fn goal(dir: &Path, slug: &str) -> Option<String> {
    let text = fs::read_to_string(linked_file(dir, slug, "brief.md")?).ok()?;
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    lines.find(|l| l.trim().eq_ignore_ascii_case("## Goal"))?;
    let goal: Vec<&str> = lines
        .take_while(|l| !l.starts_with('#'))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let goal = goal.join(" ");
    if goal.is_empty() {
        return None;
    }
    Some(match goal.char_indices().nth(GOAL_CHARS) {
        Some((i, _)) => format!("{}…", goal[..i].trim_end()),
        None => goal,
    })
}

/// A worker on the board: who it is, what it works on, where that is, and
/// what needs the manager's eye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub slug: String,
    /// `session`, `subagent`, or `unassigned` for an entry without a worker.
    pub kind: String,
    /// The worker's name or id, as the board gives it.
    pub name: String,
    pub state: String,
    pub gate: String,
    pub goal: Option<String>,
    /// The report the manager collected to the board's folder.
    pub report: Option<Report>,
    /// The report the worker posted in its worktree, when it differs from the
    /// collected one (or none was collected): the latest word.
    pub posted: Option<Report>,
    /// The reports and the brief as written, for the card to fold out.
    pub report_text: Option<String>,
    pub posted_text: Option<String>,
    pub brief_text: Option<String>,
    pub issues: Vec<String>,
}

impl Agent {
    /// The report that speaks for the worker now: the posted one, else the
    /// collected one.
    pub fn live(&self) -> Option<&Report> {
        self.posted.as_ref().or(self.report.as_ref())
    }
}

/// The loop guard's escalation lines of the project at `root`.
fn escalations(root: Option<&Path>) -> Vec<String> {
    root.and_then(|r| fs::read_to_string(r.join(ESCALATIONS)).ok())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim_end_matches('\r').strip_prefix("- "))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// The confidence bar of the project at `root`: its worker gate's, else
/// [`DEFAULT_BAR`].
fn confidence_bar(root: Option<&Path>) -> u8 {
    root.and_then(|r| crate::render::Project::load_state(r).ok())
        .map(|p| p.claude.workflow.subagent_confidence)
        .filter(|&c| c > 0)
        .unwrap_or(DEFAULT_BAR)
}

/// `NN%` as a number, if that is what `s` is.
fn percent(s: &str) -> Option<u8> {
    s.trim().trim_end_matches('%').trim().parse().ok()
}

/// What needs the manager's eye about `entry`'s worker, from its report, its
/// board line, the board's differs and the loop guard: each one sentence.
fn issues(
    entry: &Entry,
    report: Option<&Report>,
    posted_newer: bool,
    differs: &[(String, String)],
    bar: u8,
    escalations: &[String],
) -> Vec<String> {
    let get = |k: &str| entry.get(k).unwrap_or("").trim();
    let slug = get("Workstream");
    let mut out = Vec::new();
    if let Some(r) = report {
        let rget = |k: &str| r.get(k).unwrap_or("").trim();
        let status = rget("Status");
        if let Some(why) = status
            .strip_prefix("partial")
            .or_else(|| status.strip_prefix("blocked"))
        {
            let word = if status.starts_with("partial") {
                "partial"
            } else {
                "blocked"
            };
            out.push(format!("{word}: {}", why.trim_start_matches(':').trim()));
        }
        let check = rget("Check");
        if let Some(why) = check.strip_prefix("fail") {
            out.push(format!(
                "check failed: {}",
                why.trim_start_matches(':').trim()
            ));
        } else if let Some(why) = check.strip_prefix("not run") {
            out.push(format!(
                "check not run: {}",
                why.trim_start_matches(':').trim()
            ));
        }
        match percent(rget("Confidence")) {
            Some(c) if c < bar => out.push(format!("confidence {c}% is below the bar of {bar}%")),
            Some(_) => {}
            None => out.push("no confidence stated in the report".to_string()),
        }
        let tip = rget("Tip");
        let board_tip = get("Tip");
        let hex = |s: &str| s.len() >= 7 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if tip.starts_with("uncommitted") {
            out.push("uncommitted work in the worktree".to_string());
        } else if hex(tip)
            && hex(board_tip)
            && !tip.starts_with(board_tip)
            && !board_tip.starts_with(tip)
        {
            let short = |s: &str| s[..s.len().min(SHORT_SHA)].to_string();
            out.push(format!(
                "the branch moved since the report: board tip {}, report {}",
                short(board_tip),
                short(tip)
            ));
        }
    } else if step(get("State")).is_some_and(|at| at >= 2) {
        out.push(format!("{}, but no report on file", token(get("State"))));
    }
    if posted_newer {
        out.push("a newer report in its worktree, not yet collected".to_string());
    }
    if get("Worktree").eq_ignore_ascii_case("none") {
        out.push("the worktree is gone".to_string());
    }
    for (who, what) in differs {
        if who == slug {
            out.push(format!("differs: {what}"));
        }
    }
    let name = get("Worker")
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let branch = get("Branch").rsplit('/').next().unwrap_or("");
    let keys: Vec<&str> = [slug, name, branch]
        .into_iter()
        .filter(|k| k.len() >= 3 && !unknown(k))
        .collect();
    for line in escalations {
        if keys.iter().any(|k| line.contains(k)) {
            out.push(format!("loop guard: {line}"));
        }
    }
    out
}

/// The agents of board `b` in `dir`, in board order, and the loop guard's
/// escalations none of them accounts for.
pub fn agents(b: &Board, dir: Option<&Path>) -> (Vec<Agent>, Vec<String>) {
    let root = dir.and_then(project_root);
    let bar = confidence_bar(root);
    let all = escalations(root);
    let mut agents = Vec::new();
    let mut taken = vec![false; all.len()];
    for e in &b.entries {
        let get = |k: &str| e.get(k).unwrap_or("").trim().to_string();
        let slug = get("Workstream");
        let worker = get("Worker");
        let mut words = worker.split_whitespace();
        let (kind, name) = match words.next() {
            Some(w) if w.eq_ignore_ascii_case("session") || w.eq_ignore_ascii_case("subagent") => {
                (w.to_ascii_lowercase(), words.collect::<Vec<_>>().join(" "))
            }
            Some(_) if !unknown(&worker) => ("worker".to_string(), worker.clone()),
            _ => ("unassigned".to_string(), "—".to_string()),
        };
        let report = dir.and_then(|d| report(d, &slug));
        let text = |name: &str| {
            dir.and_then(|d| linked_file(d, &slug, name))
                .and_then(|p| fs::read_to_string(p).ok())
        };
        let report_text = text("report.md");
        let posted_text = posted_report(root, e)
            .filter(|p| report_text.as_deref().is_none_or(|r| r.trim() != p.trim()));
        let posted = posted_text.as_deref().map(parse_report);
        let live = posted.as_ref().or(report.as_ref());
        let issues = issues(e, live, posted.is_some(), &b.differs, bar, &all);
        for (i, line) in all.iter().enumerate() {
            if issues.iter().any(|x| x.ends_with(line.as_str())) {
                taken[i] = true;
            }
        }
        agents.push(Agent {
            goal: dir.and_then(|d| goal(d, &slug)),
            report_text,
            posted,
            posted_text,
            brief_text: text("brief.md"),
            slug,
            kind,
            name: if name.is_empty() {
                "—".to_string()
            } else {
                name
            },
            state: get("State"),
            gate: get("Gate"),
            report,
            issues,
        });
    }
    let loose = all
        .into_iter()
        .zip(taken)
        .filter(|(_, t)| !t)
        .map(|(l, _)| l)
        .collect();
    (agents, loose)
}

/// What the manager does next, read off the board the way the skill's steps
/// go: one move per line, in board order — `(slug, what)`, the slug for a
/// move about one workstream.
pub fn next_steps(b: &Board, agents: &[Agent], loose: usize) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let status = b.status().unwrap_or("").trim();
    if status.starts_with("CLOSED") {
        out.push((
            None,
            "the board is closed; `/workstreams <goal>` starts a new one".to_string(),
        ));
        return out;
    }
    if status.is_empty() || status.starts_with("DRAFT") {
        out.push((None, "approve the board: `Status: APPROVED session=<id>` before anything is dispatched (step 1)".to_string()));
    }
    if b.entries.is_empty() {
        out.push((
            None,
            "plan the workstreams: disjoint files, a brief each, `After:` where one waits (step 1)"
                .to_string(),
        ));
        return out;
    }
    let state_of = |slug: &str| {
        b.entries
            .iter()
            .find(|e| e.get("Workstream").map(str::trim) == Some(slug))
            .map(|e| token(e.get("State").unwrap_or("")))
    };
    let mut all_done = true;
    for (e, a) in b.entries.iter().zip(agents) {
        let get = |k: &str| e.get(k).unwrap_or("").trim().to_string();
        let slug = get("Workstream");
        let name = if unknown(&slug) {
            None
        } else {
            Some(slug.clone())
        };
        let state = token(&get("State"));
        let gate = token(&get("Gate"));
        let after = get("After");
        let waits = !unknown(&after)
            && !after.eq_ignore_ascii_case("none")
            && state_of(&after).as_deref() != Some("integrated");
        let posted = a.posted.is_some();
        if state != "integrated" {
            all_done = false;
        }
        let what = match state.as_str() {
            "planned" if waits => format!("hold it until {after} is integrated"),
            "planned" => format!(
                "dispatch it ({} worker) and put the worker on the board line (step 2)",
                if a.kind == "unassigned" { "session or subagent" } else { a.kind.as_str() }
            ),
            "dispatched" | "reported" if posted => format!(
                "collect the report its worker posted: copy it to .claude/workstreams/{slug}/report.md (step 3)"
            ),
            "dispatched" => format!("wait for its worker ({}); nothing to do yet", a.name),
            "reported" => match a.live().and_then(|r| r.get("Status")).map(str::trim) {
                Some(s) if s.starts_with("partial") || s.starts_with("blocked") => {
                    format!("send its worker back once with what is missing ({s})")
                }
                _ => "check it (step 4): confidence at the bar, tip unchanged, worktree clean, only the brief's files — then `Gate: ready`".to_string(),
            },
            "checked" if gate != "ready" => "mark it `Gate: ready` once the checks of step 4 hold".to_string(),
            "checked" if waits => format!("wait for {after} to be integrated before merging it"),
            "checked" => format!(
                "merge it (step 5): `git merge --no-ff --no-edit {}`, then the project check",
                get("Branch")
            ),
            "integrated" => continue,
            _ => "put its `State:` back on the way: planned, dispatched, reported, checked, integrated".to_string(),
        };
        out.push((name, what));
    }
    if !b.differs.is_empty() {
        out.push((
            None,
            format!(
                "decide on the {} differs: accept or reject each at close (step 7)",
                b.differs.len()
            ),
        ));
    }
    if loose > 0 {
        out.push((
            None,
            format!(
                "read the loop guard's {loose} escalation{} ({ESCALATIONS})",
                if loose == 1 { "" } else { "s" }
            ),
        ));
    }
    if all_done {
        out.push((
            None,
            "check the whole (step 6), then close the board: `Status: CLOSED <date>` (step 7)"
                .to_string(),
        ));
    }
    out
}

/// The next-steps pane.
fn next_html(steps: &[(Option<String>, String)], slugs: &[&str]) -> String {
    let mut out = String::from(
        "<section class=\"sec\" data-sec=\"next\"><details class=\"pane\" data-pane=\"next\" open><summary><h2>Next steps</h2></summary>\n<ol class=\"next\">\n",
    );
    for (slug, what) in steps {
        match slug.as_deref().filter(|s| slugs.contains(s)) {
            Some(s) => out.push_str(&format!(
                r##"<li data-slug="{s}"><a class="pick" href="#ws-{s}">{s}</a>: {}</li>"##,
                escape(what)
            )),
            None => out.push_str(&format!("<li>{}</li>", escape(what))),
        }
        out.push('\n');
    }
    out.push_str("</ol>\n</details></section>\n");
    out
}

/// The agents panel: a pane of a card per worker — folded unless it has
/// issues, its state remembered by the page — and the escalations tied to
/// none. A selected workstream narrows it to its agents.
fn agents_html(agents: &[Agent], loose: &[String], slugs: &[&str], root: Option<&Path>) -> String {
    let issues: usize = agents.iter().map(|a| a.issues.len()).sum();
    let mut out = format!(
        "<section class=\"sec\" data-sec=\"agents\"><details class=\"pane\" data-pane=\"agents\" open><summary><h2>Agents</h2> <span class=\"count\">{} {} · {} {}</span></summary>\n",
        agents.len(),
        if agents.len() == 1 { "worker" } else { "workers" },
        issues,
        if issues == 1 { "issue" } else { "issues" }
    );
    out.push_str(r#"<p class="selection" hidden>Showing <b class="which"></b> <button type="button" id="show-all">Show all</button></p>"#);
    out.push_str("\n<div class=\"agents\">\n");
    for a in agents {
        let id = if is_slug(&a.slug) {
            format!(r#" id="agent-{}""#, a.slug)
        } else {
            String::new()
        };
        let slug_attr = if is_slug(&a.slug) {
            a.slug.as_str()
        } else {
            ""
        };
        let flagged = if a.issues.is_empty() {
            ""
        } else {
            " has-issues"
        };
        let open = if a.issues.is_empty() { "" } else { " open" };
        let named = if is_slug(&a.slug) {
            format!(r#" style="view-transition-name: agent-{}""#, a.slug)
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<details class="agent agent-{}{flagged}"{id} data-slug="{slug_attr}"{named}{open}><summary class="agent-head">{}"#,
            token(&a.kind),
            icon(&a.kind)
        ));
        let on = if slugs.contains(&a.slug.as_str()) {
            format!(r##"<a href="#ws-{}">{}</a>"##, a.slug, escape(&a.slug))
        } else {
            escape(if unknown(&a.slug) { "—" } else { &a.slug })
        };
        // What the worker is up to, as one icon: issues, done, still at it,
        // or waiting to start.
        let status = if !a.issues.is_empty() {
            "warn"
        } else if token(&a.state) == "integrated"
            || a.live().is_some_and(|r| {
                r.get("Status")
                    .is_some_and(|s| s.trim().starts_with("done"))
            })
        {
            "tick"
        } else if step(&a.state).is_some_and(|at| at == 0) {
            "clock"
        } else {
            "working"
        };
        out.push_str(&format!(
            r#"<span class="kind">{}</span> <b class="name">{}</b> <span class="on">on {on}</span> {} {} {}</summary>"#,
            escape(&a.kind),
            escape(&a.name),
            badge("state", &a.state),
            badge("gate", &a.gate),
            icon(status)
        ));
        out.push_str(r#"<div class="agent-body">"#);
        if let Some(g) = &a.goal {
            out.push_str(&format!(r#"<p class="goal">{}</p>"#, escape(g)));
        }
        if a.posted.is_some() {
            out.push_str(r#"<p class="posted">Posted from its worktree, not yet collected</p>"#);
        }
        match a.live() {
            Some(r) => {
                out.push_str(r#"<dl class="facts">"#);
                for key in ["Check", "Status", "Confidence", "Tip", "Changed"] {
                    let Some(v) = r.get(key).map(str::trim).filter(|v| !v.is_empty()) else {
                        continue;
                    };
                    let shown = match key {
                        "Tip" | "Changed" => value_html(v, root),
                        _ => escape(v),
                    };
                    out.push_str(&format!("<dt>{key}</dt><dd>{shown}</dd>"));
                }
                out.push_str("</dl>");
            }
            None => {
                let waiting = step(&a.state).is_some_and(|at| at < 2);
                out.push_str(&format!(
                    r#"<p class="facts none">{}</p>"#,
                    if waiting {
                        "No report yet"
                    } else {
                        "No report on file"
                    }
                ));
            }
        }
        if a.issues.is_empty() {
            out.push_str(r#"<p class="ok">No issues</p>"#);
        } else {
            out.push_str(r#"<ul class="issues">"#);
            for i in &a.issues {
                out.push_str(&format!("<li>{}</li>", escape(i)));
            }
            out.push_str("</ul>");
        }
        for (pane, name, text) in [
            ("posted", "Report (posted)", &a.posted_text),
            ("report", "Report (collected)", &a.report_text),
            ("brief", "Brief", &a.brief_text),
        ] {
            if let Some(t) = text {
                out.push_str(&format!(
                    r#"<details class="text" data-pane="{pane}-{slug_attr}"><summary>{name}</summary><pre>{}</pre></details>"#,
                    escape(t.trim_end())
                ));
            }
        }
        out.push_str("</div></details>\n");
    }
    out.push_str("</div>\n");
    if !loose.is_empty() {
        out.push_str(r#"<p class="escalations-note">Loop guard escalations tied to no workstream:</p><ul class="escalations">"#);
        for l in loose {
            out.push_str(&format!("<li>{}</li>", escape(l)));
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</details></section>\n");
    out
}

/// The board's live page, as the viewer serves it from `dir`: rendered fresh
/// from the Markdown, at revision `rev`, its one script carrying `nonce`.
pub fn live_page(dir: &Path, rev: &str, nonce: &str) -> Result<String> {
    let md = dir.join(FILE);
    let text = fs::read_to_string(&md).with_context(|| format!("reading {}", md.display()))?;
    let z = Zen {
        kind: ZenKind::Board,
        topic: TOPIC,
        list: None,
        live: true,
        switch: &Switch::default(),
        original: None,
    };
    let source = format!("{}/{FILE}", Kind::Board.dir());
    page(&text, Some(dir), &source, rev, nonce, "English", &z)
}

/// Show board `md`'s page: refresh the tabs that show it, or open one, through
/// the viewer for its directory (started if need be) — or, with no viewer to
/// run, the static page as a file, once (`explicit`: every time).
pub fn show(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let dir = md
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if is_board_dir(dir) {
        if let Some(info) = viewer::ensure(dir, env) {
            if let Some(action) = viewer::reload(&info, TOPIC, explicit) {
                return draft::shown(Some(action), || info.url(TOPIC), env);
            }
        }
    }
    super::show_file_page(md, env, explicit)
}

/// The board's page for text `md` (revision `rev`, at `source`, the file in
/// `dir`), as [`intents::page_with`] makes one: the header fields, the table
/// where the entries were, the rest as GitHub would show it.
pub fn page(
    md: &str,
    dir: Option<&Path>,
    source: &str,
    rev: &str,
    nonce: &str,
    language: &str,
    z: &Zen,
) -> Result<String> {
    let b = parse(md);
    // The board sits in `<root>/.claude/workstreams`.
    let root = dir.and_then(Path::parent).and_then(Path::parent);
    // The agents first: who works on what, where it is, what needs an eye.
    let (agents, loose) = agents(&b, dir);
    let issues: usize = agents.iter().map(|a| a.issues.len()).sum();
    let slugs: Vec<&str> = b
        .entries
        .iter()
        .filter_map(|e| e.get("Workstream"))
        .map(str::trim)
        .filter(|s| is_slug(s))
        .collect();
    let mut body = String::new();
    // The broad picture first: progress, what to do next, the agents.
    body.push_str(&summary(&b.entries, issues));
    body.push_str(&next_html(&next_steps(&b, &agents, loose.len()), &slugs));
    if !agents.is_empty() {
        body.push_str(&agents_html(&agents, &loose, &slugs, root));
    }
    // Each `## ` section is wrapped and named (`data-sec="log"`), so the page
    // can draw the log as a timeline and the differs as flags; the table opens
    // with its summary.
    let mut open = false;
    let close = |body: &mut String, open: &mut bool| {
        if *open {
            body.push_str("</details></section>\n");
            *open = false;
        }
    };
    for s in &b.segments {
        match s {
            Segment::Markdown(text) => {
                let mut chunk = String::new();
                for line in text.lines() {
                    if let Some(h) = line.strip_prefix("## ") {
                        if !chunk.trim().is_empty() {
                            body.push_str(&draft::render_safe(&chunk, false));
                            body.push('\n');
                        }
                        chunk.clear();
                        close(&mut body, &mut open);
                        body.push_str(&format!(
                            r#"<section class="sec" data-sec="{0}"><details class="pane" data-pane="{0}" open><summary><h2>{1}</h2></summary>"#,
                            token(h),
                            escape(h.trim())
                        ));
                        body.push('\n');
                        open = true;
                        // The heading is the pane's summary now.
                        continue;
                    }
                    chunk.push_str(line);
                    chunk.push('\n');
                }
                if !chunk.trim().is_empty() {
                    body.push_str(&draft::render_safe(&chunk, false));
                    body.push('\n');
                }
            }
            Segment::Table => {
                body.push_str(&table(&b.entries, dir, root));
                body.push('\n');
            }
        }
    }
    close(&mut body, &mut open);
    let content = Content {
        title: Some(b.title.clone().unwrap_or_else(|| "Workstreams".to_string())),
        fields: b.fields.clone(),
        body,
    };
    intents::render_page(md, &content, source, rev, nonce, language, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_split_on_the_dash_before_a_key() {
        let f = fields_of("Workstream: a — Worker: session a (yours) — Branch: — — Worktree: — — Tip: — — State: reported (partial: tests) — Gate: pending — After: none");
        let get = |k: &str| f.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        assert_eq!(get("Workstream"), Some("a"));
        assert_eq!(get("Worker"), Some("session a (yours)"));
        assert_eq!(get("Branch"), Some("—"));
        assert_eq!(get("Worktree"), Some("—"));
        assert_eq!(get("Tip"), Some("—"));
        assert_eq!(get("State"), Some("reported (partial: tests)"));
        assert_eq!(get("After"), Some("none"));
        assert_eq!(f.len(), 8);
        // An en dash separates too; text before the first key is dropped.
        let f = fields_of("note – Gate: ready – Tip: abc1234");
        assert_eq!(
            f,
            [
                ("Gate".to_string(), "ready".to_string()),
                ("Tip".to_string(), "abc1234".to_string())
            ]
        );
        assert!(fields_of("").is_empty());
    }

    #[test]
    fn values_are_shown_safely() {
        assert_eq!(value_html("—", None), "—");
        assert_eq!(value_html("", None), "—");
        assert_eq!(value_html("abc1234", None), "<code>abc1234</code>");
        assert_eq!(
            value_html("0123456789abcdef0123456789abcdef01234567", None),
            r#"<code title="0123456789abcdef0123456789abcdef01234567">0123456789ab</code>"#
        );
        assert_eq!(
            value_html("refs/heads/x", None),
            "<code>refs/heads/x</code>"
        );
        assert_eq!(value_html("<b>x</b>", None), "&lt;b&gt;x&lt;/b&gt;");
        assert_eq!(value_html("a shared / helper", None), "a shared / helper");
        assert_eq!(
            value_html(r"C:\p\.claude\worktrees\a", None),
            r"<code>C:\p\.claude\worktrees\a</code>"
        );
        // A path under the project is shown relative to it, the whole on hover.
        let root = Path::new("/p/proj");
        assert_eq!(
            value_html("/p/proj/.claude/worktrees/a", Some(root)),
            r#"<code title="/p/proj/.claude/worktrees/a">.claude/worktrees/a</code>"#
        );
        assert_eq!(
            value_html("/p/proj-2/.claude/worktrees/a", Some(root)),
            "<code>/p/proj-2/.claude/worktrees/a</code>"
        );
        assert_eq!(value_html("/p/proj", Some(root)), "<code>/p/proj</code>");
        assert_eq!(
            value_html(
                r"C:\p\proj\.claude\worktrees\a",
                Some(Path::new(r"C:\p\proj"))
            ),
            r#"<code title="C:\p\proj\.claude\worktrees\a">.claude/worktrees/a</code>"#
        );
        assert_eq!(
            badge("gate", "ready"),
            r#"<span class="gate gate-ready">ready</span>"#
        );
        assert_eq!(
            badge("state", "reported (partial)"),
            r#"<span class="state state-reported">reported (partial)</span>"#
        );
        assert_eq!(badge("gate", "—"), "—");
        assert_eq!(token("Ready!"), "ready");
        // A slug selects its workstream and links the files there; one that
        // can't be a path gets neither, even with the files there.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/brief.md"), "").unwrap();
        assert_eq!(
            slug_html("a", Some(dir.path())),
            r##"<a class="pick" href="#ws-a">a</a> <span class="files"><a href="a/brief.md">brief</a></span>"##
        );
        assert_eq!(
            slug_html("a", None),
            r##"<a class="pick" href="#ws-a">a</a>"##
        );
        assert_eq!(slug_html("../a", Some(dir.path())), "<b>../a</b>");
        assert_eq!(slug_html("—", Some(dir.path())), "—");
        // The gate's tick draws itself once merged.
        assert!(badge("gate", "merged")
            .starts_with(r#"<span class="gate gate-merged">merged<svg class="ico ico-tick""#));
        assert!(icon("working").contains(r#"class="ico ico-working""#));
        assert_eq!(icon("nothing").matches("<path").count(), 0);
    }

    #[test]
    fn the_word_is_exact() {
        assert!(wanted("workstreams"));
        assert!(wanted("WORKSTREAMS"));
        assert!(wanted("/p/.claude/workstreams/board.md"));
        assert!(!wanted("workstream"));
        assert!(!wanted("/p/.claude/workstreams/notes.md"));
        assert!(!wanted(""));
    }
}
