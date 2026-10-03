//! The `/inquire` ledger's visual companion. Every ledger
//! (`.claude/notes/<topic-slug>.md`) gets a sibling `<topic-slug>.html`, rendered
//! by ocgen from the Markdown — never written by the model — so the two cannot
//! drift. The `inquire-notes` hook re-renders it after each write and shows it:
//! an open tab is refreshed, and a new one is opened only when none shows it
//! (see [`viewer`]).
//!
//! The pages speak the project's answer language ([`words`]). An /intent reading
//! copy (`.claude/intent/view/<slug>.md`, the English intent file translated
//! into that language) gets a page of its own the same way.

pub mod blocks;
pub mod browser;
pub mod html;
pub mod ledger;
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
fn project_of(path: &Path) -> Option<(PathBuf, crate::render::Project)> {
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
/// ledger changes while rendering, it is rendered again. Anything but a ledger
/// is refused (see [`ledger_slug`]): the sibling page would overwrite a real
/// `.html`, and the `*` .gitignore would hide its directory from git.
pub fn render_file(md: &Path) -> Result<PathBuf> {
    if let Some(slug) = intent_view_target(&absolute(md).to_string_lossy()) {
        return render_intent_view(md, &slug);
    }
    let slug = ledger_slug(md)?;
    render_ledger(md, &slug)
}

/// Render /intent reading copy `md` (slug `slug`) to its sibling `.html`, in
/// the project's answer language, linking the English intent file it
/// translates; the view directory keeps itself out of git.
fn render_intent_view(md: &Path, slug: &str) -> Result<PathBuf> {
    let dir = match md.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let out = dir.join(format!("{slug}.html"));
    let project = project_of(md);
    let language = project
        .as_ref()
        .map_or("English", |(_, p)| p.response_language())
        .to_string();
    let original = project
        .as_ref()
        .and_then(|(root, p)| original_intent(root, &p.claude.intent.dir, slug));
    let source = format!(".claude/intent/view/{slug}.md");
    for _ in 0..3 {
        let src = fs::read_to_string(md).with_context(|| format!("reading {}", md.display()))?;
        let page = html::render_intent(&src, &source, original.as_deref(), &language)?;
        write_if_changed(&out, page.as_bytes())?;
        if fs::read_to_string(md).ok().as_deref() == Some(src.as_str()) {
            break;
        }
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = fs::write(ignore, "*\n");
    }
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

/// [`render_file`] for ledger `md` of slug `slug`, already known to be one: the
/// viewer serves the notes directory it checked at start by its canonical path,
/// which need not end in `.claude/notes` (a linked `.claude`).
fn render_ledger(md: &Path, slug: &str) -> Result<PathBuf> {
    let dir = match md.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let out = dir.join(format!("{slug}.html"));
    let language = answer_language(md);
    for _ in 0..3 {
        let src = fs::read_to_string(md).with_context(|| format!("reading {}", md.display()))?;
        let source = format!(".claude/notes/{slug}.md");
        let page = html::render_page_in(&ledger::parse(&src), Some(&source), &language)?;
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
        .filter(|s| is_slug(s))
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

/// The ledgers in `dir`, most recently changed first.
pub fn ledgers(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(ledger_target(&p.to_string_lossy()), Target::Ledger { .. }))
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
