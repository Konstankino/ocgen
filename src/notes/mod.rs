//! The `/inquire` ledger's visual companion. Every ledger
//! (`.claude/notes/<topic-slug>.md`) gets a sibling `<topic-slug>.html`, rendered
//! by ocgen from the Markdown — never written by the model — so the two cannot
//! drift. The `inquire-notes` hook re-renders it after each write and shows it:
//! an open tab is refreshed, and a new one is opened only when none shows it
//! (see [`viewer`]).
//!
//! The pages speak the project's answer language ([`words`]). An /intent reading
//! copy (`.claude/intent/view/<slug>.md`, the English intent file translated
//! into that language) gets a page of its own the same way. The /intent issue
//! draft (`.claude/intent/drafts/<name>.md`) gets an editor instead ([`draft`]).
//!
//! The word `note` or `notes`, sent alone in Claude Code and caught by the same
//! hook, opens the ledger there is, or their list ([`LIST`], [`listing`]) with
//! the one this session wrote last selected ([`session`]). The intent files get
//! a read-only page and a list of their own ([`intents`]).

pub mod blocks;
pub mod browser;
pub mod draft;
pub mod html;
pub mod intents;
pub mod ledger;
pub mod listing;
pub mod session;
pub mod versions;
pub mod viewer;
pub mod words;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// What a written file is, as far as the notes view is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A ledger: `.claude/notes/<slug>.md`.
    Ledger {
        slug: String,
    },
    /// A Markdown file in `.claude/notes/` whose name isn't a usable slug.
    BadSlug,
    NotLedger,
}

/// A topic slug: lowercase letters, digits and hyphens, starting with a letter
/// or digit (it ends up in a URL).
pub fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.len() <= 80
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Classify a written path (POSIX or Windows form, absolute or relative).
pub fn ledger_target(path: &str) -> Target {
    let p = path.replace('\\', "/");
    let mut parts = p.rsplit('/');
    let (Some(file), Some(notes), Some(claude)) = (parts.next(), parts.next(), parts.next()) else {
        return Target::NotLedger;
    };
    if notes != "notes" || claude != ".claude" {
        return Target::NotLedger;
    }
    let Some(stem) = file.strip_suffix(".md") else {
        return Target::NotLedger;
    };
    if stem.starts_with('.') || stem.is_empty() {
        Target::NotLedger
    } else if is_slug(stem) {
        Target::Ledger { slug: stem.into() }
    } else {
        Target::BadSlug
    }
}

