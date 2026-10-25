//! The /brief brief's live page and the `brief` word, end to end through the
//! real binary: the `inquire-notes` hook opens a brief's editor after the first
//! write and tells the open tab of later ones; the editor saves back to the
//! file and recomputes the coverage strip on request; `brief` typed alone opens
//! the session's brief, the only one, or their list, and reaches Claude as
//! typed where /brief is off; both hooks name the same folders for Claude
//! Code's file watcher, and each leaves the other's files alone. The browser is
//! a fake (`OCGEN_NOTES_BROWSER`) that logs the URLs it is asked to open.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::notes::viewer::{self, Info, INFO_FILE};
use ocgen::notes::{brief, browser, draft};
use ocgen::render::Project as State;
use ocgen::target::Target;
use tempfile::TempDir;

/// A finished brief for /deliver: every section covered, nothing open.
const READY: &str = include_str!("fixtures/briefs/deliver.md");
/// A brief in progress: two thin sections, one missing, two Open items.
const THIN: &str = include_str!("fixtures/briefs/thin.md");
/// How every answer to the word ends.
const ENDING: &str = "to send the word to Claude, write it in a sentence.";

fn touch(p: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

// -------------------------------------------------------------- harness --

struct Project {
    _dir: TempDir,
    root: PathBuf,
    log: PathBuf,
    browser: String,
}

impl Project {
    /// A generated project with /brief (for /deliver), /inquire and /intent on.
    fn new() -> Self {
        Self::build("project", |_| {})
    }

    /// [`Project::new`], its state changed by `tweak` before it is written.
    fn with(tweak: impl FnOnce(&mut State)) -> Self {
        Self::build("project", tweak)
    }

    /// The project in folder `folder` of a fresh temp dir.
    fn build(folder: &str, tweak: impl FnOnce(&mut State)) -> Self {
        let project = Self::bare(folder);
        let mut p = State::from_manifest(&Manifest::load().unwrap(), "English");
        p.target = Target::ClaudeCode;
        p.project_name = "briefs".into();
        p.providers.clear();
        p.agents = agent::claude_default_pipeline("English").unwrap();
        p.claude.workflow.deliver = true;
        p.claude.workflow.inquire = true;
        p.claude.workflow.intent = true;
        p.claude.workflow.brief = true;
        tweak(&mut p);
        p.scaffold(&project.root, false).unwrap();
        project
    }

    /// Folder `folder` of a fresh temp dir with no ocgen state in it — a
    /// repository the hooks reach from a plugin — and the fake browser.
    fn bare(folder: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(folder);
        fs::create_dir_all(&root).unwrap();
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
        Project {
            _dir: dir,
            root,
            log,
            browser,
        }
    }

    fn briefs(&self) -> PathBuf {
        let d = self.root.join(brief::DIR);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn drafts(&self) -> PathBuf {
        let d = self.root.join(draft::DIR);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write brief `slug` with `text`; its path.
    fn brief(&self, slug: &str, text: &str) -> PathBuf {
        let md = self.briefs().join(format!("{slug}.md"));
        fs::write(&md, text).unwrap();
        md
    }

    fn cmd(&self, args: &[&str], open: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ocgen"));
        c.args(args)
            .current_dir(&self.root)
            .env("CLAUDE_PROJECT_DIR", ocgen::paths::for_shell(&self.root))
            .env("OCGEN_NOTES_OPEN", open)
            .env("OCGEN_NOTES_BROWSER", &self.browser)
            // The override runs detached; wait for it so the log is complete.
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .env("OCGEN_NOTES_GRACE_MS", "5000")
            .env("OCGEN_NOTES_IDLE_SECS", "60");
        c
    }

    /// `ocgen hook <hook>` with `event` on stdin, as Claude Code runs it.
    fn run(&self, hook: &str, event: serde_json::Value, open: &str) -> Output {
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
        out
    }

    /// What `hook` says on stdout for `event` (nothing on stderr), trimmed.
    fn said(&self, hook: &str, event: serde_json::Value, open: &str) -> String {
        let out = self.run(hook, event, open);
        assert!(out.stderr.is_empty(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `hook` after Claude writes `md` in session `sid`.
    fn wrote_with(&self, hook: &str, sid: &str, md: &Path, open: &str) -> Output {
        self.run(
            hook,
            serde_json::json!({
                "hook_event_name": "PostToolUse", "session_id": sid, "tool_name": "Write",
                "tool_input": { "file_path": ocgen::paths::for_shell(md) }
            }),
            open,
        )
    }

    /// The notes hook after Claude writes brief `md` in session `sid`, with the
    /// browser on: silent.
    fn wrote(&self, sid: &str, md: &Path) {
        let out = self.wrote_with("inquire-notes", sid, md, "1");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    }

    /// What Claude was told after a write, if anything.
    fn context(out: &Output) -> Option<String> {
        let said = String::from_utf8_lossy(&out.stdout);
        let v: serde_json::Value = serde_json::from_str(said.trim()).ok()?;
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .map(str::to_string)
    }

    /// The user sends `prompt` in session `sid`: `hook`'s answer, shown to the
    /// user in place of the prompt, or "" when the prompt goes on to Claude.
    fn say_with(&self, hook: &str, sid: &str, prompt: &str, open: &str) -> String {
        let said = self.said(
            hook,
            serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": sid, "prompt": prompt
            }),
            open,
        );
        if said.is_empty() {
            return said;
        }
        let v: serde_json::Value = serde_json::from_str(&said).unwrap();
        assert_eq!(v["decision"], "block", "{said}");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert_eq!(v["hookSpecificOutput"]["suppressOriginalPrompt"], true);
        assert!(v["hookSpecificOutput"].get("additionalContext").is_none());
        v["reason"].as_str().unwrap().to_string()
    }

    fn say(&self, sid: &str, prompt: &str) -> String {
        self.say_with("inquire-notes", sid, prompt, "1")
    }

    /// `prompt` with the browser switched off.
    fn say_off(&self, prompt: &str) -> String {
        self.say_with("inquire-notes", "s-off", prompt, "0")
    }

    /// `hook` after Claude Code's file watcher saw `file` change (`event`: add,
    /// change or unlink) in session `sid`: its output.
    fn changed(&self, hook: &str, sid: &str, file: &Path, event: &str) -> String {
        self.said(
            hook,
            serde_json::json!({
                "hook_event_name": "FileChanged", "session_id": sid,
                "file_path": file.to_string_lossy(), "event": event
            }),
            "1",
        )
    }

    /// The folders `hook` names for the file watcher at `event`.
    fn watched(&self, hook: &str, event: &str) -> Vec<PathBuf> {
        let out = self.said(
            hook,
            serde_json::json!({
                "hook_event_name": event, "session_id": "s-1", "source": "startup"
            }),
            "1",
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], event, "{out}");
        v["hookSpecificOutput"]["watchPaths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PathBuf::from(p.as_str().unwrap()))
            .collect()
    }

    fn opened(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn info(&self) -> Info {
        viewer::read_info(&self.briefs()).expect("the brief editor runs")
    }

    fn session_brief(&self, sid: &str) -> Option<String> {
        brief::session_brief(&self.briefs(), sid)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        for dir in [self.root.join(brief::DIR), self.root.join(draft::DIR)] {
            if let Some(info) = viewer::read_info(&dir) {
                viewer::quit(&info);
            }
        }
    }
}

/// A raw HTTP request: (status, head, body).
fn http(
    info: &Info,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Length: {}\r\n",
        info.port,
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let code = buf.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    let (head, body) = buf.split_once("\r\n\r\n").unwrap_or((&buf, ""));
    (code, head.to_string(), body.to_string())
}

fn origin(info: &Info) -> String {
    format!("http://127.0.0.1:{}", info.port)
}

/// The brief page for `slug`: (status, head, page).
fn page(info: &Info, slug: &str) -> (u16, String, String) {
    http(
        info,
        "GET",
        &format!("/{}/{slug}.html", info.token),
        &[],
        "",
    )
}

/// Brief `slug`'s current text and revision, as the editor sees them.
fn load(info: &Info, slug: &str) -> (String, String) {
    let (code, _, body) = http(info, "GET", &format!("/{}/{slug}.md", info.token), &[], "");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    (
        v["text"].as_str().unwrap().to_string(),
        v["rev"].as_str().unwrap().to_string(),
    )
}

fn save(info: &Info, slug: &str, rev: &str, text: &str, from: &str) -> (u16, String) {
    let (code, _, body) = http(
        info,
        "POST",
        &format!("/{}/{slug}.md?rev={rev}", info.token),
        &[
            ("Origin", from),
            ("Content-Type", "text/plain;charset=UTF-8"),
        ],
        text,
    );
    (code, body)
}

/// An open tab: the page's event stream.
struct Tab(TcpStream);

impl Tab {
    /// A tab of `topic`: a brief's slug, or the list's.
    fn on(info: &Info, topic: &str, rev: &str) -> Tab {
        let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
        write!(
            s,
            "GET /{}/events?topic={topic}&rev={rev} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            info.token, info.port
        )
        .unwrap();
        let mut tab = Tab(s);
        assert!(tab.wait_for("retry:"), "the stream opens");
        // The server registers the tab right after the stream header.
        std::thread::sleep(Duration::from_millis(100));
        tab
    }

    /// Whether `needle` arrives within 4 s.
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

// ------------------------------------------------------------- the page --

#[test]
fn the_first_write_opens_the_editor_and_later_writes_tell_the_tab() {
    let p = Project::new();
    let md = p.brief("billing-retry", READY);
    p.wrote("s-1", &md);
    let opened = p.opened();
    let info = p.info();
    assert_eq!(opened, [info.url("billing-retry")]);
    assert!(browser::is_viewer_url(&opened[0]));
    // The folder keeps itself out of git, and the brief is the session's.
    assert_eq!(
        fs::read_to_string(p.briefs().join(".gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(p.session_brief("s-1").as_deref(), Some("billing-retry"));
    // No static page is ever written beside it.
    assert!(!p.briefs().join("billing-retry.html").exists());

    // The page: the editor under the strip, a strict policy for scripts.
    let (code, head, page) = page(&info, "billing-retry");
    assert_eq!(code, 200, "{page}");
    for must in [
        r#"<main class="draft brief">"#,
        r#"data-handoff="/deliver .claude/briefs/billing-retry.md""#,
        r#"<li class="cell covered" title="Goal: covered""#,
        r#"id="copy-handoff""#,
        "Add retries with backoff",
        r#"href="_briefs/billing-retry""#,
    ] {
        assert!(page.contains(must), "missing {must:?} in:\n{page}");
    }
    let csp = head
        .lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with("content-security-policy")
        })
        .expect("a CSP")
        .to_string();
    assert!(
        csp.contains("script-src 'nonce-") && csp.contains("default-src 'none'"),
        "{csp}"
    );

    // An open tab hears of each later write: the hook's, then a plain one.
    let (_, rev) = load(&info, "billing-retry");
    let mut tab = Tab::on(&info, "billing-retry", &rev);
    fs::write(&md, READY.replace("Status: ready", "Status: draft")).unwrap();
    p.wrote("s-1", &md);
    assert!(
        tab.wait_for("event: changed"),
        "the open tab hears of the hook's write"
    );
    assert_eq!(p.opened().len(), 1, "no second tab");
    fs::write(&md, THIN).unwrap();
    assert!(tab.wait_for("event: changed"), "and of a plain write");
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
}

#[test]
fn the_editor_saves_refuses_stale_or_foreign_saves_and_recomputes_coverage() {
    let p = Project::new();
    let md = p.brief("billing-retry", READY);
    p.wrote("s-1", &md);
    let info = p.info();
    let (text, rev) = load(&info, "billing-retry");
    assert_eq!(text, READY);

    // A save from the page writes the file as typed.
    let edited = READY.replace("Status: ready", "Status: draft");
    let (code, body) = save(&info, "billing-retry", &rev, &edited, &origin(&info));
    assert_eq!(code, 200, "{body}");
    assert_eq!(fs::read_to_string(&md).unwrap(), edited);
    let new_rev = serde_json::from_str::<serde_json::Value>(&body).unwrap()["rev"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(new_rev, rev);
    // A save based on an older version is refused, with what is on disk now.
    let (code, body) = save(&info, "billing-retry", &rev, "mine", &origin(&info));
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("Status: draft"), "{body}");
    // Only the page itself may save: another origin, or none, is refused.
    for from in ["http://evil.example", "null", ""] {
        let (code, _) = save(&info, "billing-retry", &new_rev, "pwned", from);
        assert_eq!(code, 403, "origin {from:?}");
    }
    assert_eq!(fs::read_to_string(&md).unwrap(), edited);

    // The strip, recomputed for the text the page sends.
    let from = origin(&info);
    let same = [
        ("Origin", from.as_str()),
        ("Content-Type", "text/plain;charset=UTF-8"),
    ];
    let coverage = |path: &str, headers: &[(&str, &str)], body: &str| {
        http(
            &info,
            "POST",
            &format!("/{}/{path}", info.token),
            headers,
            body,
        )
    };
    let (code, _, strip) = coverage("coverage?slug=billing-retry", &same, THIN);
    assert_eq!(code, 200, "{strip}");
    for must in [
        r#"<li class="cell thin" title="Goal: thin""#,
        r#"<li class="cell missing" title="Risks: missing""#,
        r#"<li class="cell covered" title="Scope: covered""#,
        "What the dry run still asks (2)",
        "<li>How many retries, and with what cap.</li>",
    ] {
        assert!(strip.contains(must), "missing {must:?} in:\n{strip}");
    }
    let (code, _, strip) = coverage("coverage?slug=billing-retry", &same, READY);
    assert_eq!(code, 200);
    assert!(
        strip.contains("cell covered")
            && !strip.contains("cell thin")
            && !strip.contains("cell missing")
            && strip.contains("What the dry run still asks (0)"),
        "{strip}"
    );
    // Without a slug, or with one that is none: 400; from elsewhere: 403.
    for path in [
        "coverage",
        "coverage?slug=",
        "coverage?slug=Billing%20Retry",
        "coverage?slug=..%2fx",
    ] {
        assert_eq!(coverage(path, &same, THIN).0, 400, "{path}");
    }
    assert_eq!(
        coverage(
            "coverage?slug=billing-retry",
            &[("Origin", "http://evil.example")],
            THIN
        )
        .0,
        403
    );
}

#[test]
fn the_list_opens_with_none_selected_and_the_writer_gets_its_brief() {
    let p = Project::new();
    p.brief("thin", THIN);
    let md = p.brief("billing-retry", READY);
    p.wrote("s-1", &md);
    // thin changes after it: the newest, but not this session's.
    touch(
        &p.briefs().join("thin.md"),
        SystemTime::now() + Duration::from_secs(60),
    );
    assert!(brief::briefs(&p.briefs())[0].ends_with("thin.md"));

    // A session that wrote none: the list.
    let said = p.say("s-2", "brief");
    let info = p.info();
    assert_eq!(
        p.opened().last(),
        Some(&info.list_url_of(brief::LIST, None)),
        "{:?}",
        p.opened()
    );
    assert!(browser::is_viewer_url(p.opened().last().unwrap()));
    assert!(
        said.contains("list of 2 briefs")
            && said.contains("hasn't written a brief")
            && said.ends_with(ENDING),
        "{said}"
    );

    // The list page, open on a brief when asked for it: its Goal, its tags.
    let (code, head, list) = http(
        &info,
        "GET",
        &format!("/{}/{}/billing-retry", info.token, brief::LIST),
        &[],
        "",
    );
    assert_eq!(code, 200, "{list}");
    assert!(
        regex::Regex::new(r#"data-slug="billing-retry"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&list),
        "{list}"
    );
    assert!(list.contains(r#"href="../thin.html""#), "{list}");
    assert!(list.contains("Add retries with backoff"), "{list}");
    assert!(head.contains("script-src 'nonce-") && head.contains("default-src 'none'"));

    // A list tab is open: `brief` from another session moves it.
    let mut tab = Tab::on(&info, brief::LIST, "");
    let said = p.say("s-3", "brief");
    assert!(
        tab.wait_for(&format!("event: show\ndata: {}", brief::LIST)),
        "the tab is moved"
    );
    assert!(said.contains("already open"), "{said}");

    // The session that wrote one gets it, not the list.
    let said = p.say("s-1", "brief");
    assert_eq!(p.opened().last(), Some(&info.url("billing-retry")));
    assert!(
        said.contains("opened the brief .claude/briefs/billing-retry.md")
            && said.contains(
                "Coverage for /deliver, status ready: 7 of 7 sections covered, 0 thin, 0 \
                 missing, 0 open."
            )
            && said.ends_with(ENDING),
        "{said}"
    );
}

// ------------------------------------------------------------- the word --

#[test]
fn brief_answers_in_each_state_and_says_how_to_send_the_word() {
    let p = Project::new();
    // None yet: nothing opens, no editor starts.
    let said = p.say("s-1", "brief");
    assert!(
        said.contains("there are no briefs in .claude/briefs/ yet")
            && said.contains("/brief writes one")
            && said.ends_with(ENDING),
        "{said}"
    );
    assert!(p.opened().is_empty());
    assert!(!p.root.join(brief::DIR).join(INFO_FILE).exists());

    // The only one, in any case: opened, with how far it is.
    p.brief("thin", THIN);
    let said = p.say("s-1", "BRIEF");
    let info = p.info();
    assert_eq!(p.opened(), [info.url("thin")]);
    assert!(
        said.contains("`brief`: ocgen opened the brief .claude/briefs/thin.md")
            && said.contains(
                "Coverage for /deliver, status draft: 4 of 7 sections covered, 2 thin, 1 \
                 missing, 2 open."
            )
            && said.ends_with(ENDING),
        "{said}"
    );
    // The word only opens the page: it never reaches Claude.
    assert!(said.contains("to send the word to Claude"), "{said}");

    // A tab holds it: no second one.
    let (_, rev) = load(&info, "thin");
    let _tab = Tab::on(&info, "thin", &rev);
    let said = p.say("s-1", " brief ");
    assert!(said.contains("already open"), "{said}");
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());

    // The browser switched off: where the file is, and that /brief goes on.
    let said = p.say_off("brief");
    assert!(
        said.contains("OCGEN_NOTES_OPEN=0")
            && said.contains(".claude/briefs/thin.md")
            && said.contains("/brief")
            && said.ends_with(ENDING),
        "{said}"
    );
    assert_eq!(p.opened().len(), 1);

    // A file the folder can't show is named.
    fs::write(p.briefs().join("Billing Retry.md"), THIN).unwrap();
    let said = p.say_off("brief");
    assert!(said.contains("Also: Billing Retry.md"), "{said}");
    assert!(said.ends_with(ENDING), "{said}");
}

#[test]
fn the_words_pass_through_where_their_skill_is_off() {
    // /brief off: `brief` reaches Claude as typed; `note` stays the ledgers'.
    let p = Project::with(|s| s.claude.workflow.brief = false);
    p.brief("thin", THIN);
    assert_eq!(p.say_off("brief"), "");
    assert!(
        p.say_off("note").contains("no /inquire notes"),
        "`note` is still answered"
    );
    assert!(!p.briefs().join(INFO_FILE).exists(), "no editor started");
    assert!(p.opened().is_empty());

    // /inquire off with /brief on (English answers: no reading copies):
    // `note` and `notes` reach Claude as typed, `brief` is answered.
    let p = Project::with(|s| {
        s.claude.workflow.inquire = false;
        s.claude.workflow.recap = false;
    });
    for word in ["note", "notes", "Note"] {
        assert_eq!(p.say_off(word), "", "{word}");
    }
    assert!(
        p.say_off("brief").contains("no briefs"),
        "`brief` is answered"
    );

    // Not the word: passes, whatever is on.
    for other in ["brief it", "a brief", "briefs", "/brief", "brief me", ""] {
        assert_eq!(p.say_off(other), "", "{other:?}");
    }
}

#[test]
fn without_a_readable_state_note_is_answered_and_brief_is_left_alone() {
    // A repository with no ocgen state, where the hooks run from a plugin:
    // `note` was the ledgers' word there before /brief rode on this hook and
    // stays so; `brief`, whose skill only the state can turn on, reaches
    // Claude as typed.
    let p = Project::bare("consumer");
    assert!(State::load_state(&p.root).is_err());
    for word in ["note", "notes", "Note"] {
        let said = p.say_off(word);
        assert!(
            said.contains("there are no /inquire notes in .claude/notes/ yet"),
            "{word}: {said}"
        );
    }
    p.brief("thin", THIN);
    for word in ["brief", "BRIEF", " brief "] {
        assert_eq!(p.say_off(word), "", "{word:?}");
    }
    assert_eq!(p.say_with("inquire-notes", "s-1", "brief", "1"), "");
    assert!(!p.briefs().join(INFO_FILE).exists(), "no editor started");
    assert!(p.opened().is_empty(), "{:?}", p.opened());
}

// ------------------------------------------------------- the file watcher --

#[test]
fn both_hooks_name_the_same_folders_to_watch() {
    let p = Project::new();
    for event in ["SessionStart", "CwdChanged"] {
        let notes = p.watched("inquire-notes", event);
        let intent = p.watched("intent-draft", event);
        assert_eq!(notes, intent, "{event}");
        assert!(notes.iter().all(|w| w.is_absolute()), "{notes:?}");
        for folder in [".claude/intent/drafts", "docs/adr", ".claude/briefs"] {
            assert!(
                notes.iter().any(|w| w.ends_with(folder)),
                "{folder}: {notes:?}"
            );
        }
        assert_eq!(notes.len(), 3, "{notes:?}");
    }
    // The same list in-process.
    assert_eq!(
        ocgen::hooks::watch_paths(&p.root),
        p.watched("inquire-notes", "SessionStart")
    );

    // /intent off: only the briefs' folder, from either hook.
    let p = Project::with(|s| s.claude.workflow.intent = false);
    let notes = p.watched("inquire-notes", "SessionStart");
    assert_eq!(notes, [p.root.join(brief::DIR)]);
    assert_eq!(p.watched("intent-draft", "CwdChanged"), notes);

    // /brief off: the drafts' and intent files' folders, from either hook.
    let p = Project::with(|s| s.claude.workflow.brief = false);
    let notes = p.watched("inquire-notes", "SessionStart");
    assert_eq!(notes, [p.root.join(draft::DIR), p.root.join("docs/adr")]);
    assert_eq!(p.watched("intent-draft", "SessionStart"), notes);
}

#[test]
fn a_brief_the_watcher_sees_change_is_the_sessions_and_nothing_else_is() {
    let p = Project::new();
    let a = p.brief("billing-retry", READY);
    let b = p.brief("thin", THIN);
    // Made by a Bash command (`cp`), or saved from the page: remembered, silently.
    assert_eq!(p.changed("inquire-notes", "s-1", &a, "add"), "");
    assert_eq!(p.session_brief("s-1").as_deref(), Some("billing-retry"));
    assert_eq!(p.changed("inquire-notes", "s-1", &b, "change"), "");
    assert_eq!(p.session_brief("s-1").as_deref(), Some("thin"));
    // Removed: the record points at nothing until the next change.
    fs::remove_file(&b).unwrap();
    assert_eq!(p.changed("inquire-notes", "s-1", &b, "unlink"), "");
    assert_eq!(p.session_brief("s-1"), None);
    assert_eq!(p.changed("inquire-notes", "s-1", &a, "change"), "");
    assert_eq!(p.session_brief("s-1").as_deref(), Some("billing-retry"));
    // No tab opened for any of it.
    assert!(p.opened().is_empty(), "{:?}", p.opened());

    // Not briefs: the folder's own records, a page, a file elsewhere, a draft.
    let draft = p.drafts().join("adr-0001-cache.md");
    fs::write(&draft, "## Intent\nKeep it warm.\n").unwrap();
    fs::write(p.briefs().join("billing-retry.html"), "<p>").unwrap();
    fs::write(p.root.join("brief.md"), "x").unwrap();
    for other in [
        p.briefs().join(brief::SESSIONS).join("s-1"),
        p.briefs().join(".gitignore"),
        p.briefs().join("billing-retry.html"),
        p.root.join("brief.md"),
        draft.clone(),
    ] {
        assert!(other.is_file(), "{other:?}");
        assert_eq!(
            p.changed("inquire-notes", "s-2", &other, "change"),
            "",
            "{other:?}"
        );
        assert_eq!(p.session_brief("s-2"), None, "{other:?}");
    }

    // Each hook leaves the other's files alone: a brief's change records no
    // draft, and a draft's change is the intent hook's.
    assert_eq!(p.changed("intent-draft", "s-2", &a, "change"), "");
    assert_eq!(draft::session_draft(&p.drafts(), "s-2"), None);
    assert_eq!(p.session_brief("s-2"), None);
    assert_eq!(p.changed("intent-draft", "s-2", &draft, "add"), "");
    assert_eq!(
        draft::session_draft(&p.drafts(), "s-2").as_deref(),
        Some("adr-0001-cache")
    );
    assert_eq!(p.session_brief("s-2"), None);

    // Nor does a session id that can't name a file record anything.
    assert_eq!(p.changed("inquire-notes", "../s", &a, "change"), "");
    assert!(!p.briefs().join(brief::SESSIONS).join("../s").exists());
}

// ------------------------------------------------------------ the names --

#[test]
fn a_file_whose_name_is_no_slug_gets_the_rename_context() {
    let p = Project::new();
    for name in ["Billing Retry.md", "billing-retry.ukrainian.md"] {
        let md = p.briefs().join(name);
        fs::write(&md, THIN).unwrap();
        let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
        let context = Project::context(&out).unwrap_or_else(|| panic!("{name}: {out:?}"));
        assert!(
            context.contains("is no slug") && context.contains(".claude/briefs/billing-retry.md"),
            "{context}"
        );
        assert!(out.stderr.is_empty(), "{out:?}");
    }
    // A name that is no slug at all: the rule, with an example made of the name.
    let md = p.briefs().join("Billing Retry.md");
    let context = Project::context(&p.wrote_with("inquire-notes", "s-1", &md, "1")).unwrap();
    assert!(
        context.contains("Rename it")
            && context.contains("ASCII a-z, 0-9 and hyphens")
            && context.contains("at most 80 characters")
            && context.contains("for example .claude/briefs/billing-retry.md"),
        "{context}"
    );
    // A language version written beside the brief, the habit of a bilingual
    // project: the one file to fold the text into, and no example made of the
    // name — `billing-retry-ukrainian.md` would be a second brief.
    let md = p.briefs().join("billing-retry.ukrainian.md");
    let context = Project::context(&p.wrote_with("inquire-notes", "s-1", &md, "1")).unwrap();
    assert!(
        context.contains("language version")
            && context.contains("one file")
            && context.contains("delete this one")
            && !context.contains("billing-retry-ukrainian")
            && !context.contains("for example")
            && !context.contains("Rename it"),
        "{context}"
    );
    // No page, no record, nothing opened.
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert!(!p.briefs().join(INFO_FILE).exists());
    assert_eq!(p.session_brief("s-1"), None);
    assert_eq!(
        brief::skipped(&p.briefs()),
        ["Billing Retry.md", "billing-retry.ukrainian.md"]
    );
    // A ledger named that way keeps the ledgers' note, on stderr.
    let bad = p.root.join(".claude/notes/Bad Name.md");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, "Topic: Bad\n## Q&A log\n").unwrap();
    let out = p.wrote_with("inquire-notes", "s-1", &bad, "1");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("inquire-notes:"),
        "{out:?}"
    );
}

#[test]
fn a_dotted_tail_folds_only_a_language_version() {
    let p = Project::new();
    let told = |name: &str| {
        let md = p.briefs().join(name);
        fs::write(&md, THIN).unwrap();
        let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
        assert!(out.stderr.is_empty(), "{out:?}");
        Project::context(&out).unwrap_or_else(|| panic!("{name}: {out:?}"))
    };
    // A language suffix in any case, with no brief beside it: the one file the
    // text belongs in is the slug before it.
    let context = told("Payment-Retry.Ukrainian.md");
    assert!(
        context.contains("language version")
            && context.contains("delete this one")
            && context.contains(".claude/briefs/payment-retry.md")
            && !context.contains("for example"),
        "{context}"
    );
    // And beside the brief it is a version of: that one file, nothing else.
    p.brief("billing-retry", READY);
    let context = told("billing-retry.ukrainian.md");
    assert!(
        context.contains("language version of .claude/briefs/billing-retry.md")
            && context.contains("delete this one")
            && !context.contains("for example")
            && !context.contains("Rename it"),
        "{context}"
    );
    // A dotted number is part of a name — python-3.md beside python-3.12.md is
    // another brief, not the original, and folding would merge the two — and
    // so is any other dotted word, with a brief beside it or not: a copy or a
    // backup kept under a dotted name is no language version, so it gets the
    // rule and the slugified example, never a file to fold into and never the
    // brief beside it as the target.
    p.brief("python-3", READY);
    for (name, example, shorter) in [
        ("release-1.2.md", "release-1-2", "release-1.md"),
        ("python-3.12.md", "python-3-12", "python-3.md"),
        (
            "migrate-to-next.js.md",
            "migrate-to-next-js",
            "migrate-to-next.md",
        ),
        (
            "billing-retry.copy.md",
            "billing-retry-copy",
            "billing-retry.md",
        ),
        (
            "billing-retry.backup.md",
            "billing-retry-backup",
            "billing-retry.md",
        ),
        (
            "billing-retry.old.md",
            "billing-retry-old",
            "billing-retry.md",
        ),
        // A language neither of this project nor one ocgen offers is a word.
        (
            "billing-retry.german.md",
            "billing-retry-german",
            "billing-retry.md",
        ),
    ] {
        let context = told(name);
        assert!(
            context.contains("Rename it")
                && context.contains(&format!("for example .claude/briefs/{example}.md"))
                && !context.contains("delete this one")
                && !context.contains(shorter),
            "{name}: {context}"
        );
    }
    // The briefs beside them are untouched and nothing opened: the note is all
    // the hook does for a misnamed file.
    assert_eq!(
        fs::read_to_string(p.briefs().join("billing-retry.md")).unwrap(),
        READY
    );
    assert_eq!(
        fs::read_to_string(p.briefs().join("python-3.md")).unwrap(),
        READY
    );
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert_eq!(p.session_brief("s-1"), None);

    // The same tail in a project that answers in that language: its state
    // says it is a language, and the file folds like the Ukrainian one.
    let p = Project::with(|s| s.response_language = "German".into());
    p.brief("billing-retry", READY);
    let md = p.briefs().join("billing-retry.german.md");
    fs::write(&md, THIN).unwrap();
    let context = Project::context(&p.wrote_with("inquire-notes", "s-1", &md, "1")).unwrap();
    assert!(
        context.contains("language version of .claude/briefs/billing-retry.md")
            && context.contains("delete this one")
            && !context.contains("for example"),
        "{context}"
    );
    assert_eq!(
        fs::read_to_string(p.briefs().join("billing-retry.md")).unwrap(),
        READY
    );
}

#[test]
fn a_misnamed_file_beside_the_brief_it_names_is_merged_into_it_never_moved_over_it() {
    let p = Project::new();
    let live = p.brief("billing-retry", READY);
    // The brief's own name, written wrongly, beside it: slugified, the name is
    // the live brief's, so the note never offers that file as the rename
    // target — a Claude that followed it would move this text over the live
    // brief — and asks for a merge into it instead.
    for name in ["Billing Retry.md", "billing_retry.md"] {
        let md = p.briefs().join(name);
        fs::write(&md, THIN).unwrap();
        let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
        assert!(out.stderr.is_empty(), "{out:?}");
        let context = Project::context(&out).unwrap_or_else(|| panic!("{name}: {out:?}"));
        assert!(
            context.contains("is no slug")
                && context.contains("brief .claude/briefs/billing-retry.md exists already")
                && context.contains("merge what is new in this file into that brief")
                && context.contains("keep what the brief holds")
                && context.contains("then remove this file"),
            "{name}: {context}"
        );
        for never in [
            "Rename it",
            "move the file",
            "for example",
            "delete this one",
            "put the text in that file",
        ] {
            assert!(!context.contains(never), "{name}: {never:?} in {context}");
        }
        // Both files are as they were: the note is all the hook does.
        assert_eq!(fs::read_to_string(&live).unwrap(), READY, "{name}");
        assert_eq!(fs::read_to_string(&md).unwrap(), THIN, "{name}");
    }
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert!(!p.briefs().join(INFO_FILE).exists());
    assert_eq!(p.session_brief("s-1"), None);
    // Without a brief of that name beside it, the same file is told to take it.
    fs::remove_file(&live).unwrap();
    let md = p.briefs().join("Billing Retry.md");
    let context = Project::context(&p.wrote_with("inquire-notes", "s-1", &md, "1")).unwrap();
    assert!(
        context.contains("Rename it")
            && context.contains("for example .claude/briefs/billing-retry.md")
            && !context.contains("exists already"),
        "{context}"
    );
}

/// The `.md` names the briefs folder of `p` holds, byte for byte as the disk
/// lists them, sorted.
fn listed(p: &Project) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(p.briefs())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    names.sort();
    names
}

#[test]
fn a_name_that_differs_from_its_slug_only_in_case_is_renamed_in_place_never_removed() {
    let p = Project::new();
    // Alone in the folder: the rule with the example, never the merge note. On
    // a disk that folds case (macOS, Windows) `billing-retry.md` already
    // reaches this very file — a Claude told to merge into it and remove this
    // file would delete the only copy — and there the note says to rename in
    // place. The assertions hold on either kind of disk.
    for name in ["Billing-Retry.md", "BILLING-RETRY.md"] {
        let md = p.briefs().join(name);
        fs::write(&md, THIN).unwrap();
        let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
        assert!(out.stderr.is_empty(), "{out:?}");
        let context = Project::context(&out).unwrap_or_else(|| panic!("{name}: {out:?}"));
        assert!(
            context.contains("Rename it")
                && context.contains("for example .claude/briefs/billing-retry.md")
                && !context.contains("exists already")
                && !context.contains("remove this file"),
            "{name}: {context}"
        );
        assert_eq!(listed(&p), [name], "{name}");
        let folds = p.briefs().join("billing-retry.md").is_file();
        assert_eq!(
            context.contains("rename it in place"),
            folds,
            "{name}: {context}"
        );
        assert_eq!(fs::read_to_string(&md).unwrap(), THIN, "{name}");
        fs::remove_file(&md).unwrap();
    }
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert_eq!(p.session_brief("s-1"), None);

    // Written over the live brief. On a disk that folds case the write lands
    // in `billing-retry.md` itself (the folder lists that one entry), and the
    // note says so: nothing to rename or remove, write it by that name. On a
    // disk that tells the two apart they are two files, and the merge note
    // holds.
    let live = p.brief("billing-retry", READY);
    let md = p.briefs().join("Billing-Retry.md");
    fs::write(&md, THIN).unwrap();
    let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
    assert!(out.stderr.is_empty(), "{out:?}");
    let context = Project::context(&out).unwrap_or_else(|| panic!("{out:?}"));
    let names = listed(&p);
    if names == ["billing-retry.md"] {
        assert_eq!(
            fs::read_to_string(&live).unwrap(),
            THIN,
            "the write landed in the brief"
        );
        assert!(
            context.contains("reaches .claude/briefs/billing-retry.md itself")
                && context.contains("nothing to rename or remove")
                && context.contains("write it as .claude/briefs/billing-retry.md"),
            "{context}"
        );
        for never in [
            "Rename it",
            "exists already",
            "remove this file",
            "delete",
            "for example",
        ] {
            assert!(!context.contains(never), "{never:?} in {context}");
        }
    } else {
        assert_eq!(names, ["Billing-Retry.md", "billing-retry.md"]);
        assert_eq!(fs::read_to_string(&live).unwrap(), READY);
        assert!(
            context.contains("brief .claude/briefs/billing-retry.md exists already")
                && context.contains("merge what is new in this file into that brief")
                && !context.contains("Rename it"),
            "{context}"
        );
    }
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert!(!p.briefs().join(INFO_FILE).exists());
    assert_eq!(p.session_brief("s-1"), None);
}

#[test]
fn a_misnamed_file_beside_a_case_variant_of_its_slug_is_never_moved_over_it() {
    let p = Project::new();
    // A stale misnamed file, the only copy of its text, never renamed; then a
    // differently misnamed file whose slug is the same. On a disk that folds
    // case `billing-retry.md` reaches the stale file, not this one, so a Claude
    // told to rename this file to it "in place" would move this text over the
    // stale one: the note names the file that spelling reaches, says never to
    // move this file over it, and offers no example. On a disk that tells the
    // two apart the slug name is free, and the rule with the example holds.
    let stale = p.briefs().join("BILLING-RETRY.md");
    fs::write(&stale, READY).unwrap();
    let md = p.briefs().join("Billing Retry.md");
    fs::write(&md, THIN).unwrap();
    let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
    assert!(out.stderr.is_empty(), "{out:?}");
    let context = Project::context(&out).unwrap_or_else(|| panic!("{out:?}"));
    assert_eq!(listed(&p), ["BILLING-RETRY.md", "Billing Retry.md"]);
    assert!(
        context.contains("is no slug")
            && !context.contains("rename it in place")
            && !context.contains("this very file")
            && !context.contains("exists already"),
        "{context}"
    );
    if p.briefs().join("billing-retry.md").is_file() {
        assert!(
            context.contains("already reaches .claude/briefs/BILLING-RETRY.md")
                && context.contains("moving this file there would overwrite it")
                && context.contains("Rename that one in place")
                && context.contains("never this one over it")
                && context.contains("rename this file to a slug")
                && !context.contains("for example"),
            "{context}"
        );
    } else {
        assert!(
            context.contains("Rename it")
                && context.contains("for example .claude/briefs/billing-retry.md")
                && !context.contains("BILLING-RETRY.md"),
            "{context}"
        );
    }
    // Both files are as they were: the note is all the hook does.
    assert_eq!(fs::read_to_string(&stale).unwrap(), READY);
    assert_eq!(fs::read_to_string(&md).unwrap(), THIN);
    assert!(p.opened().is_empty(), "{:?}", p.opened());
    assert!(!p.briefs().join(INFO_FILE).exists());
    assert_eq!(p.session_brief("s-1"), None);
}

#[test]
fn a_brief_shaped_file_in_the_drafts_folder_is_a_draft() {
    let p = Project::new();
    let md = p.drafts().join("billing-retry.md");
    fs::write(&md, READY).unwrap();
    assert_eq!(brief::brief_target(&md.to_string_lossy()), None);
    assert_eq!(
        draft::draft_target(&md.to_string_lossy()).as_deref(),
        Some("billing-retry")
    );
    // The notes hook: not its file — silent, no briefs folder, no editor.
    let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert!(!p.root.join(brief::DIR).exists());
    assert!(p.opened().is_empty());
    // The intent hook: a draft, remembered as one.
    let out = p.wrote_with("intent-draft", "s-1", &md, "1");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert_eq!(
        draft::session_draft(&p.drafts(), "s-1").as_deref(),
        Some("billing-retry")
    );
}

// ------------------------------------------------------- without a page --

#[test]
fn the_browser_switched_off_keeps_the_hook_silent() {
    let p = Project::new();
    let md = p.brief("billing-retry", READY);
    let out = p.wrote_with("inquire-notes", "s-1", &md, "0");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert!(p.opened().is_empty());
    assert!(!p.briefs().join(INFO_FILE).exists(), "no editor started");
    // The write still counts: the brief is the session's, the folder ignored.
    assert_eq!(p.session_brief("s-1").as_deref(), Some("billing-retry"));
    assert_eq!(
        fs::read_to_string(p.briefs().join(".gitignore")).unwrap(),
        "*\n"
    );
    assert!(!p.briefs().join("billing-retry.html").exists());
}

#[test]
fn notes_render_refuses_a_brief_and_writes_no_page() {
    let p = Project::new();
    let md = p.brief("billing-retry", READY);
    let out = Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["notes", "render"])
        .arg(&md)
        .current_dir(&p.root)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("has no static page") && err.contains("`brief`"),
        "{err}"
    );
    assert!(!p.briefs().join("billing-retry.html").exists());
}

#[test]
fn a_project_path_with_a_space_stays_plain() {
    let p = Project::build("my project", |_| {});
    let md = p.brief("billing-retry", READY);
    assert!(md.to_string_lossy().contains("my project"));
    p.wrote("s-1", &md);
    let info = p.info();
    assert_eq!(p.opened(), [info.url("billing-retry")]);
    assert!(!p.opened()[0].contains(' '));
    assert_eq!(p.session_brief("s-1").as_deref(), Some("billing-retry"));
    // The watcher's folders carry the space as it is.
    let watched = p.watched("inquire-notes", "SessionStart");
    assert!(
        !watched.is_empty() && watched.iter().all(|w| w.starts_with(&p.root)),
        "{watched:?}"
    );
    // The page serves, and a change the watcher reports by that path counts.
    let (code, _, page) = page(&info, "billing-retry");
    assert_eq!(code, 200);
    assert!(page.contains("Add retries with backoff"));
    assert_eq!(p.changed("inquire-notes", "s-2", &md, "change"), "");
    assert_eq!(p.session_brief("s-2").as_deref(), Some("billing-retry"));
}

#[test]
fn a_bilingual_project_asks_for_no_second_brief_and_holds_no_turn() {
    let p = Project::with(|s| s.response_language = "Ukrainian".into());
    let md = p.brief("billing-retry", READY);
    let out = p.wrote_with("inquire-notes", "s-1", &md, "1");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert!(!p.briefs().join(".versions").exists());
    assert!(!p.briefs().join("billing-retry.ukrainian.md").exists());
    for (event, agent) in [("Stop", ""), ("SubagentStop", "a-1")] {
        let said = p.said(
            "inquire-notes",
            serde_json::json!({
                "hook_event_name": event, "session_id": "s-1", "agent_id": agent,
                "stop_hook_active": false
            }),
            "1",
        );
        assert_eq!(said, "", "{event} holds no turn");
    }
    // The page speaks the answer language.
    let (code, _, page) = page(&p.info(), "billing-retry");
    assert_eq!(code, 200);
    assert!(page.contains(r#"<html lang="uk""#), "{page}");
    assert!(page.contains(">Мета</li>"), "{page}");
}

#[test]
fn the_viewer_refuses_a_directory_that_is_not_the_briefs() {
    let p = Project::new();
    let other = p.root.join(".claude/brief");
    fs::create_dir_all(&other).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ocgen"))
        .args(["notes", "serve"])
        .arg(&other)
        .env("OCGEN_NOTES_IDLE_SECS", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("or .claude/briefs directory") && err.contains(".claude/intent/drafts"),
        "{err}"
    );
    assert!(!other.join(INFO_FILE).exists());
}
