//! /intent's intent files (`<dir>/<PREFIX>-NNNN-<slug>.md`, by default
//! `docs/adr/ADR-0007-slug.md`), read in the browser. The project's prefix in
//! lowercase (`adr`, or `rfc` for `RFC`), sent alone in Claude Code and caught
//! by the `intent-draft` hook — or `ocgen adr [name]` — opens the one there is,
//! or their list ([`LIST`]): highest number first, [`super::draft::PER_PAGE`] a
//! page, with the file this session wrote last selected ([`super::session`]).
//!
//! The page is read-only and made safe the way the issue draft's preview is
//! ([`super::draft::preview`]): raw HTML keeps only the tags GitHub allows, and
//! links only http(s) and mailto targets. A newline is a space, as GitHub shows
//! a Markdown file.
//!
//! The intent files are tracked, so nothing is written beside them: the viewer
//! and the session records live in [`VIEWER_DIR`], which git-ignores itself.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use minijinja::{AutoEscape, Environment, Value};
use regex::Regex;

use super::draft::{self, is_name};
use super::html::escape;
use super::listing::{self, Row};
use super::versions::Switch;
use super::{session, words, Shown};
use crate::claude::IntentSettings;
use crate::templates;

/// The intent file's page.
pub const TEMPLATE: &str = "claude/notes/adr.html.j2";
/// The list's path under the viewer's token, and its tabs' topic: not a name an
/// intent file can have (those start with a letter or digit).
pub const LIST: &str = "_intents";
/// Where the intent files' viewer and session records live, relative to the
/// project root.
pub const VIEWER_DIR: &str = ".claude/intent/viewer";
/// Words ocgen already has: a prefix named like one leaves it alone.
const TAKEN: [&str; 3] = [draft::WORD, "note", "notes"];

/// The word that opens the intent files of a project with prefix `prefix`: the
/// prefix in lowercase, unless that is a word ocgen already has.
pub fn word(prefix: &str) -> Option<String> {
    let w = prefix.trim().to_ascii_lowercase();
    (!w.is_empty() && !TAKEN.contains(&w.as_str())).then_some(w)
}

/// Whether a prompt is the word for prefix `prefix` (any case, nothing else).
pub fn is_prompt(prompt: &str, prefix: &str) -> bool {
    word(prefix).is_some_and(|w| prompt.trim().eq_ignore_ascii_case(&w))
}

/// The /intent settings of the project at `root`: its state's, or the defaults.
pub fn settings(root: &Path) -> IntentSettings {
    crate::render::Project::load_state(root)
        .map(|p| p.claude.intent)
        .unwrap_or_default()
}

/// The root and /intent settings of the ocgen project containing `start`.
pub fn project(start: &Path) -> Option<(PathBuf, IntentSettings)> {
    let start = start.canonicalize().ok()?;
    super::project_of(&start).map(|(root, p)| (crate::paths::plain(&root), p.claude.intent))
}

/// The intent files' directory in project `root`.
pub fn dir(root: &Path, s: &IntentSettings) -> PathBuf {
    root.join(s.dir.trim_matches('/'))
}

/// Their directory as messages name it: project-relative, with a `/`.
pub fn shown_dir(s: &IntentSettings) -> String {
    format!("{}/", s.dir.trim_matches('/'))
}

/// Whether file name `file` is an intent file's (the rule /intent's hooks use).
fn is_intent_name(file: &str, s: &IntentSettings) -> bool {
    crate::intent::kind(s, &format!("{}/{file}", s.dir.trim_matches('/')))
        == Some(crate::intent::Kind::Intent)
}

/// The number in an intent file's name (`ADR-0007-slug` or `0007-slug` → 7).
pub fn number(stem: &str, prefix: &str) -> Option<u64> {
    let lower = stem.to_ascii_lowercase();
    let rest = lower
        .strip_prefix(&format!("{}-", prefix.to_ascii_lowercase()))
        .unwrap_or(&lower);
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !rest[digits..].starts_with('-') {
        return None;
    }
    rest[..digits].parse().ok()
}

/// The number a user's name asks for: `7`, `0007`, `ADR-7`, `adr 7`.
fn wanted_number(name: &str, prefix: &str) -> Option<u64> {
    let lower = name.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix(&prefix.to_ascii_lowercase())
        .map(|r| r.trim_start_matches(['-', ' ']))
        .unwrap_or(&lower);
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

/// A file's name without `.md`.
fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The `.md` files in `dir` that are intent files by name, as `(file name,
/// servable)`.
fn named(dir: &Path, s: &IntentSettings) -> Vec<(String, PathBuf)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| is_intent_name(n, s))
        .collect()
}

