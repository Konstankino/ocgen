//! The words that open a project's documents in the browser, like `draft` does
//! for the /intent issue draft: `note` or `notes` for the /inquire ledgers, and
//! the project's intent prefix in lowercase (`adr` by default) for its intent
//! files. Each opens nothing (Claude is told why), the one document there is, or
//! the list of them with the one this session wrote last selected. Notes keep
//! their ledger page; an intent file gets a read-only page, sanitized like the
//! draft's preview. Also `ocgen adr [name]`, the terminal way to the same pages.
//!
//! End to end through the real binary: the browser is a fake
//! (`OCGEN_NOTES_BROWSER`) that logs the URLs it is asked to open.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::notes::viewer::{self, Info};
use ocgen::notes::{self, draft, intents};
use ocgen::render::Project as State;
use ocgen::target::Target;
use tempfile::TempDir;

/// An intent file as /intent writes it, with markup a page must never run.
fn intent(id: &str, title: &str) -> String {
    format!(
        "<!--\nIntent file template for /intent.\n-->\n# {id}: {title}\n\nStatus: Proposed\n\
         Approvers: @alice\nDate: 2026-10-05\nIssue: https://github.com/acme/app/issues/12\n\n\
         ## Context\nThe cache goes cold\non every deploy.\n\n## Decision\nVersion the keys.\n"
    )
}

/// A ledger as /inquire writes it.
fn ledger(topic: &str) -> String {
    format!(
        "Topic: {topic}\nStatus: active\nUpdated: 2026-10-05   Commit: abc1234\nSummary: How it works.\n\
         ## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\nCites: src/main.rs:1\n"
    )
}

fn touch(p: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

// ------------------------------------------------------------- the words --

#[test]
fn the_words_are_the_whole_prompt() {
    for yes in ["note", "notes", " Note \n", "NOTES"] {
        assert!(notes::is_prompt(yes), "{yes:?}");
    }
    for no in ["note it", "a note", "notes?", "/note", "notebook", ""] {
        assert!(!notes::is_prompt(no), "{no:?}");
    }
    // The intent word is the project's prefix, lowercased.
    assert_eq!(intents::word("ADR").as_deref(), Some("adr"));
    assert_eq!(intents::word("RFC").as_deref(), Some("rfc"));
    for yes in ["adr", " ADR ", "Adr\n"] {
        assert!(intents::is_prompt(yes, "ADR"), "{yes:?}");
    }
    for no in ["adr please", "adr 7", "/adr", "adrs", "rfc", ""] {
        assert!(!intents::is_prompt(no, "ADR"), "{no:?}");
    }
    assert!(intents::is_prompt("rfc", "RFC") && !intents::is_prompt("adr", "RFC"));
    // A prefix named like a word ocgen already has leaves that word alone.
    for taken in ["DRAFT", "Note", "NOTES"] {
        assert_eq!(intents::word(taken), None, "{taken}");
        assert!(!intents::is_prompt(&taken.to_lowercase(), taken));
    }
}

// ----------------------------------------------------------- intent files --

fn adr_settings(prefix: &str) -> ocgen::claude::IntentSettings {
    ocgen::claude::IntentSettings {
        prefix: prefix.into(),
        ..Default::default()
    }
}

#[test]
fn intent_files_are_listed_highest_number_first() {
    let dir = tempfile::tempdir().unwrap();
    let adr = dir.path().join("docs/adr");
    fs::create_dir_all(&adr).unwrap();
    for name in [
        "ADR-0001-cache.md",
        "ADR-0010-retry.md",
        "ADR-0002-queue.md",
        "0003-unprefixed.md",
        "README.md",
        "ADR-0004-bad.name.md",
        "notes.txt",
    ] {
        fs::write(adr.join(name), intent("ADR-0000", "x")).unwrap();
    }
    // Newest by time is not newest by number: a fresh clone gives every file
    // the same time anyway.
    touch(
        &adr.join("ADR-0001-cache.md"),
        SystemTime::now() + Duration::from_secs(60),
    );
    let s = adr_settings("ADR");
    let names: Vec<String> = intents::files(&adr, &s)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "ADR-0010-retry.md",
            "0003-unprefixed.md",
            "ADR-0002-queue.md",
            "ADR-0001-cache.md"
        ]
    );
    assert_eq!(intents::number("ADR-0010-retry", "ADR"), Some(10));
    assert_eq!(intents::number("0003-unprefixed", "ADR"), Some(3));
    assert_eq!(intents::number("adr-0002-queue", "ADR"), Some(2));
    assert_eq!(intents::number("README", "ADR"), None);
    // A file ocgen can't serve is named, not dropped.
    assert_eq!(intents::skipped(&adr, &s), ["ADR-0004-bad.name.md"]);
}

