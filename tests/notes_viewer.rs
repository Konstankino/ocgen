//! The live viewer, end to end through the real binary: the `inquire-notes` hook
//! opens a ledger's page once, refreshes the tab that shows it on later updates,
//! and opens a new one only when no tab is left. The browser is a fake
//! (`OCGEN_NOTES_BROWSER`) that logs the URLs it is asked to open; a test client
//! plays the open tab by holding the page's event stream.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use ocgen::notes::viewer::{self, Info, INFO_FILE};
use tempfile::TempDir;

const LEDGER: &str = "Topic: Request flow\nUpdated: 2026-10-01   Commit: abc1234\n\
## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\nCites: src/main.rs:1\n";
const UK_LEDGER: &str =
    "Topic: Потік запиту\n## Q&A log\n### Q1 · Flow · Verified\nQ: Як?\nA: Так.\n";

struct Project {
    dir: TempDir,
    log: PathBuf,
    browser: String,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join(".claude/notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(notes.join("flow.md"), LEDGER).unwrap();
        let log = dir.path().join("opened.log");
        let fake = dir.path().join("fake-browser.sh");
        fs::write(
            &fake,
            format!(
                "printf '%s\\n' \"$1\" >> '{}'\n",
                ocgen::paths::for_shell(&log)
            ),
        )
        .unwrap();
        let browser = format!("sh '{}'", ocgen::paths::for_shell(&fake));
        Project { dir, log, browser }
    }

    /// A project whose prompts are English and answers Ukrainian, with its
    /// ledger in Ukrainian.
    fn bilingual() -> Self {
        let p = Project::new();
        let mut state = ocgen::render::Project::from_manifest(
            &ocgen::manifest::Manifest::load().unwrap(),
            "English",
        );
        state.target = ocgen::target::Target::ClaudeCode;
        state.project_name = "bi".into();
        state.providers.clear();
        state.response_language = "Ukrainian".into();
        state.agents = vec![];
        state.scaffold(p.dir.path(), false).unwrap();
        fs::write(p.md(), UK_LEDGER).unwrap();
        p
    }

    fn notes(&self) -> PathBuf {
        self.dir.path().join(".claude/notes")
    }

    fn md(&self) -> PathBuf {
        self.notes().join("flow.md")
    }

    /// Run the hook as Claude Code would after a write to the ledger.
    fn hook(&self, env: &[(&str, &str)]) -> Output {
        let out = self.hook_on(&self.md(), env);
        assert!(out.stdout.is_empty(), "{out:?}");
        out
    }

    /// Run the hook as Claude Code would after a write to `md`.
    fn hook_on(&self, md: &std::path::Path, env: &[(&str, &str)]) -> Output {
        let payload = serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": ocgen::paths::for_shell(md) }
        });
        let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        c.args(["hook", "inquire-notes"])
            .env(
                "CLAUDE_PROJECT_DIR",
                ocgen::paths::for_shell(self.dir.path()),
            )
            .env("OCGEN_NOTES_OPEN", "1")
            .env("OCGEN_NOTES_BROWSER", &self.browser)
            // The override runs detached; wait for it so the log is complete.
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .env("OCGEN_NOTES_GRACE_MS", "5000")
            .env("OCGEN_NOTES_IDLE_SECS", "60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        out
    }

    fn opened(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn info(&self) -> Info {
        viewer::read_info(&self.notes()).expect("a viewer is running")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if let Some(info) = viewer::read_info(&self.notes()) {
            viewer::quit(&info);
        }
    }
}

/// A raw HTTP request; `(status, body)`.
fn http(port: u16, method: &str, path: &str, host: &str) -> (u16, String) {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    (
        code,
        buf.split_once("\r\n\r\n")
            .map(|x| x.1.to_string())
            .unwrap_or_default(),
    )
}

fn get(info: &Info, path: &str) -> (u16, String) {
    http(info.port, "GET", path, &format!("127.0.0.1:{}", info.port))
}

/// An open tab: the page's event stream.
struct Tab(TcpStream);

