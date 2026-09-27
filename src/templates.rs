//! Template resolution: user override dir first, embedded defaults second.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

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

/// Load a template by its relative path (e.g. `"manifest.toml"`,
/// `"archetypes/reviewer.toml"`, `"opencode/agents/_agent.md.j2"`).
/// An override file, if present, wins over the embedded default.
pub fn load(path: &str) -> Result<String> {
    if let Some(dir) = override_dir() {
        let p = dir.join(path);
        if p.is_file() {
            let src = fs::read_to_string(&p)
                .with_context(|| format!("reading override template {}", p.display()))?;
            return Ok(normalize_newlines(src));
        }
    }
    let file = Assets::get(path).ok_or_else(|| anyhow!("template not found: {path}"))?;
    let src = String::from_utf8(file.data.into_owned())
        .with_context(|| format!("template {path} is not valid UTF-8"))?;
    Ok(normalize_newlines(src))
}

/// Normalize CRLF to LF. Templates checked out on Windows (git autocrlf) or an
/// override edited on Windows arrive with `\r\n`; the generator must always emit
/// LF so rendered files — and `\n`-based assertions — behave the same everywhere.
fn normalize_newlines(s: String) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n")
    } else {
        s
    }
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

/// Names of every available project preset (embedded ∪ override), sorted.
pub fn preset_names() -> Vec<String> {
    let mut set = BTreeSet::new();
    for path in Assets::iter() {
        if let Some(name) = path
            .strip_prefix("presets/")
            .and_then(|n| n.strip_suffix(".toml"))
        {
            set.insert(name.to_string());
        }
    }
    if let Some(dir) = override_dir() {
        if let Ok(entries) = fs::read_dir(dir.join("presets")) {
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

/// Copy the embedded template set into the override dir so the user can edit it.
/// Returns the directory it wrote to.
pub fn init_override() -> Result<PathBuf> {
    let dir = override_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    for path in Assets::iter() {
        let file = Assets::get(&path).expect("embedded path must resolve");
        let dest = dir.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, file.data.as_ref())
            .with_context(|| format!("writing {}", dest.display()))?;
    }
    Ok(dir)
}

/// List every resolved template path with whether an override copy exists.
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

/// Every editable template path: the embedded defaults unioned with any files
/// present in the override dir (which may include user-added templates such as a
/// new archetype). Sorted and de-duplicated.
pub fn editable_paths() -> Vec<String> {
    merge_paths(override_dir().as_deref())
}

/// Save `content` as the override for template `rel_path`, creating parent
/// directories as needed. Returns the file it wrote.
pub fn save_override(rel_path: &str, content: &str) -> Result<PathBuf> {
    let dir = override_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    let dest = dir.join(rel_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&dest, content).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}

/// Embedded template paths ∪ files under `override_dir`. Factored out so the
/// merge logic can be unit-tested against an arbitrary directory.
fn merge_paths(override_dir: Option<&std::path::Path>) -> Vec<String> {
    let mut set: BTreeSet<String> = Assets::iter().map(|p| p.to_string()).collect();
    if let Some(dir) = override_dir {
        collect_files(dir, dir, &mut set);
    }
    set.into_iter().collect()
}

/// Recursively add every file under `dir` to `out`, keyed by its path relative
/// to `root` (with `/` separators).
fn collect_files(root: &std::path::Path, dir: &std::path::Path, out: &mut BTreeSet<String>) {
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
    fn crlf_template_content_is_normalized_to_lf() {
        // Reproduces the Windows autocrlf failure: a CRLF-checked-out template
        // (what `all_opencode_agent_fields_render` renders) must read as LF, so
        // `options:\n  reasoningEffort: high` matches on every platform.
        let crlf = "options:\r\n  reasoningEffort: high\r\n".to_string();
        assert!(
            !crlf.contains("options:\n  reasoningEffort: high"),
            "precondition: CRLF content fails the LF assertion (the bug)"
        );

        let lf = normalize_newlines(crlf);
        assert!(lf.contains("options:\n  reasoningEffort: high"));
        assert!(!lf.contains('\r'), "no carriage returns survive");

        // Already-LF content is returned unchanged.
        assert_eq!(normalize_newlines("a\nb\n".to_string()), "a\nb\n");
    }

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
}