/// The slug of an /intent reading copy — `.claude/intent/view/<slug>.md`, any
/// path form — or `None` for anything else.
pub fn intent_view_target(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let mut parts = p.rsplit('/');
    let (file, view, intent, claude) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if (view, intent, claude) != ("view", "intent", ".claude") {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    is_slug(stem).then(|| stem.to_string())
}

/// The ocgen project containing `path`: its root and state.
pub(crate) fn project_of(path: &Path) -> Option<(PathBuf, crate::render::Project)> {
    let abs = absolute(path);
    let root = abs.ancestors().find(|a| {
        crate::target::Target::state_files()
            .iter()
            .any(|f| a.join(f).is_file())
    })?;
    let project = crate::render::Project::load_state(root).ok()?;
    Some((root.to_path_buf(), project))
}

/// The answer language of the project containing `path` — what its pages
/// speak — or English outside a project.
pub fn answer_language(path: &Path) -> String {
    project_of(path)
        .map(|(_, p)| p.response_language().to_string())
        .unwrap_or_else(|| "English".to_string())
}

/// A short content revision (FNV-1a, 64-bit): pages carry it so the viewer can
/// tell a tab that loaded an older version.
pub fn rev(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `p` made absolute against the current directory, without resolving links
/// (`.` components dropped, `..` kept), so a relative path classifies the same
/// as the absolute one.
fn absolute(p: &Path) -> PathBuf {
    let p = if p.as_os_str().is_empty() {
        Path::new(".")
    } else {
        p
    };
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .components()
        .collect()
}

/// Whether absolute path `abs` ends in `.claude/notes`, by its components (on
/// Unix a `\` is part of a name, not a separator).
fn ends_in_notes(abs: &Path) -> bool {
    let mut names = abs.components().rev().map(|c| match c {
        std::path::Component::Normal(n) => n.to_str(),
        _ => None,
    });
    names.next() == Some(Some("notes")) && names.next() == Some(Some(".claude"))
}

/// Whether `dir` is a project's `.claude/notes` directory — the only place
/// ocgen writes pages, a `*` .gitignore and the viewer's `.viewer.json`.
pub fn is_notes_dir(dir: &Path) -> bool {
    ends_in_notes(&absolute(dir)) && dir.is_dir()
}

/// The slug of ledger `md` (any path that reaches `.claude/notes/<slug>.md`,
/// relative or absolute), or why it isn't one.
pub fn ledger_slug(md: &Path) -> Result<String> {
    let abs = absolute(md);
    let target = match abs.file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.contains('\\') && abs.parent().is_some_and(ends_in_notes) => {
            ledger_target(&format!(".claude/notes/{name}"))
        }
        _ => Target::NotLedger,
    };
    match target {
        Target::Ledger { slug } => Ok(slug),
        Target::BadSlug => bail!(
            "{} has no HTML view — name ledgers with a lowercase-hyphen slug (e.g. request-flow.md)",
            md.display()
        ),
        Target::NotLedger => bail!(
            "{} is not an /inquire ledger — only .claude/notes/<topic-slug>.md files are rendered",
            md.display()
        ),
    }
}

/// Render ledger `md` to its sibling `.html` (written atomically, and only when
/// it changed) and make sure the notes directory stays out of git. If the
/// ledger changes while rendering, it is rendered again. A /recap report or an
/// /intent reading copy gets a read-only Zen page beside it the same way, and
/// any language version of these (`<name>.<language>.md`, see [`versions`]) its
/// own page, linked to the others. Anything else is refused (see
/// [`ledger_slug`]): the sibling page would overwrite a real `.html`, and the
/// `*` .gitignore would hide its directory from git.
pub fn render_file(md: &Path) -> Result<PathBuf> {
    match versions::classify(md) {
        Some(doc) => render_doc(md, &doc),
        None => {
            let slug = ledger_slug(md)?;
            render_ledger(md, &slug)
        }
    }
}

/// Whether [`render_file`] renders `md`: the reason it doesn't, if not.
pub fn check_renderable(md: &Path) -> Result<()> {
    match versions::classify(md) {
        Some(_) => Ok(()),
        None => ledger_slug(md).map(|_| ()),
    }
}

/// Render version `doc` (the file `md`) to its page.
fn render_doc(md: &Path, doc: &versions::Doc) -> Result<PathBuf> {
    let name = md
        .file_stem()
        .and_then(|s| s.to_str())
        .context("a document's file name")?;
    match doc.family.kind {
        versions::Kind::Ledger => render_ledger(md, name),
        versions::Kind::Recap => render_static(md, doc, intents::ZenKind::Recap),
        versions::Kind::Copy => render_static(md, doc, intents::ZenKind::Copy),
    }
}

/// Render every version of `doc`'s document there is — each page's switch
/// shows the others — and return their pages.
pub fn render_family(doc: &versions::Doc) -> Vec<PathBuf> {
    versions::status(&doc.family)
        .into_iter()
        .filter(|v| v.exists)
        .filter_map(|v| v.path)
        .filter(|p| p.parent() == Some(doc.family.dir.as_path()))
        .filter_map(|p| render_file(&p).ok())
        .collect()
}

/// The page of version `v` beside the others in `dir`: `None` for one that
/// isn't there (a reading copy's intent file).
fn sibling_page(v: &versions::Version, dir: &Path) -> Option<String> {
    let path = v.path.as_deref().filter(|p| p.parent() == Some(dir))?;
    Some(format!("{}.html", path.file_stem()?.to_str()?))
}

fn parent_or_dot(md: &Path) -> &Path {
    match md.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    }
}

/// Render recap report or reading copy `md` (version `doc`) to its sibling
/// `.html`: a read-only Zen page that works as a file — no live view — with its
/// switch to the other versions. Its directory keeps itself out of git.
fn render_static(md: &Path, doc: &versions::Doc, kind: intents::ZenKind) -> Result<PathBuf> {
    let dir = parent_or_dot(md);
    let name = md
        .file_stem()
        .and_then(|s| s.to_str())
        .context("a document's file name")?;
    let out = dir.join(format!("{name}.html"));
    let f = &doc.family;
    let w = words::for_language(&doc.language);
    let switch = versions::Switch::of(f, &doc.language, &|v| sibling_page(v, &f.dir), w);
    let root = project_of(md).map(|(root, _)| root);
    let original =
        f.source.as_ref().map(
            |s| match root.as_deref().and_then(|r| s.strip_prefix(r).ok()) {
                Some(rel) => rel.to_string_lossy().replace('\\', "/"),
                None => s.to_string_lossy().into_owned(),
            },
        );
    let source = format!(
        "{}/{name}.md",
        match kind {
            intents::ZenKind::Recap => versions::Kind::Recap.dir(),
            _ => versions::Kind::Copy.dir(),
        }
    );
    let zen = intents::Zen {
        kind,
        topic: name,
        list: None,
        live: false,
        switch: &switch,
        original: original.as_deref(),
    };
    for _ in 0..3 {
        let src = fs::read_to_string(md).with_context(|| format!("reading {}", md.display()))?;
        let page =
            intents::page_with(&src, &source, &rev(src.as_bytes()), "", &doc.language, &zen)?;
        write_if_changed(&out, page.as_bytes())?;
        if fs::read_to_string(md).ok().as_deref() == Some(src.as_str()) {
            break;
        }
    }
    session::ignore_self(dir);
    Ok(out)
}

