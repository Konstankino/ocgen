//! The /intent issue draft's browser review. /intent writes the GitHub issue
//! description to `.claude/intent/drafts/<name>.md`; `ocgen draft` — or the word
//! `draft` sent alone in Claude Code, caught by the `intent-draft` hook — opens
//! it in a local editor run by the live viewer ([`super::viewer`]): the text
//! formatted, with the Markdown syntax hidden (Milkdown, [`EDITOR_JS`]), the
//! Markdown source, a preview drawn the way GitHub draws an issue, and buttons
//! that copy it. Save writes the file back as plain Markdown: unedited, byte
//! for byte; edited, with the original text of every block left untouched.
//!
//! The editor is a prebuilt bundle (built by `tools/milkdown`, never by Cargo)
//! inlined into the page's one nonce-tagged script, so the page's policy still
//! allows nothing from outside. In the editor a draft's raw HTML is text, and
//! images are shown as their Markdown.
//!
//! The preview is made safe here, after pulldown-cmark parses it: raw HTML keeps
//! only a few tags GitHub allows (`<details>`, `<summary>`, `<kbd>`…), without
//! attributes; anything else is shown as text. Comments are dropped (GitHub hides
//! them), links keep only http(s) and mailto targets, and images become links —
//! nothing in a draft runs or loads anything.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use minijinja::{AutoEscape, Environment, Value};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use regex::Regex;

use super::html::escape;
use super::words::{self, Words};
use super::Shown;
use crate::templates;

/// Where /intent keeps issue drafts, relative to the project root.
pub const DIR: &str = ".claude/intent/drafts";
/// The one-word prompt that opens the newest draft.
pub const WORD: &str = "draft";
/// The editor page.
pub const TEMPLATE: &str = "claude/notes/draft.html.j2";
/// The Milkdown release the editor bundle is built from (pinned in
/// `tools/milkdown/package.json`).
pub const MILKDOWN_VERSION: &str = "7.22.2";
/// The formatted editor: Milkdown with CommonMark and GitHub Markdown, built by
/// `cd tools/milkdown && npm ci && npm run build` (licenses: `milkdown/LICENSES.txt`).
/// It defines `window.OcgenDraft`.
pub const EDITOR_JS: &str = include_str!("milkdown/editor.min.js");

/// Whether a prompt is the one word that opens the draft (any case, nothing else).
pub fn is_prompt(prompt: &str) -> bool {
    prompt.trim().eq_ignore_ascii_case(WORD)
}