impl Tab {
    fn connect(info: &Info, rev: &str) -> Tab {
        let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
        write!(
            s,
            "GET /{}/events?topic=flow&rev={rev} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            info.token, info.port
        )
        .unwrap();
        let mut tab = Tab(s);
        assert!(tab.wait_for("retry:"), "the stream opens");
        // The server registers the tab right after the stream header.
        std::thread::sleep(Duration::from_millis(100));
        tab
    }

    /// Whether `needle` arrives within 3 s.
    fn wait_for(&mut self, needle: &str) -> bool {
        let start = Instant::now();
        let mut seen = String::new();
        self.0
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        while start.elapsed() < Duration::from_secs(3) {
            let mut buf = [0u8; 512];
            if let Ok(n) = self.0.read(&mut buf) {
                if n == 0 {
                    return false;
                }
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                if seen.contains(needle) {
                    return true;
                }
            }
        }
        false
    }
}

/// The revision the served page carries.
fn page_rev(info: &Info) -> String {
    let (code, page) = get(info, &format!("/{}/flow.html", info.token));
    assert_eq!(code, 200);
    let i = page.find("&rev=").expect("live-reload script injected") + 5;
    page[i..i + 16].to_string()
}

#[test]
fn first_update_opens_one_tab_then_reloads_it() {
    let p = Project::new();
    p.hook(&[]);
    let opened = p.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let info = p.info();
    assert_eq!(opened[0], info.url("flow"));
    assert!(ocgen::notes::browser::is_viewer_url(&opened[0]));

    let (code, page) = get(&info, &format!("/{}/flow.html", info.token));
    assert_eq!(code, 200);
    assert!(page.contains("EventSource") && page.contains("Request flow"));
    let mut tab = Tab::connect(&info, &page_rev(&info));

    fs::write(
        p.md(),
        format!("{LEDGER}### Q2 · Failure · Inferred 60%\nQ: And?\n"),
    )
    .unwrap();
    p.hook(&[]);
    assert!(tab.wait_for("event: reload"), "the open tab is refreshed");
    assert_eq!(p.opened().len(), 1, "no second tab");
    assert!(fs::read_to_string(p.notes().join("flow.html"))
        .unwrap()
        .contains("Inferred 60%"));
}

/// A ledger's other language version is served beside it; writing it only
/// refreshes the tabs already open, so a ledger never opens two.
#[test]
fn a_language_version_is_served_and_refreshed_without_opening() {
    let p = Project::bilingual();
    let first = String::from_utf8(p.hook_on(&p.md(), &[]).stdout).unwrap();
    assert!(first.contains("flow.english.md (English)"), "{first}");
    // Already owed: said once.
    assert!(p.hook_on(&p.md(), &[]).stdout.is_empty());
    assert_eq!(p.opened().len(), 1);
    let info = p.info();
    let mut tab = Tab::connect(&info, &page_rev(&info));

    let english = p.notes().join("flow.english.md");
    fs::write(&english, LEDGER).unwrap();
    p.hook_on(&english, &[]);
    assert!(
        tab.wait_for("event: reload"),
        "the Ukrainian page's switch changed"
    );
    assert_eq!(p.opened().len(), 1, "no tab for the English version");

    let t = &info.token;
    let (code, page) = get(&info, &format!("/{t}/flow.english.html"));
    assert_eq!(code, 200);
    assert!(
        page.contains(r#"<html lang="en">"#) && page.contains(r#"href="flow.html""#),
        "{page}"
    );
    let (_, page) = get(&info, &format!("/{t}/flow.html"));
    assert!(page.contains(r#"href="flow.english.html""#), "{page}");
    for path in [
        "flow.English.html",
        "flow..html",
        "flow.a.b.html",
        "flow.english.md",
    ] {
        assert_eq!(get(&info, &format!("/{t}/{path}")).0, 404, "{path}");
    }
}

#[test]
fn updates_while_a_tab_is_loading_do_not_open_duplicates() {
    let p = Project::new();
    p.hook(&[]);
    p.hook(&[]);
    p.hook(&[]);
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
}

#[test]
fn closing_the_tab_means_the_next_update_opens_a_new_one() {
    let p = Project::new();
    let fast = [("OCGEN_NOTES_GRACE_MS", "300")];
    p.hook(&fast);
    let info = p.info();
    let tab = Tab::connect(&info, &page_rev(&info));
    p.hook(&fast);
    assert_eq!(p.opened().len(), 1);
    drop(tab);
    std::thread::sleep(Duration::from_millis(900));
    p.hook(&fast);
    assert_eq!(p.opened().len(), 2, "{:?}", p.opened());
}

#[test]
fn a_stale_page_reloads_as_soon_as_it_connects() {
    let p = Project::new();
    p.hook(&[]);
    let mut tab =
        Tab(TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], p.info().port))).unwrap());
    let info = p.info();
    write!(
        tab.0,
        "GET /{}/events?topic=flow&rev=0000000000000000 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        info.token, info.port
    )
    .unwrap();
    assert!(tab.wait_for("event: reload"));
}