/// The intent files in `dir`, highest number first, then by name.
pub fn files(dir: &Path, s: &IntentSettings) -> Vec<PathBuf> {
    let mut found: Vec<(u64, String, PathBuf)> = named(dir, s)
        .into_iter()
        .filter_map(|(n, p)| {
            let st = n.strip_suffix(".md")?.to_string();
            is_name(&st).then(|| (number(&st, &s.prefix).unwrap_or(0), n, p))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, _, p)| p).collect()
}

/// The intent files in `dir` whose names ocgen can't serve ([`is_name`]),
/// sorted. They are named, never dropped.
pub fn skipped(dir: &Path, s: &IntentSettings) -> Vec<String> {
    let mut names: Vec<String> = named(dir, s)
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| !is_name(n.trim_end_matches(".md")))
        .collect();
    names.sort();
    names
}

/// One sentence on the intent files ocgen can't open, if any.
pub fn skipped_note(dir: &Path, s: &IntentSettings) -> Option<String> {
    let names = skipped(dir, s);
    (!names.is_empty()).then(|| {
        format!(
            "ocgen can't open {} — rename {} to letters, digits, `-` and `_`, starting with a \
             letter or digit, at most 200 characters",
            names.join(", "),
            if names.len() == 1 { "it" } else { "them" },
        )
    })
}

/// An intent file by name — its file name, its stem, its number (`7`, `ADR-7`)
/// or the start of its name, in any case — or, with no name, the one with the
/// highest number.
pub fn find(dir: &Path, s: &IntentSettings, name: Option<&str>) -> Result<PathBuf> {
    let all = files(dir, s);
    let skipped = skipped_note(dir, s)
        .map(|n| format!(". Also: {n}"))
        .unwrap_or_default();
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return all.into_iter().next().with_context(|| {
            format!(
                "no intent files in {} yet — /intent writes them ({}-<slug>.md){skipped}",
                dir.display(),
                s.first_id()
            )
        });
    };
    let want = name.trim_end_matches(".md").to_ascii_lowercase();
    let lower = |p: &PathBuf| stem(p).to_ascii_lowercase();
    if let Some(p) = all.iter().find(|p| lower(p) == want) {
        return Ok(p.clone());
    }
    if let Some(n) = wanted_number(name, &s.prefix) {
        if let Some(p) = all.iter().find(|p| number(&stem(p), &s.prefix) == Some(n)) {
            return Ok(p.clone());
        }
    }
    if let Some(p) = all.iter().find(|p| lower(p).starts_with(&want)) {
        return Ok(p.clone());
    }
    bail!(
        "no intent file matches '{name}' in {} (intent files: {}){skipped}",
        dir.display(),
        all.iter().map(|p| stem(p)).collect::<Vec<_>>().join(", ")
    )
}

// ----------------------------------------------------------------- page --

/// A line of the file's header: `Key: value`.
fn field_re() -> Regex {
    Regex::new(r"^([^:`|#>*\-<\s][^:`|]{0,30}):\s+(\S.*)$").unwrap()
}

/// An intent file split for its page: its `# ` title, the `Key: value` lines
/// right after it, and the rest (what GitHub would show below them).
struct Parts {
    title: Option<String>,
    fields: Vec<(String, String)>,
    body: String,
}

fn parts(md: &str) -> Parts {
    let lines: Vec<&str> = md.lines().collect();
    let mut comment = false;
    let mut title_at = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if comment {
            comment = !t.contains("-->");
            continue;
        }
        if t.starts_with("<!--") {
            comment = !t.contains("-->");
            continue;
        }
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix("# ") {
            title_at = Some((i, h.trim().to_string()));
        }
        break;
    }
    let Some((at, title)) = title_at else {
        return Parts {
            title: None,
            fields: Vec::new(),
            body: md.to_string(),
        };
    };
    let re = field_re();
    let mut fields = Vec::new();
    let mut used = vec![at];
    let mut i = at + 1;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    while let Some(c) = lines.get(i).and_then(|l| re.captures(l.trim())) {
        fields.push((c[1].trim().to_string(), c[2].trim().to_string()));
        used.push(i);
        i += 1;
    }
    let body = lines
        .iter()
        .enumerate()
        .filter(|(n, _)| !used.contains(n))
        .map(|(_, l)| *l)
        .collect::<Vec<_>>()
        .join("\n");
    Parts {
        title: Some(title),
        fields,
        body,
    }
}

