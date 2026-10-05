//! The notes viewer stays inside `.claude/notes`, never stalls on a browser
//! command, and isn't kept alive by strangers: `ocgen notes render` and the
//! hidden `notes serve` refuse anything that isn't a ledger or a notes
//! directory, the `OCGEN_NOTES_BROWSER` override runs detached like the system
//! opener, only authenticated requests count as viewer activity, and
//! concurrent renders never share a temp file.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use ocgen::notes::viewer::{self, Info};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::TempDir;

const LEDGER: &str = "Topic: Request flow\nUpdated: 2026-10-01   Commit: abc1234\n\
## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\nCites: src/main.rs:1\n";

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

/// A git repo (so short and canonical temp paths agree on Windows) with one
/// ledger and a docs page that isn't one.
fn project() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let _ = StdCommand::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status();
    let notes = dir.path().join(".claude/notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("flow.md"), LEDGER).unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("docs/design.md"), "# Design\n").unwrap();
    fs::write(dir.path().join("docs/design.html"), "<p>hand-written</p>\n").unwrap();
    (dir, notes)
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A fake browser that sleeps `secs` and then logs the target it was given.
fn slow_browser(dir: &Path, secs: u32) -> (String, PathBuf) {
    let log = dir.join("opened.log");
    let fake = dir.join("slow-browser.sh");
    fs::write(
        &fake,
        format!(
            "sleep {secs}\nprintf '%s\\n' \"$1\" >> '{}'\n",
            ocgen::paths::for_shell(&log)
        ),
    )
    .unwrap();
    (format!("sh '{}'", ocgen::paths::for_shell(&fake)), log)
}

fn wait_for_file(path: &Path, within: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < within {
        if fs::read_to_string(path).is_ok_and(|s| !s.is_empty()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

// ------------------------------------------------- render stays in notes --

#[test]
fn notes_render_refuses_a_markdown_file_that_is_not_a_ledger() {
    let (dir, _) = project();
    let docs = dir.path().join("docs");
    ocgen()
        .args(["notes", "render"])
        .arg(docs.join("design.md"))
        .assert()
        .failure()
        .stderr(contains("not an /inquire ledger").and(contains(".claude/notes/")));
    assert_eq!(
        fs::read_to_string(docs.join("design.html")).unwrap(),
        "<p>hand-written</p>\n",
        "a real page next to it is left alone"
    );
    assert!(!docs.join(".gitignore").exists(), "git still sees docs/");

    // A bare file name resolves against the current directory: the repo root.
    fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();
    ocgen()
        .args(["notes", "render", "README.md"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("not an /inquire ledger"));
    assert!(!dir.path().join(".gitignore").exists());
    assert!(!dir.path().join("README.html").exists());
}

#[test]
fn notes_render_refuses_a_badly_named_ledger_and_a_path_out_of_notes() {
    let (dir, notes) = project();
    let bad = notes.join("Bad Name.md");
    fs::write(&bad, LEDGER).unwrap();
    ocgen()
        .args(["notes", "render"])
        .arg(&bad)
        .assert()
        .failure()
        .stderr(contains("lowercase-hyphen slug"));
    assert!(!notes.join("Bad Name.html").exists());

    // `..` out of the notes directory isn't a ledger, whatever it starts with.
    ocgen()
        .args(["notes", "render", ".claude/notes/../../docs/design.md"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("not an /inquire ledger"));
    assert!(!dir.path().join("docs/.gitignore").exists());

    // One bad argument stops the batch before anything is written.
    ocgen()
        .args(["notes", "render", ".claude/notes/flow.md", "docs/design.md"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("docs/design.md"));
    assert!(!notes.join("flow.html").exists());
}

#[test]
fn notes_render_takes_a_ledger_by_any_path_that_reaches_it() {
    let (dir, notes) = project();
    // Relative to the project, and a bare name from inside the notes directory.
    ocgen()
        .args(["notes", "render", ".claude/notes/flow.md"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(contains("flow.html"));
    fs::remove_file(notes.join("flow.html")).unwrap();
    fs::remove_file(notes.join(".gitignore")).unwrap();
    ocgen()
        .args(["notes", "render", "flow.md"])
        .current_dir(&notes)
        .assert()
        .success();
    assert!(notes.join("flow.html").is_file());
    assert_eq!(fs::read_to_string(notes.join(".gitignore")).unwrap(), "*\n");
}

#[test]
fn render_file_refuses_non_ledgers_for_every_caller() {
    let (dir, _) = project();
    let md = dir.path().join("docs/design.md");
    let err = ocgen::notes::render_file(&md).unwrap_err();
    assert!(
        format!("{err:#}").contains("not an /inquire ledger"),
        "{err:#}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("docs/design.html")).unwrap(),
        "<p>hand-written</p>\n"
    );
    assert!(!dir.path().join("docs/.gitignore").exists());
}

/// On Unix a `\` is part of a file name, not a separator: a file in the repo
/// root named like a ledger path is not one, and its page would land there.
#[cfg(unix)]
#[test]
fn a_backslash_in_a_unix_file_name_does_not_make_a_ledger() {
    let (dir, _) = project();
    fs::write(dir.path().join("index.html"), "<p>home</p>\n").unwrap();
    let odd = dir.path().join(r"x\.claude\notes\index.md");
    fs::write(&odd, LEDGER).unwrap();
    let err = ocgen::notes::render_file(&odd).unwrap_err();
    assert!(
        format!("{err:#}").contains("not an /inquire ledger"),
        "{err:#}"
    );
    ocgen()
        .args(["notes", "render", r"x\.claude\notes\index.md"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("not an /inquire ledger"));
    assert_eq!(
        fs::read_to_string(dir.path().join("index.html")).unwrap(),
        "<p>home</p>\n"
    );
}

#[test]
fn notes_serve_refuses_a_directory_that_is_not_a_notes_directory() {
    let (dir, _) = project();
    let docs = dir.path().join("docs");
    ocgen()
        .args(["notes", "serve"])
        .arg(&docs)
        .env("OCGEN_NOTES_IDLE_SECS", "1")
        .timeout(Duration::from_secs(20))
        .assert()
        .failure()
        .stderr(contains(
            "not a .claude/notes, .claude/intent/drafts or .claude/intent/viewer directory",
        ));
    assert!(!docs.join(viewer::INFO_FILE).exists());
    // `.claude` itself isn't one either.
    ocgen()
        .args(["notes", "serve"])
        .arg(dir.path().join(".claude"))
        .env("OCGEN_NOTES_IDLE_SECS", "1")
        .timeout(Duration::from_secs(20))
        .assert()
        .failure();
}

// ------------------------------------------------ the browser override --

#[test]
fn browser_override_runs_detached() {
    let dir = tempfile::tempdir().unwrap();
    let (browser, log) = slow_browser(dir.path(), 5);
    let start = Instant::now();
    ocgen::notes::browser::open("page.html", &env(&[("OCGEN_NOTES_BROWSER", &browser)])).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "open waited {:?} for the browser command",
        start.elapsed()
    );
    // It still runs, in the background.
    assert!(wait_for_file(&log, Duration::from_secs(20)), "never ran");
    assert_eq!(fs::read_to_string(&log).unwrap(), "page.html\n");
}

#[test]
fn browser_override_wait_switch_is_synchronous_and_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (browser, log) = slow_browser(dir.path(), 1);
    ocgen::notes::browser::open(
        "page.html",
        &env(&[
            ("OCGEN_NOTES_BROWSER", &browser),
            ("OCGEN_NOTES_BROWSER_WAIT", "1"),
        ]),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap_or_default(),
        "page.html\n",
        "logged before open returned"
    );
    let err = ocgen::notes::browser::open(
        "page.html",
        &env(&[
            ("OCGEN_NOTES_BROWSER", "exit 3;"),
            ("OCGEN_NOTES_BROWSER_WAIT", "1"),
        ]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("failed"), "{err}");
}

#[test]
fn notes_open_returns_while_the_browser_command_still_runs() {
    let (dir, notes) = project();
    let (browser, log) = slow_browser(dir.path(), 12);
    let start = Instant::now();
    ocgen()
        .args(["notes", "open", "flow", "--path"])
        .arg(dir.path())
        .env("OCGEN_NOTES_BROWSER", &browser)
        .env("OCGEN_NOTES_OPEN", "1")
        .env("OCGEN_NOTES_IDLE_SECS", "30")
        .env_remove("OCGEN_NOTES_BROWSER_WAIT")
        .timeout(Duration::from_secs(30))
        .assert()
        .success()
        .stdout(contains("Opened"));
    let took = start.elapsed();
    if let Some(info) = viewer::read_info(&notes) {
        viewer::quit(&info);
    }
    assert!(took < Duration::from_secs(8), "notes open took {took:?}");
    assert!(wait_for_file(&log, Duration::from_secs(30)), "never ran");
}

// ------------------------------------------------------- idle shutdown --

/// `ocgen notes serve` on `notes`, idling out after `idle` seconds; returns it
/// once its address card is up.
fn serve(notes: &Path, idle: &str) -> (Child, Info) {
    let mut child = StdCommand::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["notes", "serve"])
        .arg(notes)
        .env("OCGEN_NOTES_IDLE_SECS", idle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(info) = viewer::read_info(notes) {
            return (child, info);
        }
        if start.elapsed() > Duration::from_secs(10) || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!("the viewer did not start");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A raw GET; the status code (0 when the server is gone) and the response.
fn get(port: u16, path: &str, host: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))) else {
        return (0, String::new());
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (code, buf)
}

/// A raw request; the status code (0 when the server is gone).
fn http(port: u16, path: &str, host: &str) -> u16 {
    get(port, path, host).0
}

/// Keep sending `path` (with `host`) for up to `secs`; whether the server
/// exited meanwhile.
fn exits_while_probed(child: &mut Child, port: u16, path: &str, host: &str, secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        http(port, path, host);
        std::thread::sleep(Duration::from_millis(100));
    }
    let exited = child.try_wait().unwrap().is_some();
    let _ = child.kill();
    let _ = child.wait();
    exited
}

#[test]
fn unauthenticated_requests_do_not_keep_the_viewer_alive() {
    let (_dir, notes) = project();
    // A foreign Host header (403) and a wrong token (404) are not activity.
    let (mut child, info) = serve(&notes, "1");
    let port = info.port;
    let bad_host = |i: u16| format!("evil.example:{i}");
    let mut n = 0u16;
    let start = Instant::now();
    let mut exited = false;
    while start.elapsed() < Duration::from_secs(6) {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        n = n.wrapping_add(1);
        assert!(matches!(
            http(port, &format!("/{}/ping", info.token), &bad_host(n)),
            403 | 0
        ));
        assert!(matches!(
            http(
                port,
                "/0123456789abcdef0123456789abcdef/ping",
                &format!("127.0.0.1:{port}")
            ),
            404 | 0
        ));
        std::thread::sleep(Duration::from_millis(100));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(exited, "rejected requests kept the idle viewer running");
    assert!(
        viewer::read_info(&notes).is_none(),
        "it cleaned up its card"
    );
}

#[test]
fn authenticated_requests_still_keep_the_viewer_alive() {
    let (_dir, notes) = project();
    let (mut child, info) = serve(&notes, "2");
    let host = format!("127.0.0.1:{}", info.port);
    let ping = format!("/{}/ping", info.token);
    assert!(
        !exits_while_probed(&mut child, info.port, &ping, &host, 3),
        "a client in use was dropped"
    );
}

/// The viewer serves its notes directory by the canonical path, which no
/// longer ends in `.claude/notes` when `.claude` is a link (say, into a dotfiles
/// store). It must still re-render a page whose ledger changed behind the
/// hook's back.
#[cfg(unix)]
#[test]
fn the_viewer_re_renders_through_a_linked_claude_dir() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store/proj-claude");
    fs::create_dir_all(store.join("notes")).unwrap();
    let proj = dir.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    std::os::unix::fs::symlink(&store, proj.join(".claude")).unwrap();
    let notes = proj.join(".claude/notes");
    let md = notes.join("flow.md");
    fs::write(&md, LEDGER).unwrap();
    ocgen::notes::render_file(&md).unwrap();

    let (mut child, info) = serve(&notes, "30");
    // An edit the hook didn't see (an editor, a `sed`): the ledger is newer.
    fs::write(&md, LEDGER.replace("Like this.", "Like that, MARKER-42.")).unwrap();
    fs::File::options()
        .write(true)
        .open(&md)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let (code, body) = get(
        info.port,
        &format!("/{}/flow.html", info.token),
        &format!("127.0.0.1:{}", info.port),
    );
    viewer::quit(&info);
    let _ = child.wait();
    assert_eq!(code, 200);
    assert!(body.contains("MARKER-42"), "served a stale page");
}

// --------------------------------------------------- concurrent renders --

#[test]
fn concurrent_renders_never_fail_or_leave_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join(".claude/notes");
    fs::create_dir_all(&notes).unwrap();
    let md = notes.join("flow.md");
    let html = notes.join("flow.html");
    let version = |tag: &str| LEDGER.replace("Request flow", &format!("Request flow {tag}"));
    let (a, b) = (version("a"), version("b"));
    fs::write(&md, &a).unwrap();
    ocgen::notes::render_file(&md).unwrap();

    // The viewer re-renders on each GET of a page older than its ledger, one
    // thread per connection; meanwhile the ledger keeps changing.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (stop, md) = (Arc::clone(&stop), md.clone());
        std::thread::spawn(move || {
            let mut flip = false;
            while !stop.load(Ordering::SeqCst) {
                flip = !flip;
                let tmp = md.with_extension("md.w");
                fs::write(&tmp, if flip { &b } else { &a }).unwrap();
                let _ = fs::rename(&tmp, &md);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let reader = {
        let (stop, html) = (Arc::clone(&stop), html.clone());
        std::thread::spawn(move || {
            let mut partial = 0;
            while !stop.load(Ordering::SeqCst) {
                if let Ok(page) = fs::read(&html) {
                    if !String::from_utf8_lossy(&page)
                        .trim_end()
                        .ends_with("</html>")
                    {
                        partial += 1;
                    }
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            partial
        })
    };
    let renderers: Vec<_> = (0..8)
        .map(|_| {
            let md = md.clone();
            std::thread::spawn(move || {
                (0..20)
                    .filter_map(|_| ocgen::notes::render_file(&md).err())
                    .map(|e| format!("{e:#}"))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let errors: Vec<String> = renderers
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    let partial = reader.join().unwrap();
    assert!(
        errors.is_empty(),
        "{} failed renders: {:?}",
        errors.len(),
        errors.first()
    );
    assert_eq!(partial, 0, "a reader saw a partly written page");
    let leftovers: Vec<String> = fs::read_dir(&notes)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