#[test]
fn concurrent_hooks_start_one_viewer() {
    let p = Project::new();
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| p.hook(&[]));
        }
    });
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
    let info = p.info();
    assert_eq!(
        viewer::ping(&info, &viewer::root_key(&p.notes())),
        viewer::Ping::Ours
    );
    let stray: Vec<String> = fs::read_dir(p.notes())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("stale") || n.contains(".tmp"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

#[test]
fn the_hook_returns_while_the_viewer_keeps_running() {
    let p = Project::new();
    let start = Instant::now();
    p.hook(&[]); // output() waits for stdout/stderr to close
    assert!(start.elapsed() < Duration::from_secs(10));
    let info = p.info();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        viewer::ping(&info, &viewer::root_key(&p.notes())),
        viewer::Ping::Ours,
        "the viewer outlives the hook"
    );
}

#[test]
fn a_stale_viewer_file_is_replaced() {
    let p = Project::new();
    let dead_port = {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        l.local_addr().unwrap().port()
    };
    let stale = Info {
        app: "ocgen-notes".into(),
        protocol: viewer::VIEWER_PROTOCOL,
        port: dead_port,
        pid: 1,
        token: "0123456789abcdef".into(),
        root: viewer::root_key(&p.notes()),
    };
    fs::write(
        p.notes().join(INFO_FILE),
        serde_json::to_string(&stale).unwrap(),
    )
    .unwrap();
    p.hook(&[]);
    let info = p.info();
    assert_ne!(info.token, stale.token);
    assert_eq!(p.opened(), [info.url("flow")]);
}

#[test]
fn viewer_refuses_traversal_bad_tokens_and_foreign_hosts() {
    let p = Project::new();
    p.hook(&[]);
    let info = p.info();
    let t = &info.token;
    assert_eq!(get(&info, &format!("/{t}/flow.html")).0, 200);
    for path in [
        format!("/{t}/../flow.md"),
        format!("/{t}/%2e%2e/flow.md"),
        format!("/{t}/.viewer.json"),
        format!("/{t}/flow.md"),
        format!("/{t}/Flow.html"),
        "/0000000000000000/flow.html".to_string(),
        "/flow.html".to_string(),
    ] {
        assert_eq!(get(&info, &path).0, 404, "{path}");
    }
    let page = format!("/{t}/flow.html");
    assert_eq!(http(info.port, "GET", &page, "evil.example").0, 403);
    assert_eq!(
        http(
            info.port,
            "GET",
            &page,
            &format!("evil.example:{}", info.port)
        )
        .0,
        403
    );
    assert_eq!(get(&info, &format!("/{t}/reload?topic=flow")).0, 405);
    assert_eq!(get(&info, &format!("/{t}/quit")).0, 405);
}

#[test]
fn opening_is_off_with_the_opt_out_and_in_ci() {
    for env in [
        &[("OCGEN_NOTES_OPEN", "0")][..],
        &[("OCGEN_NOTES_OPEN", ""), ("CI", "true")][..],
    ] {
        let p = Project::new();
        p.hook(env);
        assert!(p.notes().join("flow.html").is_file(), "still rendered");
        assert!(!p.notes().join(INFO_FILE).exists(), "no viewer started");
        assert!(p.opened().is_empty());
    }
}

#[test]
fn an_idle_viewer_exits_and_removes_its_file() {
    let p = Project::new();
    p.hook(&[("OCGEN_NOTES_IDLE_SECS", "1")]);
    let start = Instant::now();
    while p.notes().join(INFO_FILE).exists() && start.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!p.notes().join(INFO_FILE).exists(), "the idle viewer left");
}
