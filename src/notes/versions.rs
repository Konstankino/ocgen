//! A local document's language versions. Each document ocgen's skills keep on
//! the machine — an /inquire ledger, a /recap report, an /intent reading copy —
//! is kept in each of the project's languages ([`Project::doc_languages`]): the
//! unsuffixed file in the first, every other language's as
//! `<name>.<language>.md` ([`suffix`]) — `request-flow.md` and
//! `request-flow.english.md`. A reading copy's English version is the intent
//! file it translates.
//!
//! A record per document (`<dir>/.versions/<name>.json`) keeps each version's
//! revision as the notes hook last saw it, and the versions that must follow a
//! change in another: pending, with the sessions (or subagents) that owe them.
//! A pending version is done once its file changes, whatever writes it. The hook
//! tells Claude after a write ([`touch`]) and once more when a turn ends
//! ([`owed`]); the pages show which version is missing or behind ([`status`]).
//!
//! [`Project::doc_languages`]: crate::render::Project::doc_languages

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::render::{canonical_language, Project};

/// Where, in a document's directory, the version records are kept.
pub const RECORDS: &str = ".versions";

/// What kind of local document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An /inquire ledger: `.claude/notes/<slug>.md`.
    Ledger,
    /// A /recap report: `.claude/notes/recap/<date>.md`.
    Recap,
    /// An /intent reading copy: `.claude/intent/view/<intent file, lowercased>.md`.
    Copy,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Ledger, Kind::Recap, Kind::Copy];

    /// The kind's directory, relative to the project root.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Ledger => ".claude/notes",
            Kind::Recap => ".claude/notes/recap",
            Kind::Copy => ".claude/intent/view",
        }
    }
}

/// The languages a version's suffix may name besides the project's own: those
/// ocgen offers, so a version left from an earlier language still reads as one.
const KNOWN: [&str; 2] = ["English", "Ukrainian"];

/// Whether suffix `s` names a language of `project` (or one ocgen offers): a
/// `flow.backup.md` is no language version.
pub fn names_language(s: &str, project: Option<&Project>) -> bool {
    project
        .map(|p| p.doc_languages())
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .chain(KNOWN)
        .any(|l| suffix(l) == s)
}

/// The file-name suffix of `language`'s version: `English` → `english`.
pub fn suffix(language: &str) -> String {
    super::slugify(&canonical_language(language))
}

/// Whether `s` can be a language suffix: lowercase words joined by hyphens.
fn is_suffix(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.split('-')
            .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
}

/// A version's name — a file stem or a page name — split into the document's
/// name and its language suffix: `flow.english` → (`flow`, `Some("english")`),
/// `flow` → (`flow`, `None`). `None` for anything else; `base_ok` judges the
/// document's name (none of the names ocgen accepts has a dot).
pub fn split(name: &str, base_ok: fn(&str) -> bool) -> Option<(&str, Option<&str>)> {
    match name.rsplit_once('.') {
        Some((base, lang)) => (base_ok(base) && is_suffix(lang)).then_some((base, Some(lang))),
        None => base_ok(name).then_some((name, None)),
    }
}

/// A document: the versions of one ledger, report or reading copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub kind: Kind,
    /// The directory of the local versions.
    pub dir: PathBuf,
    /// The document's name: the unsuffixed file's stem.
    pub base: String,
    /// The languages kept in `dir`, the unsuffixed file's first.
    pub local: Vec<String>,
    /// A reading copy's English version: the intent file it translates, when
    /// there is one.
    pub source: Option<PathBuf>,
}

impl Family {
    /// The family of document `base` of `kind` in `dir`, in the languages of
    /// `project` (English only outside one).
    pub fn new(kind: Kind, dir: &Path, base: &str, project: Option<(&Path, &Project)>) -> Family {
        let local = match (kind, project) {
            (Kind::Copy, Some((_, p))) => p.copy_languages(),
            (_, Some((_, p))) => p.doc_languages(),
            (Kind::Copy, None) => Vec::new(),
            (_, None) => vec!["English".to_string()],
        };
        let source = match (kind, project) {
            (Kind::Copy, Some((root, p))) => {
                super::original_intent(root, &p.claude.intent.dir, base).map(|rel| root.join(rel))
            }
            _ => None,
        };
        Family {
            kind,
            dir: dir.to_path_buf(),
            base: base.to_string(),
            local,
            source,
        }
    }

