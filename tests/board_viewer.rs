//! The `/workstreams` board's live page, end to end through the real binary:
//! the `inquire-notes` hook starts a viewer for `.claude/workstreams`, opens
//! the board's page once, refreshes the tab that shows it on later writes — and
//! on a change the hook never saw — and serves the briefs and reports the page
//! links. The browser is a fake (`OCGEN_NOTES_BROWSER`) that logs the URLs it
//! is asked to open; a test client plays the open tab by holding the page's
//! event stream.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use ocgen::notes::viewer::{self, Info};
use tempfile::TempDir;

const BOARD: &str = "# Workstreams — Ship 0.9.7\n\
Status: APPROVED session=abc123\n\
\n\
## Workstreams\n\
- Workstream: billing-retry — Worker: session billing-retry — Branch: refs/heads/worktree-billing-retry — Worktree: — — Tip: 89abcdef0123456789abcdef0123456789abcdef — State: checked — Gate: ready — After: none\n\
- Workstream: login-timeout — Worker: subagent a1b2 — Branch: refs/heads/worktree-login-timeout — Worktree: — — Tip: — — State: dispatched — Gate: pending — After: billing-retry\n\
\n\
## Log\n\
- 2026-10-24T10:00:00Z billing-retry reported: Check pass, Confidence 97%\n";

struct Project {
    dir: TempDir,
    log: PathBuf,
    browser: String,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join(".claude/workstreams");
        fs::create_dir_all(ws.join("billing-retry")).unwrap();
        fs::create_dir_all(ws.join("login-timeout")).unwrap();
        fs::write(ws.join("board.md"), BOARD).unwrap();
        fs::write(
            ws.join("billing-retry/brief.md"),
            "# Workstream: billing-retry\n\n## Goal\nRetry failed charges.\n",
        )
        .unwrap();
        fs::write(
            ws.join("billing-retry/report.md"),
            "Workstream: billing-retry\nStatus: done\nConfidence: 97%\n",
        )
        .unwrap();
        fs::write(
            ws.join("login-timeout/brief.md"),
            "# Workstream: login-timeout\n",
        )
        .unwrap();
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

    fn ws(&self) -> PathBuf {
        self.dir.path().join(".claude/workstreams")
    }

    fn md(&self) -> PathBuf {
        self.ws().join("board.md")
    }

    fn env(&self, c: &mut Command) {
        c.env(
            "CLAUDE_PROJECT_DIR",
            ocgen::paths::for_shell(self.dir.path()),
        )
        .env("OCGEN_NOTES_OPEN", "1")
        .env("OCGEN_NOTES_BROWSER", &self.browser)
        // The override runs detached; wait for it so the log is complete.
        .env("OCGEN_NOTES_BROWSER_WAIT", "1")
        .env("OCGEN_NOTES_GRACE_MS", "5000")
        .env("OCGEN_NOTES_IDLE_SECS", "60");
    }

