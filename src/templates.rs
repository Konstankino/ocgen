//! Template resolution: user override dir first, embedded defaults second.
//!
//! Hook scripts (and the statusline script) are the exception: they are always
//! the copies embedded in this binary. Each one has a Rust twin (`ocgen hook`)
//! that the generated hook command prefers, and the command is stamped with the
//! hook protocol — so the script a project ships must be the one that protocol
//! means. An override would either bring back a fixed vulnerability after an
//! upgrade, or take effect only on machines without ocgen.
//!
//! So are the templates that teach the gate protocol — the plan, owner, check and
//! confidence lines the hooks parse ([`GATE_PROTOCOL`]). A copy made by an older
//! `ocgen templates init` would keep teaching the old protocol, which the new
//! hooks may read differently (a risk owned by a role holds no one).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rust_embed::RustEmbed;

/// Default template set baked into the binary at compile time.
#[derive(RustEmbed)]
#[folder = "templates/"]
struct Assets;

/// `~/.config/ocgen/templates` — where users keep their edited copies.
pub fn override_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("ocgen").join("templates"))
}

/// The templates that teach the gate protocol: what the hooks parse in a team plan
/// (`Status: APPROVED`, `Owner: <teammate-name>`, `Check:`) and in a worker's last
/// message (`Confidence: NN%`). They can't be overridden (see the module docs).
pub const GATE_PROTOCOL: [&str; 4] = [
    "claude/commands/team.md.j2",
    "claude/commands/team-plan.md.j2",
    "claude/commands/fanout.md.j2",
    "claude/rules/ocgen-team.md.j2",
];

/// Why some templates can't be overridden, for messages.
pub const NOT_OVERRIDABLE: &str =
    "hook scripts and the gate-protocol templates always come from the ocgen binary";

/// Whether a template may be overridden from the override dir. Hook scripts, the
/// statusline script and the [`GATE_PROTOCOL`] templates may not (see the module
/// docs).
pub fn is_overridable(path: &str) -> bool {
    let path = path.replace('\\', "/");
    !(path.starts_with("claude/hooks/")
        || path == "claude/statusline.sh"
        || GATE_PROTOCOL.contains(&path.as_str()))
}

/// Load a template by its relative path (e.g. `"manifest.toml"`,
/// `"archetypes/reviewer.toml"`, `"opencode/agents/_agent.md.j2"`).
/// An override file, if present, wins over the embedded default — except for the
/// templates that can't be overridden ([`is_overridable`]).
pub fn load(path: &str) -> Result<String> {
    load_from(override_dir().as_deref(), path)
}

/// The template embedded in this binary, ignoring any override.
pub fn load_embedded(path: &str) -> Result<String> {
    load_from(None, path)
}

fn load_from(override_dir: Option<&Path>, path: &str) -> Result<String> {
    if let Some(dir) = override_dir.filter(|_| is_overridable(path)) {
        let p = dir.join(path);
        if p.is_file() {
            return fs::read_to_string(&p)
                .with_context(|| format!("reading override template {}", p.display()));
        }
    }
    let file = Assets::get(path).ok_or_else(|| anyhow!("template not found: {path}"))?;
    String::from_utf8(file.data.into_owned())
        .with_context(|| format!("template {path} is not valid UTF-8"))
}

