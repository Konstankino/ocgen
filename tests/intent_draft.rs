//! The /intent issue draft's browser review: the Markdown preview, the editor
//! page, the list of drafts, and — end to end through the real binary — `ocgen
//! draft` and the `draft` word opening a draft (or, with several, the list) in a
//! local editor that saves back to the file. The browser is a fake
//! (`OCGEN_NOTES_BROWSER`) that logs the URLs it is asked to open.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

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

/// Set `p`'s modification time.
fn touch(p: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// `n` drafts, `adr-0001-topic` to `adr-00nn-topic`, each a minute newer than
/// the one before: the last is the newest.
fn many_drafts(drafts: &Path, n: usize) -> Vec<String> {
    let base = SystemTime::now() - Duration::from_secs(24 * 3600);
    (1..=n)
        .map(|i| {
            let slug = format!("adr-{i:04}-topic");
            let p = drafts.join(format!("{slug}.md"));
            fs::write(&p, format!("## Intent\nTopic number {i}.\n")).unwrap();
            touch(&p, base + Duration::from_secs(60 * i as u64));
            slug
        })
        .collect()
}

#[test]
fn drafts_are_listed_ten_a_page_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    let slugs = many_drafts(&drafts, 23);
    let all = draft::drafts(&drafts);
    assert_eq!(draft::PER_PAGE, 10);
    for (n, pages) in [(0, 1), (1, 1), (10, 1), (11, 2), (23, 3)] {
        assert_eq!(draft::pages(n), pages, "{n} drafts");
    }
    // Newest first: 23 to 14 on page 1, 13 to 4 on page 2, 3 to 1 on page 3.
    for (i, page) in [(22, 1), (13, 1), (12, 2), (3, 2), (2, 3), (0, 3)] {
        assert_eq!(draft::page_of(&all, Some(&slugs[i])), page, "{}", slugs[i]);
    }
    assert_eq!(draft::page_of(&all, None), 1);
    assert_eq!(draft::page_of(&all, Some("adr-9999-gone")), 1);
}

#[test]
fn a_session_remembers_the_draft_it_wrote_last() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    fs::write(drafts.join("adr-0082-cache.md"), DRAFT).unwrap();
    fs::write(drafts.join("adr-0084-retry.md"), DRAFT).unwrap();
    assert_eq!(draft::session_draft(&drafts, "s-1"), None);

    assert!(draft::remember(&drafts, "s-1", "adr-0082-cache"));
    assert!(draft::remember(&drafts, "s-1", "adr-0084-retry"));
    assert!(draft::remember(&drafts, "s-2", "adr-0082-cache"));
    assert_eq!(
        draft::session_draft(&drafts, "s-1").as_deref(),
        Some("adr-0084-retry")
    );
    assert_eq!(
        draft::session_draft(&drafts, "s-2").as_deref(),
        Some("adr-0082-cache")
    );
    // The records are neither drafts nor in git (the folder ignores itself).
    assert_eq!(draft::drafts(&drafts).len(), 2);

    // A draft that is gone is no session's.
    fs::remove_file(drafts.join("adr-0084-retry.md")).unwrap();
    assert_eq!(draft::session_draft(&drafts, "s-1"), None);

    // Only a plain session id and a draft's slug make a record.
    for bad in ["", "../x", "a/b", "x y", "s\\1"] {
        assert!(!draft::remember(&drafts, bad, "adr-0082-cache"), "{bad:?}");
    }
    assert!(!draft::remember(&drafts, "s-3", "../../etc/passwd"));
    assert_eq!(draft::session_draft(&drafts, "../s-1"), None);

    // A record older than a month is dropped on the next write.
    let old = drafts.join(draft::SESSIONS).join("s-old");
    fs::write(&old, "adr-0082-cache").unwrap();
    touch(
        &old,
        SystemTime::now() - Duration::from_secs(31 * 24 * 3600),
    );
    assert!(draft::remember(&drafts, "s-1", "adr-0082-cache"));
    assert!(!old.exists(), "pruned");
    assert!(drafts.join(draft::SESSIONS).join("s-2").exists(), "kept");
}