/// The slug of an issue draft — `.claude/intent/drafts/<slug>.md`, any path
/// form — or `None` for anything else.
pub fn draft_target(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let mut parts = p.rsplit('/');
    let (file, drafts, intent, claude) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if (drafts, intent, claude) != ("drafts", "intent", ".claude") {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    super::is_slug(stem).then(|| stem.to_string())
}

/// Whether `dir` is a project's `.claude/intent/drafts` directory, by its
/// components (on Unix a `\` is part of a name, not a separator).
pub fn is_drafts_dir(dir: &Path) -> bool {
    let abs = super::absolute(dir);
    let mut names = abs.components().rev().map(|c| match c {
        std::path::Component::Normal(n) => n.to_str(),
        _ => None,
    });
    names.next() == Some(Some("drafts"))
        && names.next() == Some(Some("intent"))
        && names.next() == Some(Some(".claude"))
        && dir.is_dir()
}

/// The drafts directory of the project containing `start`.
pub fn drafts_dir(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok()?;
    start
        .ancestors()
        .map(|a| a.join(DIR))
        .find(|d| d.is_dir())
        .map(|d| crate::paths::plain(&d))
}

/// The drafts in `dir`, most recently changed first.
pub fn drafts(dir: &Path) -> Vec<PathBuf> {
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
                .is_some_and(super::is_slug)
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

/// A draft by name — its file name, its stem, or the start of it in any case
/// (`ADR-0007`) — or, with no name, the most recently changed one.
pub fn find(dir: &Path, name: Option<&str>) -> Result<PathBuf> {
    let all = drafts(dir);
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return all.into_iter().next().with_context(|| {
            format!(
                "no issue drafts in {} yet — /intent and /review-intent write one when they draft a GitHub issue",
                dir.display()
            )
        });
    };
    let want = name.trim_end_matches(".md").to_ascii_lowercase();
    let stem = |p: &PathBuf| {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if let Some(p) = all.iter().find(|p| stem(p) == want) {
        return Ok(p.clone());
    }
    if let Some(p) = all.iter().find(|p| stem(p).starts_with(&want)) {
        return Ok(p.clone());
    }
    bail!(
        "no issue draft matches '{name}' in {} (drafts: {})",
        dir.display(),
        all.iter().map(stem).collect::<Vec<_>>().join(", ")
    )
}

// ------------------------------------------------------------- preview --

/// Tags kept from a draft's raw HTML (attributes dropped): what GitHub renders
/// in an issue and nothing that loads or runs anything.
const ALLOWED_TAGS: &[&str] = &[
    "details",
    "summary",
    "kbd",
    "sub",
    "sup",
    "b",
    "i",
    "em",
    "strong",
    "code",
    "br",
    "hr",
    "p",
    "ins",
    "del",
    "s",
    "u",
    "mark",
    "small",
    "blockquote",
    "ul",
    "ol",
    "li",
    "pre",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

/// One piece of raw HTML from a draft (an HTML block, or one inline HTML
/// event — a comment never spans two) made safe.
fn sanitize(html: &str) -> String {
    let tag = Regex::new(r"^<(/?)([A-Za-z][A-Za-z0-9]*)(\s[^<>]*?)?\s*(/?)>").unwrap();
    let mut out = String::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            out.push_str(&escape(rest));
            break;
        };
        out.push_str(&escape(&rest[..lt]));
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            // `<!-->` and `<!--->` are whole (empty) comments.
            let body = after.strip_prefix('-').unwrap_or(after);
            rest = match body.strip_prefix('>') {
                Some(r) => r,
                None => after.find("-->").map_or("", |i| &after[i + 3..]),
            };
            continue;
        }
        match tag.captures(rest) {
            Some(c) if ALLOWED_TAGS.contains(&c[2].to_ascii_lowercase().as_str()) => {
                let name = c[2].to_ascii_lowercase();
                let open = c.get(3).map_or("", |a| a.as_str()).trim();
                // `<details open>` is the one attribute worth keeping.
                let attrs = if name == "details" && c[1].is_empty() && open == "open" {
                    " open"
                } else {
                    ""
                };
                out.push_str(&format!("<{}{name}{attrs}>", &c[1]));
                rest = &rest[c[0].len()..];
            }
            _ => {
                out.push_str("&lt;");
                rest = &rest[1..];
            }
        }
    }
    out
}

/// A link target the preview may keep: the web or mail.
fn safe_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("https://") || u.starts_with("http://") || u.starts_with("mailto:")
}

/// The draft as GitHub shows an issue description, made safe (see the module
/// docs): headings, lists, task lists, tables, code, `<details>` blocks, and a
/// single newline as a line break.
pub fn preview(md: &str) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut events: Vec<Event> = Vec::new();
    let mut block: Option<String> = None;
    // For each open link or image: whether it is kept (as a link).
    let mut links: Vec<bool> = Vec::new();
    for ev in Parser::new_ext(md, opts) {
        match ev {
            Event::Start(Tag::HtmlBlock) => block = Some(String::new()),
            Event::End(TagEnd::HtmlBlock) => {
                let html = sanitize(&block.take().unwrap_or_default());
                if !html.trim().is_empty() {
                    events.push(Event::Html(html.into()));
                }
            }
            Event::Html(h) => match block.as_mut() {
                Some(b) => b.push_str(&h),
                None => events.push(Event::Html(sanitize(&h).into())),
            },
            Event::InlineHtml(h) => events.push(Event::InlineHtml(sanitize(&h).into())),
            // An issue keeps the line breaks it was written with.
            Event::SoftBreak => events.push(Event::HardBreak),
            Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    ..
                },
            ) => {
                // An email autolink's target has no scheme; push_html adds `mailto:`.
                let email = link_type == LinkType::Email;
                let keep = email || safe_url(&dest_url);
                links.push(keep);
                if keep {
                    events.push(Event::Start(Tag::Link {
                        link_type: if email { link_type } else { LinkType::Inline },
                        dest_url,
                        title: "".into(),
                        id: "".into(),
                    }));
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                if links.pop() == Some(true) {
                    events.push(Event::End(TagEnd::Link));
                }
            }
            other => events.push(other),
        }
    }
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

// ---------------------------------------------------------------- page --

