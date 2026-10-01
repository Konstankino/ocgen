//! Whether to show a ledger's HTML view, and opening it in the user's browser.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

/// The running OS, as [`decide`] names it.
pub fn this_os() -> &'static str {
    std::env::consts::OS
}

/// Whether a ledger update should be shown in a browser.
///
/// `OCGEN_NOTES_OPEN=0` never shows (it always wins); `=1` always does. Otherwise
/// `explicit` requests (`ocgen notes open`) do, and automatic ones (the hook)
/// don't under CI or on a Linux machine with no display.
pub fn decide(env: &HashMap<String, String>, os: &str, explicit: bool) -> bool {
    let get = |k: &str| env.get(k).map(|v| v.trim()).unwrap_or("");
    match get("OCGEN_NOTES_OPEN") {
        "0" | "false" | "no" | "off" => return false,
        "1" | "true" | "yes" | "on" => return true,
        _ => {}
    }
    if explicit {
        return true;
    }
    if !get("CI").is_empty() {
        return false;
    }
    if os == "linux" && get("DISPLAY").is_empty() && get("WAYLAND_DISPLAY").is_empty() {
        return false;
    }
    true
}

/// A viewer URL we are willing to pass to a shell (`cmd /C start` treats `&`,
/// `^`, `|` specially): loopback, a port, a hex token and a slug.
pub fn is_viewer_url(url: &str) -> bool {
    regex::Regex::new(r"^http://127\.0\.0\.1:\d{1,5}/[0-9a-f]{8,64}/[a-z0-9][a-z0-9-]{0,79}\.html$")
        .unwrap()
        .is_match(url)
}

/// Open `target` (a viewer URL or a file path) in the default browser.
/// `OCGEN_NOTES_BROWSER` replaces the system opener (`sh -c "$CMD \"$1\""`, like
/// `OCGEN_FORMAT_CMD`); it is waited for, the system opener is not.
pub fn open(target: &str, env: &HashMap<String, String>) -> std::io::Result<()> {
    if let Some(cmd) = env
        .get("OCGEN_NOTES_BROWSER")
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
    {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{cmd} \"$1\""))
            .arg("ocgen-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("'{cmd}' failed")))
        };
    }
    let mut c = system_opener(target)?;
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::viewer::detach(&mut c);
    c.spawn().map(|_| ())
}

fn system_opener(target: &str) -> std::io::Result<Command> {
    if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(target);
        Ok(c)
    } else if cfg!(windows) {
        if target.starts_with("http") {
            if !is_viewer_url(target) {
                return Err(std::io::Error::other("refusing to open an unexpected URL"));
            }
            let mut c = Command::new("cmd");
            c.args(["/C", "start", "", target]);
            Ok(c)
        } else {
            // A file: explorer opens it with the default .html handler, no shell.
            let mut c = Command::new("explorer.exe");
            c.arg(crate::paths::plain(Path::new(target)));
            Ok(c)
        }
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        Ok(c)
    }
}