#[test]
fn a_draft_is_summed_up_by_its_first_line_of_text() {
    assert_eq!(draft::summary(DRAFT), "Deploys keep the cache warm.");
    assert_eq!(
        draft::summary("<!-- Issue: https://github.com/o/r/issues/84 -->\n<!--\nmany\nlines\n-->\n\n# Title\n\n**Bold** start\n"),
        "**Bold** start"
    );
    assert_eq!(draft::summary("## Only headings\n"), "");
    let long = format!("## Intent\n{}\n", "word ".repeat(60));
    let s = draft::summary(&long);
    assert!(s.chars().count() <= 121 && s.ends_with('…'), "{s}");
}

/// Names a Windows session really wrote: 66, 89 and 102 characters. ocgen
/// once dropped the two over 80 without a word, and kept opening the third.
const SHORT: &str = "082-a-flow-subject-id-is-checked-for-shape-and-against-dim-subject";
const LONG: &str =
    "084-the-ci-runner-lives-in-its-own-non-routable-subnets-and-holds-no-standing-data-access";
const LONGER: &str = "084-the-ci-runner-lives-in-its-own-non-routable-subnets-and-holds-no-standing-data-access-comment-1031";

#[test]
fn long_names_are_drafts_and_the_rest_are_named_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (i, name) in [LONG, LONGER, "ADR_0090-Mixed", SHORT].iter().enumerate() {
        let p = drafts.join(format!("{name}.md"));
        fs::write(&p, DRAFT).unwrap();
        touch(&p, base + Duration::from_secs(60 * i as u64));
    }
    let too_long = "a".repeat(201);
    for odd in ["has space", "dots.in.name", "Ünicode", too_long.as_str()] {
        fs::write(drafts.join(format!("{odd}.md")), DRAFT).unwrap();
    }
    fs::write(drafts.join("notes.txt"), "not Markdown").unwrap();
    fs::write(drafts.join(".gitignore"), "*\n").unwrap();

    // Every draft, newest first, whatever its length or case.
    let stems: Vec<String> = draft::drafts(&drafts)
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(stems, [SHORT, "ADR_0090-Mixed", LONGER, LONG]);
    for (name, ok) in [
        (LONG, true),
        (LONGER, true),
        ("ADR_0090-Mixed", true),
        (&"a".repeat(200)[..], true),
        (&too_long[..], false),
        ("_x", false),
        ("-x", false),
        ("x y", false),
        ("a.b", false),
        ("", false),
    ] {
        assert_eq!(draft::is_name(name), ok, "{name:?}");
    }

    // By name: exact first (LONGER starts with LONG), then by its start, any case.
    assert!(draft::find(&drafts, Some(LONG))
        .unwrap()
        .ends_with(format!("{LONG}.md")));
    assert!(
        draft::find(&drafts, Some("084"))
            .unwrap()
            .ends_with(format!("{LONGER}.md")),
        "the newer of the two"
    );
    assert!(draft::find(&drafts, Some("adr_0090"))
        .unwrap()
        .ends_with("ADR_0090-Mixed.md"));

    // What isn't a draft is named, with why — never dropped without a word.
    let skipped = draft::skipped(&drafts);
    let names: Vec<&str> = skipped.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        [
            format!("{too_long}.md").as_str(),
            "dots.in.name.md",
            "has space.md",
            "Ünicode.md"
        ]
    );
    let err = draft::find(&drafts, Some("nope")).unwrap_err().to_string();
    assert!(
        err.contains(LONG) && err.contains("has space.md") && err.contains("rename"),
        "{err}"
    );

    // A session's record and the list take them like any draft.
    assert!(draft::remember(&drafts, "s-1", LONG));
    assert_eq!(draft::session_draft(&drafts, "s-1").as_deref(), Some(LONG));
    let page = draft::list_page(&drafts, None, Some(LONG), "../", "n").unwrap();
    assert!(
        page.contains(&format!(r#"href="../{LONG}.html""#)),
        "{page}"
    );
    assert!(
        regex::Regex::new(&format!(r#"data-slug="{LONG}"[^>]*aria-current="true""#))
            .unwrap()
            .is_match(&page),
        "{page}"
    );
    assert!(
        page.contains("has space.md") && page.contains("dots.in.name.md"),
        "{page}"
    );

    // The browser opener takes their URLs (`cmd /C start` sees nothing special).
    let url = |path: &str| format!("http://127.0.0.1:4000/0123abcd/{path}");
    for path in [
        format!("{LONG}.html"),
        format!("{}/{LONG}", draft::LIST),
        "ADR_0090-Mixed.html".to_string(),
        format!("{}/ADR_0090-Mixed", draft::LIST),
    ] {
        assert!(ocgen::notes::browser::is_viewer_url(&url(&path)), "{path}");
    }
    for path in [
        format!("{too_long}.html"),
        "a&b.html".to_string(),
        "_x.html".to_string(),
    ] {
        assert!(!ocgen::notes::browser::is_viewer_url(&url(&path)), "{path}");
    }
}

#[test]
fn the_list_shows_ten_drafts_a_page_with_the_selected_one_marked() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    let slugs = many_drafts(&drafts, 23);
    // Its text is the draft's, never markup.
    let tricky = drafts.join(format!("{}.md", slugs[5]));
    fs::write(
        &tricky,
        "<!-- Issue: https://github.com/o/r/issues/84 -->\n<!-- Status: Accepted -->\n## Intent\n<img src=x onerror=alert(1)> cache\n",
    )
    .unwrap();
    touch(
        &tricky,
        SystemTime::now() - Duration::from_secs(24 * 3600) + Duration::from_secs(6 * 60),
    );

    let page = draft::list_page(&drafts, None, Some(&slugs[12]), "../", "n0nce").unwrap();
    // Page 2 of 3: drafts 13 to 4, newest first.
    let shown: Vec<&str> = regex::Regex::new(r#"data-slug="([^"]+)""#)
        .unwrap()
        .captures_iter(&page)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let want: Vec<&str> = slugs[3..=12].iter().rev().map(String::as_str).collect();
    assert_eq!(shown, want);
    // The selected one is marked, and each links to its editor.
    let selected = regex::Regex::new(r#"data-slug="([^"]+)"[^>]*aria-current="true""#)
        .unwrap()
        .captures(&page)
        .expect("a selected row")[1]
        .to_string();
    assert_eq!(selected, slugs[12]);
    assert!(
        page.contains(&format!(r#"href="../{}.html""#, slugs[12])),
        "{page}"
    );
    // The pager: newer is page 1, older page 3.
    assert!(
        page.contains(r#"href="?page=1""#) && page.contains(r#"href="?page=3""#),
        "{page}"
    );
    assert!(page.contains("Page 2 of 3"), "{page}");
    assert!(page.contains(r#"<script nonce="n0nce">"#));
    // The draft's markers, and its text as text.
    assert!(page.contains("https://github.com/o/r/issues/84") && page.contains("Accepted"));
    assert!(
        page.contains("&lt;img src=x") && !page.contains("<img src=x"),
        "{page}"
    );

    // Asked for a page: that page, the selection kept; out of range: the last.
    let page3 = draft::list_page(&drafts, Some(3), Some(&slugs[12]), "../", "n").unwrap();
    assert_eq!(page3.matches("data-slug=").count(), 3);
    assert!(!page3.contains(r#"aria-current="true""#));
    let last = draft::list_page(&drafts, Some(9), None, "./", "n").unwrap();
    assert!(last.contains("Page 3 of 3"));
    // Nothing selected: page 1, no mark.
    let first = draft::list_page(&drafts, None, None, "./", "n").unwrap();
    assert!(first.contains("Page 1 of 3") && !first.contains(r#"aria-current="true""#));
    assert!(first.contains(&format!(r#"href="./{}.html""#, slugs[22])));
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

    /// `ocgen hook intent-draft` with `event` on stdin, as Claude Code runs it.
    fn hook(&self, event: serde_json::Value) -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ocgen"))
            .args(["hook", "intent-draft"])
            .current_dir(self.dir.path())
            .env(
                "CLAUDE_PROJECT_DIR",
                ocgen::paths::for_shell(self.dir.path()),
            )
            .env("OCGEN_NOTES_OPEN", "1")
            .env("OCGEN_NOTES_BROWSER", &self.browser)
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .env("OCGEN_NOTES_GRACE_MS", "5000")
            .env("OCGEN_NOTES_IDLE_SECS", "60")
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
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The hook after Claude writes draft `name` in session `sid`.
    fn wrote(&self, sid: &str, name: &str) -> String {
        let file = ocgen::paths::for_shell(&self.drafts().join(name));
        self.hook(serde_json::json!({
            "hook_event_name": "PostToolUse", "session_id": sid,
            "tool_name": "Write", "tool_input": { "file_path": file }
        }))
    }

    /// The hook when the user sends `draft` in session `sid`.
    fn word(&self, sid: &str) -> String {
        self.hook(serde_json::json!({
            "hook_event_name": "UserPromptSubmit", "session_id": sid, "prompt": "draft"
        }))
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
        Tab::on(info, "adr-0001-cache", rev)
    }

    /// A tab of `topic`: a draft's slug, or the list's.
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
fn draft_lists_the_drafts_with_the_one_this_session_wrote_selected() {
    let p = Project::new();
    let drafts = p.drafts();
    // The reported case: the session works on 0084; 0082 changes after it.
    fs::write(drafts.join("adr-0084-retry.md"), DRAFT).unwrap();
    assert_eq!(
        p.wrote("s-1", "adr-0084-retry.md"),
        "",
        "a write gets no note"
    );
    fs::write(drafts.join("adr-0082-cache.md"), DRAFT).unwrap();
    touch(
        &drafts.join("adr-0082-cache.md"),
        SystemTime::now() + Duration::from_secs(60),
    );
    assert!(draft::find(&drafts, None)
        .unwrap()
        .ends_with("adr-0082-cache.md"));

    // `draft`: the list, open on the session's draft, not on the newest.
    let said = p.word("s-1");
    let info = p.info();
    let opened = p.opened();
    assert_eq!(opened, [info.list_url(Some("adr-0084-retry"))]);
    assert!(
        ocgen::notes::browser::is_viewer_url(&opened[0]),
        "{}",
        opened[0]
    );
    assert!(
        said.contains("list of 3 issue drafts")
            && said.contains(".claude/intent/drafts/adr-0084-retry.md")
            && said.contains("this session last wrote"),
        "{said}"
    );
    assert!(!said.contains("adr-0082"), "{said}");

    // The page it opens: the session's draft selected, a strict policy.
    let (code, head, page) = http(
        &info,
        "GET",
        &format!("/{}/{}/adr-0084-retry", info.token, draft::LIST),
        &[],
        "",
    );
    assert_eq!(code, 200, "{page}");
    assert!(
        regex::Regex::new(r#"data-slug="adr-0084-retry"[^>]*aria-current="true""#)
            .unwrap()
            .is_match(&page),
        "{page}"
    );
    assert!(page.contains(r#"href="../adr-0082-cache.html""#), "{page}");
    assert!(head.contains("script-src 'nonce-") && head.contains("default-src 'none'"));
    // With nothing selected, and only for drafts directories.
    let (code, _, page) = http(
        &info,
        "GET",
        &format!("/{}/{}", info.token, draft::LIST),
        &[],
        "",
    );
    assert_eq!(code, 200);
    assert!(page.contains(r#"href="./adr-0082-cache.html""#) && !page.contains("aria-current"));

    // A list tab is open: `draft` moves it instead of opening a second one.
    let mut tab = Tab::on(&info, draft::LIST, "");
    let said = p.word("s-2");
    assert!(
        tab.wait_for(&format!("event: show\ndata: {}", draft::LIST)),
        "the tab is moved"
    );
    assert_eq!(p.opened().len(), 1, "{:?}", p.opened());
    assert!(
        said.contains("already open") && said.contains("hasn't written"),
        "{said}"
    );

    // A draft changes on disk: the open list reloads.
    fs::write(drafts.join("adr-0001-cache.md"), "## Changed\n").unwrap();
    assert!(tab.wait_for("event: reload"), "the list hears of it");
}

#[test]
fn a_draft_bash_writes_is_the_sessions_too() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    let short = drafts.join(format!("{SHORT}.md"));
    let long = drafts.join(format!("{LONG}.md"));
    fs::write(&short, DRAFT).unwrap();
    let base = SystemTime::now() - Duration::from_secs(3600);
    touch(&short, base);

    // The reported case: `cp 082 084` in a Bash call makes 084 the session's.
    assert!(draft::before_bash(&drafts, "toolu_1"));
    fs::copy(&short, &long).unwrap();
    assert_eq!(
        draft::after_bash(&drafts, "toolu_1", "s-1").as_deref(),
        Some(LONG)
    );
    assert_eq!(draft::session_draft(&drafts, "s-1").as_deref(), Some(LONG));
    // The listing taken before the call is gone once it's used.
    assert!(
        fs::read_dir(drafts.join(draft::SESSIONS).join(draft::BEFORE))
            .unwrap()
            .next()
            .is_none()
    );

    // A call that changes no draft changes nothing; nor does one never seen before.
    assert!(draft::before_bash(&drafts, "toolu_2"));
    assert_eq!(draft::after_bash(&drafts, "toolu_2", "s-1"), None);
    assert_eq!(draft::after_bash(&drafts, "toolu_unseen", "s-1"), None);
    assert_eq!(draft::session_draft(&drafts, "s-1").as_deref(), Some(LONG));

    // Changed in place with its time and size kept (`cp -p`, a restore): seen by its text.
    assert!(draft::before_bash(&drafts, "toolu_3"));
    let same_size = DRAFT.replace("Deploys keep", "Deploys kept");
    assert_eq!(same_size.len(), DRAFT.len());
    fs::write(&short, &same_size).unwrap();
    touch(&short, base);
    assert_eq!(
        draft::after_bash(&drafts, "toolu_3", "s-2").as_deref(),
        Some(SHORT)
    );

    // Renamed (`mv`): the new name.
    assert!(draft::before_bash(&drafts, "toolu_4"));
    fs::rename(&short, drafts.join("adr-0082-renamed.md")).unwrap();
    assert_eq!(
        draft::after_bash(&drafts, "toolu_4", "s-2").as_deref(),
        Some("adr-0082-renamed")
    );

    // Several at once: the newest, then by name — the same answer every time.
    assert!(draft::before_bash(&drafts, "toolu_5"));
    let t = SystemTime::now();
    for name in ["b-two", "a-one", "c-old"] {
        let p = drafts.join(format!("{name}.md"));
        fs::write(&p, DRAFT).unwrap();
        touch(&p, if name == "c-old" { base } else { t });
    }
    assert_eq!(
        draft::after_bash(&drafts, "toolu_5", "s-3").as_deref(),
        Some("a-one")
    );

    // Bad call ids make no listing; a listing left by a call that never
    // finished (a refused command) is dropped after a day.
    for bad in ["", "../x", "a b"] {
        assert!(!draft::before_bash(&drafts, bad), "{bad:?}");
    }
    let before = drafts.join(draft::SESSIONS).join(draft::BEFORE);
    fs::write(before.join("toolu_orphan"), "").unwrap();
    touch(
        &before.join("toolu_orphan"),
        SystemTime::now() - Duration::from_secs(2 * 24 * 3600),
    );
    assert!(draft::before_bash(&drafts, "toolu_6"));
    assert!(!before.join("toolu_orphan").exists());
    // Nor are listings drafts.
    assert!(draft::drafts(&drafts)
        .iter()
        .all(|p| p.parent() == Some(drafts.as_path())));
}

#[test]
fn a_long_named_draft_made_by_bash_is_remembered_listed_opened_and_saved() {
    let p = Project::new();
    let md = p.drafts().join(format!("{LONG}.md"));
    // Made by `cp` in a Bash call, as the reported session did.
    let bash = |event: &str| {
        p.hook(serde_json::json!({
            "hook_event_name": event, "session_id": "s-1", "tool_name": "Bash",
            "tool_use_id": "toolu_cp", "tool_input": { "command": "cp a b" }
        }))
    };
    assert_eq!(bash("PreToolUse"), "", "silent before");
    fs::copy(p.md(), &md).unwrap();
    assert_eq!(bash("PostToolUse"), "", "silent after");
    assert_eq!(
        draft::session_draft(&p.drafts(), "s-1").as_deref(),
        Some(LONG)
    );
    // Another draft changes after it: the session's own is still the one.
    fs::write(p.drafts().join("adr-0001-cache.md"), "## Later\n").unwrap();
    touch(
        &p.drafts().join("adr-0001-cache.md"),
        SystemTime::now() + Duration::from_secs(60),
    );

    let said = p.word("s-1");
    let info = p.info();
    assert_eq!(p.opened(), [info.list_url(Some(LONG))]);
    assert!(ocgen::notes::browser::is_viewer_url(&p.opened()[0]));
    assert!(said.contains(&format!("{LONG}.md")), "{said}");

    let t = &info.token;
    let (code, _, page) = http(
        &info,
        "GET",
        &format!("/{t}/{}/{LONG}", draft::LIST),
        &[],
        "",
    );
    assert_eq!(code, 200, "{page}");
    assert!(page.contains(&format!(r#"data-slug="{LONG}""#)), "{page}");
    let (code, _, page) = http(&info, "GET", &format!("/{t}/{LONG}.html"), &[], "");
    assert_eq!(code, 200, "{page}");
    assert!(page.contains("Deploys keep the cache warm."));

    // Saved from the editor like any draft.
    let (code, _, body) = http(&info, "GET", &format!("/{t}/{LONG}.md"), &[], "");
    assert_eq!(code, 200, "{body}");
    let rev = serde_json::from_str::<serde_json::Value>(&body).unwrap()["rev"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, body) = http(
        &info,
        "POST",
        &format!("/{t}/{LONG}.md?rev={rev}"),
        &[("Origin", &origin(&info))],
        "## Edited\n",
    );
    assert_eq!(code, 200, "{body}");
    assert_eq!(fs::read_to_string(&md).unwrap(), "## Edited\n");

    // And by the start of its name, from a terminal.
    p.draft(&["084-the-ci-runner-lives"]);
    assert_eq!(p.opened().last().unwrap(), &info.url(LONG));
}

#[test]
fn ocgen_draft_lists_several_drafts_and_opens_one_by_name() {
    let p = Project::new();
    fs::write(p.drafts().join("issue-retry-budget.md"), DRAFT).unwrap();
    let out = p.draft(&[]);
    let info = p.info();
    assert_eq!(p.opened(), [info.list_url(None)]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("2 issue drafts"), "{said}");

    // A name still opens that draft.
    p.draft(&["adr-0001"]);
    assert_eq!(p.opened()[1], info.url("adr-0001-cache"));
    let (_, _, page) = http(
        &info,
        "GET",
        &format!("/{}/adr-0001-cache.html", info.token),
        &[],
        "",
    );
    // The editor links back to the list, open on this draft.
    assert!(
        page.contains(&format!(r#"href="{}/adr-0001-cache""#, draft::LIST)),
        "a way back to the list"
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
