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
/// `^`, `|` specially): loopback, a port, a hex token and a page — a ledger's
/// slug, or a draft's or intent file's name ([`super::draft::is_name`]) — or a
/// list (of drafts, ledgers or intent files), optionally open on one of them.
pub fn is_viewer_url(url: &str) -> bool {
    regex::Regex::new(
        r"^http://127\.0\.0\.1:\d{1,5}/[0-9a-f]{8,64}/(?:[A-Za-z0-9][A-Za-z0-9_-]{0,199}\.html|_(?:drafts|notes|intents)(?:/[A-Za-z0-9][A-Za-z0-9_-]{0,199})?)$",
    )
    .unwrap()
    .is_match(url)
}

/// Open `target` (a viewer URL or a file path) in the default browser, without
/// waiting for it. `OCGEN_NOTES_BROWSER` replaces the system opener (`sh -c
/// "$CMD \"$1\""`, like `OCGEN_FORMAT_CMD`) and runs detached like it: a browser
/// command such as `firefox` may not return until the browser quits, which
/// would stall the hook and `ocgen notes open`.
///
/// `OCGEN_NOTES_BROWSER_WAIT=1` is for tests only: it waits for the override and
/// reports its failure, so a fake browser's log is complete when this returns.
pub fn open(target: &str, env: &HashMap<String, String>) -> std::io::Result<()> {
    if let Some(cmd) = env
        .get("OCGEN_NOTES_BROWSER")
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
    {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(format!("{cmd} \"$1\""))
            .arg("ocgen-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if env
            .get("OCGEN_NOTES_BROWSER_WAIT")
            .is_some_and(|v| v.trim() == "1")
        {
            return if c.status()?.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("'{cmd}' failed")))
            };
        }
        super::viewer::detach(&mut c);
        return c.spawn().map(|_| ());
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