/// The project-relative path of the intent file that reading copy `slug`
/// translates: the file in the intent directory whose name, lowercased, is
/// `<slug>.md`.
fn original_intent(root: &Path, dir: &str, slug: &str) -> Option<String> {
    let dir = dir.trim_end_matches('/');
    let want = format!("{slug}.md");
    fs::read_dir(root.join(dir))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .find(|n| n.to_ascii_lowercase() == want)
        .map(|n| format!("{dir}/{n}"))
}

/// [`render_file`] for ledger version `md` named `name` (`<slug>` or
/// `<slug>.<language>`), already known to be one: the viewer serves the notes
/// directory it checked at start by its canonical path, which need not end in
/// `.claude/notes` (a linked `.claude`).
fn render_ledger(md: &Path, name: &str) -> Result<PathBuf> {
    let dir = parent_or_dot(md);
    let out = dir.join(format!("{name}.html"));
    let project = project_of(md);
    let doc = versions::doc_in(
        versions::Kind::Ledger,
        dir,
        &format!("{name}.md"),
        project.as_ref().map(|(r, p)| (r.as_path(), p)),
    );
    let (language, switch) = match &doc {
        Some(d) => {
            let w = words::for_language(&d.language);
            let switch = versions::Switch::of(&d.family, &d.language, &|v| sibling_page(v, dir), w);
            (d.language.clone(), switch)
        }
        // `flow.backup` names no language: not a version, not a ledger.
        None if name.contains('.') => bail!(
            "{} has no HTML view — name ledgers with a lowercase-hyphen slug (e.g. request-flow.md)",
            md.display()
        ),
        None => (answer_language(md), versions::Switch::default()),
    };
    for _ in 0..3 {
        let src = fs::read_to_string(md).with_context(|| format!("reading {}", md.display()))?;
        let source = format!(".claude/notes/{name}.md");
        let page = html::render_version(&ledger::parse(&src), Some(&source), &language, &switch)?;
        write_if_changed(&out, page.as_bytes())?;
        if fs::read_to_string(md).ok().as_deref() == Some(src.as_str()) {
            break;
        }
    }
    let ignore = dir.join(".gitignore");
    if is_notes_dir(dir) && !ignore.exists() {
        let _ = fs::write(ignore, "*\n");
    }
    Ok(out)
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    // A temp file of this call's own: the viewer renders on one thread per
    // connection, and a shared name would let one write truncate another's.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("page");
    let tmp = path.with_file_name(format!(".{name}.tmp-{}-{seq}-{nanos}", std::process::id()));
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    // Windows refuses to replace a file another process has open for a moment.
    let mut tries = 0;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if tries < 5 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("writing {}", path.display()));
            }
        }
    }
}

/// What [`show`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// Showing is off here (`OCGEN_NOTES_OPEN=0`, CI, no display).
    Off,
    /// Open tabs were told to reload.
    Reloaded(usize),
    /// A tab is opening or reloading and will pick the update up.
    Pending,
    /// A new tab was opened at this URL.
    Opened(String),
    /// No viewer could run: the file was opened directly.
    OpenedFile(PathBuf),
    /// No viewer could run, and the file was already opened once.
    FileAlreadyOpened(PathBuf),
}

/// Show ledger `md`'s page: refresh the tabs that show it, or open one.
/// `explicit` is a user's `ocgen notes open` (always opens when no tab shows the
/// page); otherwise it is the hook after a write.
pub fn show(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let dir = md.parent().unwrap_or(Path::new("."));
    let Some(slug) = md
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| versions::split(s, is_slug).is_some())
    else {
        bail!(
            "{} is not a ledger name (lowercase-hyphen slug)",
            md.display()
        );
    };
    if let Some(info) = viewer::ensure(dir, env) {
        match viewer::reload(&info, slug, explicit) {
            Some(viewer::Action::Reloaded(n)) => return Ok(Shown::Reloaded(n)),
            Some(viewer::Action::Pending) => return Ok(Shown::Pending),
            Some(viewer::Action::Open) => {
                let url = info.url(slug);
                browser::open(&url, env).context("opening the browser")?;
                return Ok(Shown::Opened(url));
            }
            None => {}
        }
    }
    open_once(dir, slug, env, explicit)
}