    /// Every language the document is kept in: a reading copy's English (its
    /// intent file) first, then the local ones.
    pub fn languages(&self) -> Vec<String> {
        let mut all = Vec::new();
        if self.kind == Kind::Copy {
            all.push("English".to_string());
        }
        all.extend(self.local.iter().cloned());
        all
    }

    /// Whether there is another version to keep in step.
    pub fn bilingual(&self) -> bool {
        self.languages().len() > 1
    }

    /// The file stem of `language`'s local version: `<base>` for the first
    /// language, `<base>.<suffix>` for the others.
    pub fn stem(&self, language: &str) -> Option<String> {
        match self.local.iter().position(|l| l == language)? {
            0 => Some(self.base.clone()),
            _ => Some(format!("{}.{}", self.base, suffix(language))),
        }
    }

    /// The file of `language`'s version (it may not exist yet): `None` for a
    /// language the document isn't kept in, or a reading copy's intent file
    /// that wasn't found.
    pub fn path(&self, language: &str) -> Option<PathBuf> {
        if self.kind == Kind::Copy && language == "English" {
            return self.source.clone();
        }
        self.stem(language)
            .map(|s| self.dir.join(format!("{s}.md")))
    }

    fn record(&self) -> PathBuf {
        self.dir.join(RECORDS).join(format!("{}.json", self.base))
    }

    /// Whether the version in `suffix` leads: any version of a ledger or a
    /// report, only the intent file of a reading copy.
    fn leads(&self, suffix: &str) -> bool {
        self.kind != Kind::Copy || suffix == ENGLISH
    }

    /// Each language's suffix, name, file and current revision.
    fn now(&self) -> BTreeMap<String, Now> {
        self.languages()
            .into_iter()
            .map(|language| {
                let path = self.path(&language);
                let rev = path.as_deref().and_then(rev_of);
                (
                    suffix(&language),
                    Now {
                        language,
                        path,
                        rev,
                    },
                )
            })
            .collect()
    }
}

const ENGLISH: &str = "english";

struct Now {
    language: String,
    path: Option<PathBuf>,
    rev: Option<String>,
}

fn rev_of(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| super::rev(&b))
}

/// One version of a document: the file written, read or served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub family: Family,
    pub language: String,
    /// A file named like a version that isn't one: a language the project no
    /// longer keeps, or a second file for the unsuffixed one's language.
    pub orphan: bool,
}

/// The document version at `path` (any form, relative or absolute): a ledger,
/// a recap report or a reading copy, named `<name>.md` or `<name>.<language>.md`,
/// in the languages of the project it is in.
pub fn classify(path: &Path) -> Option<Doc> {
    let abs = super::absolute(path);
    let file = abs.file_name()?.to_str()?;
    if file.contains('\\') {
        return None;
    }
    let dir = abs.parent()?;
    let kind = kind_of(dir)?;
    let project = super::project_of(&abs);
    doc_in(
        kind,
        dir,
        file,
        project.as_ref().map(|(r, p)| (r.as_path(), p)),
    )
}

/// The version named `file` (`<name>[.<language>].md`) of a document of `kind`
/// in `dir`.
pub fn doc_in(
    kind: Kind,
    dir: &Path,
    file: &str,
    project: Option<(&Path, &Project)>,
) -> Option<Doc> {
    let stem = file.strip_suffix(".md")?;
    let (base, lang) = split(stem, super::is_slug)?;
    if lang.is_some_and(|s| !names_language(s, project.map(|(_, p)| p))) {
        return None;
    }
    let family = Family::new(kind, dir, base, project);
    let language = match lang {
        None => family
            .local
            .first()
            .cloned()
            .unwrap_or_else(|| "English".to_string()),
        Some(s) => family
            .languages()
            .into_iter()
            .find(|l| suffix(l) == s)
            .unwrap_or_else(|| canonical_language(s)),
    };
    let orphan = family.path(&language).as_deref() != Some(dir.join(file).as_path());
    Some(Doc {
        family,
        language,
        orphan,
    })
}

