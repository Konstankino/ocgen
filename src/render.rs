//! Turning a [`Project`] into concrete files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::claude::{ClaudeConfig, McpServer, PermissionRules, RuleList, Skill};
use crate::manifest::{Manifest, Provider};
use crate::target::Target;
use crate::templates;

/// The state file's schema. Bump it when a field changes meaning, so an older
/// ocgen refuses to regenerate a project instead of misreading its state.
pub const STATE_SCHEMA: u32 = 1;

/// Everything needed to render a scaffold. Serializable so it can be saved to
/// the target's state file (e.g. `.opencode/.ocgen-state.json`) and reloaded by
/// `ocgen add agent`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)] // tolerate state files from older versions (see migrate_state)
pub struct Project {
    /// The ocgen that last wrote this state (empty: one from before it was recorded).
    pub ocgen_version: String,
    /// [`STATE_SCHEMA`] of that ocgen (0: from before it was recorded).
    pub schema: u32,
    /// Which platform this project targets. Defaults to OpenCode for old state files.
    pub target: Target,
    pub project_name: String,
    /// The instruction language: the agents' prompts and descriptions.
    pub language: String,
    /// The answer language: what the user reads. Empty means the same as
    /// `language` (every project from before it existed) — see
    /// [`Project::response_language`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub response_language: String,
    pub providers: Vec<Provider>,
    /// Provider + model for the built-in compaction/title/summary agents.
    pub utility_provider: String,
    pub utility_model: String,
    /// Fallback model id (under `utility_provider`) when no agent is `primary`.
    pub default_primary_model: String,
    pub agents: Vec<Agent>,
    /// Claude Code–only settings (ignored for the OpenCode target).
    pub claude: ClaudeConfig,
    /// Claude Code skills to emit (Claude target only).
    pub skills: Vec<Skill>,
    /// Fingerprint of every file the last scaffold wrote (relative path → hash), so
    /// `doctor` can tell a hand edit from an ocgen change.
    pub generated: BTreeMap<String, String>,
    /// The permission rules the last scaffold generated (`None`: not recorded), so
    /// a rule ocgen stopped generating is never mistaken for one added by hand.
    pub generated_rules: Option<PermissionRules>,
    /// Keys this ocgen doesn't know (written by a newer one), kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Outcome of [`Project::install_pre_push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrePush {
    /// ocgen's hook was written (or refreshed) at this path.
    Installed(PathBuf),
    /// Another pre-push hook (or a custom `core.hooksPath`) is in place; it was
    /// left alone. Chain `.claude/hooks/git-pre-push.sh` from it ([`pre_push_chain`])
    /// to get the gate.
    Foreign(PathBuf),
    /// Not a git repository, not a Claude project, or the approval gate is off.
    NotApplicable,
}

/// The project's half of the git pre-push gate. Someone else's pre-push hook may
/// chain it ([`pre_push_chain`]), so once generated it is never removed: with the
/// gate off it lets every push through.
const PRE_PUSH_SCRIPT: &str = ".claude/hooks/git-pre-push.sh";

/// The line to add to another pre-push hook (husky, lefthook, your own) to chain
/// the gate of the project at `root`: it runs the project's script, and passes
/// when there is none. git runs hooks from the repository's top, so a project in
/// a subfolder is named by its path from there.
pub fn pre_push_chain(root: &Path) -> String {
    let prefix = std::process::Command::new("git")
        .arg("-C")
        .arg(crate::paths::plain(root))
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let script = format!("{prefix}{PRE_PUSH_SCRIPT}");
    let plain = script
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
    let script = if plain {
        script
    } else {
        format!("'{}'", script.replace('\'', "'\\''"))
    };
    format!("[ ! -f {script} ] || sh {script} || exit 1")
}

/// The role (archetype) whose subagent turns on the coordinator's adversary loop:
/// run it last, send its serious findings back to the implementer, re-check.
pub const ADVERSARY_ROLE: &str = "adversary";

/// Rework rounds the adversary loop allows before the rest is reported UNRESOLVED.
pub const ADVERSARY_ROUNDS: u32 = 2;

/// The answer lines the coordinator presets end with. When the answers are in
/// another language, the one ending a coordinator's text gives way to ocgen's.
const RESPONSE_LINES: [&str; 2] = ["Respond in English.", "Відповідай українською."];

/// `text` without the answer line it ends with (its trailing newlines kept), or
/// `None` when it doesn't end with one.
fn without_response_line(text: &str) -> Option<String> {
    let head = text.trim_end_matches('\n');
    let tail = &text[head.len()..];
    let (rest, last) = match head.rfind('\n') {
        Some(i) => (&head[..i], &head[i + 1..]),
        None => ("", head),
    };
    RESPONSE_LINES
        .contains(&last.trim())
        .then(|| format!("{}{tail}", rest.trim_end_matches('\n')))
}

/// A language name as the templates compare it: `ukrainian` → `Ukrainian`;
/// empty → `English`.
pub fn canonical_language(name: &str) -> String {
    let name = name.trim();
    let mut chars = name.chars();
    match chars.next() {
        None => "English".to_string(),
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
    }
}

/// `extra` after `base`, a blank line between them; `base`'s trailing newlines
/// end the result, so the text sits in its template exactly as `base` did.
fn join_section(base: &str, extra: &str) -> String {
    let head = base.trim_end_matches('\n');
    if head.is_empty() {
        return extra.to_string();
    }
    let tail = &base[head.len()..];
    format!("{head}\n\n{}{tail}", extra.trim_end_matches('\n'))
}

/// User-owned files: ocgen creates them once (with its default) and never
/// overwrites them, so they're exempt from conflicts, doctor's plan and fingerprints.
pub const USER_OWNED: [&str; 3] = [
    "CLAUDE.md",
    crate::claude::INTENT_ISSUE_TEMPLATE,
    crate::claude::INTENT_FILE_TEMPLATE,
];

fn user_owned(rel: &Path) -> bool {
    let rel = rel.to_string_lossy().replace('\\', "/");
    USER_OWNED.contains(&rel.as_str())
}

/// Opens ocgen's block in a `.gitattributes`. ocgen adds the block once and never
/// rewrites the file: the rest of it (and the block, once there) is the user's.
const GITATTRIBUTES_MARK: &str =
    "# ocgen: generated files keep LF line endings (CRLF breaks the sh hooks)";