/// Open page `<slug>.html` in `dir` as a file — once, or the browser would gain
/// a tab on every update (unless `explicit`).
fn open_once(
    dir: &Path,
    slug: &str,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Shown> {
    let page = crate::paths::plain(&dir.join(format!("{slug}.html")));
    let marker = dir.join(format!(".{slug}.opened"));
    if !explicit && marker.exists() {
        return Ok(Shown::FileAlreadyOpened(page));
    }
    browser::open(&page.to_string_lossy(), env).context("opening the browser")?;
    let _ = fs::write(marker, "");
    Ok(Shown::OpenedFile(page))
}

/// Refresh the open tabs of pages `names` of the live viewer for notes
/// directory `dir`, if one runs — never opening one.
pub fn refresh_pages(dir: &Path, names: &[String], env: &HashMap<String, String>) {
    if names.is_empty() || !browser::decide(env, browser::this_os(), false) {
        return;
    }
    if let Some(info) = viewer::read_info(dir) {
        for n in names {
            viewer::refresh(&info, n);
        }
    }
}

/// Show the static page of /recap report or reading copy `md` (any language
/// version): the file itself, opened once — a file page can't refresh itself.
pub fn show_file_page(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let Some(name) = md
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| versions::split(s, is_slug).is_some())
    else {
        bail!("{} has no page", md.display());
    };
    open_once(md.parent().unwrap_or(Path::new(".")), name, env, explicit)
}

/// Show the page of /intent reading copy `md`: the file itself, opened once
/// (the live viewer serves only `.claude/notes`).
pub fn show_intent_view(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let Some(slug) = intent_view_target(&absolute(md).to_string_lossy()) else {
        bail!("{} is not an /intent reading copy", md.display());
    };
    open_once(md.parent().unwrap_or(Path::new(".")), &slug, env, explicit)
}

/// The `.claude/notes` directory of the project containing `start`.
pub fn notes_dir(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok()?;
    start
        .ancestors()
        .map(|a| a.join(".claude/notes"))
        .find(|d| d.is_dir())
        .map(|d| crate::paths::plain(&d))
}

// ---------------------------------------------------------------- the word --

/// The words that open the ledgers, sent alone.
pub const WORDS: [&str; 2] = ["note", "notes"];
/// The list of ledgers' path under the viewer's token, and its tabs' topic: not
/// a slug, so no ledger can take it.
pub const LIST: &str = "_notes";
/// Where /inquire keeps its ledgers, relative to the project root.
pub const DIR: &str = ".claude/notes";

/// Whether a prompt is a word that opens the ledgers (any case, nothing else).
pub fn is_prompt(prompt: &str) -> bool {
    WORDS.iter().any(|w| prompt.trim().eq_ignore_ascii_case(w))
}

/// Remember that `session` wrote ledger `slug` (in notes directory `dir`) last.
pub fn remember(dir: &Path, session_id: &str, slug: &str) -> bool {
    session::remember(dir, session_id, slug, is_slug)
}

/// The ledger `session` wrote last, while it still exists.
pub fn session_ledger(dir: &Path, session_id: &str) -> Option<String> {
    session::recall(dir, dir, session_id, is_slug)
}

/// The list page of the ledgers in notes directory `dir`, newest first: page
/// `page`, or the one holding `selected` (see [`listing::page`]).
pub fn list_page(
    dir: &Path,
    page: Option<usize>,
    selected: Option<&str>,
    root: &str,
    nonce: &str,
) -> Result<String> {
    let all = ledgers(dir);
    let w = words::for_language(&answer_language(dir));
    let row = |_: &Path, text: &str| {
        let l = ledger::parse(text);
        let mut tags = Vec::new();
        if !l.status.trim().is_empty() {
            tags.push(l.status.trim().to_string());
        }
        if !l.updated.trim().is_empty() {
            tags.push(format!("{} {}", w.docs.updated, l.updated.trim()));
        }
        listing::Row {
            summary: l.topic.replace('`', ""),
            tags,
            page: None,
        }
    };
    listing::page(
        &listing::List {
            kind: listing::Kind::Notes,
            docs: &all,
            row: &row,
            source: DIR,
            list: LIST,
            skipped: &[],
            language: &answer_language(dir),
        },
        page,
        selected,
        root,
        nonce,
    )
}

