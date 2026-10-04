//! The /intent issue draft's browser review: the Markdown preview, the editor
//! page, and — end to end through the real binary — `ocgen draft` opening the
//! newest draft in a local editor that saves back to the file. The browser is a
//! fake (`OCGEN_NOTES_BROWSER`) that logs the URLs it is asked to open.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use ocgen::notes::draft;
use ocgen::notes::viewer::{self, Info};
use tempfile::TempDir;

/// The team's issue convention, filled in.
const DRAFT: &str = "\
## Intent
<!-- 2-3 lines. What should be true afterwards. -->
Deploys keep the cache warm.
Links: docs/adr/ADR-0001-cache.md

**Blocks prod?** No
**Depends on the platform view** (what we need to protect)? Yes

## Options and trade-offs
- **A.** Version the keys — cheap; a stale read for one TTL.
- **Do nothing.** Cold cache after each deploy.

| Option | Cost |
|--------|------|
| A      | low  |

## Needs from @alice
- [ ] @alice — pending

<details><summary>Plan (fill in once the intent is agreed)</summary>

- **Changes:** templates, code, and tests touched
- **Revert:** how to undo it
</details>
";

// ------------------------------------------------------------- preview --

#[test]
fn preview_renders_what_github_renders() {
    let html = draft::preview(DRAFT);
    for part in [
        "<h2>Intent</h2>",
        "<strong>Blocks prod?</strong>",
        "<table>",
        "<th>Option</th>",
        "<details><summary>Plan (fill in once the intent is agreed)</summary>",
        "</details>",
        "<strong>Changes:</strong>",
        r#"type="checkbox""#,
    ] {
        assert!(html.contains(part), "preview misses {part}:\n{html}");
    }
    // GitHub hides comments and keeps an issue's line breaks.
    assert!(!html.contains("2-3 lines"), "{html}");
    assert!(
        html.contains("<strong>Blocks prod?</strong> No<br"),
        "a single newline is a line break in an issue:\n{html}"
    );
}