#[test]
fn intent_files_are_found_by_name_start_or_number() {
    let dir = tempfile::tempdir().unwrap();
    let adr = dir.path().join("docs/adr");
    fs::create_dir_all(&adr).unwrap();
    let s = adr_settings("ADR");
    // None yet: the error says where they go.
    let err = intents::find(&adr, &s, None).unwrap_err().to_string();
    assert!(
        err.contains("no intent files") && err.contains("/intent"),
        "{err}"
    );

    fs::write(adr.join("ADR-0002-queue.md"), intent("ADR-0002", "Queue")).unwrap();
    let only = intents::find(&adr, &s, None).unwrap();
    assert!(only.ends_with("ADR-0002-queue.md"));
    fs::write(adr.join("ADR-0007-retry.md"), intent("ADR-0007", "Retry")).unwrap();
    fs::write(adr.join("ADR-0017-cache.md"), intent("ADR-0017", "Cache")).unwrap();
    let found = |name: &str| {
        intents::find(&adr, &s, Some(name))
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    for (name, want) in [
        ("ADR-0007-retry", "ADR-0007-retry.md"),
        ("adr-0007-retry.md", "ADR-0007-retry.md"),
        ("ADR-0007", "ADR-0007-retry.md"),
        ("adr-00", "ADR-0017-cache.md"),
        ("7", "ADR-0007-retry.md"),
        ("0007", "ADR-0007-retry.md"),
        ("ADR-7", "ADR-0007-retry.md"),
        ("17", "ADR-0017-cache.md"),
        (" 2 ", "ADR-0002-queue.md"),
    ] {
        assert_eq!(found(name), want, "{name:?}");
    }
    let err = intents::find(&adr, &s, Some("99")).unwrap_err().to_string();
    assert!(
        err.contains("'99'") && err.contains("ADR-0017-cache, ADR-0007-retry, ADR-0002-queue"),
        "{err}"
    );
}

#[test]
fn the_intent_page_is_read_only_and_runs_nothing_from_the_file() {
    let md = format!(
        "{}\n## Hostile\n<script>alert(1)</script>\n\n<img src=x onerror=alert(2)>\n\n\
         <iframe src=\"https://evil.example\"></iframe>\n\n<div onclick=\"alert(3)\">div</div>\n\n\
         [js](javascript:alert(4)) [data](data:text/html,x) [rel](ADR-0002-queue.md) \
         [web](https://example.com/a) <mailto:a@b.dev>\n\n![pic](https://evil.example/p.png)\n\n\
         <details><summary>More</summary>\n\nHidden <kbd>Ctrl</kbd>\n</details>\n",
        intent("ADR-0007", "Retry with a budget")
    );
    let page = intents::page(
        &md,
        "docs/adr/ADR-0007-retry.md",
        "rev1",
        "n0nce",
        "English",
    )
    .unwrap();
    // What the file says, in a page of its own.
    assert!(page.contains("ADR-0007: Retry with a budget"), "{page}");
    assert!(
        page.contains("Proposed") && page.contains("@alice"),
        "{page}"
    );
    assert!(page.contains("docs/adr/ADR-0007-retry.md"));
    assert!(
        page.contains(r#"href="https://github.com/acme/app/issues/12""#),
        "{page}"
    );
    assert!(page.contains("<h2>Context</h2>") && page.contains("Version the keys."));
    // A file renders the way GitHub renders a file: a newline is a space.
    assert!(
        page.contains("The cache goes cold\non every deploy."),
        "{page}"
    );
    // The shared palette, black by default.
    assert!(page.contains(r#"data-palette="black""#) && page.contains("palette-white"));
    // Read-only: nothing to edit or save.
    for editing in ["<textarea", "contenteditable", "id=\"save\"", "OcgenDraft"] {
        assert!(!page.contains(editing), "{editing}");
    }
    // GitHub's allowlist and nothing else: no scripts but the page's own one,
    // no handlers, no frames, no links but the web and mail.
    let open = r#"<div class="markdown-body">"#;
    let body = &page[page.find(open).unwrap() + open.len()..page.find("</article>").unwrap()];
    for bad in [
        "<script",
        "<iframe",
        "<img",
        "<div",
        "javascript:",
        "data:text",
        r#"href="ADR-0002"#,
    ] {
        assert!(!body.contains(bad), "{bad}: {body}");
    }
    // A handler survives only as text, never on a tag.
    let handler = regex::Regex::new(r"<[A-Za-z][^<>]*\son[a-z]+\s*=").unwrap();
    assert!(!handler.is_match(body), "{body}");
    assert!(
        body.contains("&lt;img src=x onerror=alert(2)&gt;"),
        "{body}"
    );
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{body}"
    );
    assert!(
        body.contains(r#"<a href="https://example.com/a">web</a>"#),
        "{body}"
    );
    assert!(body.contains(r#"href="mailto:a@b.dev""#), "{body}");
    assert!(
        body.contains(r#"<a href="https://evil.example/p.png">pic</a>"#),
        "{body}"
    );
    assert!(body.contains("<details><summary>More</summary>") && body.contains("<kbd>Ctrl</kbd>"));
    assert!(
        body.contains(" rel ") && !body.contains("Intent file template"),
        "{body}"
    );
    // The page's one script carries the nonce.
    assert_eq!(page.matches("<script").count(), 1);
    assert!(page.contains(r#"<script nonce="n0nce">"#));
}

/// `n` intent files, ADR-0001 to ADR-00nn, in `adr`.
fn many_intents(adr: &Path, n: usize) -> Vec<String> {
    (1..=n)
        .map(|i| {
            let stem = format!("ADR-{i:04}-topic");
            fs::write(
                adr.join(format!("{stem}.md")),
                intent(&format!("ADR-{i:04}"), &format!("Topic {i}")),
            )
            .unwrap();
            stem
        })
        .collect()
}

#[test]
fn the_intent_list_shows_ten_a_page_with_the_selected_one_marked() {
    let dir = tempfile::tempdir().unwrap();
    let adr = dir.path().join("docs/adr");
    fs::create_dir_all(&adr).unwrap();
    let stems = many_intents(&adr, 23);
    let s = adr_settings("ADR");
    // Highest first: 23 to 14 on page 1, 13 to 4 on page 2.
    let page = intents::list_page(dir.path(), &s, None, Some(&stems[7]), "../", "n").unwrap();
    assert!(
        page.contains("Page 2 of 3") && page.contains("23 intent files"),
        "{page}"
    );
    assert_eq!(page.matches("data-slug=").count(), 10);
    assert!(
        regex::Regex::new(r#"data-slug="ADR-0008-topic"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&page),
        "{page}"
    );
    assert!(page.contains(r#"href="../ADR-0013-topic.html""#), "{page}");
    assert!(
        page.contains("Topic 8") && page.contains("Proposed"),
        "{page}"
    );
    assert!(page.contains("docs/adr/"));
    assert!(page.contains(r#"<script nonce="n">"#) && !page.contains("<textarea"));
    let first = intents::list_page(dir.path(), &s, None, None, "./", "n").unwrap();
    assert!(first.contains("Page 1 of 3") && !first.contains("aria-current"));
    assert!(first.contains(r#"href="./ADR-0023-topic.html""#), "{first}");
}

#[test]
fn the_notes_list_shows_ten_a_page_newest_first_with_the_selected_one_marked() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join(".claude/notes");
    fs::create_dir_all(&notes_dir).unwrap();
    let base = SystemTime::now() - Duration::from_secs(24 * 3600);
    let slugs: Vec<String> = (1..=12)
        .map(|i| {
            let slug = format!("topic-{i:02}");
            let p = notes_dir.join(format!("{slug}.md"));
            fs::write(&p, ledger(&format!("Topic number {i}"))).unwrap();
            touch(&p, base + Duration::from_secs(60 * i as u64));
            slug
        })
        .collect();
    let page = notes::list_page(&notes_dir, None, Some(&slugs[0]), "../", "n").unwrap();
    assert!(
        page.contains("Page 2 of 2") && page.contains("12 notes"),
        "{page}"
    );
    assert!(
        regex::Regex::new(r#"data-slug="topic-01"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&page),
        "{page}"
    );
    assert!(page.contains("Topic number 1") && page.contains(r#"href="../topic-01.html""#));
    let first = notes::list_page(&notes_dir, None, None, "./", "n").unwrap();
    assert!(first.contains(r#"href="./topic-12.html""#) && !first.contains("aria-current"));
}

// ------------------------------------------------------------ end to end --

struct Project {
    dir: TempDir,
    log: PathBuf,
    browser: String,
}

impl Project {
    /// A generated project with /intent and /inquire on, and intent prefix `prefix`.
    fn new(prefix: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut p = State::from_manifest(&Manifest::load().unwrap(), "English");
        p.target = Target::ClaudeCode;
        p.project_name = "words".into();
        p.providers.clear();
        p.agents = agent::claude_default_pipeline("English").unwrap();
        p.claude.workflow.intent = true;
        p.claude.workflow.inquire = true;
        p.claude.intent.prefix = prefix.into();
        p.scaffold(dir.path(), false).unwrap();
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

    fn adr(&self) -> PathBuf {
        let d = self.dir.path().join("docs/adr");
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn notes(&self) -> PathBuf {
        let d = self.dir.path().join(".claude/notes");
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn drafts(&self) -> PathBuf {
        let d = self.dir.path().join(draft::DIR);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn viewer(&self) -> PathBuf {
        self.dir.path().join(intents::VIEWER_DIR)
    }

    fn cmd(&self, args: &[&str], open: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        c.args(args)
            .current_dir(self.dir.path())
            .env(
                "CLAUDE_PROJECT_DIR",
                ocgen::paths::for_shell(self.dir.path()),
            )
            .env("OCGEN_NOTES_OPEN", open)
            .env("OCGEN_NOTES_BROWSER", &self.browser)
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .env("OCGEN_NOTES_GRACE_MS", "5000")
            .env("OCGEN_NOTES_IDLE_SECS", "60");
        c
    }

    /// `ocgen adr [args]` from the project directory.
    fn ocgen_adr(&self, args: &[&str]) -> Output {
        let mut all = vec!["adr"];
        all.extend_from_slice(args);
        self.cmd(&all, "1").stdin(Stdio::null()).output().unwrap()
    }

    /// `ocgen hook <hook>` with `event` on stdin, as Claude Code runs it: what
    /// it tells Claude ("" for nothing).
    fn hook_with(&self, hook: &str, event: serde_json::Value, open: &str) -> String {
        let mut child = self
            .cmd(&["hook", hook], open)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "never blocks: {out:?}");
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if said.is_empty() {
            return said;
        }
        let v: serde_json::Value = serde_json::from_str(&said).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The user sends `prompt` in session `sid`; the hook's answer.
    fn say(&self, hook: &str, sid: &str, prompt: &str) -> String {
        self.hook_with(
            hook,
            serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": sid, "prompt": prompt
            }),
            "1",
        )
    }

    /// `prompt` with the browser switched off.
    fn say_off(&self, hook: &str, prompt: &str) -> String {
        self.hook_with(
            hook,
            serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": "s-off", "prompt": prompt
            }),
            "0",
        )
    }

    /// The hook after Claude writes `path` in session `sid` (the browser off, so
    /// the ledger's own page doesn't open).
    fn wrote(&self, hook: &str, sid: &str, path: &Path) -> String {
        self.hook_with(
            hook,
            serde_json::json!({
                "hook_event_name": "PostToolUse", "session_id": sid, "tool_name": "Write",
                "tool_input": { "file_path": ocgen::paths::for_shell(path) }
            }),
            "0",
        )
    }

    fn opened(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn intents_info(&self) -> Info {
        viewer::read_info(&self.viewer()).expect("an intent viewer is running")
    }

    fn notes_info(&self) -> Info {
        viewer::read_info(&self.notes()).expect("a notes viewer is running")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        for dir in [
            self.viewer(),
            self.dir.path().join(".claude/notes"),
            self.dir.path().join(draft::DIR),
        ] {
            if let Some(info) = viewer::read_info(&dir) {
                viewer::quit(&info);
            }
        }
    }
}

/// A raw HTTP request: (status, head, body).
fn http(info: &Info, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String, String) {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Length: 2\r\n",
        info.port
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\nhi");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    let (head, body) = buf.split_once("\r\n\r\n").unwrap_or((&buf, ""));
    (code, head.to_string(), body.to_string())
}

/// An open tab: the page's event stream.
struct Tab(TcpStream);

impl Tab {
    fn on(info: &Info, topic: &str) -> Tab {
        let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
        write!(
            s,
            "GET /{}/events?topic={topic}&rev= HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            info.token, info.port
        )
        .unwrap();
        let mut tab = Tab(s);
        assert!(tab.wait_for("retry:"), "the stream opens");
        std::thread::sleep(Duration::from_millis(100));
        tab
    }

    fn wait_for(&mut self, needle: &str) -> bool {
        let start = Instant::now();
        let mut seen = String::new();
        self.0
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        while start.elapsed() < Duration::from_secs(4) {
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

fn csp(head: &str) -> String {
    head.lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with("content-security-policy")
        })
        .expect("a CSP")
        .to_string()
}

#[test]
fn adr_opens_nothing_one_file_or_the_list_with_the_sessions_file_selected() {
    let p = Project::new("ADR");
    let adr = p.adr();

    // None: nothing opens, and Claude is told why.
    let said = p.say("intent-draft", "s-1", "adr");
    assert!(
        said.contains("no intent file") && said.contains("docs/adr/"),
        "{said}"
    );
    assert!(p.opened().is_empty(), "{:?}", p.opened());

    // One: its page opens directly, whatever the case and the spaces.
    fs::write(adr.join("ADR-0001-cache.md"), intent("ADR-0001", "Cache")).unwrap();
    let said = p.say("intent-draft", "s-1", "  ADR \n");
    let info = p.intents_info();
    assert_eq!(p.opened(), [info.url("ADR-0001-cache")]);
    assert!(ocgen::notes::browser::is_viewer_url(&p.opened()[0]));
    assert!(
        said.contains("docs/adr/ADR-0001-cache.md") && said.contains("read-only"),
        "{said}"
    );
    // Its state stays out of the tracked folder, and out of git.
    let tracked: Vec<String> = fs::read_dir(&adr)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(tracked, ["ADR-0001-cache.md"]);
    assert_eq!(
        fs::read_to_string(p.viewer().join(".gitignore")).unwrap(),
        "*\n"
    );

    // The page: read-only, under a strict policy; nothing can be posted to it.
    let (code, head, page) = http(
        &info,
        "GET",
        &format!("/{}/ADR-0001-cache.html", info.token),
        &[],
    );
    assert_eq!(code, 200, "{page}");
    assert!(page.contains("ADR-0001: Cache") && !page.contains("<textarea"));
    let policy = csp(&head);
    assert!(
        policy.contains("default-src 'none'") && policy.contains("script-src 'nonce-"),
        "{policy}"
    );
    for loose in ["unsafe-inline' https", "unsafe-eval", "cdn"] {
        assert!(!policy.contains(loose), "{loose}: {policy}");
    }
    let origin = format!("http://127.0.0.1:{}", info.port);
    for path in ["ADR-0001-cache.md", "ADR-0001-cache.html", "render"] {
        let (code, _, _) = http(
            &info,
            "POST",
            &format!("/{}/{path}", info.token),
            &[("Origin", &origin)],
        );
        assert!(code == 404 || code == 405, "POST {path}: {code}");
    }
    assert_eq!(
        fs::read_to_string(adr.join("ADR-0001-cache.md")).unwrap(),
        intent("ADR-0001", "Cache")
    );
    // Nothing outside the intent folder is served.
    let (code, _, _) = http(&info, "GET", &format!("/{}/settings.html", info.token), &[]);
    assert_eq!(code, 404);

    // Sent again with the tab open: the tab is reused, not a second one opened.
    let mut tab = Tab::on(&info, "ADR-0001-cache");
    let said = p.say("intent-draft", "s-1", "adr");
    assert!(said.contains("already open"), "{said}");
    assert!(tab.wait_for("event: reload"));
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
    drop(tab);

    // Several: the list, highest number first, with this session's file selected.
    fs::write(adr.join("ADR-0007-retry.md"), intent("ADR-0007", "Retry")).unwrap();
    fs::write(adr.join("ADR-0003-queue.md"), intent("ADR-0003", "Queue")).unwrap();
    assert_eq!(
        p.wrote("intent-draft", "s-1", &adr.join("ADR-0003-queue.md")),
        ""
    );
    let said = p.say("intent-draft", "s-1", "adr");
    assert_eq!(
        p.opened().last().unwrap(),
        &info.list_url_of(intents::LIST, Some("ADR-0003-queue"))
    );
    assert!(ocgen::notes::browser::is_viewer_url(
        p.opened().last().unwrap()
    ));
    assert!(
        said.contains("list of 3 intent files in docs/adr/")
            && said.contains("docs/adr/ADR-0003-queue.md")
            && said.contains("this session last wrote"),
        "{said}"
    );
    let (code, head, list) = http(
        &info,
        "GET",
        &format!("/{}/{}/ADR-0003-queue", info.token, intents::LIST),
        &[],
    );
    assert_eq!(code, 200, "{list}");
    assert!(csp(&head).contains("script-src 'nonce-"));
    let order: Vec<usize> = ["ADR-0007-retry", "ADR-0003-queue", "ADR-0001-cache"]
        .iter()
        .map(|s| list.find(&format!(r#"data-slug="{s}""#)).unwrap())
        .collect();
    assert!(order[0] < order[1] && order[1] < order[2], "{list}");
    assert!(
        regex::Regex::new(r#"data-slug="ADR-0003-queue"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&list),
        "{list}"
    );

    // A session that wrote none gets the list with nothing selected.
    let said = p.say("intent-draft", "s-2", "adr");
    assert!(
        said.contains("list of 3 intent files") && said.contains("hasn't written"),
        "{said}"
    );

    // A list tab is open: the word moves it instead of opening another.
    let before = p.opened().len();
    let mut tab = Tab::on(&info, intents::LIST);
    let said = p.say("intent-draft", "s-1", "adr");
    assert!(tab.wait_for(&format!(
        "event: show\ndata: {}/ADR-0003-queue",
        intents::LIST
    )));
    assert!(said.contains("already open"), "{said}");
    assert_eq!(p.opened().len(), before);
    // A file changing on disk refreshes the open list.
    fs::write(
        adr.join("ADR-0007-retry.md"),
        intent("ADR-0007", "Retry, changed"),
    )
    .unwrap();
    assert!(tab.wait_for("event: reload"), "the list hears of it");

    // Anything else passes through untouched.
    for other in ["adr please", "adr 7", "/adr", "adrs", "rfc"] {
        assert_eq!(p.say("intent-draft", "s-1", other), "", "{other:?}");
    }
}

#[test]
fn an_intent_file_a_bash_command_writes_is_the_sessions_too() {
    let p = Project::new("ADR");
    let adr = p.adr();
    fs::write(adr.join("ADR-0001-cache.md"), intent("ADR-0001", "Cache")).unwrap();
    fs::write(adr.join("ADR-0002-queue.md"), intent("ADR-0002", "Queue")).unwrap();
    let bash = |event: &str| {
        p.hook_with(
            "intent-draft",
            serde_json::json!({
                "hook_event_name": event, "session_id": "s-1", "tool_name": "Bash",
                "tool_use_id": "toolu_1", "tool_input": { "command": "sed -i s/x/y/ docs/adr/ADR-0001-cache.md" }
            }),
            "0",
        )
    };
    assert_eq!(bash("PreToolUse"), "");
    fs::write(
        adr.join("ADR-0001-cache.md"),
        intent("ADR-0001", "Cache, rewritten"),
    )
    .unwrap();
    assert_eq!(bash("PostToolUse"), "");
    let said = p.say_off("intent-draft", "adr");
    assert!(said.contains("2 intent files"), "{said}");
    let state = p.viewer();
    assert_eq!(
        notes::session::recall(&state, &adr, "s-1", draft::is_name).as_deref(),
        Some("ADR-0001-cache")
    );
}

#[test]
fn a_project_prefix_is_its_word() {
    let p = Project::new("RFC");
    let adr = p.adr();
    fs::write(adr.join("RFC-0001-auth.md"), intent("RFC-0001", "Auth")).unwrap();
    // An ADR-named file isn't one of this project's intent files.
    fs::write(adr.join("ADR-0009-old.md"), intent("ADR-0009", "Old")).unwrap();
    assert_eq!(
        p.say("intent-draft", "s-1", "adr"),
        "",
        "not this project's word"
    );
    let said = p.say("intent-draft", "s-1", "rfc");
    let info = p.intents_info();
    assert_eq!(p.opened(), [info.url("RFC-0001-auth")]);
    assert!(
        said.contains("`rfc`") && said.contains("docs/adr/RFC-0001-auth.md"),
        "{said}"
    );
}

#[test]
fn note_opens_nothing_one_ledger_or_the_list_with_the_sessions_ledger_selected() {
    let p = Project::new("ADR");
    let notes_dir = p.notes();

    // None: nothing opens, and Claude is told there are no notes yet.
    let said = p.say("inquire-notes", "s-1", "note");
    assert!(said.contains("no /inquire notes"), "{said}");
    assert!(p.opened().is_empty());

    // One: its ledger page opens directly — the page /inquire already has.
    fs::write(notes_dir.join("request-flow.md"), ledger("Request flow")).unwrap();
    let said = p.say("inquire-notes", "s-1", " NOTES ");
    let info = p.notes_info();
    assert_eq!(p.opened(), [info.url("request-flow")]);
    assert!(said.contains(".claude/notes/request-flow.md"), "{said}");
    let (code, _, page) = http(
        &info,
        "GET",
        &format!("/{}/request-flow.html", info.token),
        &[],
    );
    assert_eq!(code, 200);
    assert!(
        page.contains("Request flow") && page.contains("ev verified"),
        "{page}"
    );

    // Sent again with its tab open: refreshed, not opened twice.
    let mut tab = Tab::on(&info, "request-flow");
    let said = p.say("inquire-notes", "s-1", "note");
    assert!(said.contains("already open"), "{said}");
    assert!(tab.wait_for("event: reload"));
    assert_eq!(p.opened().len(), 1);
    drop(tab);

    // Several: the list, newest first, with this session's ledger selected.
    let older = notes_dir.join("cache-keys.md");
    fs::write(&older, ledger("Cache keys")).unwrap();
    touch(&older, SystemTime::now() - Duration::from_secs(3600));
    assert_eq!(p.wrote("inquire-notes", "s-1", &older), "");
    touch(&older, SystemTime::now() - Duration::from_secs(3600));
    fs::write(notes_dir.join("deploys.md"), ledger("Deploys")).unwrap();
    let said = p.say("inquire-notes", "s-1", "notes");
    assert_eq!(
        p.opened().last().unwrap(),
        &info.list_url_of(notes::LIST, Some("cache-keys"))
    );
    assert!(ocgen::notes::browser::is_viewer_url(
        p.opened().last().unwrap()
    ));
    assert!(
        said.contains("list of 3 /inquire notes")
            && said.contains(".claude/notes/cache-keys.md")
            && said.contains("this session last wrote"),
        "{said}"
    );
    let (code, _, list) = http(
        &info,
        "GET",
        &format!("/{}/{}/cache-keys", info.token, notes::LIST),
        &[],
    );
    assert_eq!(code, 200, "{list}");
    assert!(
        list.contains("Cache keys") && list.contains(r#"href="../deploys.html""#),
        "{list}"
    );
    assert!(
        regex::Regex::new(r#"data-slug="cache-keys"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&list),
        "{list}"
    );
    // The list's own page is read-only too.
    let (code, _, _) = http(
        &info,
        "POST",
        &format!("/{}/request-flow.md", info.token),
        &[],
    );
    assert!(code == 404 || code == 405, "{code}");

    // Anything else passes through untouched.
    for other in ["note this", "notes?", "/notes", "a note", "noted"] {
        assert_eq!(p.say("inquire-notes", "s-1", other), "", "{other:?}");
    }
}

#[test]
fn nothing_opens_with_the_browser_switched_off() {
    let p = Project::new("ADR");
    fs::write(
        p.adr().join("ADR-0001-cache.md"),
        intent("ADR-0001", "Cache"),
    )
    .unwrap();
    fs::write(p.notes().join("request-flow.md"), ledger("Request flow")).unwrap();
    let said = p.say_off("intent-draft", "adr");
    assert!(
        said.contains("OCGEN_NOTES_OPEN=0")
            && said.contains("docs/adr/ADR-0001-cache.md")
            && said.contains("ocgen adr"),
        "{said}"
    );
    let said = p.say_off("inquire-notes", "note");
    assert!(
        said.contains("OCGEN_NOTES_OPEN=0")
            && said.contains(".claude/notes/request-flow.md")
            && said.contains("ocgen notes open"),
        "{said}"
    );
    // Several, still off.
    fs::write(
        p.adr().join("ADR-0002-queue.md"),
        intent("ADR-0002", "Queue"),
    )
    .unwrap();
    fs::write(p.notes().join("deploys.md"), ledger("Deploys")).unwrap();
    assert!(p
        .say_off("intent-draft", "adr")
        .contains("OCGEN_NOTES_OPEN=0"));
    assert!(p
        .say_off("inquire-notes", "notes")
        .contains("OCGEN_NOTES_OPEN=0"));
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert!(viewer::read_info(&p.viewer()).is_none(), "no server either");
}

#[test]
fn draft_is_unchanged_beside_notes_and_intent_files() {
    let p = Project::new("ADR");
    fs::write(
        p.adr().join("ADR-0001-cache.md"),
        intent("ADR-0001", "Cache"),
    )
    .unwrap();
    fs::write(p.notes().join("request-flow.md"), ledger("Request flow")).unwrap();
    fs::write(
        p.drafts().join("adr-0001-cache.md"),
        "## Intent\nKeep it warm.\n",
    )
    .unwrap();
    let said = p.say("intent-draft", "s-1", "draft");
    let info = viewer::read_info(&p.drafts()).expect("the draft editor runs");
    assert_eq!(p.opened(), [info.url("adr-0001-cache")]);
    assert!(
        said.contains("issue draft .claude/intent/drafts/adr-0001-cache.md"),
        "{said}"
    );
    // `draft` isn't the notes' word, and `adr` opens the intent file, not the draft.
    assert_eq!(p.say("inquire-notes", "s-1", "draft"), "");
    let said = p.say("intent-draft", "s-1", "adr");
    assert!(said.contains("docs/adr/ADR-0001-cache.md"), "{said}");
    assert_eq!(p.opened().len(), 2, "{:?}", p.opened());
    assert_eq!(p.opened()[1], p.intents_info().url("ADR-0001-cache"));
    assert_eq!(
        viewer::read_info(&p.drafts()).unwrap(),
        info,
        "the editor is untouched"
    );
}

#[test]
fn a_prefix_named_like_a_word_ocgen_has_leaves_that_word_alone() {
    let p = Project::new("NOTE");
    fs::write(p.adr().join("NOTE-0001-x.md"), intent("NOTE-0001", "X")).unwrap();
    fs::write(p.notes().join("request-flow.md"), ledger("Request flow")).unwrap();
    // `note` is the ledgers' word: the intent hook stays out of it.
    assert_eq!(p.say("intent-draft", "s-1", "note"), "");
    assert!(p
        .say_off("inquire-notes", "note")
        .contains("request-flow.md"));
    // `ocgen adr` still opens the file.
    let out = p.ocgen_adr(&["1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("docs/adr/NOTE-0001-x.md"));

    let p = Project::new("DRAFT");
    fs::write(p.adr().join("DRAFT-0001-x.md"), intent("DRAFT-0001", "X")).unwrap();
    let said = p.say_off("intent-draft", "draft");
    assert!(
        said.contains("no issue draft"),
        "`draft` stays the drafts' word: {said}"
    );
}

#[test]
fn ocgen_adr_opens_one_by_name_start_or_number_and_lists_several() {
    let p = Project::new("ADR");
    let adr = p.adr();
    // None yet.
    let out = p.ocgen_adr(&[]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no intent files"),
        "{out:?}"
    );

    fs::write(adr.join("ADR-0007-retry.md"), intent("ADR-0007", "Retry")).unwrap();
    let out = p.ocgen_adr(&[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let info = p.intents_info();
    assert_eq!(p.opened(), [info.url("ADR-0007-retry")]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("docs/adr/ADR-0007-retry.md"));

    // Several and no name: the list.
    fs::write(adr.join("ADR-0002-queue.md"), intent("ADR-0002", "Queue")).unwrap();
    let out = p.ocgen_adr(&[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        p.opened().last().unwrap(),
        &info.list_url_of(intents::LIST, None)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("2 intent files in docs/adr/"),
        "{out:?}"
    );

    // By number, start or name.
    for (name, stem) in [
        ("2", "ADR-0002-queue"),
        ("ADR-0007", "ADR-0007-retry"),
        ("adr-0002-queue", "ADR-0002-queue"),
    ] {
        let out = p.ocgen_adr(&[name]);
        assert_eq!(out.status.code(), Some(0), "{name}: {out:?}");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            said.contains(&format!("docs/adr/{stem}.md")),
            "{name}: {said}"
        );
    }
    assert!(p.opened().contains(&info.url("ADR-0002-queue")));
    let out = p.ocgen_adr(&["99"]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("'99'"),
        "{out:?}"
    );

    // From a subdirectory, too.
    let sub = p.dir.path().join("src");
    fs::create_dir_all(&sub).unwrap();
    let out = p
        .cmd(&["adr", "7", "--path", &ocgen::paths::for_shell(&sub)], "0")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("docs/adr/ADR-0007-retry.md (not opened"));
}

#[test]
fn the_notes_word_is_registered_with_the_ledgers() {
    let p = Project::new("ADR");
    let settings = fs::read_to_string(p.dir.path().join(".claude/settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let on = |event: &str, hook: &str| {
        v["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g.to_string().contains(&format!("{hook}.sh")))
    };
    assert!(on("UserPromptSubmit", "inquire-notes"));
    assert!(on("UserPromptSubmit", "intent-draft"));
    // No new hook: projects generated before get `adr` with the new binary.
    assert_eq!(ocgen::hooks::PROTOCOL, "ocgen-hooks 13");
}
