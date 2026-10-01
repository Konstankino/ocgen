//! The `/inquire` ledger's visual companion. Every ledger
//! (`.claude/notes/<topic-slug>.md`) gets a sibling `<topic-slug>.html`, rendered
//! by ocgen from the Markdown — never written by the model — so the two cannot
//! drift. The `inquire-notes` hook re-renders it after each write and shows it:
//! an open tab is refreshed, and a new one is opened only when none shows it
//! (see [`viewer`]).

pub mod blocks;
pub mod browser;
pub mod html;
pub mod ledger;
pub mod viewer;

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

/// Render ledger `md` to its sibling `.html` (written atomically, and only when
/// it changed) and make sure the notes directory stays out of git. If the
/// ledger changes while rendering, it is rendered again.
pub fn render_file(md: &Path) -> Result<PathBuf> {
    let dir = md.parent().unwrap_or(Path::new("."));
    let slug = md
        .file_stem()
        .and_then(|s| s.to_str())
        .context("ledger has no file name")?;
    let out = dir.join(format!("{slug}.html"));
    for _ in 0..3 {
        let src = fs::read_to_string(md).with_context(|| format!("reading {}", md.display()))?;
        let source = format!(".claude/notes/{slug}.md");
        let page = html::render_page(&ledger::parse(&src), Some(&source))?;
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

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("page");
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
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
    // No live viewer: open the file itself — once, or the browser would gain a
    // tab on every update.
    let page = crate::paths::plain(&dir.join(format!("{slug}.html")));
    let marker = dir.join(format!(".{slug}.opened"));
    if !explicit && marker.exists() {
        return Ok(Shown::FileAlreadyOpened(page));
    }
    browser::open(&page.to_string_lossy(), env).context("opening the browser")?;
    let _ = fs::write(marker, "");
    Ok(Shown::OpenedFile(page))
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