/// A header value as HTML: a web link when it is one, else text.
fn field_html(value: &str) -> String {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if (lower.starts_with("https://") || lower.starts_with("http://"))
        && !v.contains(char::is_whitespace)
    {
        let e = escape(v);
        format!(r#"<a href="{e}" rel="noreferrer">{e}</a>"#)
    } else {
        escape(v)
    }
}

/// What a read-only Zen page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenKind {
    /// An intent file.
    Intent,
    /// An /intent reading copy: an intent file translated.
    Copy,
    /// A /recap report.
    Recap,
}

/// How a read-only Zen page is shown.
pub struct Zen<'a> {
    pub kind: ZenKind,
    /// The page's name in the viewer: its tabs' topic.
    pub topic: &'a str,
    /// The list the page links back to, and its entry there — on the viewer's
    /// pages only.
    pub list: Option<(&'a str, &'a str)>,
    /// Served by the viewer, so it follows the file; a static page doesn't.
    pub live: bool,
    pub switch: &'a Switch,
    /// A reading copy's intent file, shown with the fields.
    pub original: Option<&'a str>,
}

/// The read-only page for intent file text `md` (revision `rev`, at `source`),
/// whose one script carries `nonce`; its own words in `language`.
pub fn page(md: &str, source: &str, rev: &str, nonce: &str, language: &str) -> Result<String> {
    let name = source.rsplit('/').next().unwrap_or(source);
    let slug = name.trim_end_matches(".md");
    let z = Zen {
        kind: ZenKind::Intent,
        topic: slug,
        list: Some((LIST, slug)),
        live: true,
        switch: &Switch::default(),
        original: None,
    };
    page_with(md, source, rev, nonce, language, &z)
}

/// The read-only Zen page for Markdown `md` (revision `rev`, at `source`) —
/// an intent file, a reading copy or a recap report, as `z` says — whose one
/// script carries `nonce`; its own words in `language`. The page shows the file
/// rendered the way GitHub would, or its Markdown as written.
pub fn page_with(
    md: &str,
    source: &str,
    rev: &str,
    nonce: &str,
    language: &str,
    z: &Zen,
) -> Result<String> {
    let w = words::for_language(language);
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template_owned("page.css", templates::load(super::html::STYLE)?)
        .context("parsing the notes page style")?;
    env.add_template_owned("palette.css", templates::load(draft::PALETTE)?)
        .context("parsing the pages' palettes")?;
    env.add_template_owned("langs.html", templates::load(super::html::LANGS)?)
        .context("parsing the language switch template")?;
    env.add_template_owned("adr.html", templates::load(TEMPLATE)?)
        .context("parsing the intent file template")?;
    let name = source.rsplit('/').next().unwrap_or(source).to_string();
    let p = parts(md);
    // Escaped here: minijinja's own escaping also turns `/` into `&#x2f;`.
    let safe = |s: &str| Value::from_safe_string(escape(s));
    let field = |k: &str, v: Value| {
        let mut m: BTreeMap<&str, Value> = BTreeMap::new();
        m.insert("key", safe(k));
        m.insert("value", v);
        Value::from(m)
    };
    let mut fields: Vec<Value> = p
        .fields
        .iter()
        .map(|(k, v)| field(k, Value::from_safe_string(field_html(v))))
        .collect();
    if let Some(o) = z.original {
        fields.push(field(
            w.versions.original.trim_end_matches(':'),
            Value::from_safe_string(format!("<code>{}</code>", escape(o))),
        ));
    }
    let (kind_title, footer, generator) = match z.kind {
        ZenKind::Intent => (
            w.docs.intent_title,
            w.docs.intent_footer,
            "ocgen — an /intent file, read-only; edit the .md file",
        ),
        ZenKind::Copy => (
            w.versions.copy_title,
            w.versions.copy_footer,
            "ocgen — an /intent reading copy, read-only; edit the .md file",
        ),
        ZenKind::Recap => (
            w.versions.recap_title,
            w.versions.recap_footer,
            "ocgen — a /recap report, read-only; edit the .md file",
        ),
    };
    let (list, base) = z.list.unwrap_or(("", ""));
    let ctx = minijinja::context! {
        w => listing::words_value(w),
        title => safe(p.title.as_deref().unwrap_or(name.trim_end_matches(".md"))),
        fields => fields,
        body => Value::from_safe_string(draft::render_safe(&p.body, false)),
        source_text => safe(md),
        source => safe(source),
        slug => safe(z.topic),
        rev => safe(rev),
        nonce => safe(nonce),
        list => list,
        base => safe(base),
        live => z.live,
        eyebrow => safe(if z.kind == ZenKind::Recap { "/recap" } else { "/intent" }),
        kind_title => kind_title,
        footer => footer,
        generator => generator,
        offline => Value::from_safe_string(w.docs.intent_offline.to_string()),
        versions => z.switch.versions.clone(),
        notice => z.switch.notice.clone(),
    };
    let mut page = env
        .get_template("adr.html")?
        .render(ctx)
        .context("rendering the intent file page")?;
    if !page.ends_with('\n') {
        page.push('\n');
    }
    Ok(page)
}