/// The kind of document a directory holds, by its last components.
fn kind_of(dir: &Path) -> Option<Kind> {
    let names: Vec<&str> = dir
        .components()
        .rev()
        .take(3)
        .map(|c| match c {
            Component::Normal(n) => n.to_str().unwrap_or(""),
            _ => "",
        })
        .collect();
    match names.as_slice() {
        ["recap", "notes", ".claude"] => Some(Kind::Recap),
        ["notes", ".claude", ..] => Some(Kind::Ledger),
        ["view", "intent", ".claude"] => Some(Kind::Copy),
        _ => None,
    }
}

/// The reading copies of intent file `rel` (project-relative), as their English
/// version: `None` when `rel` isn't an intent file of the project at `root`.
pub fn intent_doc(root: &Path, rel: &str) -> Option<Doc> {
    let project = Project::load_state(root).ok()?;
    let stem = super::intents::target(&project.claude.intent, rel)?;
    let base = stem.to_ascii_lowercase();
    if !super::is_slug(&base) {
        return None;
    }
    let dir = root.join(Kind::Copy.dir());
    let family = Family::new(Kind::Copy, &dir, &base, Some((root, &project)));
    Some(Doc {
        orphan: family.source.as_deref() != Some(root.join(rel).as_path()),
        family,
        language: "English".to_string(),
    })
}

// ---------------------------------------------------------------- records --

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Record {
    /// Each version's revision when the hook last saw it, by language suffix.
    #[serde(default)]
    seen: BTreeMap<String, String>,
    /// The versions that must follow a change in another, by language suffix.
    #[serde(default)]
    pending: BTreeMap<String, Pending>,
    /// The version that just caught up, and who wrote it: until that writer's
    /// turn ends, its further edits finish the update — they don't lead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    following: Option<Following>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Following {
    /// Its language suffix.
    lang: String,
    owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Pending {
    /// The version's revision when it fell behind; `None`: it didn't exist.
    rev: Option<String>,
    /// Who owes it: session ids, or `<session>/<agent>` for a subagent's.
    #[serde(default)]
    owners: Vec<String>,
    /// The owners whose turn was already held once for it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    held: Vec<String>,
}

impl Pending {
    /// Whether the version has changed since it fell behind: done.
    fn done(&self, now: Option<&String>) -> bool {
        now.is_some() && now != self.rev.as_ref()
    }
}

fn load(path: &Path) -> Option<Record> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Write `record` to `path`: whether it was written.
fn save(path: &Path, record: &Record) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    super::session::ignore_self(dir);
    serde_json::to_string_pretty(record)
        .is_ok_and(|json| super::write_if_changed(path, format!("{json}\n").as_bytes()).is_ok())
}

/// Who owes a version: session `session`, or its subagent `agent`. `None` for
/// an id that can't be kept.
pub fn owner(session: &str, agent: &str) -> Option<String> {
    let plain = super::session::plain_id;
    match agent {
        "" if plain(session) => Some(session.to_string()),
        a if plain(session) && plain(a) => Some(format!("{session}/{a}")),
        _ => None,
    }
}

