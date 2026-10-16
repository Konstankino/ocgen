//! Path forms that survive being handed to other programs.
//!
//! `Path::canonicalize` on Windows returns a "verbatim" path (`\\?\C:\Users\…`).
//! Rust's own file APIs accept it, but bash strips the backslashes and git does not
//! understand the prefix — so any path that leaves the process (as an argument, an
//! environment variable, or a key shared with a shell script) must be plain.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `p` without the Windows verbatim prefix (`\\?\C:\x` → `C:\x`,
/// `\\?\UNC\srv\share` → `\\srv\share`). Unchanged on other platforms.
pub fn plain(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// `p` as a shell-safe string: plain, with forward slashes (`C:/Users/x`). Both
/// bash on Windows (Git Bash) and git accept this form, and it needs no escaping
/// in JSON.
pub fn for_shell(p: &Path) -> String {
    plain(p).to_string_lossy().replace('\\', "/")
}

/// A program in `path` (a PATH value) with PATHEXT `exts`; empty `exts` means the bare
/// name. Only extensions `Command` can start (batch files via cmd) count.
pub fn find_in(path: &OsStr, name: &str, exts: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if exts.is_empty() {
        vec![String::new()]
    } else {
        exts.split(';')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.as_str()))
            .collect()
    };
    std::env::split_paths(path)
        .flat_map(|d| {
            exts.iter()
                .map(|e| d.join(format!("{name}{e}")))
                .collect::<Vec<_>>()
        })
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_windows_verbatim_prefix() {
        assert_eq!(
            plain(Path::new(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\srv\share\x")),
            PathBuf::from(r"\\srv\share\x")
        );
        assert_eq!(plain(Path::new("/home/x")), PathBuf::from("/home/x"));
        assert_eq!(
            for_shell(Path::new(r"\\?\C:\Users\x\.claude")),
            "C:/Users/x/.claude"
        );
        assert_eq!(for_shell(Path::new("/home/x/.claude")), "/home/x/.claude");
    }
}