    /// Run the hook as Claude Code would after a write to the board.
    fn hook(&self) -> Output {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Write",
            "tool_input": { "file_path": ocgen::paths::for_shell(&self.md()) }
        });
        let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        c.args(["hook", "inquire-notes"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.env(&mut c);
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
        out
    }

    /// `ocgen notes open workstreams`, from a subfolder of the project.
    fn open(&self) -> String {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        c.args(["notes", "open", "workstreams", "-p"])
            .arg(self.ws().join("billing-retry"));
        self.env(&mut c);
        let out = c.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn opened(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn info(&self) -> Info {
        viewer::read_info(&self.ws()).expect("a viewer is running for the board's folder")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if let Some(info) = viewer::read_info(&self.ws()) {
            viewer::quit(&info);
        }
    }
}

/// A raw HTTP request; `(status, headers and body)`.
fn http(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    (code, buf)
}

fn get(info: &Info, rest: &str) -> (u16, String) {
    http(info.port, "GET", &format!("/{}/{rest}", info.token))
}

/// The served board page, and the revision it carries.
fn page(info: &Info) -> (String, String) {
    let (code, raw) = get(info, "board.html");
    assert_eq!(code, 200, "{raw}");
    let body = raw.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    let i = body
        .find(r#"data-rev=""#)
        .expect("the page carries its revision")
        + 10;
    let rev = body[i..].split('"').next().unwrap().to_string();
    (body.to_string(), rev)
}

/// An open tab: the page's event stream.
struct Tab(TcpStream);

impl Tab {
    fn connect(info: &Info, rev: &str) -> Tab {
        let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
        write!(
            s,
            "GET /{}/events?topic=board&rev={rev} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            info.token, info.port
        )
        .unwrap();
        let mut tab = Tab(s);
        assert!(tab.wait_for("retry:"), "the stream opens");
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

#[test]
fn the_first_write_opens_the_board_and_later_writes_reload_the_tab() {
    let p = Project::new();
    p.hook();
    let opened = p.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let info = p.info();
    assert_eq!(opened[0], info.url("board"));
    assert!(ocgen::notes::browser::is_viewer_url(&opened[0]));
    // The static page is still beside the board, for a machine without a viewer.
    assert!(p.ws().join("board.html").is_file());

    let (body, rev) = page(&info);
    for must in [
        r#"<body class="intent-file board""#,
        "EventSource",
        r#"<table class="board">"#,
        r#"<span class="gate gate-ready">ready</span>"#,
        r#"<a href="billing-retry/brief.md">brief</a>"#,
        // The banner when the viewer is gone names the command that brings it back.
        "ocgen notes open workstreams",
        // What moved since the tab last showed the board is highlighted.
        "ocgen-board:",
        "@keyframes board-flash",
    ] {
        assert!(body.contains(must), "missing {must:?} in:\n{body}");
    }
    let mut tab = Tab::connect(&info, &rev);

    fs::write(p.md(), BOARD.replace("Gate: ready", "Gate: merged")).unwrap();
    p.hook();
    assert!(tab.wait_for("event: reload"), "the open tab is refreshed");
    assert_eq!(p.opened().len(), 1, "no second tab");
    let (body, rev2) = page(&info);
    assert!(body.contains(r#"<span class="gate gate-merged">merged<svg class="ico ico-tick""#));
    assert_ne!(rev, rev2);
}

#[test]
fn a_change_the_hook_never_saw_reloads_the_tab() {
    let p = Project::new();
    p.hook();
    let info = p.info();
    let (_, rev) = page(&info);
    let mut tab = Tab::connect(&info, &rev);
    // A hand edit, or one from Bash: the viewer watches the board itself.
    fs::write(p.md(), BOARD.replace("State: checked", "State: integrated")).unwrap();
    assert!(
        tab.wait_for("event: reload"),
        "a watched board reloads its tab"
    );
    // A report appearing changes what the page links: that reloads it too.
    let (_, rev) = page(&info);
    let mut tab = Tab::connect(&info, &rev);
    fs::write(
        p.ws().join("login-timeout/report.md"),
        "Workstream: login-timeout\n",
    )
    .unwrap();
    assert!(
        tab.wait_for("event: reload"),
        "a new report reloads the tab"
    );
    let (body, rev) = page(&info);
    assert!(body.contains(r#"<a href="login-timeout/report.md">report</a>"#));
    // A report's content changing (a hand-back, a new confidence) reloads too:
    // the agents panel reads it.
    let mut tab = Tab::connect(&info, &rev);
    fs::write(
        p.ws().join("login-timeout/report.md"),
        "Workstream: login-timeout\nStatus: blocked: needs the schema\nConfidence: 60%\n",
    )
    .unwrap();
    assert!(
        tab.wait_for("event: reload"),
        "a changed report reloads the tab"
    );
    let (body, _) = page(&info);
    assert!(
        body.contains("<li>blocked: needs the schema</li>"),
        "{body}"
    );
    // A worker posting its report in its worktree reloads too: the page shows
    // it before the manager has collected it.
    let (_, rev) = page(&info);
    let mut tab = Tab::connect(&info, &rev);
    let scratch = p
        .dir
        .path()
        .join(".claude/worktrees/billing-retry/.claude/scratch");
    fs::create_dir_all(&scratch).unwrap();
    fs::write(
        scratch.join("workstream-report.md"),
        "Workstream: billing-retry\nStatus: done\nConfidence: 99%\nTip: 89abcdef0123456789abcdef0123456789abcdef\nCheck: pass\n",
    )
    .unwrap();
    assert!(
        tab.wait_for("event: reload"),
        "a posted report reloads the tab"
    );
    let (body, _) = page(&info);
    assert!(
        body.contains("Posted from its worktree, not yet collected"),
        "{body}"
    );
    assert!(body.contains("<dt>Confidence</dt><dd>99%</dd>"), "{body}");
    // A tab that loaded an older page is told at once.
    let mut stale = Tab::connect(&info, "0000000000000000");
    assert!(stale.wait_for("event: reload"));
}

#[test]
fn briefs_and_reports_are_served_beside_the_board_and_nothing_else() {
    let p = Project::new();
    p.hook();
    let info = p.info();
    let (code, raw) = get(&info, "billing-retry/brief.md");
    assert_eq!(code, 200, "{raw}");
    assert!(raw.contains("text/plain; charset=utf-8"), "{raw}");
    assert!(raw.contains("Content-Security-Policy"), "{raw}");
    assert!(raw.ends_with("## Goal\nRetry failed charges.\n"), "{raw}");
    let (code, raw) = get(&info, "billing-retry/report.md");
    assert_eq!(code, 200, "{raw}");
    assert!(raw.ends_with("Confidence: 97%\n"));
    for missing in [
        "login-timeout/report.md",
        "billing-retry/other.md",
        "billing-retry/brief.html",
        "Bad%20Slug/brief.md",
        "../board.md",
        "billing-retry/../board.md",
        "board.md",
        "_board",
        "_notes",
        "flow.html",
        "",
    ] {
        let (code, raw) = get(&info, missing);
        assert_eq!(code, 404, "{missing}: {raw}");
    }
    // Only the board is a topic.
    let (code, _) = http(
        info.port,
        "POST",
        &format!("/{}/reload?topic=flow", info.token),
    );
    assert_eq!(code, 400);
    let (code, _) = http(
        info.port,
        "POST",
        &format!("/{}/reload?topic=billing-retry", info.token),
    );
    assert_eq!(code, 400);
    // Writes are refused.
    let (code, _) = http(
        info.port,
        "POST",
        &format!("/{}/billing-retry/brief.md", info.token),
    );
    assert_eq!(code, 404);
}

#[test]
fn notes_open_workstreams_reuses_the_open_tab() {
    let p = Project::new();
    let out = p.open();
    assert!(out.starts_with("Opened http://127.0.0.1:"), "{out}");
    let info = p.info();
    assert_eq!(p.opened(), [info.url("board")]);
    let (_, rev) = page(&info);
    let mut tab = Tab::connect(&info, &rev);
    let out = p.open();
    assert!(out.starts_with("Refreshed 1 open tab"), "{out}");
    assert!(tab.wait_for("event: reload"));
    assert_eq!(
        p.opened().len(),
        1,
        "no second tab while one shows the board"
    );
    drop(tab);
    std::thread::sleep(Duration::from_millis(200));
    // Asked again with the tab gone: a new one, even right after the last.
    let out = p.open();
    assert!(out.starts_with("Opened http://127.0.0.1:"), "{out}");
    assert_eq!(p.opened().len(), 2);
}