/// Open the list of the ledgers in notes directory `dir` — on the page holding
/// `selected`, which is marked — in a tab that shows the list already, or a new
/// one.
pub fn show_list(
    dir: &Path,
    selected: Option<&str>,
    env: &HashMap<String, String>,
    explicit: bool,
) -> Result<Shown> {
    if !is_notes_dir(dir) {
        bail!("{} is not a .claude/notes directory", dir.display());
    }
    if !browser::decide(env, browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let selected = selected.filter(|s| is_slug(s));
    let info = viewer::ensure(dir, env).context(
        "could not start the local viewer (it needs the ocgen binary and a free loopback port)",
    )?;
    let action = viewer::reload_list_of(&info, LIST, selected, explicit);
    draft::shown(action, || info.list_url_of(LIST, selected), env)
}

/// `s` as a topic slug (`Request flow!` → `request-flow`).
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The ledgers in `dir`, most recently changed first: each topic once, by its
/// unsuffixed file — or, when it has none (its languages changed), by its
/// first other language version.
pub fn ledgers(dir: &Path) -> Vec<PathBuf> {
    let mut topics: std::collections::BTreeMap<String, PathBuf> = std::collections::BTreeMap::new();
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let project = project_of(dir);
    let language = |s: &str| versions::names_language(s, project.as_ref().map(|(_, p)| p));
    for p in files {
        let Some(stem) = p
            .extension()
            .filter(|e| *e == "md")
            .and(p.file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        match versions::split(&stem, is_slug) {
            Some((base, None)) => {
                topics.insert(base.to_string(), p);
            }
            Some((base, Some(s))) if language(s) => {
                topics.entry(base.to_string()).or_insert(p);
            }
            _ => {}
        }
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = topics
        .into_values()
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

/// Find a ledger by slug, by its slugified name, or by its `Topic:` line; with
/// no topic, the most recently changed one.
pub fn find_ledger(dir: &Path, topic: Option<&str>) -> Result<PathBuf> {
    let all = ledgers(dir);
    let Some(topic) = topic.map(str::trim).filter(|t| !t.is_empty()) else {
        return all.into_iter().next().with_context(|| {
            format!(
                "no ledgers in {} yet — start one with /inquire",
                dir.display()
            )
        });
    };
    for slug in [topic.to_string(), slugify(topic)] {
        let p = dir.join(format!("{slug}.md"));
        if is_slug(&slug) && p.is_file() {
            return Ok(p);
        }
    }
    let wanted = topic.to_lowercase();
    let by_topic = |exact: bool| {
        all.iter().find(|p| {
            let t = ledger::parse(&fs::read_to_string(p).unwrap_or_default())
                .topic
                .to_lowercase();
            if exact {
                t == wanted
            } else {
                t.contains(&wanted)
            }
        })
    };
    match by_topic(true).or_else(|| by_topic(false)) {
        Some(p) => Ok(p.clone()),
        None => bail!(
            "no ledger matches '{topic}' in {} (ledgers: {})",
            dir.display(),
            all.iter()
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The viewer renders on one thread per connection: concurrent writes of
    /// one page must each go through their own temp file, so none fails and a
    /// reader only ever sees a whole page.
    #[test]
    fn concurrent_writes_use_their_own_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("flow.html"));
        let pages: Arc<[Vec<u8>; 2]> = Arc::new([vec![b'a'; 1 << 20], vec![b'b'; 1 << 20]]);
        let writers: Vec<_> = (0..8)
            .map(|t| {
                let (path, pages) = (Arc::clone(&path), Arc::clone(&pages));
                std::thread::spawn(move || {
                    (0..40)
                        .filter_map(|i| write_if_changed(&path, &pages[(t + i) % 2]).err())
                        .map(|e| format!("{e:#}"))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let reader = {
            let (path, pages) = (Arc::clone(&path), Arc::clone(&pages));
            std::thread::spawn(move || {
                let mut torn = 0;
                for _ in 0..400 {
                    if let Ok(b) = fs::read(&*path) {
                        if b != pages[0] && b != pages[1] {
                            torn += 1;
                        }
                    }
                }
                torn
            })
        };
        let errors: Vec<String> = writers
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        assert!(
            errors.is_empty(),
            "{} failed: {:?}",
            errors.len(),
            errors.first()
        );
        assert_eq!(reader.join().unwrap(), 0, "a reader saw a torn page");
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["flow.html"]);
    }
}