/// Record a write of `doc`, and mark the versions that must follow it pending,
/// owed by `owner`. Returns the languages that just fell behind. A document kept
/// in one language, or a file that isn't one of its versions, records nothing.
///
/// A version that changed since the hook last saw it leads (for a reading copy,
/// only the intent file does), unless the change is the one it owed — or its
/// writer finishing that update in more than one edit. The first time a
/// document is seen, only the version written leads, and an existing version
/// falls behind only when it is older.
pub fn touch(doc: &Doc, owner: Option<&str>) -> Vec<String> {
    let f = &doc.family;
    if !f.bilingual() || doc.orphan {
        return Vec::new();
    }
    let now = f.now();
    let path = f.record();
    let fresh = load(&path);
    let first = fresh.is_none();
    let mut r = fresh.unwrap_or_default();
    // Owed versions in a language no longer kept, or changed since: done.
    let mut done = Vec::new();
    r.pending.retain(|s, p| match now.get(s) {
        None => false,
        Some(n) if p.done(n.rev.as_ref()) => {
            done.push(s.clone());
            false
        }
        Some(_) => true,
    });
    let written = suffix(&doc.language);
    if done.contains(&written) {
        r.following = Some(Following {
            lang: written.clone(),
            owner: owner.map(str::to_string),
        });
    }
    let finishing = |s: &str| {
        r.following
            .as_ref()
            .is_some_and(|f| f.lang == s && (f.owner.is_none() || f.owner.as_deref() == owner))
    };
    let changed: Vec<String> = if first {
        vec![written.clone()]
    } else {
        now.iter()
            .filter(|(s, n)| n.rev.is_some() && n.rev.as_ref() != r.seen.get(*s))
            .map(|(s, _)| s.clone())
            .collect()
    };
    let leads = changed
        .iter()
        .any(|s| !done.contains(s) && f.leads(s) && !finishing(s));
    let mut fell = Vec::new();
    if leads {
        r.following = None;
        let newest = now
            .get(&written)
            .and_then(|n| n.path.as_deref())
            .and_then(modified);
        for (s, n) in &now {
            if changed.contains(s) || (f.kind == Kind::Copy && s == ENGLISH) {
                continue;
            }
            let older = n.path.as_deref().and_then(modified) < newest;
            if first && n.rev.is_some() && !older {
                continue;
            }
            match r.pending.get_mut(s) {
                // A new debt for this owner: its turn may be held for it again.
                Some(p) => {
                    if let Some(o) = owner.filter(|o| !p.owners.iter().any(|x| x == o)) {
                        p.owners.push(o.to_string());
                        p.held.retain(|x| x != o);
                    }
                }
                None => {
                    r.pending.insert(
                        s.clone(),
                        Pending {
                            rev: n.rev.clone(),
                            owners: owner.into_iter().map(str::to_string).collect(),
                            held: Vec::new(),
                        },
                    );
                    fell.push(n.language.clone());
                }
            }
        }
    }
    r.seen = now
        .iter()
        .filter_map(|(s, n)| n.rev.clone().map(|rev| (s.clone(), rev)))
        .collect();
    save(&path, &r);
    fell
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// One version of a document, as its pages show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub language: String,
    pub path: Option<PathBuf>,
    pub exists: bool,
    /// Behind another version: owed since it changed, or left as it was when
    /// another changed without the hook seeing it (a Bash write, a hand edit).
    pub stale: bool,
}

