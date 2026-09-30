//! Path forms that survive being handed to other programs.
//!
//! `Path::canonicalize` on Windows returns a "verbatim" path (`\\?\C:\Users\…`).
//! Rust's own file APIs accept it, but bash strips the backslashes and git does not
//! understand the prefix — so any path that leaves the process (as an argument, an
//! environment variable, or a key shared with a shell script) must be plain.

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