#[test]
fn preview_never_runs_or_loads_anything_from_the_draft() {
    let evil = "\
<script>alert(1)</script>

<img src=x onerror=alert(2)>

Inline <b onclick=\"x()\">bold</b> and <kbd>Ctrl</kbd>.

[click](javascript:alert(3)) [ok](https://example.com/a) ![pixel](https://evil.example/p.png?d=1)

<!-- a comment
over two lines -->
";
    let html = draft::preview(evil);
    for bad in ["<script", "<img", "javascript:"] {
        assert!(!html.contains(bad), "{bad} survived:\n{html}");
    }
    // No element handles events or loads anything (escaped text may mention it).
    for attr in [r"\son\w+\s*=", r"\ssrc\s*="] {
        let re = regex::Regex::new(&format!(r"(?i)<[a-z][^>]*{attr}")).unwrap();
        assert!(!re.is_match(&html), "{attr} in a tag:\n{html}");
    }
    // The image is a link to it, never loaded.
    assert!(
        html.contains(r#"<a href="https://evil.example/p.png?d=1">pixel</a>"#),
        "{html}"
    );
    // Shown as text, not dropped silently; safe tags and links still work.
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(html.contains("<kbd>Ctrl</kbd>"), "{html}");
    assert!(
        html.contains(r#"<a href="https://example.com/a">ok</a>"#),
        "{html}"
    );
    assert!(!html.contains("over two lines"), "{html}");

    // `<!-->` is a whole (empty) comment: what follows still renders.
    let html = draft::preview("a <!--> b\n\n<details><summary>Plan</summary>\n\nx\n</details>\n");
    assert!(html.contains(" b"), "{html}");
    assert!(html.contains("<details><summary>Plan</summary>"), "{html}");
}

// ---------------------------------------------------------------- page --

/// Every element /intent drafts use: raw HTML, comments, task lists, tables,
/// code, links, @mentions and issue numbers.
const EVERYTHING: &str = include_str!("fixtures/intent-draft-everything.md");

/// The page without its one script (the editor bundle and the page's code).
fn markup(page: &str) -> String {
    let start = page.find("<script").expect("a script");
    let end = page[start..].find("</script>").expect("its end") + start;
    format!("{}{}", &page[..start], &page[end..])
}

/// The Source view's text, as the browser reads it.
fn source_text(page: &str) -> String {
    let open = page
        .find(r#"<textarea id="source""#)
        .expect("the Source view");
    let body = &page[open..];
    // The parser drops the newline that follows the start tag.
    let start = body.find(">\n").expect("start tag") + 2;
    let end = body.find("</textarea>").expect("end tag");
    body[start..end]
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[test]
fn page_holds_the_editor_the_source_the_preview_and_the_controls() {
    let page = draft::page(
        "# Title & <b>\n",
        ".claude/intent/drafts/adr-0001-cache.md",
        "0123456789abcdef",
        "n0nce",
        "English",
        &[],
    )
    .unwrap();
    // The formatted editor mounts here; the Source view holds the raw
    // Markdown, escaped.
    assert!(page.contains(r#"<div id="editor""#), "{}", markup(&page));
    assert!(
        page.contains(r#"<textarea id="source""#) && page.contains("# Title &amp; &lt;b&gt;"),
        "{}",
        markup(&page)
    );
    // The GitHub preview, Save, Copy, Source, Focus; the file it saves.
    for part in [
        r#"id="pane-preview""#,
        "GitHub preview",
        "Save",
        "Copy Markdown",
        "Copy formatted",
        "Source",
        "Focus",
        r#"id="palette-black" aria-pressed="true">Black<"#,
        r#"id="palette-white" aria-pressed="false">White<"#,
        "0123456789abcdef",
        ".claude/intent/drafts/adr-0001-cache.md",
    ] {
        assert!(page.contains(part), "page misses {part}");
    }
    // One script, nonce-tagged, and nothing loaded from anywhere.
    assert_eq!(
        page.matches("<script").count(),
        1,
        "only the page's own script"
    );
    assert!(page.contains(r#"<script nonce="n0nce">"#));
    // The dimming is on from the start; the Focus button takes the page full
    // screen (no browser toolbar), which only a click can do.
    assert!(page.contains(r#"<body class="zen focus""#));
    assert!(page.contains(
        r#"id="focus-mode" aria-pressed="false" title="Full screen: hides the browser’s toolbar (Esc leaves)">Focus<"#
    ));
    for api in ["requestFullscreen", "exitFullscreen", "fullscreenchange"] {
        assert!(page.contains(api), "the page doesn't use {api}");
    }
    // In full screen the menu goes too, all but the Focus button.
    assert!(page.contains(
        "body.full .where, body.full .tools > :not(#focus-mode), body.full footer { display: none; }"
    ));
    // Black by default, whatever the system's setting; white on request.
    assert!(page.contains(r#"<html lang="en" data-palette="black">"#));
    for palette in [
        r#"html[data-palette="black"]"#,
        r#"html[data-palette="white"]"#,
    ] {
        assert!(page.contains(palette), "no {palette} palette");
    }
    let html = markup(&page);
    for bad in [" src=", "<link", "<iframe", "<object", "<embed", "@import"] {
        assert!(!html.contains(bad), "{bad} in the page:\n{html}");
    }

    // The page's own words follow the answer language; the draft doesn't change.
    let uk = draft::page("Text\n", "d.md", "r", "n", "Ukrainian", &[]).unwrap();
    for part in [
        r#"lang="uk""#,
        "Зберегти",
        "Код Markdown",
        "Як на GitHub",
        "Фокус",
        "На весь екран: ховає панель браузера (Esc — вийти)",
        "Чорна",
        "Біла",
    ] {
        assert!(uk.contains(part), "Ukrainian page misses {part}");
    }
    // No Russian in the Ukrainian words.
    let words = markup(&uk);
    for letter in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
        assert!(!words.contains(letter), "{letter} in the Ukrainian page");
    }
}

#[test]
fn page_carries_the_original_markdown_verbatim() {
    let page = draft::page(EVERYTHING, "d.md", "r", "n", "English", &[]).unwrap();
    assert_eq!(source_text(&page), EVERYTHING);
    // A leading newline and quotes survive too.
    let odd = "\n  indented \"quoted\" 'single' & <tag>\n\n";
    let page = draft::page(odd, "d.md", "r", "n", "English", &[]).unwrap();
    assert_eq!(source_text(&page), odd);
}

#[test]
fn page_embeds_the_pinned_milkdown_bundle() {
    let version = draft::MILKDOWN_VERSION;
    assert!(
        version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()),
        "an exact version: {version}"
    );
    // The bundle says which Milkdown it is and how it was built.
    let js = draft::EDITOR_JS;
    let head: String = js.lines().take(3).collect::<Vec<_>>().join("\n");
    assert!(
        head.starts_with(&format!(
            "/*! ocgen /intent draft editor — Milkdown {version} "
        )),
        "{head}"
    );
    assert!(head.contains("npm ci && npm run build"), "{head}");
    assert!(js.contains("OcgenDraft"), "the editor's entry point");
    // Safe to inline: nothing ends the script or opens an HTML comment, and
    // nothing needs eval.
    for bad in ["</script", "</SCRIPT", "<!--", "new Function("] {
        assert!(!js.contains(bad), "{bad} in the bundle");
    }

    // The page inlines it in its one script.
    let page = draft::page("Text\n", "d.md", "r", "n0nce", "English", &[]).unwrap();
    let script = &page[page.find(r#"<script nonce="n0nce">"#).unwrap()..];
    assert!(script.contains(head.lines().next().unwrap()));

    // The build recipe pins the same version, exactly, and its lockfile too.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let pkg: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("tools/milkdown/package.json")).unwrap(),
    )
    .unwrap();
    let deps = pkg["dependencies"].as_object().unwrap();
    let milkdown: Vec<_> = deps
        .keys()
        .filter(|k| k.starts_with("@milkdown/"))
        .collect();
    assert!(milkdown.len() >= 4, "{milkdown:?}");
    for (name, v) in deps
        .iter()
        .chain(pkg["devDependencies"].as_object().unwrap())
    {
        let v = v.as_str().unwrap();
        assert!(
            v.chars().next().unwrap().is_ascii_digit(),
            "{name} is not pinned: {v}"
        );
        if name.starts_with("@milkdown/") {
            assert_eq!(v, version, "{name}");
        }
    }
    let lock: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("tools/milkdown/package-lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        lock["packages"]["node_modules/@milkdown/core"]["version"],
        version
    );

    // Milkdown's license ships with it.
    let licenses = fs::read_to_string(root.join("src/notes/milkdown/LICENSES.txt")).unwrap();
    assert!(
        licenses.contains(&format!("@milkdown/core@{version} — MIT"))
            && licenses.contains("Copyright (c) 2020-present Mirone")
            && licenses.contains("Permission is hereby granted, free of charge"),
        "{}",
        &licenses[..licenses.len().min(600)]
    );
}

// ------------------------------------------------------------ discovery --

#[test]
fn drafts_are_found_newest_first_and_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    assert!(draft::find(&drafts, None).is_err(), "none yet");
    fs::write(drafts.join("adr-0001-cache.md"), DRAFT).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(drafts.join("adr-0002-retry.md"), DRAFT).unwrap();
    fs::write(drafts.join("Not A Slug.md"), DRAFT).unwrap();
    fs::write(drafts.join(".gitignore"), "*\n").unwrap();

    let names = |v: Vec<PathBuf>| -> Vec<String> {
        v.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(draft::drafts(&drafts)),
        ["adr-0002-retry.md", "adr-0001-cache.md"]
    );
    assert!(draft::find(&drafts, None)
        .unwrap()
        .ends_with("adr-0002-retry.md"));
    // By file name, its stem, or the intent's id in any case.
    for name in ["adr-0001-cache", "adr-0001-cache.md", "ADR-0001"] {
        assert!(
            draft::find(&drafts, Some(name))
                .unwrap()
                .ends_with("adr-0001-cache.md"),
            "{name}"
        );
    }
    let err = draft::find(&drafts, Some("nope")).unwrap_err().to_string();
    assert!(err.contains("adr-0002-retry"), "{err}");

    assert_eq!(
        draft::draft_target("/p/.claude/intent/drafts/adr-0001-cache.md"),
        Some("adr-0001-cache".to_string())
    );
    assert_eq!(
        draft::draft_target(r"C:\p\.claude\intent\drafts\x.md"),
        Some("x".to_string())
    );
    assert_eq!(draft::draft_target("/p/.claude/intent/view/x.md"), None);
    assert_eq!(draft::draft_target("/p/.claude/intent/drafts/X Y.md"), None);
}

// ------------------------------------------------------ live editor e2e --

struct Project {
    dir: TempDir,
    log: PathBuf,
    browser: String,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let drafts = dir.path().join(".claude/intent/drafts");
        fs::create_dir_all(&drafts).unwrap();
        fs::write(drafts.join("adr-0001-cache.md"), DRAFT).unwrap();
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

    fn drafts(&self) -> PathBuf {
        self.dir.path().join(".claude/intent/drafts")
    }

    fn md(&self) -> PathBuf {
        self.drafts().join("adr-0001-cache.md")
    }

    /// `ocgen draft [args]` from the project directory.
    fn draft(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_ocgen"))
            .arg("draft")
            .args(args)
            .current_dir(self.dir.path())
            .env("OCGEN_NOTES_OPEN", "1")
            .env("OCGEN_NOTES_BROWSER", &self.browser)
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .env("OCGEN_NOTES_GRACE_MS", "5000")
            .env("OCGEN_NOTES_IDLE_SECS", "60")
            .stdin(Stdio::null())
            .output()
            .unwrap();
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
        viewer::read_info(&self.drafts()).expect("an editor is running")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if let Some(info) = viewer::read_info(&self.drafts()) {
            viewer::quit(&info);
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

/// The draft's current text and revision, as the editor sees them.
fn load(info: &Info) -> (String, String) {
    let (code, _, body) = http(
        info,
        "GET",
        &format!("/{}/adr-0001-cache.md", info.token),
        &[],
        "",
    );
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    (
        v["text"].as_str().unwrap().to_string(),
        v["rev"].as_str().unwrap().to_string(),
    )
}

fn save(info: &Info, rev: &str, text: &str, from: &str) -> (u16, String) {
    let (code, _, body) = http(
        info,
        "POST",
        &format!("/{}/adr-0001-cache.md?rev={rev}", info.token),
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
    fn connect(info: &Info, rev: &str) -> Tab {
        let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
        write!(
            s,
            "GET /{}/events?topic=adr-0001-cache&rev={rev} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
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

#[test]
fn ocgen_draft_opens_the_newest_draft_in_a_local_editor() {
    let p = Project::new();
    let out = p.draft(&[]);
    let opened = p.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let info = p.info();
    assert_eq!(opened[0], info.url("adr-0001-cache"));
    assert!(ocgen::notes::browser::is_viewer_url(&opened[0]));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(".claude/intent/drafts/adr-0001-cache.md"),
        "{said}"
    );
    // The drafts stay out of git.
    assert_eq!(
        fs::read_to_string(p.drafts().join(".gitignore")).unwrap(),
        "*\n"
    );

    // The page: the raw text in the editor, a strict policy for scripts.
    let (code, head, page) = http(
        &info,
        "GET",
        &format!("/{}/adr-0001-cache.html", info.token),
        &[],
        "",
    );
    assert_eq!(code, 200);
    assert!(page.contains("Deploys keep the cache warm."), "{page}");
    assert!(page.contains(draft::EDITOR_JS.lines().next().unwrap()));
    let csp = head
        .lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with("content-security-policy")
        })
        .expect("a CSP")
        .to_string();
    assert!(csp.contains("script-src 'nonce-"), "{csp}");
    assert!(!csp.contains("'unsafe-inline' https"), "{csp}");
    assert!(csp.contains("img-src data:;"), "no remote images: {csp}");
    // The editor bundle is inline: the policy stays as strict as before.
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    for loose in ["unsafe-eval", "https:", "http:", "cdn"] {
        assert!(!csp.contains(loose), "{loose}: {csp}");
    }

    // Asking again while the tab is open doesn't open a second one.
    let (_, rev) = load(&info);
    let _tab = Tab::connect(&info, &rev);
    let out = p.draft(&["adr-0001"]);
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("already open"),
        "{out:?}"
    );
}

#[test]
fn ocgen_draft_warns_about_lines_wrapped_by_hand() {
    let p = Project::new();
    let wrapped = include_str!("fixtures/intent-draft-wrapped.md");
    fs::write(p.md(), wrapped).unwrap();
    let out = p.draft(&[]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(
            "has 9 lines wrapped by hand mid-sentence (lines 3, 4, 5, 8, 9, 10, 11, 19, 23)"
        ) && said.contains("GitHub shows every newline in an issue as a line break"),
        "{said}"
    );
    // The convention's own line breaks are meant: no warning.
    fs::write(p.md(), DRAFT).unwrap();
    let said = String::from_utf8_lossy(&p.draft(&[]).stdout).into_owned();
    assert!(!said.contains("wrapped by hand"), "{said}");
}

#[test]
fn the_editor_saves_back_to_the_file_and_never_over_a_newer_one() {
    let p = Project::new();
    p.draft(&[]);
    let info = p.info();
    let (text, rev) = load(&info);
    assert_eq!(text, DRAFT);

    // A save from the page writes the file as typed.
    let edited = DRAFT.replace("Deploys keep", "Deploys always keep");
    let (code, body) = save(&info, &rev, &edited, &origin(&info));
    assert_eq!(code, 200, "{body}");
    assert_eq!(fs::read_to_string(p.md()).unwrap(), edited);
    let new_rev = serde_json::from_str::<serde_json::Value>(&body).unwrap()["rev"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(new_rev, rev);

    // A save based on an older version is refused, with what is on disk now.
    let (code, body) = save(&info, &rev, "mine", &origin(&info));
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("Deploys always keep"), "{body}");
    assert_eq!(fs::read_to_string(p.md()).unwrap(), edited);

    // A request without the token is refused before its body is read: one that
    // announces a body and never sends it still gets its answer at once.
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], info.port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    write!(
        s,
        "POST /0000000000000000/adr-0001-cache.md?rev= HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
         Origin: {}\r\nContent-Length: 900000\r\n\r\n",
        info.port,
        origin(&info)
    )
    .unwrap();
    let mut head = [0u8; 12];
    s.read_exact(&mut head).expect("an answer without the body");
    assert_eq!(&head, b"HTTP/1.1 404");

    // Only the page itself may save: another origin, or none, is refused.
    for from in ["http://evil.example", "null", ""] {
        let (code, _) = save(&info, &new_rev, "pwned", from);
        assert_eq!(code, 403, "origin {from:?}");
    }
    assert_eq!(fs::read_to_string(p.md()).unwrap(), edited);

    // Only existing drafts, by a plain name.
    let t = &info.token;
    for path in [
        format!("/{t}/new-file.md?rev="),
        format!("/{t}/..%2fx.md?rev="),
        format!("/{t}/.viewer.json?rev="),
    ] {
        let (code, _, _) = http(&info, "POST", &path, &[("Origin", &origin(&info))], "x");
        assert!(code == 404 || code == 400, "{path}: {code}");
    }
}

#[test]
fn the_editor_previews_on_request_and_hears_about_changes_on_disk() {
    let p = Project::new();
    p.draft(&[]);
    let info = p.info();

    let (code, _, html) = http(
        &info,
        "POST",
        &format!("/{}/render", info.token),
        &[("Origin", &origin(&info))],
        "## Heading\n- [x] done\n",
    );
    assert_eq!(code, 200);
    assert!(
        html.contains("<h2>Heading</h2>") && html.contains("checked"),
        "{html}"
    );

    // A change made elsewhere (Claude, another editor) reaches the open tab.
    let (_, rev) = load(&info);
    let mut tab = Tab::connect(&info, &rev);
    fs::write(p.md(), "## Changed elsewhere\n").unwrap();
    assert!(tab.wait_for("event: changed"), "the open tab hears of it");
}