/// The editor page for draft text `md` (revision `rev`, at `source`), whose one
/// script carries `nonce`; its own words in `language`. It warns while the text
/// doesn't @mention every one of `approvers` (the project's, now).
pub fn page(
    md: &str,
    source: &str,
    rev: &str,
    nonce: &str,
    language: &str,
    approvers: &[String],
) -> Result<String> {
    let w = words::for_language(language);
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template_owned("page.css", templates::load(super::html::STYLE)?)
        .context("parsing the notes page style")?;
    env.add_template_owned("draft.html", templates::load(TEMPLATE)?)
        .context("parsing the draft editor template")?;
    let name = source.rsplit('/').next().unwrap_or(source).to_string();
    let gaps = crate::intent::gaps(md, approvers);
    // Escaped here: minijinja's own escaping also turns `/` into `&#x2f;`.
    let safe = |s: &str| Value::from_safe_string(escape(s));
    let ctx = minijinja::context! {
        w => words_value(w),
        name => safe(&name),
        source => safe(source),
        rev => safe(rev),
        nonce => safe(nonce),
        text => safe(md),
        preview => Value::from_safe_string(preview(md)),
        // Ours, built to be inlined (nothing in it ends the script).
        editor => Value::from_safe_string(EDITOR_JS.to_string()),
        approvers => safe(&approvers.join(" ")),
        missing => safe(&gaps.missing.join(", ")),
        says_none => gaps.says_none,
        gaps => !gaps.is_empty(),
        wraps => crate::intent::hard_wraps(md).len(),
    };
    let mut page = env
        .get_template("draft.html")?
        .render(ctx)
        .context("rendering the draft editor")?;
    if !page.ends_with('\n') {
        page.push('\n');
    }
    Ok(page)
}

fn words_value(w: &'static Words) -> Value {
    let d = &w.draft;
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("lang", Value::from(w.lang));
    for (k, v) in [
        ("title", d.title),
        ("editor", d.editor),
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
        ("copy_markdown", d.copy_markdown),
        ("copy_formatted", d.copy_formatted),
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
        ("missing", d.missing),
        ("says_none", d.says_none),
        ("wrapped", d.wrapped),
        ("join_lines", d.join_lines),
        ("footer", d.footer),
    ] {
        m.insert(k, Value::from(v));
    }
    // Ours, not the draft's: they name a command or keys in markup.
    m.insert("offline", Value::from_safe_string(d.offline.to_string()));
    m.insert("keys", Value::from_safe_string(d.keys.to_string()));
    Value::from(m)
}

// ---------------------------------------------------------------- show --

/// The approvers of the project containing draft `md`, as its state has them
/// now (none when /intent is off or there is no project).
pub fn approvers_for(md: &Path) -> Vec<String> {
    super::project_of(md)
        .filter(|(_, p)| p.claude.workflow.intent)
        .map(|(_, p)| p.claude.intent.approvers)
        .unwrap_or_default()
}

/// How draft `md` falls short of its project's current approvers.
pub fn gaps_of(md: &Path) -> crate::intent::Gaps {
    let approvers = approvers_for(md);
    fs::read_to_string(md)
        .map(|t| crate::intent::gaps(&t, &approvers))
        .unwrap_or_default()
}

/// Open draft `md` in the browser editor: tell an open tab, or open one.
/// `explicit` is a user's request (`ocgen draft`, the `draft` word): it opens a
/// tab unless one already shows the draft.
pub fn show(md: &Path, env: &HashMap<String, String>, explicit: bool) -> Result<Shown> {
    let Some(slug) = draft_target(&super::absolute(md).to_string_lossy()) else {
        bail!("{} is not an /intent issue draft", md.display());
    };
    if !super::browser::decide(env, super::browser::this_os(), explicit) {
        return Ok(Shown::Off);
    }
    let dir = md.parent().unwrap_or(Path::new("."));
    // Drafts are working copies: the issue on GitHub is the record.
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = fs::write(ignore, "*\n");
    }
    let info = super::viewer::ensure(dir, env).context(
        "could not start the local editor (it needs the ocgen binary and a free loopback port)",
    )?;
    match super::viewer::reload(&info, &slug, explicit) {
        Some(super::viewer::Action::Reloaded(n)) => Ok(Shown::Reloaded(n)),
        Some(super::viewer::Action::Pending) => Ok(Shown::Pending),
        Some(super::viewer::Action::Open) => {
            let url = info.url(&slug);
            super::browser::open(&url, env).context("opening the browser")?;
            Ok(Shown::Opened(url))
        }
        None => bail!("the local editor did not answer"),
    }
}