/// Names of every available archetype (embedded ∪ override), sorted.
pub fn archetype_names() -> Vec<String> {
    let mut set = BTreeSet::new();
    for path in Assets::iter() {
        if let Some(name) = path
            .strip_prefix("archetypes/")
            .and_then(|n| n.strip_suffix(".toml"))
        {
            set.insert(name.to_string());
        }
    }
    if let Some(dir) = override_dir() {
        if let Ok(entries) = fs::read_dir(dir.join("archetypes")) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        set.insert(stem.to_string());
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

/// What `ocgen templates init` did.
#[derive(Debug, Default)]
pub struct InitReport {
    /// The override dir.
    pub dir: PathBuf,
    /// Templates copied (or, with `force`, overwritten).
    pub written: Vec<String>,
    /// Existing override files left as they were (no `force`).
    pub kept: Vec<String>,
}

/// Copy the embedded templates into the override dir so the user can edit them.
/// Existing override files are kept unless `force`; templates that can't be
/// overridden (hook scripts, the gate protocol) are never copied.
pub fn init_override(force: bool) -> Result<InitReport> {
    let dir = override_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    init_into(&dir, force)
}

fn init_into(dir: &Path, force: bool) -> Result<InitReport> {
    let mut report = InitReport {
        dir: dir.to_path_buf(),
        ..InitReport::default()
    };
    for path in Assets::iter().filter(|p| is_overridable(p)) {
        let file = Assets::get(&path).expect("embedded path must resolve");
        let dest = dir.join(path.as_ref());
        if dest.exists() && !force {
            report.kept.push(path.to_string());
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, file.data.as_ref())
            .with_context(|| format!("writing {}", dest.display()))?;
        report.written.push(path.to_string());
    }
    Ok(report)
}

/// List every resolved template path with whether an override copy exists on
/// disk (for a template that can't be overridden, that copy is ignored — see
/// [`ignored_overrides`]).
pub fn list() -> Vec<(String, bool)> {
    let dir = override_dir();
    Assets::iter()
        .map(|p| {
            let overridden = dir
                .as_ref()
                .map(|d| d.join(p.as_ref()).is_file())
                .unwrap_or(false);
            (p.to_string(), overridden)
        })
        .collect()
}

/// Files in the override dir that ocgen ignores because their template can't be
/// overridden (e.g. hook scripts or gate-protocol templates left by an older
/// `ocgen templates init`), as paths relative to the override dir. Sorted.
pub fn ignored_overrides() -> Vec<String> {
    override_dir().map(|d| ignored_in(&d)).unwrap_or_default()
}

fn ignored_in(dir: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    collect_files(dir, dir, &mut found);
    found.into_iter().filter(|p| !is_overridable(p)).collect()
}

/// Every editable template path: the embedded defaults unioned with any files
/// present in the override dir (which may include user-added templates such as a
/// new archetype), minus those that can't be overridden. Sorted and de-duplicated.
pub fn editable_paths() -> Vec<String> {
    merge_paths(override_dir().as_deref())
}

/// Save `content` as the override for template `rel_path`, creating parent
/// directories as needed. Returns the file it wrote.
pub fn save_override(rel_path: &str, content: &str) -> Result<PathBuf> {
    if !is_overridable(rel_path) {
        return Err(anyhow!(
            "{rel_path} can't be overridden — {NOT_OVERRIDABLE}"
        ));
    }
    let dir = override_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    let dest = dir.join(rel_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&dest, content).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}

/// Embedded template paths ∪ files under `override_dir`, minus the templates
/// that can't be overridden. Factored out so the merge logic can be unit-tested
/// against an arbitrary directory.
fn merge_paths(override_dir: Option<&Path>) -> Vec<String> {
    let mut set: BTreeSet<String> = Assets::iter().map(|p| p.to_string()).collect();
    if let Some(dir) = override_dir {
        collect_files(dir, dir, &mut set);
    }
    set.into_iter().filter(|p| is_overridable(p)).collect()
}

/// Recursively add every file under `dir` to `out`, keyed by its path relative
/// to `root` (with `/` separators).
fn collect_files(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_paths_unions_embedded_and_override() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("archetypes")).unwrap();
        fs::write(tmp.path().join("archetypes/custom.toml"), "x").unwrap();
        fs::write(tmp.path().join("seeds.toml"), "y").unwrap();

        let paths = merge_paths(Some(tmp.path()));
        assert!(paths.contains(&"manifest.toml".to_string())); // embedded only
        assert!(paths.contains(&"seeds.toml".to_string())); // embedded + override
        assert!(paths.contains(&"archetypes/custom.toml".to_string())); // override only

        // Sorted and de-duplicated (seeds.toml appears once).
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        assert_eq!(paths.iter().filter(|p| *p == "seeds.toml").count(), 1);
    }

    #[test]
    fn hook_scripts_ignore_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("claude/hooks")).unwrap();
        fs::write(tmp.path().join("claude/hooks/notify.sh"), "exit 0\n").unwrap();
        fs::write(tmp.path().join("claude/statusline.sh"), "echo x\n").unwrap();
        fs::write(tmp.path().join("seeds.toml"), "mine = 1\n").unwrap();

        assert_eq!(
            load_from(Some(tmp.path()), "claude/hooks/notify.sh").unwrap(),
            load_embedded("claude/hooks/notify.sh").unwrap()
        );
        assert_eq!(
            load_from(Some(tmp.path()), "seeds.toml").unwrap(),
            "mine = 1\n"
        );
        assert_eq!(
            ignored_in(tmp.path()),
            vec!["claude/hooks/notify.sh", "claude/statusline.sh"]
        );
        assert!(!merge_paths(Some(tmp.path()))
            .iter()
            .any(|p| !is_overridable(p)));
    }

    #[test]
    fn gate_protocol_templates_ignore_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        for path in GATE_PROTOCOL {
            assert!(Assets::get(path).is_some(), "{path} isn't a template");
            let p = tmp.path().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            // What an older `templates init` copied: the old owner protocol.
            fs::write(&p, "Owner: <role>\n").unwrap();
            assert!(!is_overridable(path), "{path}");
            assert_eq!(
                load_from(Some(tmp.path()), path).unwrap(),
                load_embedded(path).unwrap()
            );
        }
        let mut stale = GATE_PROTOCOL.map(String::from).to_vec();
        stale.sort();
        assert_eq!(ignored_in(tmp.path()), stale);
        assert!(!merge_paths(Some(tmp.path()))
            .iter()
            .any(|p| GATE_PROTOCOL.contains(&p.as_str())));
        // Still overridable: the agent prompt, the other commands and rules.
        for path in [
            "claude/agent.md.j2",
            "claude/commands/intake.md.j2",
            "claude/rules/ocgen-workflow.md.j2",
        ] {
            assert!(is_overridable(path), "{path}");
        }

        let fresh = tempfile::tempdir().unwrap();
        let r = init_into(fresh.path(), false).unwrap();
        for path in GATE_PROTOCOL {
            assert!(!r.written.iter().any(|p| p == path), "{path} copied");
            assert!(!fresh.path().join(path).exists(), "{path} copied");
        }
        assert!(fresh.path().join("claude/commands/intake.md.j2").exists());
    }

    #[test]
    fn init_keeps_existing_overrides_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("seeds.toml"), "mine = 1\n").unwrap();

        let r = init_into(tmp.path(), false).unwrap();
        assert_eq!(r.kept, vec!["seeds.toml"]);
        assert!(r.written.iter().any(|p| p == "manifest.toml"));
        assert!(!r.written.iter().any(|p| !is_overridable(p)));
        assert_eq!(
            fs::read_to_string(tmp.path().join("seeds.toml")).unwrap(),
            "mine = 1\n"
        );
        assert!(!tmp.path().join("claude/hooks").exists());

        let r = init_into(tmp.path(), true).unwrap();
        assert!(r.kept.is_empty());
        assert_eq!(
            fs::read_to_string(tmp.path().join("seeds.toml")).unwrap(),
            load_embedded("seeds.toml").unwrap()
        );
    }
}