/// The project's block: the trees ocgen generates (`text=auto`, so binaries
/// stay untouched).
const GITATTRIBUTES_PROJECT: &str = "\
# ocgen: generated files keep LF line endings (CRLF breaks the sh hooks)
.claude/** text=auto eol=lf
.mcp.json text=auto eol=lf
.worktreeinclude text=auto eol=lf
# ocgen: end
";

/// The plugin tree's block: it is published as a repository of its own.
const GITATTRIBUTES_PLUGIN: &str = "\
# ocgen: generated files keep LF line endings (CRLF breaks the sh hooks)
* text=auto eol=lf
# ocgen: end
";

/// `existing` with ocgen's `block` appended, or `None` when it already has it.
fn with_gitattributes_block(existing: Option<&str>, block: &str) -> Option<String> {
    match existing {
        Some(s) if s.contains(GITATTRIBUTES_MARK) => None,
        Some(s) if s.trim().is_empty() => Some(block.to_string()),
        Some(s) => {
            let sep = if s.ends_with('\n') { "" } else { "\n" };
            Some(format!("{s}{sep}\n{block}"))
        }
        None => Some(block.to_string()),
    }
}

/// Things on disk that ocgen didn't write and that a regeneration would drop
/// unless they're adopted as the user's own: permission rules in `settings.json`
/// and MCP servers in `.mcp.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HandAdded {
    /// Valid rules, by list, that could be adopted as the user's own.
    pub rules: PermissionRules,
    /// Entries that aren't valid permission rules (never adopted).
    pub invalid: Vec<String>,
    /// MCP servers in `.mcp.json` that neither this project nor the last write had.
    pub servers: Vec<McpServer>,
    /// Hand-added servers whose name ocgen can't use (never adopted).
    pub invalid_servers: Vec<String>,
    /// `settings.json` / `.mcp.json` when it is a symbolic link (relative paths):
    /// nothing is adopted from it, as ocgen never writes through it.
    pub linked: Vec<String>,
}

impl HandAdded {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
            && self.invalid.is_empty()
            && self.servers.is_empty()
            && self.invalid_servers.is_empty()
    }
}

/// What [`Project::apply`] did, for the short report a regeneration prints.
#[derive(Debug, Clone, Default)]
pub struct Applied {
    /// Every file written.
    pub written: Vec<PathBuf>,
    /// Where the previous versions of the overwritten and removed files went.
    pub backup: Option<PathBuf>,
    /// Files removed because ocgen no longer generates them (relative paths).
    pub removed: Vec<String>,
    /// Overwritten or removed files that had been edited by hand (relative paths).
    pub hand_edited: Vec<String>,
    /// What was found added by hand and adopted (or dropped, when invalid).
    pub adopted: HandAdded,
    /// Adopted rules settings.json leaves out, as (list, rule, the generated list
    /// that holds it): a looser copy of a generated rule has no effect.
    pub no_effect: Vec<(RuleList, String, RuleList)>,
    /// Hand-added things that couldn't be adopted, and why.
    pub problems: Vec<String>,
}

impl Applied {
    /// The report, one line each (empty when there's nothing to say).
    pub fn notice(&self, target: &Path) -> Vec<String> {
        let mut lines = Vec::new();
        let a = &self.adopted;
        let rules: Vec<String> = a
            .rules
            .entries()
            .into_iter()
            .filter(|(list, r)| !self.no_effect.iter().any(|(l, n, _)| l == list && n == r))
            .map(|(_, r)| r)
            .collect();
        if !rules.is_empty() {
            lines.push(format!(
                "kept {} permission rule(s) added to settings.json by hand as yours: {}",
                rules.len(),
                rules.join(", ")
            ));
        }
        for (list, rule, by) in &self.no_effect {
            lines.push(no_effect_note(*list, rule, *by));
        }
        lines.extend(a.linked.iter().map(|rel| linked_note(rel)));
        if !a.servers.is_empty() {
            let names: Vec<&str> = a.servers.iter().map(|s| s.name.as_str()).collect();
            lines.push(format!(
                "kept MCP server(s) added to .mcp.json by hand as yours: {}",
                names.join(", ")
            ));
        }
        for bad in &a.invalid {
            lines.push(format!(
                "dropped {bad} from settings.json: not a valid permission rule"
            ));
        }
        for bad in &a.invalid_servers {
            lines.push(format!(
                "dropped MCP server '{bad}' from .mcp.json: ocgen can't use that name (letters, digits, '-', '_')"
            ));
        }
        lines.extend(self.problems.iter().cloned());
        if !self.removed.is_empty() {
            lines.push(format!(
                "removed (no longer generated): {}",
                self.removed.join(", ")
            ));
        }
        if let Some(b) = &self.backup {
            // Forward slashes on every platform (it's shown, and matched by tests).
            let shown = crate::paths::for_shell(b.strip_prefix(target).unwrap_or(b));
            lines.push(format!(
                "backup: {shown}/ (the previous versions; copy a file back to restore it)"
            ));
        }
        if !self.hand_edited.is_empty() {
            lines.push(format!(
                "edited by hand, so their old versions are in the backup: {}",
                self.hand_edited.join(", ")
            ));
        }
        lines
    }
}

/// The report line for an adopted `rule` in `list` that settings.json leaves out
/// because ocgen's stricter list `by` holds it.
pub fn no_effect_note(list: RuleList, rule: &str, by: RuleList) -> String {
    format!(
        "{rule} ({}) is kept in your rules but has no effect: ocgen's {} rule wins",
        list.key(),
        by.key()
    )
}

/// The report line for a `settings.json` / `.mcp.json` that is a symbolic link.
pub fn linked_note(rel: &str) -> String {
    format!("{rel} is a symbolic link: ocgen adopts nothing from it, and never writes through it")
}

/// What the last write recorded, read from the state file on disk: the project
/// in memory may already hold the user's changes.
struct Previous {
    /// The ocgen that wrote it, and its state schema.
    version: String,
    schema: u32,
    generated: BTreeMap<String, String>,
    /// The permission rules ocgen generated (`None`: not recorded).
    rules: Option<PermissionRules>,
    /// The user's own permission rules.
    user_rules: PermissionRules,
    /// The MCP servers the project had.
    servers: Vec<String>,
    /// The CODEOWNERS it was linked to (empty: none).
    codeowners: String,
}

/// A file ocgen generated before and no longer does.
struct Stale {
    path: PathBuf,
    rel: String,
    hand_edited: Option<bool>,
}

/// A relative path as the state and the plan spell it (forward slashes).
fn rel_str(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// What `doctor` would do to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
    Unchanged,
}

/// One entry of [`Project::plan_changes`].
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Path relative to the project root (as written).
    pub rel: String,
    pub kind: ChangeKind,
    /// Current content on disk (for modified/removed files).
    pub old: Option<String>,
    /// Content ocgen would write (for added/modified files).
    pub new: Option<String>,
    /// For a modified file: `Some(true)` = changed by hand since ocgen wrote it,
    /// `Some(false)` = untouched (the change is ocgen's), `None` = unknown (the
    /// project predates fingerprints).
    pub hand_edited: Option<bool>,
}

/// FNV-1a 64-bit fingerprint of file content (stable across Rust versions), over
/// LF line endings so a CRLF checkout (git's `autocrlf` on Windows) isn't an edit.
fn fingerprint(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in lf(content).as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `s` with CRLF line endings turned into LF.
fn lf(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains("\r\n") {
        s.replace("\r\n", "\n").into()
    } else {
        s.into()
    }
}

/// A path relative to the project that stays inside it: plain names only, no
/// `..`, root or drive.
fn plain_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Why ocgen won't write a file: it could reach outside the project. The message
/// says what to change — `ocgen doctor` can't, it refuses the same way.
#[derive(Debug)]
pub struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// Where a linked folder came from, as far as git can tell.
#[derive(Debug, Clone)]
enum Origin {
    /// No repository tracks it (or there's none, or no git): the user made it.
    Made,
    /// The repository holding its folder tracks it — the project's own, or one
    /// checked out inside it (a submodule) — with that repository's work tree,
    /// resolved (`None`: unknown).
    Tracked(Option<PathBuf>),
}

/// Where ocgen may write in a project. One per regeneration: it remembers where
/// each linked folder came from.
struct Fence<'a> {
    target: &'a Path,
    /// `target`, resolved (`None`: it doesn't exist yet, so nothing in it does).
    root: Option<PathBuf>,
    /// The linked folders met so far (relative paths), and where each came from.
    origins: std::cell::RefCell<BTreeMap<PathBuf, Origin>>,
}

impl<'a> Fence<'a> {
    fn new(target: &'a Path) -> Self {
        Fence {
            target,
            root: target.canonicalize().ok(),
            origins: Default::default(),
        }
    }

    /// `target/rel`, refused when writing it could reach outside the project:
    /// `rel` isn't plain, the file is a symbolic link, or a folder on the way is
    /// a link out of the project that a repository brought in (git tracks it, in
    /// the project's repository or one inside it, like a submodule) and that
    /// leads out of that repository — it may point anywhere (into `~/.claude`,
    /// say). A linked folder the user made (git doesn't track it, or there's no
    /// repository) is theirs to put where they like, and so is everything past
    /// it: `.claude` kept in a dotfiles store, say. A tracked link that stays in
    /// its repository (a monorepo's shared folder) is fine too.
    fn path(&self, rel: &Path) -> Result<PathBuf> {
        self.check(rel, false)
    }

    /// Like [`Fence::path`], but through no link out of the project at all: for
    /// files ocgen takes to be its own only by their content (an older ocgen's),
    /// which a linked folder may share with other projects.
    fn strict(&self, rel: &Path) -> Result<PathBuf> {
        self.check(rel, true)
    }

    fn check(&self, rel: &Path, strict: bool) -> Result<PathBuf> {
        let refuse = |why: String| -> Result<PathBuf> { Err(Refused(why).into()) };
        if !plain_rel(rel) {
            return refuse(format!(
                "refusing to write {}: not a plain path inside the project",
                rel.display()
            ));
        }
        let path = self.target.join(rel);
        if is_symlink(&path) {
            return refuse(format!(
                "refusing to write {}: it is a symbolic link (ocgen writes only real files inside the project) — replace it with a real file",
                path.display()
            ));
        }
        let Some(root) = &self.root else {
            return Ok(path); // nothing exists yet, so nothing can lead outside
        };
        // Each linked folder on the way out of the project.
        let mut linked_out = false;
        let mut dir = PathBuf::new();
        for c in rel.parent().into_iter().flat_map(Path::components) {
            dir.push(c);
            let abs = self.target.join(&dir);
            if !is_symlink(&abs) {
                continue;
            }
            // Dangling: nothing is written through it (making the folder fails).
            let Ok(real) = abs.canonicalize() else {
                continue;
            };
            if real.starts_with(root) {
                continue;
            }
            if strict {
                return refuse(format!(
                    "refusing to write {}: {} leads outside the project",
                    path.display(),
                    abs.display()
                ));
            }
            match self.origin(&dir) {
                // The user's link: what lies past it is theirs.
                Origin::Made => return Ok(path),
                Origin::Tracked(Some(top)) if real.starts_with(&top) => linked_out = true,
                Origin::Tracked(_) => {
                    let link = rel_str(&dir);
                    return refuse(format!(
                        "refusing to write {}: {link} is a symbolic link git tracks, and it leads out of its repository (to {}) — a link a repository brings in may point anywhere, so ocgen won't write through it: replace it with a real folder, or, if you made it and want ocgen's files there, untrack it (`git rm --cached {link}`, in the repository that holds it)",
                        path.display(),
                        real.display()
                    ));
                }
            }
        }
        if linked_out {
            return Ok(path);
        }
        // No link on the way leads out: nothing on the way may resolve outside.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Ok(real) = d.canonicalize() {
                if !real.starts_with(root) {
                    return refuse(format!(
                        "refusing to write {}: {} leads outside the project",
                        path.display(),
                        d.display()
                    ));
                }
                break;
            }
            dir = d.parent();
        }
        Ok(path)
    }

    /// Where the link at `rel` came from. Git is asked from the link's own folder,
    /// so a repository checked out inside the project (a submodule) answers for
    /// its links, not the project's, which doesn't track them.
    fn origin(&self, rel: &Path) -> Origin {
        if let Some(o) = self.origins.borrow().get(rel) {
            return o.clone();
        }
        let folder = self.target.join(rel.parent().unwrap_or(Path::new("")));
        let name = rel
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&folder)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| o.stdout)
        };
        let tracked = git(&[
            "--literal-pathspecs",
            "ls-files",
            "--stage",
            "-z",
            "--",
            &name,
        ])
        .is_some_and(|out| {
            // `<mode> <object> <stage>\t<path>`; 120000 is a symbolic link.
            out.split(|b| *b == 0).any(|e| {
                e.starts_with(b"120000 ")
                    && e.iter()
                        .position(|b| *b == b'\t')
                        .is_some_and(|tab| &e[tab + 1..] == name.as_bytes())
            })
        });
        let origin = if tracked {
            let top = git(&["rev-parse", "--show-toplevel"]).and_then(|out| {
                let top = String::from_utf8_lossy(&out);
                Path::new(top.trim_end_matches(['\n', '\r']))
                    .canonicalize()
                    .ok()
            });
            Origin::Tracked(top)
        } else {
            Origin::Made
        };
        self.origins
            .borrow_mut()
            .insert(rel.to_path_buf(), origin.clone());
        origin
    }
}

/// Whether `a` and `b` name the same file — e.g. `Reviewer.md` and `reviewer.md`
/// on a case-insensitive file system (macOS and Windows defaults).
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // canonicalize spells each name the way it is stored on disk.
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }
}

/// Whether `path`'s folder holds an entry spelled exactly like its file name (a
/// case-insensitive file system also opens `Reviewer.md` as `reviewer.md`).
fn has_entry(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    fs::read_dir(dir).is_ok_and(|d| d.filter_map(|e| e.ok()).any(|e| e.file_name() == name))
}

/// `major.minor.patch` of a version string (a pre-release suffix is ignored).
fn semver(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

/// Remove `dir` and each parent that is left empty, up to (not including) `stop`.
fn remove_empty_dirs(mut dir: Option<&Path>, stop: &Path) {
    while let Some(d) = dir {
        if d == stop || !d.starts_with(stop) {
            break;
        }
        if fs::read_dir(d).map_or(true, |mut e| e.next().is_some()) {
            break;
        }
        if fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// An MCP server as `.mcp.json` describes it; fields ocgen doesn't model are kept
/// in `extra` so the server is written back unchanged.
fn mcp_server_from_json(name: &str, v: &Value) -> McpServer {
    let mut s = McpServer {
        name: name.to_string(),
        ..Default::default()
    };
    let Some(obj) = v.as_object() else {
        return s;
    };
    let strings = |v: &Value| -> Option<Vec<String>> {
        v.as_array()?
            .iter()
            .map(|x| x.as_str().map(String::from))
            .collect()
    };
    let string_map = |v: &Value| -> Option<BTreeMap<String, String>> {
        v.as_object()?
            .iter()
            .map(|(k, x)| x.as_str().map(|x| (k.clone(), x.to_string())))
            .collect()
    };
    for (k, val) in obj {
        let taken = match k.as_str() {
            "type" => val.as_str().map(|t| s.transport = t.to_string()).is_some(),
            "command" => val.as_str().map(|c| s.command = c.to_string()).is_some(),
            "url" => val.as_str().map(|u| s.url = u.to_string()).is_some(),
            "args" => strings(val).map(|a| s.args = a).is_some(),
            "env" => string_map(val).map(|e| s.env = e).is_some(),
            "headers" => string_map(val).map(|h| s.headers = h).is_some(),
            _ => false,
        };
        if !taken {
            s.extra.insert(k.clone(), val.clone());
        }
    }
    if s.transport.is_empty() {
        // Claude Code's default: a command means a local (stdio) server.
        s.transport = if s.url.is_empty() { "stdio" } else { "http" }.into();
    }
    // Fields the transport doesn't write back stay as they were.
    let moved: &[&str] = if s.transport == "stdio" {
        &["url", "headers"]
    } else {
        &["command", "args", "env"]
    };
    for k in moved {
        if let Some(v) = obj.get(*k) {
            s.extra.insert(k.to_string(), v.clone());
        }
    }
    s
}

/// UTC `YYYYMMDD-HHMMSS` for backup folder names.
fn utc_stamp() -> String {
    crate::clock::compact_stamp()
}

/// Subagent view passed to prompt/command templates.
#[derive(Serialize)]
struct SubCtx {
    name: String,
    description: String,
}

/// Probe order for locating a project's state file (target-agnostic discovery).
fn state_file_in(dir: &Path) -> Option<PathBuf> {
    for sf in Target::state_files() {
        let p = dir.join(sf);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// YAML-safe colour: hex values need quoting so `#` isn't read as a comment.
fn color_yaml(color: &str) -> String {
    if color.starts_with('#') {
        format!("\"{color}\"")
    } else {
        color.to_string()
    }
}

impl Project {
    /// Seed provider/model fields from the manifest; agents are filled in by the caller.
    pub fn from_manifest(manifest: &Manifest, language: &str) -> Self {
        Project {
            project_name: String::new(),
            language: language.to_string(),
            providers: manifest.providers.clone(),
            utility_provider: manifest.defaults.utility_provider.clone(),
            utility_model: manifest.defaults.utility_model.clone(),
            default_primary_model: manifest.defaults.primary_model.clone(),
            agents: Vec::new(),
            ..Default::default()
        }
    }

    /// The language agents answer the user in: `response_language`, else the
    /// instruction language, else English.
    pub fn response_language(&self) -> &str {
        [&self.response_language, &self.language]
            .into_iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("English")
    }

    /// The project's language(s) for display: one name when the answers are in
    /// the instruction language, else both.
    pub fn languages_label(&self) -> String {
        if self.answers_differ() {
            format!(
                "prompts {} · answers {}",
                canonical_language(&self.language),
                canonical_language(self.response_language())
            )
        } else {
            self.language.clone()
        }
    }

    /// Whether the answers are in a language other than English: then /intent
    /// adds a translated reading copy of each (English) intent file.
    pub fn writes_reading_copies(&self) -> bool {
        self.claude.workflow.intent && !self.response_language().eq_ignore_ascii_case("English")
    }

    /// Whether the notes hook runs: it renders /inquire ledgers and /intent
    /// reading copies as pages.
    pub fn renders_notes(&self) -> bool {
        self.claude.workflow.inquire || self.writes_reading_copies()
    }

    /// Whether the answers are in another language than the instructions: only
    /// then does the coordinator get an answer line.
    pub fn answers_differ(&self) -> bool {
        let prompts = match self.language.trim() {
            "" => "English",
            l => l,
        };
        !self.response_language().eq_ignore_ascii_case(prompts)
    }

    fn subagents(&self) -> Vec<SubCtx> {
        self.agents
            .iter()
            .filter(|a| a.mode == "subagent")
            .map(|a| SubCtx {
                name: a.name.clone(),
                description: a.description.clone(),
            })
            .collect()
    }

    fn primary(&self) -> Option<&Agent> {
        self.agents.iter().find(|a| a.mode == "primary")
    }

    /// The enabled subagent playing `role`: by its role first, so a renamed one
    /// still counts, then by name.
    fn subagent_for(&self, role: &str) -> Option<&Agent> {
        let subs = || {
            self.agents
                .iter()
                .filter(|a| a.mode == "subagent" && !a.disable)
        };
        subs()
            .find(|a| a.role == role)
            .or_else(|| subs().find(|a| a.name == role))
    }

    /// Who the adversary steps in /deliver and /intent name: the adversary (empty
    /// without one), the implementer it sends findings to (may be empty), whether
    /// that implementer works in a worktree, and the rework rounds allowed.
    fn adversary_names(&self) -> (String, String, bool, u32) {
        let Some(adversary) = self.subagent_for(ADVERSARY_ROLE) else {
            return (String::new(), String::new(), false, ADVERSARY_ROUNDS);
        };
        let implementer = self.subagent_for("implementer");
        (
            adversary.name.clone(),
            implementer.map(|a| a.name.clone()).unwrap_or_default(),
            implementer.is_some_and(|a| a.isolation.trim() == "worktree"),
            ADVERSARY_ROUNDS,
        )
    }

    /// The coordinator's adversary loop, when the team has an adversary. Only
    /// then is its template loaded, so other projects render exactly as before.
    fn adversary_loop(&self, env: &Environment) -> Result<Option<String>> {
        let Some(adversary) = self.subagent_for(ADVERSARY_ROLE) else {
            return Ok(None);
        };
        let implementer = self.subagent_for("implementer");
        // A stale override missing a variable fails instead of printing nothing.
        let mut env = env.clone();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        let text = env
            .render_str(
                &templates::load("coordination/adversary.md.j2")?,
                context! {
                    adversary => adversary.name,
                    implementer => implementer.map(|a| a.name.as_str()).unwrap_or_default(),
                    implementer_isolated => implementer
                        .is_some_and(|a| a.isolation.trim() == "worktree"),
                    rounds => ADVERSARY_ROUNDS,
                    language => self.language,
                },
            )
            .context("rendering the adversary loop")?;
        Ok(Some(text))
    }

    /// The coordinator's answer line, when the answers are in another language
    /// than the instructions (written in the instruction language).
    fn response_section(&self, env: &Environment) -> Result<Option<String>> {
        if !self.answers_differ() {
            return Ok(None);
        }
        let mut env = env.clone();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        let text = env
            .render_str(
                &templates::load("coordination/response.md.j2")?,
                context! {
                    language => canonical_language(&self.language),
                    response_language => canonical_language(self.response_language()),
                },
            )
            .context("rendering the answer line")?;
        Ok(Some(text))
    }

    /// A coordinator's text as written: its own, its answer line replaced when
    /// the answers are in another language, then the adversary loop. Unchanged
    /// for a project that has neither.
    fn coordinator_text(&self, text: &str, env: &Environment) -> Result<String> {
        let mut out = text.to_string();
        if let Some(answer) = self.response_section(env)? {
            let own = without_response_line(&out).unwrap_or(out);
            out = join_section(&own, &answer);
        }
        if let Some(l) = self.adversary_loop(env)? {
            out = join_section(&out, &l);
        }
        Ok(out)
    }

    /// A coordinator that still says to answer in another language before its
    /// last line, where it would contradict the answer line ocgen adds.
    fn language_issues(&self) -> Vec<String> {
        if !self.answers_differ() {
            return Vec::new();
        }
        self.agents
            .iter()
            .filter(|a| a.mode == "primary")
            .filter_map(|a| {
                let text = match (&a.prompt_body, a.prompt_file) {
                    (Some(p), true) if self.target == Target::OpenCode => p,
                    _ => &a.body,
                };
                let stray = RESPONSE_LINES
                    .into_iter()
                    .find(|l| text.lines().any(|x| x.trim() == *l))?;
                without_response_line(text).is_none().then(|| {
                    format!(
                        "agent '{}' says \"{stray}\" before its last line, which contradicts \
                         answering in {} — remove it (`ocgen edit agent {}`)",
                        a.name,
                        self.response_language(),
                        a.name
                    )
                })
            })
            .collect()
    }

    /// Render every output file as (relative path, contents). Pure — no disk I/O.
    pub fn render_all(&self) -> Result<Vec<(PathBuf, String)>> {
        match self.target {
            Target::OpenCode => self.render_opencode(),
            Target::ClaudeCode => self.render_claude(),
        }
    }

    /// OpenCode output: `opencode.json` plus the `.opencode/` tree.
    fn render_opencode(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut env = Environment::new();
        env.set_keep_trailing_newline(true);
        let lang = &self.language;
        let subs = self.subagents();
        let mut out = Vec::new();

        // opencode.json — top-level model + utility agents are provider-qualified.
        let primary_ref = match self.primary() {
            Some(a) => format!("{}/{}", a.provider, a.model),
            None => format!("{}/{}", self.utility_provider, self.default_primary_model),
        };
        let utility_ref = format!("{}/{}", self.utility_provider, self.utility_model);
        let json = env
            .render_str(
                &templates::load("opencode.json.j2")?,
                context! {
                    primary_ref => primary_ref,
                    providers => self.providers,
                    utility_ref => utility_ref,
                },
            )
            .context("rendering opencode.json")?;
        serde_json::from_str::<serde_json::Value>(&json)
            .context("rendered opencode.json is not valid JSON (check your template)")?;
        out.push((PathBuf::from("opencode.json"), json));

        // Per-agent files (+ external prompt files).
        let agent_tmpl = templates::load("opencode/agents/_agent.md.j2")?;
        for agent in &self.agents {
            // What ocgen adds to a coordinator goes at the end of its prompt file,
            // or of its body when it has none.
            let primary = agent.mode == "primary";
            let has_prompt_file = agent.prompt_file && agent.prompt_body.is_some();
            let body = if primary && !has_prompt_file {
                self.coordinator_text(&agent.body, &env)?
            } else {
                agent.body.clone()
            };
            let mut permissions = agent.permissions.trim_end().to_string();
            if agent.mode == "primary" {
                permissions.push_str("\n  task:\n    \"*\": deny");
                for sub in &subs {
                    permissions.push_str(&format!("\n    \"{}\": allow", sub.name));
                }
            }

            let agent_val = context! {
                name => agent.name,
                description => agent.description,
                mode => agent.mode,
                provider => agent.provider,
                model => agent.model,
                variant => agent.variant,
                temperature => agent.temperature,
                top_p => agent.top_p,
                steps => agent.steps,
                color_yaml => color_yaml(&agent.color),
                disable => agent.disable,
                hidden => agent.hidden,
                options => agent.options.trim_end(),
                prompt_file => agent.prompt_file,
                permissions => permissions,
                body => body,
            };
            let md = env
                .render_str(&agent_tmpl, context! { agent => agent_val })
                .with_context(|| format!("rendering agent '{}'", agent.name))?;
            out.push((
                PathBuf::from(format!(".opencode/agents/{}.md", agent.name)),
                md,
            ));

            if agent.prompt_file {
                if let Some(prompt_src) = &agent.prompt_body {
                    let mut txt = body_as_template(
                        &env,
                        &agent.name,
                        prompt_src,
                        context! { subagents => &subs, language => lang, parallel => false },
                    );
                    if primary {
                        txt = self.coordinator_text(&txt, &env)?;
                    }
                    out.push((
                        PathBuf::from(format!(".opencode/prompts/{}.txt", agent.name)),
                        txt,
                    ));
                }
            }
        }

        // multi command — only meaningful with a coordinating primary agent.
        if let Some(primary) = self.primary() {
            let cmd = env
                .render_str(
                    &templates::load("opencode/commands/multi.md.j2")?,
                    context! {
                        primary => context! { name => primary.name },
                        subagents => &subs,
                        language => lang,
                    },
                )
                .context("rendering multi command")?;
            out.push((PathBuf::from(".opencode/commands/multi.md"), cmd));
        }

        Ok(out)
    }

    /// Claude Code output: `.claude/` agents + `/multi` command + settings.json,
    /// and `CLAUDE.md` (project instructions + roster + coordinator body).
    fn render_claude(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut env = Environment::new();
        env.set_keep_trailing_newline(true);
        let lang = &self.language;

        let subs: Vec<SubCtx> = self
            .agents
            .iter()
            .filter(|a| a.mode != "primary")
            .map(|a| SubCtx {
                name: a.name.clone(),
                description: a.description.clone(),
            })
            .collect();

        // Shared components as (path within a .claude/ or plugin tree, contents).
        let mut components: Vec<(String, String)> = Vec::new();

        let agent_tmpl = templates::load("claude/agent.md.j2")?;
        for agent in self.agents.iter().filter(|a| a.mode != "primary") {
            let model = if agent.model.trim().is_empty() {
                "opus"
            } else {
                agent.model.trim()
            };
            // An explicit tools allow-list must also name each MCP server the agent
            // uses, or its tools stay unavailable.
            let servers = crate::claude::split_list(&agent.mcp_servers);
            let mut tools = agent.tools.trim().to_string();
            if !tools.is_empty() {
                for s in &servers {
                    let t = format!("mcp__{s}");
                    if !crate::claude::split_list(&tools).contains(&t) {
                        tools = format!("{tools}, {t}");
                    }
                }
            }
            let agent_val = context! {
                name => agent.name,
                description => agent.description,
                tools => tools,
                model => model,
                color => agent.color,
                isolation => agent.isolation.trim(),
                max_turns => agent.steps,
                disallowed_tools => agent.disallowed_tools.trim(),
                permission_mode => agent.permission_mode.trim(),
                effort => agent.effort.trim(),
                memory => agent.memory.trim(),
                skills => crate::claude::split_list(&agent.preload_skills),
                mcp_servers => servers,
                background => agent.background,
                body => agent.body,
            };
            let md = env
                .render_str(&agent_tmpl, context! { agent => agent_val })
                .with_context(|| format!("rendering agent '{}'", agent.name))?;
            components.push((format!("agents/{}.md", agent.name), md));
        }

        if !subs.is_empty() {
            let cmd = env
                .render_str(
                    &templates::load("claude/commands/multi.md.j2")?,
                    context! { subagents => &subs, language => lang },
                )
                .context("rendering multi command")?;
            components.push(("commands/multi.md".to_string(), cmd));
        }
        if self.claude.workflow.intake {
            components.push((
                "commands/intake.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/intake.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering intake command")?,
            ));
        }
        if self.claude.workflow.refine {
            components.push((
                "commands/refine.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/refine.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering refine command")?,
            ));
        }
        if self.claude.workflow.improve_prompt {
            components.push((
                "commands/improve-prompt.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/improve-prompt.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering improve-prompt command")?,
            ));
        }
        if self.claude.workflow.fanout {
            components.push((
                "commands/fanout.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/fanout.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering fanout command")?,
            ));
        }
        let (adversary, implementer, implementer_isolated, rounds) = self.adversary_names();
        if self.claude.workflow.deliver {
            components.push((
                "commands/deliver.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/deliver.md.j2")?,
                    context! {
                        language => lang,
                        inquire => self.claude.workflow.inquire,
                        intent => self.claude.workflow.intent,
                        approvers => self.claude.intent.approvers.join(", "),
                        adversary => adversary,
                        implementer => implementer,
                        implementer_isolated => implementer_isolated,
                        rounds => rounds,
                    },
                )
                .context("rendering deliver command")?,
            ));
        }
        if self.claude.workflow.intent {
            let i = &self.claude.intent;
            // The linked CODEOWNERS and the rule ocgen's block holds there.
            let (codeowners, codeowners_rule) = match self.codeowners_block() {
                Some((rel, Some(rule))) => (rel, rule),
                _ => Default::default(),
            };
            components.push((
                "commands/intent.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/intent.md.j2")?,
                    context! {
                        answer_language => canonical_language(self.response_language()),
                        reading_copies => self.writes_reading_copies(),
                        prefix => i.prefix,
                        digits => i.digits,
                        dir => i.dir.trim_end_matches('/'),
                        example => i.first_id(),
                        max_words => i.max_words,
                        branch => i.branch.trim(),
                        allowed_tools => i.allowed_tools(),
                        trusted_domains => i.trusted_domains.join(", "),
                        approvers => i.approvers.join(", "),
                        codeowners => codeowners,
                        codeowners_rule => codeowners_rule,
                        // The research and review agents it hands work to: the
                        // project's own where they exist (the built-in Explore is
                        // denied when the explorer is preferred).
                        prefer_explorer => self.prefers_explorer(),
                        reviewer => self
                            .agents
                            .iter()
                            .any(|a| a.name == "reviewer" && a.mode == "subagent"),
                        issue_template => crate::claude::INTENT_ISSUE_TEMPLATE,
                        intent_template => crate::claude::INTENT_FILE_TEMPLATE,
                        adversary => adversary,
                        rounds => rounds,
                    },
                )
                .context("rendering intent command")?,
            ));
        }
        if self.claude.workflow.inquire {
            components.push((
                "commands/inquire.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/inquire.md.j2")?,
                    context! {
                        language => lang,
                        answer_language => crate::render::canonical_language(self.response_language()),
                        english_answers => self.response_language().eq_ignore_ascii_case("English"),
                    },
                )
                .context("rendering inquire command")?,
            ));
        }
        if self.claude.workflow.recap {
            components.push((
                "commands/recap.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/recap.md.j2")?,
                    context! {
                        allowed_tools => crate::claude::RECAP_TOOLS.join(", "),
                        // The gate holds every `git merge`: /recap prints the
                        // current branch's fast-forward instead of running it.
                        approval_gate => self.claude.team.enabled && self.claude.team.approval_gate,
                        answer_language => canonical_language(self.response_language()),
                        english_answers => self.response_language().eq_ignore_ascii_case("English"),
                    },
                )
                .context("rendering recap command")?,
            ));
        }

        let skill_tmpl = templates::load("claude/skill/SKILL.md.j2")?;
        for skill in &self.skills {
            let md = env
                .render_str(
                    &skill_tmpl,
                    context! {
                        skill => skill,
                        paths => skill.paths.split_whitespace().collect::<Vec<_>>(),
                    },
                )
                .with_context(|| format!("rendering skill '{}'", skill.name))?;
            components.push((format!("skills/{}/SKILL.md", skill.name), md));
        }

        if self.claude.powerups.output_style {
            components.push((
                "output-styles/ocgen-concise.md".to_string(),
                env.render_str(
                    &templates::load("claude/output-styles/ocgen-concise.md.j2")?,
                    context! {},
                )
                .context("rendering output style")?,
            ));
        }

        // Agent Teams: a /team command plus optional quality-gate hook scripts.
        if self.claude.team.enabled {
            let team_cmd = env
                .render_str(
                    &templates::load("claude/commands/team.md.j2")?,
                    context! {
                        subagents => &subs,
                        language => lang,
                        plan_gate => self.claude.team.plan_gate,
                    },
                )
                .context("rendering team command")?;
            components.push(("commands/team.md".to_string(), team_cmd));
            if self.claude.team.plan_gate {
                let plan_cmd = env
                    .render_str(
                        &templates::load("claude/commands/team-plan.md.j2")?,
                        context! {
                            subagents => &subs,
                            language => lang,
                            confidence_threshold => self.claude.team.confidence_threshold,
                        },
                    )
                    .context("rendering team-plan command")?;
                components.push(("commands/team-plan.md".to_string(), plan_cmd));
            }
            if self.claude.team.hooks {
                for h in [
                    "team-teammate-idle.sh",
                    "team-task-created.sh",
                    "team-task-completed.sh",
                ] {
                    let body = templates::load_embedded(&format!("claude/hooks/{h}"))?;
                    components.push((format!("hooks/{h}"), body));
                }
            }
            // The execution-approval gate is emitted independently of `hooks` so
            // this safety line is never silently disabled.
            if self.claude.team.approval_gate {
                let body = templates::load_embedded("claude/hooks/team-approval-gate.sh")?;
                components.push(("hooks/team-approval-gate.sh".to_string(), body));
                // The git-side half of the gate (installed into .git/hooks by
                // `scaffold`, or chained by hand when another hook is present).
                let body = templates::load_embedded("claude/hooks/git-pre-push.sh")?;
                components.push(("hooks/git-pre-push.sh".to_string(), body));
            }
        }
        // Per-worker confidence gate (SubagentStop) — independent of Agent Teams.
        if self.worker_gate() {
            let body = templates::load_embedded("claude/hooks/subagent-confidence-gate.sh")?;
            components.push(("hooks/subagent-confidence-gate.sh".to_string(), body));
        }
        // Optional quality-of-life hook scripts.
        let x = &self.claude.hooks_extra;
        for (on, script) in [
            (x.notify, "notify.sh"),
            (!x.format_cmd.trim().is_empty(), "format.sh"),
            (x.config_audit, "config-audit.sh"),
            (x.drop_noop_cd, "drop-noop-cd.sh"),
        ] {
            if on {
                let body = templates::load_embedded(&format!("claude/hooks/{script}"))?;
                components.push((format!("hooks/{script}"), body));
            }
        }
        // /intent fetches docs over HTTPS only; this PreToolUse hook enforces it.
        if self.claude.workflow.intent {
            let body = templates::load_embedded("claude/hooks/https-only-fetch.sh")?;
            components.push(("hooks/https-only-fetch.sh".to_string(), body));
        }
        // /inquire ledgers and /intent reading copies get an HTML view, rendered
        // and shown by this hook.
        if self.renders_notes() {
            let body = templates::load_embedded("claude/hooks/inquire-notes.sh")?;
            components.push(("hooks/inquire-notes.sh".to_string(), body));
        }
        // Shared loop guard, sourced by every blocking hook so no gate can hold an
        // agent forever.
        if self.has_blocking_hooks() {
            let body = templates::load_embedded("claude/hooks/loop-guard.sh")?;
            components.push(("hooks/loop-guard.sh".to_string(), body));
        }

        // Workflow commands ship as skills (`.claude/commands` is legacy): same
        // templates, rendered to skills/<name>/SKILL.md with a `name` and — for the
        // side-effecting ones — `disable-model-invocation`.
        // A user skill that has a workflow skill's name keeps its file (`issues()`
        // warns about the clash); otherwise one would silently overwrite the other.
        let mut components: Vec<(String, String)> = components
            .into_iter()
            .filter(|(rel, _)| {
                !rel.strip_prefix("commands/")
                    .and_then(|r| r.strip_suffix(".md"))
                    .is_some_and(|name| self.skills.iter().any(|s| s.name == name))
            })
            .map(|(rel, c)| {
                match rel
                    .strip_prefix("commands/")
                    .and_then(|r| r.strip_suffix(".md"))
                {
                    Some(name) => (
                        format!("skills/{name}/SKILL.md"),
                        command_to_skill(
                            name,
                            &c,
                            crate::claude::USER_RUN_WORKFLOWS.contains(&name),
                        ),
                    ),
                    None => (rel, c),
                }
            })
            .collect();

        // Coordinator body may itself be a template (the orchestrator prompt loops
        // over subagents), so render it with that context for CLAUDE.md.
        let coordinator = match self.primary() {
            Some(p) => body_as_template(
                &env,
                &p.name,
                &p.body,
                context! { subagents => &subs, language => lang, parallel => true },
            ),
            None => String::new(),
        };
        // The main session coordinates: the answer line and the adversary loop
        // are its to follow.
        let coordinator = self.coordinator_text(&coordinator, &env)?;
        // Behavioral guidance lives in .claude/rules/ (loads every session at CLAUDE.md
        // priority) so ocgen never has to touch a user-owned CLAUDE.md.
        let has_coordinator = !coordinator.is_empty();
        let rules_ctx = context! {
            coordinator => coordinator,
            subagents => &subs,
            team => self.claude.team.enabled,
            team_mode => self.teammate_mode(),
            plan_gate => self.claude.team.plan_gate,
            confidence_threshold => self.claude.team.confidence_threshold,
            risk_rounds => self.claude.team.risk_rounds,
            approval_gate => self.claude.team.approval_gate,
            fanout => self.claude.workflow.fanout,
            verify_todos => self.claude.workflow.verify_todos,
            prefer_explorer => self.prefers_explorer(),
            deliver => self.claude.workflow.deliver,
            inquire => self.claude.workflow.inquire,
            intent => self.claude.workflow.intent,
            intent_dir => self.claude.intent.dir.trim_end_matches('/'),
            subagent_confidence => self.claude.workflow.subagent_confidence,
            loop_guard_max => self.claude.workflow.loop_guard_max,
            check_cmd => self.claude.workflow.check_cmd.trim(),
        };
        components.push((
            "rules/ocgen-workflow.md".to_string(),
            env.render_str(
                &templates::load("claude/rules/ocgen-workflow.md.j2")?,
                rules_ctx.clone(),
            )
            .context("rendering ocgen-workflow rule")?,
        ));
        if has_coordinator || !subs.is_empty() || self.claude.team.enabled {
            components.push((
                "rules/ocgen-team.md".to_string(),
                env.render_str(
                    &templates::load("claude/rules/ocgen-team.md.j2")?,
                    rules_ctx.clone(),
                )
                .context("rendering ocgen-team rule")?,
            ));
        }

        // CLAUDE.md is a slim, user-owned starter (written once — never overwritten;
        // see `scaffold`).
        let claude_md = env
            .render_str(
                &templates::load("claude/CLAUDE.md.j2")?,
                context! {
                    project_name => self.project_name,
                    instructions => self.claude.instructions,
                },
            )
            .context("rendering CLAUDE.md")?;
        let settings = self.claude_settings_json()?;

        let want_plugin = self.claude.output.plugin;
        // Always write the project tree unless the user asked for plugin-only.
        let want_project = self.claude.output.project || !want_plugin;

        let mut out = Vec::new();

        if want_project {
            for (rel, c) in &components {
                out.push((PathBuf::from(format!(".claude/{rel}")), c.clone()));
            }
            out.push((PathBuf::from(".claude/settings.json"), settings.clone()));
            if self.claude.powerups.statusline {
                // Project-only: a plugin can't set statusLine.
                out.push((
                    PathBuf::from(".claude/statusline.sh"),
                    templates::load_embedded("claude/statusline.sh")?,
                ));
            }
            out.push((PathBuf::from("CLAUDE.md"), claude_md));
            if self.claude.workflow.intent {
                // User-owned starters (written once, like CLAUDE.md).
                for (rel, default) in [
                    (
                        crate::claude::INTENT_ISSUE_TEMPLATE,
                        "claude/intent/issue.md",
                    ),
                    (
                        crate::claude::INTENT_FILE_TEMPLATE,
                        "claude/intent/intent.md",
                    ),
                ] {
                    out.push((PathBuf::from(rel), templates::load(default)?));
                }
            }
            if let Some(mcp) = self.mcp_json()? {
                out.push((PathBuf::from(".mcp.json"), mcp));
            }
            if self.claude.workflow.fanout || self.claude.workflow.deliver {
                // Root-level file (not under .claude/): gitignored files copied into
                // each new worktree — `.env` plus, when `.claude/` is ignored, the
                // settings/hooks/rules that /fanout and multi-session delivery need.
                out.push((
                    PathBuf::from(".worktreeinclude"),
                    templates::load("claude/worktreeinclude")?,
                ));
            }
        }

        if want_plugin {
            let plugin_name = self.plugin_name();
            crate::validate::folder_name(&plugin_name)
                .map_err(|e| anyhow!("plugin folder '{plugin_name}': {e}"))?;
            let base = format!("plugin/{plugin_name}");
            for (rel, c) in &components {
                out.push((PathBuf::from(format!("{base}/{rel}")), c.clone()));
            }
            if let Some(mcp) = self.mcp_json()? {
                out.push((PathBuf::from(format!("{base}/.mcp.json")), mcp));
            }
            let owner = self.claude.plugin.repo_owner.clone();
            let repo = self.claude.plugin.repo_name.clone();
            let version = if self.claude.plugin.version.trim().is_empty() {
                "0.1.0".to_string()
            } else {
                self.claude.plugin.version.clone()
            };
            let display = if self.claude.plugin.display_name.trim().is_empty() {
                self.project_name.clone()
            } else {
                self.claude.plugin.display_name.clone()
            };
            let description = format!("{display} — a Claude Code multi-agent team.");
            let mctx = context! {
                name => plugin_name,
                display_name => display,
                version => version,
                description => description,
                owner => owner,
                repo => repo,
                subagents => &subs,
                intake => self.claude.workflow.intake,
                refine => self.claude.workflow.refine,
            };
            let plugin_json = env
                .render_str(
                    &templates::load("claude/plugin/plugin.json.j2")?,
                    mctx.clone(),
                )
                .context("rendering plugin.json")?;
            serde_json::from_str::<Value>(&plugin_json)
                .context("rendered plugin.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/plugin.json")),
                plugin_json,
            ));
            let market = env
                .render_str(
                    &templates::load("claude/plugin/marketplace.json.j2")?,
                    mctx.clone(),
                )
                .context("rendering marketplace.json")?;
            serde_json::from_str::<Value>(&market)
                .context("rendered marketplace.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/marketplace.json")),
                market,
            ));
            out.push((
                PathBuf::from(format!("{base}/README.md")),
                env.render_str(
                    &templates::load("claude/plugin/README.md.j2")?,
                    mctx.clone(),
                )
                .context("rendering plugin README")?,
            ));
            out.push((
                PathBuf::from(format!("{base}/.github/workflows/release.yml")),
                env.render_str(&templates::load("claude/plugin/release.yml.j2")?, mctx)
                    .context("rendering release workflow")?,
            ));
            if let Some(hooks) = self.plugin_hooks_json()? {
                out.push((PathBuf::from(format!("{base}/hooks/hooks.json")), hooks));
            }
        }

        Ok(out)
    }
    /// The effective teammate display mode (defaults to in-process).
    /// Whether the per-worker gate (SubagentStop) is on: a confidence bar, a check
    /// command, or both.
    fn worker_gate(&self) -> bool {
        self.claude.workflow.subagent_confidence > 0
            || !self.claude.workflow.check_cmd.trim().is_empty()
    }

    /// Whether any hook that can block (exit 2) is emitted — those source the loop guard.
    fn has_blocking_hooks(&self) -> bool {
        self.worker_gate()
            || (self.claude.team.enabled
                && (self.claude.team.hooks || self.claude.team.approval_gate))
    }

    fn teammate_mode(&self) -> String {
        let m = self.claude.team.mode.trim();
        if m.is_empty() {
            "in-process".to_string()
        } else {
            m.to_string()
        }
    }

    /// Build `.claude/settings.json` as validated JSON from the power-up toggles.
    fn claude_settings_json(&self) -> Result<String> {
        let model = if self.claude.model.trim().is_empty() {
            "opus"
        } else {
            self.claude.model.trim()
        };
        let mut root = json!({
            "$schema": "https://json.schemastore.org/claude-code-settings.json",
            "model": model
        });
        let obj = root.as_object_mut().unwrap();
        // Claude Code's own answer language, when it isn't the instructions'.
        if self.answers_differ() {
            obj.insert(
                "language".into(),
                json!(self.response_language().to_lowercase()),
            );
        }
        // The generated rules, then the user's own (`ocgen edit permissions`).
        let rules = self.effective_permissions();
        if !rules.is_empty() {
            let mut perms = serde_json::Map::new();
            for list in RuleList::ALL {
                if !rules.list(list).is_empty() {
                    perms.insert(list.key().into(), json!(rules.list(list)));
                }
            }
            obj.insert("permissions".into(), Value::Object(perms));
        }
        if self.claude.sandbox.enabled {
            let mut domains: Vec<String> = crate::claude::SANDBOX_DOMAINS
                .iter()
                .map(|d| d.to_string())
                .collect();
            for d in &self.claude.sandbox.extra_domains {
                if !domains.contains(d) {
                    domains.push(d.clone());
                }
            }
            // No sandboxed command may write the approval store, nor what runs
            // outside the sandbox and Claude Code's own protection leaves out:
            // the status line, ocgen's state (it regenerates the hooks) and git's
            // hooks (a human's own push runs them). The store stays readable —
            // the pre-push hook reads an approval from inside the sandbox.
            let mut no_write = vec!["~/.claude/ocgen"];
            if self.claude.powerups.statusline {
                no_write.push("./.claude/statusline.sh");
            }
            no_write.extend(["./.claude/.ocgen-state.json", "./.git/hooks"]);
            let mut sandbox = json!({
                "enabled": true,
                // Strict: a sandboxed failure can't be retried unsandboxed.
                "allowUnsandboxedCommands": false,
                "network": { "allowedDomains": domains },
                "filesystem": { "denyWrite": no_write }
            });
            if !self.claude.sandbox.allow_credentials {
                // A push or publish that needs a withheld credential fails,
                // however the command is phrased.
                let files: Vec<Value> = crate::claude::CREDENTIAL_PATHS
                    .iter()
                    .map(|p| json!({ "path": p, "mode": "deny" }))
                    .collect();
                let vars: Vec<Value> = crate::claude::CREDENTIAL_ENV
                    .iter()
                    .map(|n| json!({ "name": n, "mode": "deny" }))
                    .collect();
                sandbox["credentials"] = json!({ "files": files, "envVars": vars });
            }
            obj.insert("sandbox".into(), sandbox);
        }
        if self.claude.powerups.output_style {
            // Named apart from the built-in `Concise` so it never shadows it.
            obj.insert("outputStyle".into(), json!("ocgen-concise"));
        }
        let approved: Vec<&str> = self
            .claude
            .mcp_servers
            .iter()
            .filter(|s| s.pre_approve)
            .map(|s| s.name.as_str())
            .collect();
        if !approved.is_empty() {
            // Takes effect once the user trusts the workspace.
            obj.insert("enabledMcpjsonServers".into(), json!(approved));
        }
        if self.claude.workflow.fanout {
            // Worktrees (fanout workers, --worktree sessions) branch from the current
            // HEAD, so they see unpushed work and merge back cleanly.
            obj.insert("worktree".into(), json!({ "baseRef": "head" }));
        }
        if self.claude.powerups.statusline {
            // CLAUDE_PROJECT_DIR when set, else the repo top (works in worktrees too).
            obj.insert(
                "statusLine".into(),
                json!({
                    "type": "command",
                    "command": "sh \"${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}/.claude/statusline.sh\""
                }),
            );
        }
        let sub_conf = self.claude.workflow.subagent_confidence;
        let gate_env = self.gate_env();
        if self.claude.team.enabled
            || sub_conf > 0
            || self.worker_gate()
            || gate_env.contains_key("OCGEN_SANDBOX")
        {
            let mut env = serde_json::Map::new();
            if self.claude.team.enabled {
                env.insert("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".into(), json!("1"));
            }
            env.extend(gate_env);
            obj.insert("env".into(), Value::Object(env));
            if self.claude.team.enabled {
                obj.insert("teammateMode".into(), json!(self.teammate_mode()));
            }
        }

        let hooks = self.hook_table("${CLAUDE_PROJECT_DIR}/.claude/hooks", "");
        if !hooks.is_empty() {
            obj.insert("hooks".into(), Value::Object(hooks));
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&root)?))
    }

    /// Environment the gate hooks read (thresholds, switches, loop budget). Project
    /// output puts it in `settings.json` `env`; a plugin can't set `env`, so its hook
    /// commands carry these as a prefix instead.
    fn gate_env(&self) -> serde_json::Map<String, Value> {
        let mut env = serde_json::Map::new();
        if self.claude.team.enabled {
            if self.claude.team.hooks {
                // TaskCompleted judges teammates per task, so the worker gate
                // leaves their turn ends alone.
                env.insert("TEAM_TASK_GATE".into(), json!("1"));
            }
            if self.claude.team.plan_gate {
                env.insert("TEAM_PLAN_GATE".into(), json!("1"));
            }
            if self.claude.team.confidence_threshold > 0 {
                env.insert(
                    "TEAM_CONFIDENCE_THRESHOLD".into(),
                    json!(self.claude.team.confidence_threshold.to_string()),
                );
            }
            if self.claude.team.risk_rounds {
                env.insert("TEAM_RISK_ROUNDS".into(), json!("1"));
                // Read-only roles (no Write/Edit tool) can't mitigate, so the
                // TeammateIdle gate exempts them instead of livelocking them.
                let readonly = self.readonly_roles();
                if !readonly.is_empty() {
                    env.insert("TEAM_READONLY_ROLES".into(), json!(readonly.join(" ")));
                }
            }
            if self.claude.team.approval_gate {
                env.insert("TEAM_APPROVAL_GATE".into(), json!("1"));
            }
        }
        let sub_conf = self.claude.workflow.subagent_confidence;
        if sub_conf > 0 {
            env.insert(
                "SUBAGENT_CONFIDENCE_THRESHOLD".into(),
                json!(sub_conf.to_string()),
            );
        }
        if self.worker_gate() {
            // Read-only workers can't change files, so the worker gate never
            // holds them — including Claude Code's own read-only Explore and Plan
            // agents, unless the project defines its own under those names.
            let mut readonly = self.readonly_roles();
            for builtin in ["Explore", "Plan"] {
                if !self.agents.iter().any(|a| a.name == builtin) {
                    readonly.push(builtin);
                }
            }
            env.insert("SUBAGENT_READONLY_ROLES".into(), json!(readonly.join(" ")));
        }
        let check = self.claude.workflow.check_cmd.trim();
        if !check.is_empty() {
            env.insert("OCGEN_CHECK_CMD".into(), json!(check));
        }
        let format = self.claude.hooks_extra.format_cmd.trim();
        if self.claude.sandbox.enabled && !(check.is_empty() && format.is_empty()) {
            // Claude Code doesn't sandbox hooks: the hooks run the check and the
            // formatter in an OS sandbox of their own, from these lists.
            use crate::claude::{CREDENTIAL_ENV, CREDENTIAL_PATHS, HOOK_DENY_WRITE};
            env.insert("OCGEN_SANDBOX".into(), json!("1"));
            env.insert(
                "OCGEN_SANDBOX_DENY_WRITE".into(),
                json!(HOOK_DENY_WRITE.join(" ")),
            );
            if !self.claude.sandbox.allow_credentials {
                env.insert(
                    "OCGEN_SANDBOX_DENY_READ".into(),
                    json!(CREDENTIAL_PATHS.join(" ")),
                );
                env.insert(
                    "OCGEN_SANDBOX_DENY_ENV".into(),
                    json!(CREDENTIAL_ENV.join(" ")),
                );
            }
        }
        if self.claude.sandbox.enabled && !self.claude.sandbox.allow_credentials {
            // Git asks an OS keychain the sandbox can't hide: start with no helper.
            for (k, v) in crate::claude::GIT_HELPER_RESET {
                env.insert(k.into(), json!(v));
            }
        }
        if self.claude.workflow.intent {
            // The WebFetch guard's trusted documentation sites (empty = none).
            env.insert(
                "OCGEN_WEBFETCH_DOMAINS".into(),
                json!(self.claude.intent.trusted_domains.join(" ")),
            );
        }
        if self.has_blocking_hooks() {
            env.insert(
                "LOOP_GUARD_MAX_BLOCKS".into(),
                json!(self.claude.workflow.loop_guard_max.to_string()),
            );
        }
        env
    }

    /// The project's read-only roles: subagents with no Write/Edit tool.
    fn readonly_roles(&self) -> Vec<&str> {
        self.agents
            .iter()
            .filter(|a| a.mode != "primary")
            .filter(|a| {
                let t = a.tools.trim();
                let d = &a.disallowed_tools;
                (!t.is_empty() && !t.contains("Write") && !t.contains("Edit"))
                    || (d.contains("Write") && d.contains("Edit"))
            })
            .map(|a| a.name.as_str())
            .collect()
    }

    /// The permission rules ocgen generates itself, before the user's own.
    pub fn generated_permissions(&self) -> PermissionRules {
        let mut rules = self.default_permissions();
        if self.claude.team.enabled && self.claude.team.approval_gate {
            // The gate's own guards, kept even without the permission defaults:
            // the sandbox contains Bash, not the file tools.
            for r in crate::claude::GATE_DENY {
                rules.tighten(RuleList::Deny, r);
            }
            for r in crate::claude::GATE_ASK {
                rules.tighten(RuleList::Ask, r);
            }
        }
        if self.claude.workflow.intent {
            // /intent drafts the issue; the user files it. Kept even without the
            // permission defaults, since the workflow relies on it.
            rules.deny.push("Bash(gh issue create*)".into());
            if matches!(self.codeowners_block(), Some((_, Some(_)))) {
                // GitHub requires the approvers through ocgen's block: changing
                // it, or hiding it, asks first.
                for r in crate::claude::CODEOWNERS_ASK {
                    rules.tighten(RuleList::Ask, r);
                }
            }
        }
        if self.prefers_explorer() {
            // Research goes to the shell-free `explorer`; the built-in Explore
            // ignores CLAUDE.md and its shell one-liners prompt under the read block.
            rules.deny.push("Agent(Explore)".into());
        }
        rules
    }

    /// Whether research is routed to the generated `explorer` subagent (and the
    /// built-in `Explore` agent is denied): the flag is on and the agent exists.
    pub fn prefers_explorer(&self) -> bool {
        self.claude.workflow.prefer_explorer
            && self
                .agents
                .iter()
                .any(|a| a.name == "explorer" && a.mode == "subagent")
    }

    /// The permission defaults (the `permissions` power-up).
    fn default_permissions(&self) -> PermissionRules {
        if !self.claude.powerups.permissions {
            return PermissionRules::default();
        }
        let owned = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        let mut deny = vec!["Bash(rm -rf:*)"];
        deny.extend(crate::claude::SECRET_READ_DENY);
        deny.extend(crate::claude::GUARD_DENY);
        PermissionRules {
            allow: owned(&[
                "Read",
                "Grep",
                "Glob",
                "Edit",
                "Write",
                "Bash(git status:*)",
                "Bash(git diff:*)",
                "Bash(git log:*)",
            ]),
            // Asked even in auto mode: a second line behind the approval gate.
            ask: owned(&crate::claude::HIGH_IMPACT_ASK),
            deny: owned(&deny),
        }
    }

    /// Permission rules in the project's `settings.json` that ocgen didn't generate
    /// and that aren't already the user's own: rules added by hand, which a
    /// regeneration would drop unless they're adopted as the user's rules.
    pub fn hand_added_permissions(&self, target: &Path) -> HandAdded {
        let mut found = HandAdded::default();
        let Some(settings) = fs::read_to_string(target.join(".claude/settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            return found;
        };
        let generated = self.generated_permissions();
        for list in RuleList::ALL {
            let on_disk = settings
                .pointer(&format!("/permissions/{}", list.key()))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim);
            for rule in on_disk {
                if generated.list(list).iter().any(|r| r == rule)
                    || self.claude.permissions.list(list).iter().any(|r| r == rule)
                {
                    continue;
                }
                if crate::validate::permission_rule(rule).is_err() {
                    found.invalid.push(rule.to_string());
                } else {
                    // `add` dedups and keeps a rule in one list.
                    let _ = found.rules.add(list, rule);
                }
            }
        }
        found
    }

    /// The permission rules settings.json gets: the generated ones, then the
    /// user's own. Claude Code checks deny, then ask, then allow, so a user rule
    /// stricter than ocgen's copy (a deny of a generated ask or allow, an ask of
    /// a generated allow) takes its place, and a looser one is left out: the
    /// user's rules can tighten a generated guard, never loosen it.
    pub fn effective_permissions(&self) -> PermissionRules {
        let mut rules = self.generated_permissions();
        for (list, rule) in self.claude.permissions.entries() {
            if crate::validate::permission_rule(rule.trim()).is_ok() {
                rules.tighten(list, &rule);
            }
        }
        rules
    }

    /// The generated list that overrides `rule` in list `list`, if any. Claude
    /// Code checks deny, then ask, then allow, so e.g. an allow for a rule the
    /// generated ask list holds has no effect.
    pub fn shadowing_rule(&self, list: RuleList, rule: &str) -> Option<RuleList> {
        self.generated_permissions()
            .list_of(rule)
            .filter(|held| held.stricter_than(list))
    }

    /// The looser generated list whose copy of `rule` the user's stricter rule in
    /// `list` replaces, if any — e.g. a deny of the generated ask `git push`.
    pub fn replaced_rule(&self, list: RuleList, rule: &str) -> Option<RuleList> {
        self.generated_permissions()
            .list_of(rule)
            .filter(|held| list.stricter_than(*held))
    }

    /// The hooks table, shared by `settings.json` and the plugin's `hooks.json`.
    /// `dir` is where the hook scripts live; `prefix` is prepended to each script
    /// command (the plugin passes the gate env this way).
    fn hook_table(&self, dir: &str, prefix: &str) -> serde_json::Map<String, Value> {
        let mut hooks = serde_json::Map::new();
        if self.claude.powerups.hooks
            && (self.claude.workflow.intake || self.claude.workflow.improve_prompt)
        {
            let mut tip = String::from("Tip:");
            if self.claude.workflow.intake {
                tip.push_str(
                    " run /intake to gather requirements, then /refine to iterate before approval.",
                );
            }
            if self.claude.workflow.improve_prompt {
                tip.push_str(
                    " run /improve-prompt to sharpen a prompt or an agent's system prompt.",
                );
            }
            // Single-quote the echo argument, escaping any apostrophe in the tip
            // (e.g. "agent's") as '\'' so the command stays valid POSIX shell.
            let escaped = tip.trim().replace('\'', "'\\''");
            hooks.insert(
                "SessionStart".into(),
                json!([ { "hooks": [ command_hook(format!("echo '{escaped}'")) ] } ]),
            );
        }
        if self.claude.team.enabled && self.claude.team.hooks {
            for (event, script) in [
                ("TeammateIdle", "team-teammate-idle.sh"),
                ("TaskCreated", "team-task-created.sh"),
                ("TaskCompleted", "team-task-completed.sh"),
            ] {
                hooks.insert(
                    event.to_string(),
                    json!([ { "hooks": [ command_hook(hook_cmd(prefix, dir, script)) ] } ]),
                );
            }
        }
        // Execution-approval gate: a PreToolUse hook with no matcher, so it sees
        // every tool — any tool with a `command` (Bash, Monitor, PowerShell, MCP…)
        // can run a high-impact action, and any file tool can touch the approval
        // store. Emitted whenever the gate is on, independent of the `hooks` toggle.
        if self.claude.team.enabled && self.claude.team.approval_gate {
            hooks.insert(
                "PreToolUse".into(),
                json!([ {
                    "hooks": [ command_hook(hook_cmd(prefix, dir, "team-approval-gate.sh")) ]
                } ]),
            );
        }
        // Per-worker confidence gate: SubagentStart records the tree a subagent
        // starts from; SubagentStop blocks one that changed it but isn't confident
        // enough. Independent of teams — it pairs with worktree isolation for
        // isolated writes + enforced confidence.
        if self.worker_gate() {
            for event in ["SubagentStart", "SubagentStop"] {
                hooks.insert(
                    event.into(),
                    json!([ { "hooks": [ command_hook(hook_cmd(prefix, dir, "subagent-confidence-gate.sh")) ] } ]),
                );
            }
        }
        // Optional quality-of-life hooks. SessionStart may already hold the tip,
        // so groups are appended rather than inserted.
        let mut push = |event: &str, group: Value| {
            if let Some(arr) = hooks
                .entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
            {
                arr.push(group);
            }
        };
        let x = &self.claude.hooks_extra;
        if x.compact_context {
            push(
                "SessionStart",
                json!({ "matcher": "compact", "hooks": [ command_hook("echo 'Context was just compacted. Re-read the project rules in .claude/rules/ (workflow, team, loop discipline). If an /inquire study session is active, re-read its ledger resume point in .claude/notes/ before continuing.'") ] }),
            );
        }
        if x.notify {
            for event in ["Notification", "StopFailure"] {
                push(
                    event,
                    json!({ "hooks": [ command_hook(hook_cmd(prefix, dir, "notify.sh")) ] }),
                );
            }
        }
        let fmt = x.format_cmd.trim();
        if !fmt.is_empty() {
            let fmt = fmt.replace('\'', "'\\''");
            push(
                "PostToolUse",
                json!({ "matcher": "Edit|Write", "hooks": [ command_hook(hook_cmd(&format!("OCGEN_FORMAT_CMD='{fmt}' {prefix}"), dir, "format.sh")) ] }),
            );
        }
        if self.renders_notes() {
            // Re-render a ledger's or a reading copy's HTML view after each write
            // and show it.
            push(
                "PostToolUse",
                json!({ "matcher": "Write|Edit|MultiEdit", "hooks": [ command_hook(hook_cmd(prefix, dir, "inquire-notes.sh")) ] }),
            );
        }
        if self.claude.workflow.intent {
            // Deterministic HTTPS-only line for WebFetch (the docs /intent reads).
            push(
                "PreToolUse",
                json!({ "matcher": "WebFetch", "hooks": [ command_hook(hook_cmd(prefix, dir, "https-only-fetch.sh")) ] }),
            );
        }
        if x.drop_noop_cd {
            // `cd <this folder> && …` → `…`, so the read block can check its paths.
            push(
                "PreToolUse",
                json!({ "matcher": "Bash", "hooks": [ command_hook(hook_cmd(prefix, dir, "drop-noop-cd.sh")) ] }),
            );
        }
        if x.config_audit {
            push(
                "ConfigChange",
                json!({ "hooks": [ command_hook(hook_cmd(prefix, dir, "config-audit.sh")) ] }),
            );
        }
        hooks
    }

    /// The project's `.mcp.json`; `None` when it has no MCP servers.
    fn mcp_json(&self) -> Result<Option<String>> {
        if self.claude.mcp_servers.is_empty() {
            return Ok(None);
        }
        let servers: serde_json::Map<String, Value> = self
            .claude
            .mcp_servers
            .iter()
            .map(|s| (s.name.clone(), s.to_json()))
            .collect();
        let v = json!({ "mcpServers": Value::Object(servers) });
        Ok(Some(format!("{}\n", serde_json::to_string_pretty(&v)?)))
    }

    /// Folder name for the generated plugin (repo name, else project name).
    fn plugin_name(&self) -> String {
        let raw = if !self.claude.plugin.repo_name.trim().is_empty() {
            self.claude.plugin.repo_name.trim()
        } else {
            self.project_name.trim()
        };
        raw.to_lowercase().replace(' ', "-")
    }

    /// The plugin's `hooks/hooks.json`: the same hooks as the project, with script
    /// paths under `${CLAUDE_PLUGIN_ROOT}` and the gate env passed as a command
    /// prefix (plugins can't set `env`). `None` when there are no hooks.
    fn plugin_hooks_json(&self) -> Result<Option<String>> {
        let prefix: String = self
            .gate_env()
            .iter()
            .map(|(k, v)| {
                let v = v.as_str().unwrap_or_default().replace('\'', "'\\''");
                format!("{k}='{v}' ")
            })
            .collect();
        let hooks = self.hook_table("${CLAUDE_PLUGIN_ROOT}/hooks", &prefix);
        if hooks.is_empty() {
            return Ok(None);
        }
        let v = json!({ "hooks": Value::Object(hooks) });
        Ok(Some(format!("{}\n", serde_json::to_string_pretty(&v)?)))
    }

    /// Write the rendered files (plus a state file) under `target`.
    ///
    /// Without `force`, fails on any pre-existing output file. With `force` (every
    /// `ocgen add …` / `ocgen edit …`) it regenerates safely, as [`Project::apply`]
    /// does, and prints what it adopted, removed and backed up.
    pub fn scaffold(&self, target: &Path, force: bool) -> Result<Vec<PathBuf>> {
        if force {
            let mut project = self.clone();
            let applied = project.apply(target, &BTreeSet::new())?;
            let notice = applied.notice(target);
            if !notice.is_empty() {
                println!();
                for line in notice {
                    println!("  {line}");
                }
            }
            return Ok(applied.written);
        }
        let files = self.render_all()?;

        // User-owned files (CLAUDE.md, the /intent templates) are created once and
        // never overwritten, so they're exempt from the conflict check.
        for (rel, _) in &files {
            if user_owned(rel) {
                continue;
            }
            let p = target.join(rel);
            if p.exists() {
                bail!(
                    "{} already exists — re-run and choose to overwrite",
                    p.display()
                );
            }
        }
        self.write_files(target, &files, &BTreeSet::new())
    }

    /// Regenerate the project under `target` without losing anything silently:
    /// adopt the permission rules and MCP servers added by hand, back up every file
    /// that will be overwritten or removed, write everything except the files in
    /// `keep` (paths relative to `target`, with forward slashes), and remove the
    /// files ocgen no longer generates.
    pub fn apply(&mut self, target: &Path, keep: &BTreeSet<String>) -> Result<Applied> {
        self.check_version()?;
        // Also when this project was built afresh over one a newer ocgen wrote.
        let on_disk = self.previous(target);
        refuse_newer(self.target.state_file(), &on_disk.version, on_disk.schema)?;
        let mut adopted = self.hand_added(target);
        let mut problems = self.adopt(&adopted);
        // Report as kept only what really was (a rule can't be in two lists).
        let mine = &self.claude.permissions;
        adopted.rules.allow.retain(|r| mine.allow.contains(r));
        adopted.rules.ask.retain(|r| mine.ask.contains(r));
        adopted.rules.deny.retain(|r| mine.deny.contains(r));
        // A looser copy of a generated rule stays the user's, without effect.
        let no_effect = adopted
            .rules
            .entries()
            .into_iter()
            .filter_map(|(list, r)| self.shadowing_rule(list, &r).map(|by| (list, r, by)))
            .collect();
        let plan: Vec<FileChange> = self
            .plan_changes(target)?
            .into_iter()
            .filter(|c| !keep.contains(&c.rel))
            .collect();
        // A file still exactly as ocgen wrote it holds no work of the user's, so
        // only hand edits (or files of unknown origin) take up a backup slot.
        let at_risk: Vec<FileChange> = plan
            .iter()
            .filter(|c| c.hand_edited != Some(false))
            .cloned()
            .collect();
        let backup = Self::backup(target, &at_risk)?;
        let written = self.scaffold_keeping(target, keep)?;
        problems.extend(self.codeowners_report(target));
        let rels = |pick: &dyn Fn(&FileChange) -> bool| -> Vec<String> {
            plan.iter()
                .filter(|c| pick(c))
                .map(|c| c.rel.clone())
                .collect()
        };
        Ok(Applied {
            written,
            backup,
            removed: rels(&|c| c.kind == ChangeKind::Removed),
            hand_edited: rels(&|c| c.kind != ChangeKind::Added && c.hand_edited == Some(true)),
            adopted,
            no_effect,
            problems,
        })
    }

    /// Like a forced [`Project::scaffold`], but leaves the files in `keep` (paths
    /// relative to `target`, with forward slashes) exactly as they are on disk, and
    /// adopts and backs up nothing — the caller decides that (`ocgen new`, `doctor`).
    pub fn scaffold_keeping(&self, target: &Path, keep: &BTreeSet<String>) -> Result<Vec<PathBuf>> {
        self.write_files(target, &self.render_all()?, keep)
    }

    fn write_files(
        &self,
        target: &Path,
        files: &[(PathBuf, String)],
        keep: &BTreeSet<String>,
    ) -> Result<Vec<PathBuf>> {
        self.check_version()?;
        let fence = Fence::new(target);
        // Every path is checked before anything is written — the state file too.
        let state_path = fence.path(Path::new(self.target.state_file()))?;
        let mut planned: Vec<(PathBuf, String)> = Vec::new();
        for (rel, contents) in files {
            if user_owned(rel) && fs::symlink_metadata(target.join(rel)).is_ok() {
                continue; // preserve the user's own file (a link to anywhere, too)
            }
            let path = fence.path(rel)?;
            if keep.contains(&rel_str(rel)) {
                continue; // the user chose to keep their version
            }
            planned.push((path, contents.clone()));
        }
        for (rel, block) in self.gitattributes() {
            // The user's file: one linked in from elsewhere is left alone.
            let Ok(path) = fence.path(Path::new(&rel)) else {
                continue;
            };
            if keep.contains(&rel) {
                continue;
            }
            let existing = match fs::read(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                // Not text: it's the user's, and left alone.
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(s) => Some(s),
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if let Some(new) = with_gitattributes_block(existing.as_deref(), block) {
                planned.push((path, new));
            }
        }
        let previous = self.previous(target);
        // ocgen's block in the linked CODEOWNERS (the rest of the file is the user's),
        // and out of one linked before (unlinked or re-linked since).
        if let Some((path, _, _, Some(new))) = self.codeowners_change(&fence, keep) {
            planned.push((path, new));
        }
        let linked = self.codeowners_block().map(|(rel, _)| rel);
        if !previous.codeowners.is_empty()
            && linked.as_ref() != Some(&previous.codeowners)
            && !keep.contains(&previous.codeowners)
        {
            if let Some(change) = codeowners::remove_block(&fence, &previous.codeowners) {
                planned.push(change);
            }
        }
        let (stale, renames) = self.stale_files(&fence, files, &previous.generated);

        // A case-only rename on a case-insensitive file system: the old entry is the
        // new file, so it gets the new spelling instead of being removed.
        for (from, to) in renames {
            let _ = fs::rename(from, to);
        }
        let mut written = Vec::new();
        for (path, contents) in planned {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
        }
        // Then what ocgen no longer generates (`apply` and `doctor` back it up first).
        for s in stale {
            if keep.contains(&s.rel) {
                continue;
            }
            fs::remove_file(&s.path).with_context(|| format!("removing {}", s.path.display()))?;
            remove_empty_dirs(s.path.parent(), target);
        }
        if self.target == Target::ClaudeCode {
            if let PrePush::Installed(p) = self.install_pre_push(target)? {
                written.push(p);
            }
            codeowners::remove_old_link(target);
        }

        // Persist project state so `add agent` can reload and re-render, with a
        // fingerprint of every generated file (lets `doctor` spot hand edits — a
        // kept file is recorded too, so it shows up there as differing).
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&state_path, self.state_json(files)?)
            .with_context(|| format!("writing {}", state_path.display()))?;

        Ok(written)
    }

    /// The state file a write of `files` records: this project, stamped with this
    /// ocgen's version, plus what was generated.
    fn state_json(&self, files: &[(PathBuf, String)]) -> Result<String> {
        let mut state = self.clone();
        state.ocgen_version = crate::VERSION.to_string();
        state.schema = STATE_SCHEMA;
        state.generated = files
            .iter()
            .filter(|(rel, _)| !user_owned(rel))
            .map(|(rel, c)| (rel_str(rel), fingerprint(c)))
            .collect();
        state.generated_rules =
            (self.target == Target::ClaudeCode).then(|| self.generated_permissions());
        Ok(serde_json::to_string_pretty(&state)?)
    }

    /// The state file writing this project would leave (to compare with the one on
    /// disk, e.g. before `ocgen new` replaces an existing project).
    pub fn rendered_state(&self) -> Result<String> {
        self.state_json(&self.render_all()?)
    }

    /// Refuse to write a project whose state a newer ocgen wrote: this one would
    /// drop the settings it doesn't know and downgrade the generated files.
    pub fn check_version(&self) -> Result<()> {
        refuse_newer(self.target.state_file(), &self.ocgen_version, self.schema)
    }

    /// What the last write recorded, from the state file on disk (the project in
    /// memory may already hold the user's changes). Without one, this project's.
    fn previous(&self, target: &Path) -> Previous {
        let disk = fs::read_to_string(target.join(self.target.state_file()))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let Some(v) = disk else {
            return Previous {
                version: self.ocgen_version.clone(),
                schema: self.schema,
                generated: self.generated.clone(),
                rules: self.generated_rules.clone(),
                user_rules: self.claude.permissions.clone(),
                servers: self
                    .claude
                    .mcp_servers
                    .iter()
                    .map(|s| s.name.clone())
                    .collect(),
                codeowners: self.claude.intent.codeowners.clone(),
            };
        };
        let parse = |p: &str| {
            v.pointer(p)
                .filter(|x| !x.is_null())
                .and_then(|x| serde_json::from_value::<PermissionRules>(x.clone()).ok())
        };
        let (version, schema) = written_by(&v);
        Previous {
            version,
            schema,
            generated: v
                .get("generated")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(k, h)| Some((k.clone(), h.as_str()?.to_string())))
                .collect(),
            // A state from before the rules were recorded: what its configuration
            // generates (this ocgen's rules for it) is the closest record there is.
            rules: parse("/generated_rules").or_else(|| {
                let mut legacy = v.clone();
                migrate_state(&mut legacy).ok()?;
                serde_json::from_value::<Project>(legacy)
                    .ok()
                    .map(|p| p.generated_permissions())
            }),
            user_rules: parse("/claude/permissions").unwrap_or_default(),
            servers: v
                .pointer("/claude/mcp_servers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| s.get("name")?.as_str().map(String::from))
                .collect(),
            codeowners: v
                .pointer("/claude/intent/codeowners")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Permission rules in `settings.json` and MCP servers in `.mcp.json` that
    /// ocgen didn't write: neither generated nor the user's own, now or at the last
    /// write (a rule ocgen stopped generating, or one the user removed, isn't one).
    /// A regeneration drops them unless they're adopted ([`Project::adopt`]).
    pub fn hand_added(&self, target: &Path) -> HandAdded {
        let mut found = HandAdded::default();
        let project_tree = self.claude.output.project || !self.claude.output.plugin;
        if self.target != Target::ClaudeCode || !project_tree {
            return found;
        }
        let prev = self.previous(target);
        // A file still exactly as the last write left it holds nothing added by hand.
        let untouched = |rel: &str| {
            prev.generated.get(rel).is_some_and(|h| {
                fs::read_to_string(target.join(rel)).is_ok_and(|s| fingerprint(&s) == *h)
            })
        };
        // One linked in from elsewhere (a shared config) isn't read either: ocgen
        // never writes through it, so it couldn't keep what it adopted from it.
        let linked = |rel: &str| is_symlink(&target.join(rel));
        found.linked = [".claude/settings.json", ".mcp.json"]
            .into_iter()
            .filter(|rel| linked(rel))
            .map(String::from)
            .collect();
        if !linked(".claude/settings.json") && !untouched(".claude/settings.json") {
            let raw = self.hand_added_permissions(target);
            // In the same list only: a copy of a generated `ask` rule put under
            // `deny` by hand is the user's own, stricter rule.
            let known = |rules: &PermissionRules, list: RuleList, rule: &str| {
                rules.list(list).iter().any(|r| r == rule)
            };
            for (list, rule) in raw.rules.entries() {
                if prev.rules.as_ref().is_some_and(|r| known(r, list, &rule))
                    || known(&prev.user_rules, list, &rule)
                {
                    continue;
                }
                let _ = found.rules.add(list, &rule);
            }
            found.invalid = raw.invalid;
        }
        if !linked(".mcp.json") && !untouched(".mcp.json") {
            let read = |rel: &str| {
                fs::read_to_string(target.join(rel))
                    .ok()
                    .filter(|_| !linked(rel))
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            };
            let approved: Vec<String> = read(".claude/settings.json")
                .and_then(|s| s.get("enabledMcpjsonServers").cloned())
                .and_then(|a| serde_json::from_value(a).ok())
                .unwrap_or_default();
            let mcp = read(".mcp.json");
            let servers = mcp
                .as_ref()
                .and_then(|m| m.get("mcpServers"))
                .and_then(Value::as_object);
            for (name, v) in servers.into_iter().flatten() {
                if self.claude.mcp_servers.iter().any(|s| s.name == *name)
                    || prev.servers.contains(name)
                {
                    continue; // ocgen's now, or removed from the project on purpose
                }
                if crate::validate::ident(name).is_err() {
                    found.invalid_servers.push(name.clone());
                    continue;
                }
                let mut s = mcp_server_from_json(name, v);
                s.pre_approve = approved.contains(name);
                found.servers.push(s);
            }
        }
        found
    }

    /// Make what [`Project::hand_added`] found the project's own, so every later
    /// regeneration keeps it. Returns the rules that couldn't be adopted, and why.
    pub fn adopt(&mut self, found: &HandAdded) -> Vec<String> {
        let mut problems = Vec::new();
        for (list, rule) in found.rules.entries() {
            if let Err(e) = self.claude.permissions.add(list, &rule) {
                problems.push(format!(
                    "settings.json: {e} — the hand-added rule was dropped"
                ));
            }
        }
        for s in &found.servers {
            if !self.claude.mcp_servers.iter().any(|m| m.name == s.name) {
                self.claude.mcp_servers.push(s.clone());
            }
        }
        problems
    }

    /// Install the git `pre-push` half of the approval gate into the repository's
    /// hooks — only when the gate is on, the target is a git repository, and no
    /// other pre-push hook exists. Never overwrites someone else's hook.
    ///
    /// The hook records every gated project root in the repository (one
    /// `# ocgen:root <dir>` line each), so it also gates a project that isn't at
    /// the repository root (a monorepo service). Roots recorded by other projects
    /// are kept; ones whose folder is gone are dropped. An ocgen hook edited by
    /// hand is copied to `pre-push.ocgen-bak` before it is rewritten.
    pub fn install_pre_push(&self, target: &Path) -> Result<PrePush> {
        let gated = self.target == Target::ClaudeCode
            && self.claude.team.enabled
            && self.claude.team.approval_gate
            && (self.claude.output.project || !self.claude.output.plugin);
        if !gated {
            return Ok(PrePush::NotApplicable);
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(target)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) else {
            return Ok(PrePush::NotApplicable);
        };
        let hook = PathBuf::from(common).join("hooks/pre-push");
        // A custom hooks path (husky, lefthook…) means .git/hooks isn't used.
        if git(&["config", "core.hooksPath"]).is_some_and(|p| !p.is_empty()) {
            return Ok(PrePush::Foreign(hook));
        }
        // Only a missing file is "no hook": one we can't read, or that isn't
        // text (a compiled hook), is someone else's.
        let existing = match fs::read(&hook) {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Ok(PrePush::Foreign(hook)),
        };
        if existing
            .as_deref()
            .is_some_and(|e| !e.contains("ocgen:pre-push"))
        {
            return Ok(PrePush::Foreign(hook));
        }
        const ROOT: &str = "# ocgen:root ";
        let me = crate::paths::for_shell(
            &fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf()),
        );
        let mut roots: Vec<String> = Vec::new();
        let recorded = existing
            .iter()
            .flat_map(|e| e.lines())
            .filter_map(|l| l.strip_prefix(ROOT))
            .filter(|r| Path::new(r).is_dir())
            .map(str::to_string);
        for r in recorded.chain([me]) {
            if !roots.contains(&r) {
                roots.push(r);
            }
        }
        let script = templates::load_embedded("claude/hooks/git-pre-push.sh")?;
        let mut body = script.clone();
        for r in &roots {
            body.push_str(&format!("{ROOT}{r}\n"));
        }
        if let Some(dir) = hook.parent() {
            fs::create_dir_all(dir)?;
        }
        if let Some(old) = &existing {
            let unrooted: String = old
                .lines()
                .filter(|l| !l.starts_with(ROOT))
                .map(|l| format!("{l}\n"))
                .collect();
            if unrooted != script {
                let bak = hook.with_file_name("pre-push.ocgen-bak");
                fs::write(&bak, old).with_context(|| format!("writing {}", bak.display()))?;
            }
        }
        fs::write(&hook, body).with_context(|| format!("writing {}", hook.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
        }
        Ok(PrePush::Installed(hook))
    }

    /// The `.gitattributes` files ocgen adds its LF block to: (relative path, block).
    fn gitattributes(&self) -> Vec<(String, &'static str)> {
        if self.target != Target::ClaudeCode {
            return Vec::new();
        }
        let plugin = self.claude.output.plugin;
        let mut files = Vec::new();
        if self.claude.output.project || !plugin {
            files.push((".gitattributes".to_string(), GITATTRIBUTES_PROJECT));
        }
        if plugin {
            files.push((
                format!("plugin/{}/.gitattributes", self.plugin_name()),
                GITATTRIBUTES_PLUGIN,
            ));
        }
        files
    }

    /// Files to remove: the ones the last write generated that `files` (this
    /// render) doesn't, and ones an older ocgen generated under names it no longer
    /// uses — only while their content is still ocgen's. Also returns the files
    /// the render now spells differently only in case where the file system sees
    /// one file (old path, new path): those are renamed, never removed.
    fn stale_files(
        &self,
        fence: &Fence,
        files: &[(PathBuf, String)],
        previous: &BTreeMap<String, String>,
    ) -> (Vec<Stale>, Vec<(PathBuf, PathBuf)>) {
        let target = fence.target;
        let rendered: BTreeSet<String> = files.iter().map(|(rel, _)| rel_str(rel)).collect();
        let mut stale: Vec<Stale> = Vec::new();
        let mut renames = Vec::new();
        let state_files = Target::state_files();
        for (rel, recorded) in previous {
            let rel_path = Path::new(rel);
            if rendered.contains(rel)
                || !plain_rel(rel_path)
                || user_owned(rel_path)
                || state_files.contains(&rel.as_str())
                || rel.ends_with(".gitattributes")
                || rel == PRE_PUSH_SCRIPT
                || fence.path(rel_path).is_err()
            {
                continue;
            }
            let path = target.join(rel_path);
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                continue;
            }
            if let Some(new) = rendered
                .iter()
                .find(|r| r.to_lowercase() == rel.to_lowercase())
            {
                let new = target.join(new);
                if same_file(&path, &new) {
                    renames.push((path, new));
                    continue;
                }
            }
            let hand_edited = match fs::read(&path).map(String::from_utf8) {
                Ok(Ok(s)) => fingerprint(&s) != *recorded,
                _ => true,
            };
            stale.push(Stale {
                path,
                rel: rel.clone(),
                hand_edited: Some(hand_edited),
            });
        }
        for (rel, hand_edited) in self.legacy_generated(target, files) {
            // E.g. a `.claude/commands` linked in from elsewhere isn't the project's.
            let Ok(path) = fence.strict(Path::new(&rel)) else {
                continue;
            };
            if !stale.iter().any(|s| s.rel == rel) {
                stale.push(Stale {
                    path,
                    rel,
                    hand_edited,
                });
            }
        }
        (stale, renames)
    }

    /// Files an older ocgen generated under names it no longer uses — only those
    /// that open as ocgen's did, so a user's own file is never listed — as
    /// relative paths, each with whether it was edited by hand: `Some(false)` only
    /// when all of it is what ocgen renders now (`files`), else `None` (unknown).
    fn legacy_generated(
        &self,
        target: &Path,
        files: &[(PathBuf, String)],
    ) -> Vec<(String, Option<bool>)> {
        if self.target != Target::ClaudeCode {
            return Vec::new();
        }
        let mut roots = vec![".claude".to_string()];
        if self.claude.output.plugin {
            roots.push(format!("plugin/{}", self.plugin_name()));
        }
        let rendered = |rel: &str| {
            files
                .iter()
                .find(|(r, _)| rel_str(r) == rel)
                .map(|(_, c)| c.as_str())
        };
        let mut stale = Vec::new();
        for root in roots {
            // Workflow commands moved to skills; an old copy opens with that
            // command's own frontmatter description.
            for name in crate::claude::WORKFLOW_SKILLS {
                let rel = format!("{root}/commands/{name}.md");
                let Ok(old) = fs::read_to_string(target.join(&rel)) else {
                    continue;
                };
                if !self.legacy_command(name, &old) {
                    continue;
                }
                // Untouched: it is the skill ocgen renders now, in command form.
                let user_run = crate::claude::USER_RUN_WORKFLOWS.contains(&name);
                let untouched =
                    rendered(&format!("{root}/skills/{name}/SKILL.md")).is_some_and(|skill| {
                        lf(&command_to_skill(name, &lf(&old), user_run)) == lf(skill)
                    });
                stale.push((rel, untouched.then_some(false)));
            }
            // The old `Concise` style shadowed Claude Code's built-in style of that name.
            let rel = format!("{root}/output-styles/concise.md");
            if fs::read_to_string(target.join(&rel)).is_ok_and(|old| {
                old.contains("name: Concise")
                    && old.contains("Lead each answer with the result or recommendation")
            }) {
                stale.push((rel, None));
            }
        }
        stale
    }

    /// Whether `content` is the `/name` command an older ocgen wrote to
    /// `.claude/commands/`: it opens with that command's description (as ocgen
    /// renders it now, or did before), not just any frontmatter.
    fn legacy_command(&self, name: &str, content: &str) -> bool {
        /// Descriptions that changed after the commands moved to skills.
        const FORMER: [(&str, &str); 1] = [(
            "inquire",
            "Understand a codebase by asking questions — sharpen each one, answer with evidence, get nudged to the next",
        )];
        let content = lf(content);
        let Some(line) = content
            .strip_prefix("---\ndescription: ")
            .and_then(|rest| rest.lines().next())
        else {
            return false;
        };
        if FORMER.iter().any(|(n, d)| *n == name && *d == line) {
            return true;
        }
        let Ok(source) = templates::load(&format!("claude/commands/{name}.md.j2")) else {
            return false;
        };
        let Some(template) = source
            .strip_prefix("---\ndescription: ")
            .and_then(|rest| rest.lines().next())
        else {
            return false;
        };
        let env = Environment::new();
        ["English", "Ukrainian", self.language.as_str()]
            .iter()
            .any(|lang| {
                env.render_str(template, context! { language => lang })
                    .is_ok_and(|d| d == line)
            })
    }

    /// What a forced re-scaffold (`doctor`) would do, file by file: add, modify,
    /// remove (files ocgen no longer generates) or leave unchanged. User-owned files
    /// (`CLAUDE.md`, the `/intent` templates) that already exist are never part of
    /// the plan. Line endings don't count: a CRLF checkout is unchanged.
    pub fn plan_changes(&self, target: &Path) -> Result<Vec<FileChange>> {
        use std::io::ErrorKind;
        let previous = self.previous(target);
        let files = self.render_all()?;
        let fence = Fence::new(target);
        // The state file isn't part of the plan, but is written with it.
        fence.path(Path::new(self.target.state_file()))?;
        let mut plan = Vec::new();
        for (rel, new) in &files {
            let rel_s = rel_str(rel);
            if user_owned(rel) && fs::symlink_metadata(target.join(rel)).is_ok() {
                continue;
            }
            let path = fence.path(rel)?;
            // Unknown without fingerprints; with them, a file ocgen didn't write
            // last time (e.g. a `.mcp.json` made by `claude mcp add`) is someone's.
            let tracked = !previous.generated.is_empty();
            // After a case-only rename the file on disk is the old spelling's.
            let recorded = previous.generated.get(&rel_s).or_else(|| {
                previous
                    .generated
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&rel_s))
                    .map(|(_, h)| h)
            });
            let edited = |old: Option<&str>| match (recorded, old) {
                (Some(h), Some(old)) => Some(*h != fingerprint(old)),
                (Some(_), None) => Some(true),
                (None, _) => tracked.then_some(true),
            };
            let (kind, old, hand_edited) = match fs::read(&path) {
                Err(e) if e.kind() == ErrorKind::NotFound => (ChangeKind::Added, None, None),
                // Unreadable, or not UTF-8 (e.g. UTF-16 from PowerShell): never
                // treated as absent — it is overwritten only with a backup.
                Err(_) => (ChangeKind::Modified, None, edited(None)),
                Ok(bytes) => match String::from_utf8(bytes) {
                    Err(_) => (ChangeKind::Modified, None, edited(None)),
                    Ok(old) if lf(&old) == lf(new) => (ChangeKind::Unchanged, None, None),
                    Ok(old) => {
                        let hand = edited(Some(&old));
                        (ChangeKind::Modified, Some(old), hand)
                    }
                },
            };
            let new = (kind != ChangeKind::Unchanged).then(|| new.clone());
            plan.push(FileChange {
                path,
                rel: rel_s,
                kind,
                old,
                new,
                hand_edited,
            });
        }
        for (rel, block) in self.gitattributes() {
            // The user's file: one linked in from elsewhere is left alone.
            let Ok(path) = fence.path(Path::new(&rel)) else {
                continue;
            };
            let (kind, old, new) = match fs::read(&path) {
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    (ChangeKind::Added, None, Some(block.to_string()))
                }
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(old) => match with_gitattributes_block(Some(&old), block) {
                        None => (ChangeKind::Unchanged, None, None),
                        Some(new) => (ChangeKind::Modified, Some(old), Some(new)),
                    },
                    Err(_) => continue, // not text: the user's, left alone
                },
                Err(_) => continue,
            };
            // ocgen only ever appends its block to the user's file.
            let hand_edited = (kind == ChangeKind::Modified).then_some(false);
            plan.push(FileChange {
                path,
                rel,
                kind,
                old,
                new,
                hand_edited,
            });
        }
        // The linked CODEOWNERS: ocgen only ever changes its own block in it.
        if let Some((path, rel, old, new)) = self.codeowners_change(&fence, &BTreeSet::new()) {
            let (kind, old, hand_edited) = match new {
                Some(_) => (ChangeKind::Modified, Some(old), Some(false)),
                None => (ChangeKind::Unchanged, None, None),
            };
            plan.push(FileChange {
                path,
                rel,
                kind,
                old,
                new,
                hand_edited,
            });
        }
        let (stale, _) = self.stale_files(&fence, &files, &previous.generated);
        for s in stale {
            let old = fs::read_to_string(&s.path).ok();
            plan.push(FileChange {
                path: s.path,
                rel: s.rel,
                kind: ChangeKind::Removed,
                old,
                new: None,
                hand_edited: s.hand_edited,
            });
        }
        Ok(plan)
    }

    /// Copy the current version of every file the plan would modify or remove into
    /// `<target>/.ocgen-backup/<UTC stamp>/` (the folder git-ignores itself), byte
    /// for byte. Returns the backup folder, or `None` when nothing would be
    /// overwritten.
    pub fn backup(target: &Path, plan: &[FileChange]) -> Result<Option<PathBuf>> {
        let at_risk: Vec<&FileChange> = plan
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Modified | ChangeKind::Removed))
            .filter(|c| plain_rel(Path::new(&c.rel)))
            .collect();
        if at_risk.is_empty() {
            return Ok(None);
        }
        let root = target.join(".ocgen-backup");
        fs::create_dir_all(&root)?;
        let ignore = root.join(".gitignore");
        if !ignore.exists() {
            fs::write(&ignore, "*\n")?;
        }
        let mut dir = root.join(utc_stamp());
        let mut n = 1;
        while dir.exists() {
            n += 1;
            dir = root.join(format!("{}-{n}", utc_stamp()));
        }
        for c in at_risk {
            let bytes = match fs::read(&c.path) {
                Ok(b) => b,
                Err(e) => match &c.old {
                    Some(old) => old.clone().into_bytes(),
                    None if e.kind() == std::io::ErrorKind::NotFound => continue,
                    None => {
                        return Err(e).with_context(|| {
                            format!(
                                "backing up {} (it can't be read, so it isn't overwritten)",
                                c.rel
                            )
                        })
                    }
                },
            };
            let dest = dir.join(&c.rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, bytes).with_context(|| format!("backing up {}", c.rel))?;
        }
        Self::prune_backups(target, 5)?;
        Ok(Some(dir))
    }

    /// Keep only the newest `keep` backup folders (names sort chronologically).
    pub fn prune_backups(target: &Path, keep: usize) -> Result<()> {
        let root = target.join(".ocgen-backup");
        let Ok(entries) = fs::read_dir(&root) else {
            return Ok(());
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        let excess = dirs.len().saturating_sub(keep);
        for d in dirs.into_iter().take(excess) {
            fs::remove_dir_all(&d).with_context(|| format!("pruning {}", d.display()))?;
        }
        Ok(())
    }

    /// Consistency problems worth surfacing: agents pointing at an unknown
    /// provider or a model the provider doesn't offer, a missing/duplicate
    /// coordinator, or an undefined utility provider. Used by `landscape`.
    /// Named colours Claude Code accepts.
    const CLAUDE_COLORS: [&'static str; 8] = [
        "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
    ];

    /// Consistency checks for a Claude Code project (aliases + coordinators).
    fn claude_issues(&self) -> Vec<String> {
        let mut w = Vec::new();
        for a in &self.agents {
            let m = if a.model.trim().is_empty() {
                "opus"
            } else {
                a.model.trim()
            };
            if !crate::claude::is_valid_agent_model(m) {
                w.push(format!(
                    "agent '{}' uses unknown model alias '{}'",
                    a.name, a.model
                ));
            }
            w.extend(Self::agent_field_issues(a));
            for s in crate::claude::split_list(&a.mcp_servers) {
                if !self.claude.mcp_servers.iter().any(|m| m.name == s) {
                    w.push(format!(
                        "agent '{}' uses MCP server '{s}', which isn't defined (add it with `ocgen add mcp`)",
                        a.name
                    ));
                }
            }
        }
        for m in &self.claude.mcp_servers {
            for k in m.literal_secrets() {
                w.push(format!(
                    "MCP server '{}': {k} holds a literal value — reference an env var instead, e.g. ${{{k}}}",
                    m.name
                ));
            }
        }
        let primaries = self.agents.iter().filter(|a| a.mode == "primary").count();
        if primaries > 1 {
            w.push(format!(
                "{primaries} coordinators — usually there is a single primary agent"
            ));
        }
        for s in &self.skills {
            if crate::claude::WORKFLOW_SKILLS.contains(&s.name.as_str()) {
                w.push(format!(
                    "skill '{}': collides with the generated /{} workflow skill — rename it",
                    s.name, s.name
                ));
            }
            for issue in crate::claude::skill_issues(s) {
                w.push(format!("skill '{}': {issue}", s.name));
            }
        }
        w.extend(self.language_issues());
        w
    }

    /// Invalid values in an agent's enum fields (effort, permission mode, memory),
    /// plus a call-out for `bypassPermissions`.
    fn agent_field_issues(a: &Agent) -> Vec<String> {
        use crate::claude::{EFFORT_LEVELS, MEMORY_SCOPES, PERMISSION_MODES};
        let mut w = Vec::new();
        let bad = |v: &str, ok: &[&str]| !v.is_empty() && !ok.contains(&v);
        if bad(a.effort.trim(), &EFFORT_LEVELS) {
            w.push(format!(
                "agent '{}': unknown effort '{}' (use {})",
                a.name,
                a.effort,
                EFFORT_LEVELS.join("/")
            ));
        }
        if bad(a.permission_mode.trim(), &PERMISSION_MODES) {
            w.push(format!(
                "agent '{}': unknown permission mode '{}' (use {})",
                a.name,
                a.permission_mode,
                PERMISSION_MODES.join("/")
            ));
        }
        if a.permission_mode.trim() == "bypassPermissions" {
            w.push(format!(
                "agent '{}': permissionMode bypassPermissions skips every permission check — prefer acceptEdits or a narrower tools list",
                a.name
            ));
        }
        if bad(a.memory.trim(), &MEMORY_SCOPES) {
            w.push(format!(
                "agent '{}': unknown memory scope '{}' (use {})",
                a.name,
                a.memory,
                MEMORY_SCOPES.join("/")
            ));
        }
        w
    }

    /// Repair a Claude Code project: fix invalid model aliases, colours, and empty roles.
    fn claude_doctor(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        for a in &mut self.agents {
            let m = a.model.trim().to_string();
            if !crate::claude::is_valid_agent_model(&m) {
                fixes.push(format!(
                    "agent '{}': model alias '{}' → opus",
                    a.name, a.model
                ));
                a.model = "opus".into();
            }
            let c = a.color.trim().to_string();
            if !c.is_empty() && !Self::CLAUDE_COLORS.contains(&c.as_str()) {
                fixes.push(format!("agent '{}': colour '{}' → blue", a.name, a.color));
                a.color = "blue".into();
            }
            for (label, field, ok) in [
                ("effort", &mut a.effort, &crate::claude::EFFORT_LEVELS[..]),
                (
                    "permission mode",
                    &mut a.permission_mode,
                    &crate::claude::PERMISSION_MODES[..],
                ),
                (
                    "memory scope",
                    &mut a.memory,
                    &crate::claude::MEMORY_SCOPES[..],
                ),
            ] {
                let v = field.trim().to_string();
                if !v.is_empty() && !ok.contains(&v.as_str()) {
                    fixes.push(format!("agent '{}': unknown {label} '{v}' → unset", a.name));
                    field.clear();
                }
            }
            if a.role.trim().is_empty() {
                a.role = "custom".into();
                fixes.push(format!("agent '{}': empty role → 'custom'", a.name));
            }
        }
        if self.claude.team.enabled {
            let m = self.claude.team.mode.trim().to_string();
            if m.is_empty() || !["in-process", "auto", "tmux", "iterm2"].contains(&m.as_str()) {
                fixes.push(format!(
                    "teammate mode '{}' → in-process",
                    self.claude.team.mode
                ));
                self.claude.team.mode = "in-process".into();
            }
            if self.claude.team.confidence_threshold > 100 {
                fixes.push(format!(
                    "team confidence threshold {} → 96",
                    self.claude.team.confidence_threshold
                ));
                self.claude.team.confidence_threshold = 96;
            }
        }
        if self.claude.workflow.subagent_confidence > 100 {
            fixes.push(format!(
                "subagent confidence {} → 96",
                self.claude.workflow.subagent_confidence
            ));
            self.claude.workflow.subagent_confidence = 96;
        }
        fixes
    }

    pub fn issues(&self) -> Vec<String> {
        if self.target == Target::ClaudeCode {
            return self.claude_issues();
        }
        let mut warnings = Vec::new();
        let keys: std::collections::HashSet<&str> =
            self.providers.iter().map(|p| p.key.as_str()).collect();

        for a in &self.agents {
            match self.providers.iter().find(|p| p.key == a.provider) {
                None => warnings.push(format!(
                    "agent '{}' uses unknown provider '{}'",
                    a.name, a.provider
                )),
                Some(prov) => {
                    if !prov.models.iter().any(|m| m.id == a.model) {
                        warnings.push(format!(
                            "agent '{}' uses model '{}' not offered by provider '{}'",
                            a.name, a.model, a.provider
                        ));
                    }
                }
            }
        }

        let primaries = self.agents.iter().filter(|a| a.mode == "primary").count();
        if primaries == 0 {
            warnings.push("no primary agent — there is no coordinator or /multi command".into());
        } else if primaries > 1 {
            warnings.push(format!(
                "{primaries} primary agents — usually there is a single coordinator"
            ));
        }

        if !keys.contains(self.utility_provider.as_str()) {
            warnings.push(format!(
                "utility provider '{}' is not defined",
                self.utility_provider
            ));
        }
        warnings.extend(self.language_issues());
        warnings
    }

    /// Delete a Claude subagent's generated file (used on rename) — only the entry
    /// spelled exactly so, and only while it still is that agent's file: on a
    /// case-insensitive file system `Reviewer.md` also opens a renamed `reviewer.md`.
    pub fn remove_claude_agent_file(target: &Path, name: &str) -> Result<()> {
        let p = target.join(format!(".claude/agents/{name}.md"));
        let declares = |md: &str| {
            lf(md)
                .strip_prefix("---\n")
                .and_then(|rest| rest.split("\n---\n").next())
                .is_some_and(|front| front.lines().any(|l| l.trim() == format!("name: {name}")))
        };
        if has_entry(&p) && fs::read_to_string(&p).is_ok_and(|md| declares(&md)) {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        Ok(())
    }

    /// Delete a Claude skill's generated directory (used on rename).
    /// Create a skill's supporting-file stubs — `reference.md` (detail loaded on
    /// demand) and `scripts/README.md` (deterministic helpers) — only where absent.
    /// ocgen never rewrites these, so they're the user's from then on. Returns the
    /// files it created.
    pub fn scaffold_skill_extras(
        target: &Path,
        name: &str,
        reference: bool,
        scripts: bool,
    ) -> Result<Vec<PathBuf>> {
        let dir = target.join(format!(".claude/skills/{name}"));
        let mut made = Vec::new();
        let mut stub = |rel: &str, body: &str| -> Result<()> {
            let p = dir.join(rel);
            if !p.exists() {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&p, body).with_context(|| format!("writing {}", p.display()))?;
                made.push(p);
            }
            Ok(())
        };
        if reference {
            stub(
                "reference.md",
                &format!(
                    "# {name} — reference\n\nDetail SKILL.md points to and Claude reads only when needed:\n\
                     checklists, schemas, examples, edge cases. Keep SKILL.md itself short.\n"
                ),
            )?;
        }
        if scripts {
            stub(
                "scripts/README.md",
                "# Scripts\n\nDeterministic helpers SKILL.md tells Claude to run (e.g. `scripts/check.sh`),\n\
                 so the same logic isn't re-derived every time. Scope them in allowed-tools as\n\
                 Bash(scripts/check.sh:*).\n",
            )?;
        }
        Ok(made)
    }

    /// After a rename (and a re-scaffold that wrote the new SKILL.md), carry the
    /// old skill dir's supporting files over, then remove the old dir.
    pub fn rename_claude_skill_dir(target: &Path, old: &str, new: &str) -> Result<()> {
        let from = target.join(format!(".claude/skills/{old}"));
        let to = target.join(format!(".claude/skills/{new}"));
        if !from.exists() || from == to {
            return Ok(());
        }
        if same_file(&from, &to) {
            // Only the case differs and the file system sees one folder.
            let _ = fs::rename(&from, &to);
            return Ok(());
        }
        fs::create_dir_all(&to)?;
        for entry in fs::read_dir(&from)? {
            let entry = entry?;
            let name = entry.file_name();
            let dest = to.join(&name);
            if name == "SKILL.md" || dest.exists() {
                continue;
            }
            fs::rename(entry.path(), &dest).with_context(|| {
                format!("moving {} to {}", entry.path().display(), dest.display())
            })?;
        }
        Self::remove_claude_skill_dir(target, old)
    }

    pub fn remove_claude_skill_dir(target: &Path, name: &str) -> Result<()> {
        let p = target.join(format!(".claude/skills/{name}"));
        if has_entry(&p) {
            fs::remove_dir_all(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        Ok(())
    }

    /// Delete an agent's generated files (its agent md and any prompt file).
    /// Used to clean up stale files after an agent is renamed.
    pub fn remove_agent_artifacts(target: &Path, name: &str) -> Result<()> {
        for rel in [
            format!(".opencode/agents/{name}.md"),
            format!(".opencode/prompts/{name}.txt"),
        ] {
            let p = target.join(rel);
            // The exact entry only (see `remove_claude_agent_file`).
            if has_entry(&p) {
                fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        Ok(())
    }

    /// Load a previously scaffolded project's state from `target`, migrating the
    /// JSON forward from older schema versions first so older projects still load.
    pub fn load_state(target: &Path) -> Result<Project> {
        let path = state_file_in(target).ok_or_else(|| {
            anyhow!(
                "no ocgen project found at {} (missing {})",
                target.display(),
                Target::OpenCode.state_file()
            )
        })?;
        let data =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut value: Value =
            serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
        let (version, schema) = written_by(&value);
        let loaded = migrate_state(&mut value)
            .and_then(|()| Ok(serde_json::from_value::<Project>(value)?))
            .with_context(|| format!("loading {}", path.display()));
        let project = match loaded {
            Ok(p) => p,
            // A newer ocgen's state this one can't read: say which ocgen it needs.
            Err(e) => {
                let rel = rel_str(path.strip_prefix(target).unwrap_or(&path));
                return Err(match refuse_newer(&rel, &version, schema) {
                    Err(newer) => e.context(newer.to_string()),
                    Ok(()) => e,
                });
            }
        };
        project.check_names().with_context(|| {
            format!(
                "{} holds a name ocgen can't use in a file path — fix it there",
                path.display()
            )
        })?;
        Ok(project)
    }

    /// Every name that becomes part of a file path — agents, skills, MCP servers,
    /// provider keys and the plugin folder — must be a plain identifier, so a state
    /// file can't make ocgen write (or remove) anything outside the project.
    fn check_names(&self) -> Result<()> {
        use crate::validate::ident;
        for a in &self.agents {
            ident(&a.name).map_err(|e| anyhow!("agent name '{}': {e}", a.name))?;
        }
        for s in &self.skills {
            if !crate::claude::is_valid_skill_name(&s.name) {
                bail!(
                    "skill name '{}': use lowercase letters, digits and '-'",
                    s.name
                );
            }
        }
        for m in &self.claude.mcp_servers {
            ident(&m.name).map_err(|e| anyhow!("MCP server name '{}': {e}", m.name))?;
        }
        for p in &self.providers {
            ident(&p.key).map_err(|e| anyhow!("provider key '{}': {e}", p.key))?;
        }
        if self.target == Target::ClaudeCode && self.claude.output.plugin {
            let name = self.plugin_name();
            crate::validate::folder_name(&name)
                .map_err(|e| anyhow!("plugin folder '{name}': {e}"))?;
        }
        Ok(())
    }

    /// Repair a loaded project in place: reassign agents pointing at unknown
    /// providers/models, fill empty required fields, and fix the utility target.
    /// Returns a human-readable list of what was changed.
    pub fn doctor(&mut self) -> Vec<String> {
        if self.target == Target::ClaudeCode {
            return self.claude_doctor();
        }
        let mut fixes = Vec::new();
        let providers = self.providers.clone();
        let keys: Vec<String> = providers.iter().map(|p| p.key.clone()).collect();
        let fallback = if keys.contains(&self.utility_provider) {
            self.utility_provider.clone()
        } else {
            keys.first().cloned().unwrap_or_default()
        };

        // Utility provider + model must exist.
        if !keys.contains(&self.utility_provider) {
            if let Some(k) = keys.first() {
                fixes.push(format!(
                    "utility provider '{}' undefined → '{}'",
                    self.utility_provider, k
                ));
                self.utility_provider = k.clone();
            }
        }
        if let Some(prov) = providers.iter().find(|p| p.key == self.utility_provider) {
            if !prov.models.iter().any(|m| m.id == self.utility_model) {
                if let Some(m) = prov.models.first() {
                    fixes.push(format!(
                        "utility model '{}' not in '{}' → '{}'",
                        self.utility_model, self.utility_provider, m.id
                    ));
                    self.utility_model = m.id.clone();
                }
            }
        }

        for a in &mut self.agents {
            if !matches!(a.mode.as_str(), "primary" | "subagent" | "all") {
                fixes.push(format!(
                    "agent '{}': invalid mode '{}' → subagent",
                    a.name, a.mode
                ));
                a.mode = "subagent".into();
            }
            if a.role.trim().is_empty() {
                a.role = "custom".into();
                fixes.push(format!("agent '{}': empty role → 'custom'", a.name));
            }
            if !keys.contains(&a.provider) {
                fixes.push(format!(
                    "agent '{}': unknown provider '{}' → '{}'",
                    a.name, a.provider, fallback
                ));
                a.provider = fallback.clone();
            }
            if let Some(prov) = providers.iter().find(|p| p.key == a.provider) {
                if !prov.models.iter().any(|m| m.id == a.model) {
                    if let Some(m) = prov.models.first() {
                        fixes.push(format!(
                            "agent '{}': model '{}' not in '{}' → '{}'",
                            a.name, a.model, a.provider, m.id
                        ));
                        a.model = m.id.clone();
                    }
                }
            }
            if a.temperature.trim().is_empty() {
                a.temperature = "0.2".into();
                fixes.push(format!("agent '{}': empty temperature → 0.2", a.name));
            }
            if a.color.trim().is_empty() {
                a.color = "accent".into();
                fixes.push(format!("agent '{}': empty color → accent", a.name));
            }
            if a.permissions.trim().is_empty() {
                a.permissions = "  edit: ask\n  bash:\n    \"*\": ask".into();
                fixes.push(format!(
                    "agent '{}': empty permissions → default block",
                    a.name
                ));
            }
            let empty_prompt = a
                .prompt_body
                .as_deref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true);
            if a.prompt_file && empty_prompt {
                a.prompt_file = false;
                a.prompt_body = None;
                fixes.push(format!(
                    "agent '{}': prompt file enabled but body empty → disabled",
                    a.name
                ));
            }
        }
        fixes
    }

    /// Find the project root by looking for the state file in `start` and each of
    /// its ancestor directories (like `git` locating `.git`), and load it.
    /// Returns the resolved root alongside the project.
    pub fn discover(start: &Path) -> Result<(PathBuf, Project)> {
        let root = find_root(start).ok_or_else(|| {
            anyhow!(
                "no ocgen project found in {} or any parent directory\n\
                 run this from inside a project, pass its path, or create one with `ocgen new`",
                start.display()
            )
        })?;
        let project = Self::load_state(&root)?;
        Ok((root, project))
    }
}

/// Bring an older state-file JSON up to the current schema before typed loading:
/// reconstruct `providers` from the old single-provider fields and backfill agent
/// fields (role/provider and archetype-derived values) that older versions omitted.
fn migrate_state(value: &mut Value) -> Result<()> {
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("English")
        .to_string();

    // Old layout stored one provider inline (provider_key/provider_name/npm/...).
    let has_providers = value
        .get("providers")
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !has_providers {
        if let Some(key) = value
            .get("provider_key")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let name = value
                .get("provider_name")
                .and_then(Value::as_str)
                .unwrap_or(&key)
                .to_string();
            let npm = value
                .get("npm")
                .and_then(Value::as_str)
                .unwrap_or("@ai-sdk/openai-compatible")
                .to_string();
            let base_url = value
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let models = value.get("models").cloned().unwrap_or_else(|| json!([]));
            if let Some(obj) = value.as_object_mut() {
                obj.entry("utility_provider")
                    .or_insert_with(|| json!(key.clone()));
                obj.insert(
                    "providers".into(),
                    json!([{ "key": key, "name": name, "npm": npm, "base_url": base_url, "models": models }]),
                );
                // Consumed: not unknown keys to carry forward.
                for k in ["provider_key", "provider_name", "npm", "base_url", "models"] {
                    obj.remove(k);
                }
            }
        }
    }

    // Determine a default provider key for agents that lack one.
    let first_key = value
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|p| p.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(obj) = value.as_object_mut() {
        obj.entry("utility_provider")
            .or_insert_with(|| json!(first_key));
    }
    let default_provider = value
        .get("utility_provider")
        .and_then(Value::as_str)
        .unwrap_or(&first_key)
        .to_string();

    if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            migrate_agent(agent, &language, &default_provider)?;
        }
    }
    Ok(())
}

/// Backfill a single agent's newer fields, using its legacy `archetype` as the
/// source of truth for permissions/body/etc. where those fields are absent.
fn migrate_agent(agent: &mut Value, language: &str, default_provider: &str) -> Result<()> {
    let archetype = agent
        .get("archetype")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = agent
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let provider = agent
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(default_provider)
        .to_string();

    // Seed from the archetype only if some archetype-derived field is missing.
    let derived = [
        "mode",
        "model",
        "temperature",
        "steps",
        "color",
        "permissions",
        "body",
        "prompt_file",
        "prompt_body",
        "description",
    ];
    let needs_seed = derived.iter().any(|k| agent.get(k).is_none());
    let seed_val = if needs_seed {
        let seed = match &archetype {
            Some(arch) => Agent::from_archetype(&name, arch, language, &provider)
                .unwrap_or_else(|_| Agent::blank(&name, arch, &provider)),
            None => Agent::blank(&name, "custom", &provider),
        };
        Some(serde_json::to_value(seed)?)
    } else {
        None
    };

    let obj = agent
        .as_object_mut()
        .ok_or_else(|| anyhow!("agent entry in state file is not a JSON object"))?;

    obj.entry("role")
        .or_insert_with(|| json!(archetype.clone().unwrap_or_else(|| "custom".into())));
    obj.entry("provider")
        .or_insert_with(|| json!(default_provider));

    if let Some(seed) = &seed_val {
        for key in derived {
            if !obj.contains_key(key) {
                if let Some(v) = seed.get(key) {
                    obj.insert(key.into(), v.clone());
                }
            }
        }
    }
    // Consumed above (it lives on as `role`): not an unknown key to carry forward.
    obj.remove("archetype");
    Ok(())
}

/// Walk up from `start` looking for a directory that holds the state file.
fn find_root(start: &Path) -> Option<PathBuf> {
    // Plain, not verbatim: this path is handed to git, bash and hook environments.
    let start = crate::paths::plain(&start.canonicalize().ok()?);
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        if state_file_in(dir).is_some() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// The ocgen version and state schema a state file (as JSON) records: empty and
/// 0 for one from before they were recorded.
fn written_by(state: &Value) -> (String, u32) {
    let version = state
        .get("ocgen_version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let schema = state
        .get("schema")
        .and_then(Value::as_u64)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    (version, schema)
}

/// Refuse to write over a state a newer ocgen wrote (`version`, `schema`): this
/// one would drop the settings it doesn't know and downgrade the generated files.
fn refuse_newer(state_file: &str, version: &str, schema: u32) -> Result<()> {
    let newer = matches!(
        (semver(version), semver(crate::VERSION)),
        (Some(theirs), Some(ours)) if theirs > ours
    );
    if schema <= STATE_SCHEMA && !newer {
        return Ok(());
    }
    let by = match version.trim() {
        "" => "a newer ocgen".to_string(),
        v => format!("ocgen {v}"),
    };
    let needed = if newer {
        format!("ocgen {} or newer", version.trim())
    } else {
        format!("an ocgen that reads state schema {schema}")
    };
    bail!(
        "{state_file} was last written by {by} (state schema {schema}), and this is ocgen {} (schema {STATE_SCHEMA}); upgrade to {needed} to change this project — an older ocgen would drop the settings it doesn't know",
        crate::VERSION
    )
}

/// An agent's prompt body rendered as a template (the archetype prompts loop over
/// the subagents). A body that isn't one — e.g. a GitHub Actions `${{ … }}`, a
/// Helm `{{ .Values }}` or a `{{ version }}` ocgen knows nothing about, in the
/// user's own text — is used as written, with a warning (once per agent).
fn body_as_template(env: &Environment, agent: &str, body: &str, ctx: minijinja::Value) -> String {
    // An unknown value fails instead of printing as nothing, so no text is lost.
    let mut env = env.clone();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::SemiStrict);
    match env.render_str(body, ctx) {
        Ok(text) => text,
        Err(e) => {
            static WARNED: std::sync::Mutex<BTreeSet<String>> =
                std::sync::Mutex::new(BTreeSet::new());
            if WARNED
                .lock()
                .map(|mut w| w.insert(agent.to_string()))
                .unwrap_or(true)
            {
                eprintln!(
                    "warning: agent '{agent}': its prompt isn't a template ocgen can render ({e}) — written as plain text"
                );
            }
            body.to_string()
        }
    }
}

mod codeowners;
pub use codeowners::{check_link as check_codeowners_link, found as found_codeowners};

mod language;
pub use language::{match_language, LanguageChange};

#[cfg(test)]
mod write_tests;

/// Turn a rendered command file (`---` frontmatter + body) into a skill: add its
/// `name` and, for side-effecting workflows, `disable-model-invocation: true`.
fn command_to_skill(name: &str, command_md: &str, user_run: bool) -> String {
    let rest = command_md.strip_prefix("---\n").unwrap_or(command_md);
    let (front, body) = match rest.split_once("\n---\n") {
        Some((f, b)) => (f, b),
        None => ("", rest),
    };
    let mut out = format!("---\nname: {name}\n");
    if !front.is_empty() {
        out.push_str(front);
        out.push('\n');
    }
    if user_run {
        out.push_str("disable-model-invocation: true\n");
    }
    out.push_str("---\n");
    out.push_str(body);
    out
}

/// A `type: "command"` hook entry. The commands are POSIX sh, so the shell is
/// pinned: left to its default, Claude Code on Windows uses PowerShell when Git
/// Bash is missing, where the command fails to parse and the gate fails open.
fn command_hook(command: impl Into<String>) -> Value {
    json!({ "type": "command", "command": command.into(), "shell": "bash" })
}

/// A hook command: run `ocgen hook <name>` when the installed ocgen speaks exactly
/// this project's hook protocol, otherwise the bundled script — so a project works
/// without the binary, and a stale binary can never apply outdated gate logic.
/// `prefix` carries env assignments.
fn hook_cmd(prefix: &str, dir: &str, script: &str) -> String {
    let name = script.trim_end_matches(".sh");
    let protocol = crate::hooks::PROTOCOL;
    format!(
        "if [ \"$(ocgen hook --check 2>/dev/null)\" = \"{protocol}\" ]; then {prefix}ocgen hook {name}; else {prefix}sh \"{dir}/{script}\"; fi"
    )
}