// ----------------------------------------------------------------- list --

/// What an intent file is about, for the list: its title without its number.
fn summary(md: &str) -> String {
    let Some(title) = parts(md).title else {
        return draft::summary(md);
    };
    let id = Regex::new(r"^[A-Za-z0-9]*-?\d+\s*[:—–-]\s*").unwrap();
    id.replace(&title, "").to_string()
}

/// The list page of the intent files of project `project`: page `page`, or the
/// one holding `selected` (see [`listing::page`]).
pub fn list_page(
    project: &Path,
    s: &IntentSettings,
    page: Option<usize>,
    selected: Option<&str>,
    root: &str,
    nonce: &str,
) -> Result<String> {
    let dir = dir(project, s);
    let all = files(&dir, s);
    let row = |p: &Path, text: &str| {
        let fields = parts(text).fields;
        let tag = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.clone())
        };
        Row {
            summary: summary(text),
            tags: ["Status", "Date"].iter().filter_map(|k| tag(k)).collect(),
            page: Some(preferred_page(project, &stem(p))),
        }
    };
    let source = s.dir.trim_matches('/').to_string();
    listing::page(
        &listing::List {
            kind: listing::Kind::Intents,
            docs: &all,
            row: &row,
            source: &source,
            list: LIST,
            skipped: &skipped(&dir, s),
            language: &super::answer_language(project),
        },
        page,
        selected,
        root,
        nonce,
    )
}

// ------------------------------------------------------------- sessions --

/// The viewer directory of project `root`, made and git-ignored.
fn viewer_dir(root: &Path) -> Result<PathBuf> {
    let d = root.join(VIEWER_DIR);
    fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    session::ignore_self(&d);
    Ok(d)
}

/// Whether `dir` is a project's [`VIEWER_DIR`], by its components.
pub fn is_viewer_dir(dir: &Path) -> bool {
    let abs = super::absolute(dir);
    let mut names = abs.components().rev().map(|c| match c {
        std::path::Component::Normal(n) => n.to_str(),
        _ => None,
    });
    names.next() == Some(Some("viewer"))
        && names.next() == Some(Some("intent"))
        && names.next() == Some(Some(".claude"))
        && dir.is_dir()
}

/// The project a [`VIEWER_DIR`] belongs to.
pub fn project_of_viewer(dir: &Path) -> Option<PathBuf> {
    Some(dir.parent()?.parent()?.parent()?.to_path_buf())
}

/// Remember that `session` wrote intent file `stem` of project `root` last.
pub fn remember(root: &Path, session: &str, stem: &str) -> bool {
    session::remember(&root.join(VIEWER_DIR), session, stem, is_name)
}

/// Before Bash call `call`: keep the intent files' state (see
/// [`session::before_bash`]). Nothing when there is no intent directory.
pub fn before_bash(root: &Path, call: &str) -> bool {
    let s = settings(root);
    let dir = dir(root, &s);
    dir.is_dir() && session::before_bash(&root.join(VIEWER_DIR), call, &files(&dir, &s))
}

/// After Bash call `call`: the intent file it added or changed becomes
/// `session`'s (see [`session::after_bash`]).
pub fn after_bash(root: &Path, call: &str, session_id: &str) -> Option<String> {
    let s = settings(root);
    let dir = dir(root, &s);
    session::after_bash(
        &root.join(VIEWER_DIR),
        call,
        session_id,
        &files(&dir, &s),
        is_name,
    )
}

