//! The read-only list of a project's documents — its /inquire notes, or its
//! intent files ([`super::intents`]) — to pick one from: [`PER_PAGE`] a page, in
//! the order the caller gives, with the selected one marked. It looks like the
//! list of issue drafts ([`super::draft::list_page`]) without what only a draft
//! has (an issue link, the approvers), and opens a document's page, never an
//! editor.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use minijinja::{AutoEscape, Environment, Value};

use super::draft::{self, PER_PAGE};
use super::html::escape;
use super::words::{self, Words};
use crate::templates;

/// The list page.
pub const TEMPLATE: &str = "claude/notes/docs.html.j2";

/// Which documents a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// /inquire ledgers, newest first.
    Notes,
    /// Intent files, highest number first.
    Intents,
}

/// One listed document.
pub struct Row {
    /// What it is about, as text.
    pub summary: String,
    /// Short tags, as text: its status, a date.
    pub tags: Vec<String>,
    /// The page the row opens, when not the document's own: the version in
    /// the answer language.
    pub page: Option<String>,
}

/// What a list shows.
pub struct List<'a> {
    pub kind: Kind,
    /// The documents, in the list's order.
    pub docs: &'a [PathBuf],
    /// Each document's row.
    pub row: &'a dyn Fn(&Path, &str) -> Row,
    /// The documents' folder, as the page names it (project-relative).
    pub source: &'a str,
    /// The list's path under the viewer's token, and its tabs' topic.
    pub list: &'a str,
    /// The files left out because ocgen can't serve their names.
    pub skipped: &'a [String],
    /// The project's answer language.
    pub language: &'a str,
}

/// A document's name: its file name without `.md`.
fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A revision of a list: its documents' names, sizes and times. It changes
/// when one is written, added, removed or renamed.
pub fn rev(docs: &[PathBuf]) -> String {
    let mut sig = String::new();
    for p in docs {
        let m = fs::metadata(p).ok();
        let t = m
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let len = m.map_or(0, |m| m.len());
        sig.push_str(&format!("{}\t{len}\t{t}\n", stem(p)));
    }
    super::rev(sig.as_bytes())
}

/// The list's page `page` (out of range: the nearest), or with none, the page
/// holding `selected`, which is marked. Its links lead from `root`, the
/// viewer's root as seen from the page (`./`, or `../` with a document
/// selected); its one script carries `nonce`.
pub fn page(
    l: &List,
    page: Option<usize>,
    selected: Option<&str>,
    root: &str,
    nonce: &str,
) -> Result<String> {
    let total = draft::pages(l.docs.len());
    let page = page
        .unwrap_or_else(|| draft::page_of(l.docs, selected))
        .clamp(1, total);
    let w = words::for_language(l.language);
    // Escaped here: minijinja's own escaping also turns `/` into `&#x2f;`.
    let safe = |s: &str| Value::from_safe_string(escape(s));
    let rows: Vec<Value> = l
        .docs
        .iter()
        .skip((page - 1) * PER_PAGE)
        .take(PER_PAGE)
        .map(|p| {
            let slug = stem(p);
            let text = fs::read_to_string(p).unwrap_or_default();
            let row = (l.row)(p, &text);
            let when = fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            let (y, mo, d, h, mi, _) = crate::clock::civil(when);
            let page = row.page.clone().unwrap_or_else(|| slug.clone());
            minijinja::context! {
                slug => safe(&slug),
                page => safe(&page),
                name => safe(&format!("{slug}.md")),
                summary => safe(&row.summary),
                tags => row.tags.iter().map(|t| safe(t)).collect::<Vec<_>>(),
                when_iso => safe(&format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00Z")),
                when_text => safe(&format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")),
                selected => selected == Some(slug.as_str()),
            }
        })
        .collect();
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template_owned("page.css", templates::load(super::html::STYLE)?)
        .context("parsing the notes page style")?;
    env.add_template_owned("palette.css", templates::load(draft::PALETTE)?)
        .context("parsing the pages' palettes")?;
    env.add_template_owned("docs.html", templates::load(TEMPLATE)?)
        .context("parsing the document list template")?;
    let d = &w.docs;
    let (title, count, footer, offline, none) = match l.kind {
        Kind::Notes => (
            d.notes_title,
            d.notes_count,
            d.notes_footer,
            d.notes_offline,
            d.notes_none,
        ),
        Kind::Intents => (
            d.intents_title,
            d.intents_count,
            d.intents_footer,
            d.intents_offline,
            d.intents_none,
        ),
    };
    let fill = |s: &str| {
        s.replace("{page}", &page.to_string())
            .replace("{pages}", &total.to_string())
            .replace("{count}", &l.docs.len().to_string())
    };
    let ctx = minijinja::context! {
        w => words_value(w),
        title => safe(title),
        count => safe(&fill(count)),
        footer => safe(footer),
        // Ours, not a document's: it names a command in markup.
        offline => Value::from_safe_string(offline.to_string()),
        none_yet => safe(none),
        page_of => safe(&fill(w.draft.page_of)),
        rows => rows,
        newer => (page > 1).then(|| page - 1),
        older => (page < total).then(|| page + 1),
        root => safe(root),
        nonce => safe(nonce),
        list => safe(l.list),
        rev => safe(&rev(l.docs)),
        source => safe(l.source),
        skipped => safe(&l.skipped.join(", ")),
        skipped_label => safe(d.intents_skipped),
    };
    let mut out = env
        .get_template("docs.html")?
        .render(ctx)
        .context("rendering the document list")?;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// The words the read-only pages share with the drafts' (palette, pager, keys).
pub(crate) fn words_value(w: &'static Words) -> Value {
    let d = &w.draft;
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("lang", Value::from(w.lang));
    for (k, v) in [
        ("palette", d.palette),
        ("black", d.black),
        ("white", d.white),
        ("newer", d.newer),
        ("older", d.older),
        ("intent_title", w.docs.intent_title),
        ("all_intents", w.docs.all_intents),
        ("intent_footer", w.docs.intent_footer),
        ("languages", w.versions.languages),
        ("views", w.versions.views),
        ("preview", w.versions.preview),
        ("source", w.versions.source),
    ] {
        m.insert(k, Value::from(v));
    }
    // Ours: they name keys or a command in markup.
    m.insert("keys", Value::from_safe_string(d.list_keys.to_string()));
    m.insert(
        "intent_offline",
        Value::from_safe_string(w.docs.intent_offline.to_string()),
    );
    Value::from(m)
}