/// Every version of `f`, in [`Family::languages`] order.
pub fn status(f: &Family) -> Vec<Version> {
    let now = f.now();
    let r = if f.bilingual() {
        load(&f.record())
    } else {
        None
    };
    // Versions that changed since the hook last saw them, and lead.
    let moved: Vec<&String> = match &r {
        Some(r) => now
            .iter()
            .filter(|(s, n)| {
                f.leads(s)
                    && r.seen.contains_key(*s)
                    && n.rev.is_some()
                    && n.rev.as_ref() != r.seen.get(*s)
                    // An owed update isn't a lead.
                    && !r.pending.get(*s).is_some_and(|p| p.done(n.rev.as_ref()))
            })
            .map(|(s, _)| s)
            .collect(),
        None => Vec::new(),
    };
    now.iter()
        .map(|(s, n)| {
            let owed = r
                .as_ref()
                .and_then(|r| r.pending.get(s))
                .is_some_and(|p| !p.done(n.rev.as_ref()));
            let left = !moved.contains(&s)
                && moved.iter().any(|m| *m != s)
                && r.as_ref().is_some_and(|r| r.seen.get(s) == n.rev.as_ref());
            (
                f.languages().iter().position(|l| *l == n.language),
                Version {
                    language: n.language.clone(),
                    path: n.path.clone(),
                    exists: n.rev.is_some(),
                    stale: n.rev.is_some() && (owed || left),
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// A page's language switch — each version by its name in its own language,
/// linked when it can be — and the note for a page whose version is behind.
/// Empty for a document in one language.
#[derive(Debug, Clone, Default)]
pub struct Switch {
    pub versions: Vec<minijinja::Value>,
    pub notice: String,
}

impl Switch {
    /// The switch on the page of `f`'s version in `current`: `href` links a
    /// version's page (`None`: it can't be linked from there).
    pub fn of(
        f: &Family,
        current: &str,
        href: &dyn Fn(&Version) -> Option<String>,
        w: &super::words::Words,
    ) -> Switch {
        if !f.bilingual() {
            return Switch::default();
        }
        let all = status(f);
        let versions = all
            .iter()
            .map(|v| {
                let here = v.language == current;
                let state = if !v.exists {
                    w.versions.not_written
                } else if v.stale && !here {
                    w.versions.out_of_date
                } else {
                    ""
                };
                minijinja::context! {
                    name => super::words::endonym(&v.language),
                    lang => super::words::for_language(&v.language).lang,
                    href => (v.exists && !here).then(|| href(v)).flatten(),
                    current => here,
                    state => state,
                }
            })
            .collect();
        let behind = all.iter().any(|v| v.language == current && v.stale);
        Switch {
            versions,
            notice: if behind {
                w.versions.stale_notice.to_string()
            } else {
                String::new()
            },
        }
    }
}

/// A short revision of `f`'s versions as its pages show them — which exist,
/// which are behind — so a page served live can follow them.
pub fn fingerprint(f: &Family) -> String {
    let sig: String = status(f)
        .iter()
        .map(|v| format!("{}:{}:{};", suffix(&v.language), v.exists, v.stale))
        .collect();
    super::rev(sig.as_bytes())
}

/// A version still owed: the document, its language and its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owed {
    pub doc: Doc,
    pub path: Option<PathBuf>,
    /// The file doesn't exist yet.
    pub missing: bool,
}

/// Each document with a record in the project at `root`, with its record.
fn records(root: &Path) -> Vec<(Family, PathBuf, Record)> {
    // Without the project's languages, a record can't be read right: leave it.
    let Ok(project) = Project::load_state(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in Kind::ALL {
        let dir = root.join(kind.dir());
        let Ok(entries) = fs::read_dir(dir.join(RECORDS)) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Some(base) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|b| super::is_slug(b))
            else {
                continue;
            };
            if let Some(r) = load(&path) {
                let f = Family::new(kind, &dir, base, Some((root, &project)));
                out.push((f, path, r));
            }
        }
    }
    out
}

/// The versions `owner` still owes in the project at `root`: pending and not
/// changed since. Those done since are dropped from their records.
pub fn owed(root: &Path, owner: &str) -> Vec<Owed> {
    let mut out = Vec::new();
    for (f, path, mut r) in records(root) {
        let now = f.now();
        // A document whose every version is gone (renamed, deleted) owes nothing.
        if now.values().all(|n| n.rev.is_none()) {
            let _ = fs::remove_file(&path);
            continue;
        }
        let before = r.clone();
        r.pending
            .retain(|s, p| now.get(s).is_some_and(|n| !p.done(n.rev.as_ref())));
        if r != before {
            save(&path, &r);
        }
        for (s, p) in &r.pending {
            if !p.owners.iter().any(|o| o == owner) {
                continue;
            }
            let n = &now[s];
            out.push(Owed {
                doc: Doc {
                    family: f.clone(),
                    language: n.language.clone(),
                    orphan: false,
                },
                path: n.path.clone(),
                missing: n.rev.is_none(),
            });
        }
    }
    out
}

/// Hold `owner`'s turn for what it owes in the project at `root`, once per
/// debt: true when something it owes wasn't held for yet (and is now) — the
/// turn is held; false when all of it was — it is let go.
pub fn hold(root: &Path, owner: &str) -> bool {
    let mut held = false;
    for (f, path, mut r) in records(root) {
        let now = f.now();
        let before = r.clone();
        for (s, p) in r.pending.iter_mut() {
            let owed = p.owners.iter().any(|o| o == owner)
                && now.get(s).is_some_and(|n| !p.done(n.rev.as_ref()));
            if owed && !p.held.iter().any(|o| o == owner) {
                p.held.push(owner.to_string());
            }
        }
        // Held only once that is on disk: never twice for the same debt.
        held |= r != before && save(&path, &r);
    }
    held
}

/// `owner`'s turn ended: a version it just caught up is done being written —
/// its next edit leads again.
pub fn end_turn(root: &Path, owner: &str) {
    for (_, path, mut r) in records(root) {
        if r.following
            .as_ref()
            .is_some_and(|f| f.owner.as_deref() == Some(owner))
        {
            r.following = None;
            save(&path, &r);
        }
    }
}

/// Pass each version `from` owes in the project at `root` to `to` — `None`:
/// nobody, the version stays behind (its pages say so) but no turn is held for
/// it.
pub fn pass(root: &Path, from: &str, to: Option<&str>) {
    for (_, path, mut r) in records(root) {
        let before = r.clone();
        for p in r.pending.values_mut() {
            if !p.owners.iter().any(|o| o == from) {
                continue;
            }
            p.owners.retain(|o| o != from);
            p.held.retain(|o| o != from);
            if let Some(to) = to.filter(|t| !p.owners.iter().any(|o| o == t)) {
                p.owners.push(to.to_string());
            }
        }
        if r != before {
            save(&path, &r);
        }
    }
}

/// What [`relabel`] did: the files it renamed, and the documents it left as
/// they were because a file was in the way.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Relabeled {
    pub renamed: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<PathBuf>,
}

/// After the project at `root` changed languages (`before` → `after`), rename
/// each local document's versions so the unsuffixed file is still in the first
/// language: `flow.md` (the old answer language's) becomes `flow.<old>.md`,
/// and `flow.<new>.md` becomes `flow.md`. Their pages are rendered again under
/// the new names. A version in a language no longer kept stays as it is: never
/// listed, never asked for.
pub fn relabel(root: &Path, before: &Project, after: &Project) -> Relabeled {
    let mut out = Relabeled::default();
    for kind in Kind::ALL {
        let first = |p: &Project| match kind {
            Kind::Copy => p.copy_languages().into_iter().next(),
            _ => p.doc_languages().into_iter().next(),
        };
        let (Some(old), Some(new)) = (first(before), first(after)) else {
            continue;
        };
        if old == new {
            continue;
        }
        let (was, now) = (suffix(&old), suffix(&new));
        let dir = root.join(kind.dir());
        let bases: std::collections::BTreeSet<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.strip_suffix(".md")?.to_string();
                split(&name, super::is_slug).map(|(base, _)| base.to_string())
            })
            .collect();
        for base in bases {
            let md = |s: &str| dir.join(format!("{base}{s}.md"));
            let (plain, old_file, new_file) =
                (md(""), md(&format!(".{was}")), md(&format!(".{now}")));
            if plain.exists() && old_file.exists() {
                out.skipped.push(plain);
                continue;
            }
            let mut moved = Vec::new();
            if plain.is_file() {
                if fs::rename(&plain, &old_file).is_err() {
                    out.skipped.push(plain);
                    continue;
                }
                moved.push((plain.clone(), old_file.clone()));
            }
            // Never over a file: the first move must have made room.
            if new_file.is_file() && !plain.exists() && fs::rename(&new_file, &plain).is_ok() {
                moved.push((new_file.clone(), plain.clone()));
            }
            if moved.is_empty() {
                continue;
            }
            // The pages under the old names go; the files get theirs again.
            for s in ["".to_string(), format!(".{was}"), format!(".{now}")] {
                let _ = fs::remove_file(dir.join(format!("{base}{s}.html")));
            }
            for p in [&plain, &old_file] {
                if p.is_file() {
                    let _ = super::render_file(p);
                }
            }
            out.renamed.extend(moved);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(dir: &Path, kind: Kind, local: &[&str], source: Option<PathBuf>) -> Family {
        Family {
            kind,
            dir: dir.to_path_buf(),
            base: "flow".into(),
            local: local.iter().map(|l| l.to_string()).collect(),
            source,
        }
    }

    fn doc(f: &Family, language: &str) -> Doc {
        Doc {
            family: f.clone(),
            language: language.into(),
            orphan: false,
        }
    }

    fn write(f: &Family, language: &str, text: &str) {
        fs::write(f.path(language).unwrap(), text).unwrap();
    }

    fn stale(f: &Family) -> Vec<String> {
        status(f)
            .into_iter()
            .filter(|v| v.stale)
            .map(|v| v.language)
            .collect()
    }

    #[test]
    fn split_accepts_one_language_suffix() {
        assert_eq!(split("flow", super::super::is_slug), Some(("flow", None)));
        assert_eq!(
            split("flow.english", super::super::is_slug),
            Some(("flow", Some("english")))
        );
        assert_eq!(
            split("2026-10-06-2.brazilian-portuguese", super::super::is_slug),
            Some(("2026-10-06-2", Some("brazilian-portuguese")))
        );
        for bad in [
            "flow.English",
            "flow.",
            ".english",
            "a.b.english",
            "Bad Name.english",
            "flow.en9",
            "flow.-en",
        ] {
            assert_eq!(split(bad, super::super::is_slug), None, "{bad}");
        }
        assert_eq!(suffix("ukrainian"), "ukrainian");
        assert_eq!(suffix(" English "), "english");
    }

    #[test]
    fn names_follow_the_language_order() {
        let f = family(
            Path::new("/p/.claude/notes"),
            Kind::Ledger,
            &["Ukrainian", "English"],
            None,
        );
        assert_eq!(
            f.path("Ukrainian").unwrap(),
            Path::new("/p/.claude/notes/flow.md")
        );
        assert_eq!(
            f.path("English").unwrap(),
            Path::new("/p/.claude/notes/flow.english.md")
        );
        assert_eq!(f.path("Polish"), None);
        assert!(f.bilingual());
        assert!(!family(Path::new("/p"), Kind::Ledger, &["English"], None).bilingual());
        // A reading copy's English version is its intent file.
        let adr = PathBuf::from("/p/docs/adr/ADR-0001-flow.md");
        let c = family(
            Path::new("/p/.claude/intent/view"),
            Kind::Copy,
            &["Ukrainian"],
            Some(adr.clone()),
        );
        assert_eq!(c.languages(), ["English", "Ukrainian"]);
        assert_eq!(c.path("English"), Some(adr));
        assert_eq!(
            c.path("Ukrainian").unwrap(),
            Path::new("/p/.claude/intent/view/flow.md")
        );
    }

    #[test]
    fn kinds_are_told_by_their_directory() {
        assert_eq!(kind_of(Path::new("/p/.claude/notes")), Some(Kind::Ledger));
        assert_eq!(kind_of(Path::new(".claude/notes")), Some(Kind::Ledger));
        assert_eq!(
            kind_of(Path::new("/p/.claude/notes/recap")),
            Some(Kind::Recap)
        );
        assert_eq!(
            kind_of(Path::new("/p/.claude/intent/view")),
            Some(Kind::Copy)
        );
        for other in [
            "/p/notes",
            "/p/.claude/intent/drafts",
            "/p/x/notes/recap",
            "/p/.claude/notes/sub",
        ] {
            assert_eq!(kind_of(Path::new(other)), None, "{other}");
        }
    }

    #[test]
    fn touch_state_machine() {
        let dir = tempfile::tempdir().unwrap();
        let f = family(dir.path(), Kind::Ledger, &["Ukrainian", "English"], None);

        // The first write: the other version is owed, once.
        write(&f, "Ukrainian", "Тема: потік\n");
        assert_eq!(touch(&doc(&f, "Ukrainian"), Some("s1")), ["English"]);
        assert!(dir.path().join(".versions/.gitignore").is_file());
        // More edits of the same version add nothing new.
        write(&f, "Ukrainian", "Тема: потік\nQ1\n");
        assert_eq!(
            touch(&doc(&f, "Ukrainian"), Some("s1")),
            Vec::<String>::new()
        );
        // Writing it is what was owed: nothing leads.
        write(&f, "English", "Topic: flow\nQ1\n");
        assert!(touch(&doc(&f, "English"), Some("s1")).is_empty());
        assert!(stale(&f).is_empty());
        // A no-op edit changes nothing.
        assert!(touch(&doc(&f, "English"), Some("s1")).is_empty());
        // An edit after they were in step: the other falls behind.
        write(&f, "English", "Topic: flow\nQ1\nQ2\n");
        assert_eq!(touch(&doc(&f, "English"), Some("s2")), ["Ukrainian"]);
        assert_eq!(stale(&f), ["Ukrainian"]);
        // Its update is done whatever writes it.
        write(&f, "Ukrainian", "Тема: потік\nQ1\nQ2\n");
        assert!(stale(&f).is_empty());
        assert!(touch(&doc(&f, "Ukrainian"), Some("s2")).is_empty());
        assert!(stale(&f).is_empty());
    }

    #[test]
    fn a_document_seen_first_with_both_versions_keeps_the_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let f = family(dir.path(), Kind::Ledger, &["Ukrainian", "English"], None);
        write(&f, "English", "Topic: flow\n");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(f.path("English").unwrap())
            .unwrap()
            .set_modified(past)
            .unwrap();
        write(&f, "Ukrainian", "Тема: потік\n");
        // The English file is older than the write: it falls behind.
        assert_eq!(touch(&doc(&f, "Ukrainian"), Some("s1")), ["English"]);

        let dir = tempfile::tempdir().unwrap();
        let f = family(dir.path(), Kind::Ledger, &["Ukrainian", "English"], None);
        write(&f, "Ukrainian", "Тема: потік\n");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(f.path("Ukrainian").unwrap())
            .unwrap()
            .set_modified(past)
            .unwrap();
        write(&f, "English", "Topic: flow\n");
        // The newest is the one written: the older Ukrainian one follows it.
        assert_eq!(touch(&doc(&f, "English"), Some("s1")), ["Ukrainian"]);
    }

    #[test]
    fn status_derives_out_of_band_staleness() {
        let dir = tempfile::tempdir().unwrap();
        let f = family(dir.path(), Kind::Ledger, &["Ukrainian", "English"], None);
        write(&f, "Ukrainian", "a");
        touch(&doc(&f, "Ukrainian"), None);
        write(&f, "English", "b");
        touch(&doc(&f, "English"), None);
        assert!(stale(&f).is_empty());
        // A Bash write or a hand edit: the hook never saw it, the other is behind.
        write(&f, "Ukrainian", "a2");
        assert_eq!(stale(&f), ["English"]);
        // A version not written yet is missing, not stale.
        fs::remove_file(f.path("English").unwrap()).unwrap();
        let v = status(&f);
        assert_eq!(
            (v[1].language.as_str(), v[1].exists, v[1].stale),
            ("English", false, false)
        );
        // One language: nothing to keep in step, nothing recorded.
        let dir = tempfile::tempdir().unwrap();
        let one = family(dir.path(), Kind::Ledger, &["English"], None);
        write(&one, "English", "a");
        assert!(touch(&doc(&one, "English"), Some("s1")).is_empty());
        assert!(!dir.path().join(RECORDS).exists());
    }

    #[test]
    fn copy_family_only_the_intent_file_leads() {
        let dir = tempfile::tempdir().unwrap();
        let adr = dir.path().join("ADR-0001-flow.md");
        let view = dir.path().join("view");
        fs::create_dir_all(&view).unwrap();
        let f = family(&view, Kind::Copy, &["Ukrainian"], Some(adr.clone()));
        fs::write(&adr, "# ADR-0001: Flow\n").unwrap();
        assert_eq!(touch(&doc(&f, "English"), Some("s1")), ["Ukrainian"]);
        write(&f, "Ukrainian", "# ADR-0001: Потік\n");
        assert!(touch(&doc(&f, "Ukrainian"), Some("s1")).is_empty());
        // A fix in the copy alone asks nothing of the intent file.
        write(&f, "Ukrainian", "# ADR-0001: Потік (виправлено)\n");
        assert!(touch(&doc(&f, "Ukrainian"), Some("s1")).is_empty());
        assert!(stale(&f).is_empty());
        // A change in the intent file puts the copy behind.
        fs::write(&adr, "# ADR-0001: Flow, v2\n").unwrap();
        assert_eq!(stale(&f), ["Ukrainian"]);
        assert_eq!(touch(&doc(&f, "English"), Some("s1")), ["Ukrainian"]);
    }

    /// Handing a debt between owners needs a project (its languages): see
    /// tests/doc_versions.rs. Here: which ids can own one.
    #[test]
    fn owners_are_plain_ids() {
        assert_eq!(owner("s1", ""), Some("s1".into()));
        assert_eq!(owner("s1", "a1"), Some("s1/a1".into()));
        assert_eq!(owner("s/1", ""), None);
        assert_eq!(owner("s1", "a/1"), None);
    }
}