/// The intent file `session` wrote last, while it still exists.
pub fn session_file(root: &Path, s: &IntentSettings, session_id: &str) -> Option<String> {
    session::recall(&root.join(VIEWER_DIR), &dir(root, s), session_id, is_name)
}

/// The stem of `rel` (project-relative) when it is one of the project's intent
/// files that ocgen can open.
pub fn target(s: &IntentSettings, rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    if crate::intent::kind(s, rel) != Some(crate::intent::Kind::Intent) {
        return None;
    }
    let file = rel.rsplit('/').next()?;
    file.strip_suffix(".md")
        .filter(|st| is_name(st))
        .map(str::to_string)
}

// ----------------------------------------------------------------- show --

/// Open intent file `md` of project `root` in the browser: refresh a tab that
/// shows it, or open one. `explicit` is a user's request (`ocgen adr`, the word).
pub fn show(
    root: &Path,
    md: &Path,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Shown> {
    let slug = stem(md);
    if !is_name(&slug) {
        bail!("{} has a name ocgen can't open", md.display());
    }
    if !super::browser::decide(env, super::browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let page = preferred_page(root, &slug);
    let info = viewer(root, env)?;
    let action = super::viewer::reload(&info, &page, explicit);
    draft::shown(action, || info.url(&page), env)
}

/// The page that opens intent file `stem` of project `root`: its reading copy
/// in the answer language when there is one (`<stem>.<language>`), else the
/// intent file's own.
pub fn preferred_page(root: &Path, stem: &str) -> String {
    let Ok(project) = crate::render::Project::load_state(root) else {
        return stem.to_string();
    };
    let answer = crate::render::canonical_language(project.response_language());
    let copy = (answer != "English")
        .then(|| copy_family(root, &project, stem))
        .flatten()
        .and_then(|f| f.path(&answer))
        .filter(|p| p.is_file());
    match copy {
        Some(_) => format!("{stem}.{}", super::versions::suffix(&answer)),
        None => stem.to_string(),
    }
}

/// The reading copies of intent file `stem` of project `root`, when its name
/// can name them.
pub fn copy_family(
    root: &Path,
    project: &crate::render::Project,
    stem: &str,
) -> Option<super::versions::Family> {
    let base = stem.to_ascii_lowercase();
    super::is_slug(&base).then(|| {
        super::versions::Family::new(
            super::versions::Kind::Copy,
            &root.join(super::versions::Kind::Copy.dir()),
            &base,
            Some((root, project)),
        )
    })
}

/// After reading copy `md` of project `root` is written: refresh the tabs that
/// show it, or open it once — a later write only refreshes — or, with no
/// viewer to run, its page as a file, once.
pub fn show_copy(
    root: &Path,
    md: &Path,
    page: &str,
    env: &HashMap<String, String>,
) -> Result<Shown> {
    if !super::browser::decide(env, super::browser::this_os(), false) {
        return Ok(Shown::Off);
    }
    let Ok(info) = viewer(root, env) else {
        return super::show_file_page(md, env, false);
    };
    let n = super::viewer::refresh(&info, page);
    if n > 0 {
        return Ok(Shown::Reloaded(n));
    }
    let marker = md.with_file_name(format!(".{}.opened", stem(md)));
    if marker.exists() {
        return Ok(Shown::Pending);
    }
    let url = info.url(page);
    super::browser::open(&url, env).context("opening the browser")?;
    let _ = fs::write(marker, "");
    Ok(Shown::Opened(url))
}

/// Open the list of the intent files of project `root` — on the page holding
/// `selected`, which is marked — in a tab that shows the list already, or a new
/// one.
pub fn show_list(
    root: &Path,
    selected: Option<&str>,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Shown> {
    if !super::browser::decide(env, super::browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let selected = selected.filter(|s| is_name(s));
    let info = viewer(root, env)?;
    let action = super::viewer::reload_list_of(&info, LIST, selected, explicit);
    draft::shown(action, || info.list_url_of(LIST, selected), env)
}

/// The intent files' viewer for project `root`, started if need be.
fn viewer(root: &Path, env: &HashMap<String, String>) -> Result<super::viewer::Info> {
    let dir = viewer_dir(root)?;
    super::viewer::ensure(&dir, env).context(
        "could not start the local viewer (it needs the ocgen binary and a free loopback port)",
    )
}
