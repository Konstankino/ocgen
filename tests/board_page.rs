//! The `/workstreams` board's page: `.claude/workstreams/board.md` gets a
//! read-only `board.html` beside it — the `Status:` line, one row per
//! workstream, the other sections as written — rendered by ocgen after every
//! write and shown again by `ocgen notes open workstreams`. The board is
//! operational and English only: it never gets a second language version.

use std::fs;
use std::path::{Path, PathBuf};

use ocgen::notes::{board, check_renderable, render_file, versions};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::{tempdir, TempDir};

const BOARD: &str = "# Workstreams — Ship 0.9.7\n\
Status: APPROVED session=abc123\n\
Goal: Ship the release in three parts\n\
Base: refs/heads/main @ 0123456789abcdef0123456789abcdef01234567\n\
Check: cargo test --offline\n\
Undo: git reset --keep 0123456789abcdef0123456789abcdef01234567\n\
\n\
## Workstreams\n\
- Workstream: billing-retry — Worker: session billing-retry — Branch: refs/heads/worktree-billing-retry — Worktree: /abs/.claude/worktrees/billing-retry — Tip: 89abcdef0123456789abcdef0123456789abcdef — State: integrated — Gate: merged — After: none — Merged: fedcba9876543210fedcba9876543210fedcba98\n\
- Workstream: login-timeout — Worker: subagent a1b2 — Branch: refs/heads/worktree-login-timeout —\n\
  Worktree: /abs/.claude/worktrees/login-timeout — Tip: 0011223344556677889900112233445566778899 — State: checked — Gate: ready — After: billing-retry\n\
- Workstream: audit-log — Worker: session audit-log (yours) <i>late</i> — Branch: — — Worktree: — — Tip: — — State: dispatched — Gate: pending — After: none\n\
\n\
## Differs\n\
- login-timeout: src/auth/mod.rs — a shared helper it needed\n\
\n\
## Log\n\
- 2026-10-24T10:00:00Z billing-retry reported: Check pass, Confidence 97%\n\
- 2026-10-24T10:05:00Z login-timeout dispatched as a subagent <script>alert(1)</script>\n";

/// A bare folder (no ocgen state) holding a board, with a brief and a report
/// for one workstream and a brief alone for another.
fn board_dir() -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let ws = dir.path().join(".claude/workstreams");
    fs::create_dir_all(ws.join("billing-retry")).unwrap();
    fs::create_dir_all(ws.join("login-timeout")).unwrap();
    fs::write(
        ws.join("billing-retry/brief.md"),
        "# Workstream: billing-retry\n",
    )
    .unwrap();
    fs::write(
        ws.join("billing-retry/report.md"),
        "Workstream: billing-retry\n",
    )
    .unwrap();
    fs::write(
        ws.join("login-timeout/brief.md"),
        "# Workstream: login-timeout\n",
    )
    .unwrap();
    let md = ws.join("board.md");
    fs::write(&md, BOARD).unwrap();
    (dir, md)
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|_| panic!("missing {}", p.display()))
}

fn assert_has(page: &str, all: &[&str]) {
    for must in all {
        assert!(page.contains(must), "missing {must:?} in:\n{page}");
    }
}

#[test]
fn the_board_parses_leniently() {
    let b = board::parse(BOARD);
    assert_eq!(b.title.as_deref(), Some("Workstreams — Ship 0.9.7"));
    assert_eq!(
        b.fields[0],
        ("Status".to_string(), "APPROVED session=abc123".to_string())
    );
    assert_eq!(b.status(), Some("APPROVED session=abc123"));
    assert_eq!(b.entries.len(), 3, "{:?}", b.entries);
    let slugs: Vec<&str> = b
        .entries
        .iter()
        .filter_map(|e| e.get("Workstream"))
        .collect();
    assert_eq!(slugs, ["billing-retry", "login-timeout", "audit-log"]);
    // A wrapped entry is one entry until the next `- `.
    let wrapped = &b.entries[1];
    assert_eq!(
        wrapped.get("Worktree"),
        Some("/abs/.claude/worktrees/login-timeout")
    );
    assert_eq!(wrapped.get("Gate"), Some("ready"));
    assert_eq!(wrapped.get("After"), Some("billing-retry"));
    // An unknown value is `—`, and a `—` value never swallows the next key.
    let bare = &b.entries[2];
    assert_eq!(bare.get("Branch"), Some("—"));
    assert_eq!(bare.get("Worktree"), Some("—"));
    assert_eq!(bare.get("Tip"), Some("—"));
    assert_eq!(bare.get("State"), Some("dispatched"));
    assert_eq!(
        bare.get("Worker"),
        Some("session audit-log (yours) <i>late</i>")
    );
    assert_eq!(bare.get("Nothing"), None);
    // Keys keep their order: the page shows the extra ones after the fixed columns.
    let keys: Vec<&str> = b.entries[0]
        .fields
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "Workstream",
            "Worker",
            "Branch",
            "Worktree",
            "Tip",
            "State",
            "Gate",
            "After",
            "Merged"
        ]
    );
    // CRLF, a tab after the dash and a missing Status line don't stop it.
    let b = board::parse(
        "# Workstreams — x\r\n\r\n## Workstreams\r\n-\tWorkstream: a — Gate: ready\r\n- Workstream: b — Gate: pending\r\n",
    );
    assert_eq!(b.status(), None);
    assert_eq!(b.entries.len(), 2, "{:?}", b.entries);
    assert_eq!(b.entries[0].get("Gate"), Some("ready"));
    // A list item that isn't a workstream stays Markdown.
    assert!(board::parse("## Log\n- 2026-10-24 a reported\n")
        .entries
        .is_empty());
}

#[test]
fn the_board_gets_a_static_read_only_page_beside_it() {
    let (dir, md) = board_dir();
    let html = render_file(&md).unwrap();
    assert_eq!(html, dir.path().join(".claude/workstreams/board.html"));
    let page = read(&html);
    assert_has(
        &page,
        &[
            r#"<html lang="en" data-palette="black">"#,
            r#"<body class="intent-file board""#,
            "/workstreams · Workstreams board",
            r#"<h1 class="title">Workstreams — Ship 0.9.7</h1>"#,
            "<li><b>Status:</b> APPROVED session=abc123</li>",
            "<li><b>Goal:</b> Ship the release in three parts</li>",
            "<li><b>Check:</b> cargo test --offline</li>",
            ".claude/workstreams/board.md",
            // The table: the fixed columns, then what else an entry carries.
            r#"<table class="board">"#,
            "<th>Workstream</th><th>Worker</th><th>Branch</th><th>State</th><th>Gate</th><th>Tip</th><th>After</th><th>More</th>",
            // Each section is wrapped, so the page can draw the log as a timeline
            // and the differs as flags; the workstreams one opens with a summary.
            r#"<section class="sec" data-sec="workstreams">"#,
            // The broad picture comes first: progress, next steps, agents.
            r#"<section class="sec" data-sec="progress"><details class="pane" data-pane="progress" open><summary><h2>Progress</h2> <span class="count">1 of 3 integrated · 33%</span></summary>"#,
            r#"<section class="sec" data-sec="next"><details class="pane" data-pane="next" open><summary><h2>Next steps</h2></summary>"#,
            r#"<ol class="next">"#,
            r#"<div class="board-summary">"#,
            r#"<div class="board-strip" role="img" aria-label="1 of 3 integrated">"#,
            r#"<span class="seg state-integrated" data-slug="billing-retry" title="billing-retry — integrated"></span><span class="seg state-checked" data-slug="login-timeout" title="login-timeout — checked"></span><span class="seg state-dispatched" data-slug="audit-log" title="audit-log — dispatched"></span>"#,
            r#"<ul class="board-stats">"#,
            r#"<li class="state-dispatched"><b>1</b> dispatched</li>"#,
            r#"<li class="state-checked"><b>1</b> checked</li>"#,
            r#"<li class="state-integrated"><b>1</b> integrated</li>"#,
            r#"<li class="gate-ready"><b>1</b> ready to merge</li>"#,
            // The agents panel precedes the table (details in its own test).
            r#"<section class="sec" data-sec="agents"><details class="pane" data-pane="agents" open><summary><h2>Agents</h2>"#,
            r#"<button type="button" id="show-all">"#,
            // A row names itself, so a dependency can link to it and a click
            // selects it.
            // …and names itself to the view transition, so it glides when the
            // page changes around it rather than jumping.
            r#"<tr id="ws-billing-retry" data-slug="billing-retry" data-state="integrated" data-gate="merged" style="view-transition-name: row-billing-retry">"#,
            // A change on disk is swapped in, never reloaded: the scroll stays.
            "startViewTransition",
            "softReload",
            // Where each workstream is on the way: five steps, the done ones filled.
            r#"<span class="state state-integrated">integrated</span><ol class="track" aria-hidden="true"><li class="done" style="--i:0"></li><li class="done" style="--i:1"></li><li class="done" style="--i:2"></li><li class="done" style="--i:3"></li><li class="done now" style="--i:4"></li></ol>"#,
            r#"<span class="state state-dispatched">dispatched</span><ol class="track" aria-hidden="true"><li class="done" style="--i:0"></li><li class="done now" style="--i:1"></li><li style="--i:2"></li><li style="--i:3"></li><li style="--i:4"></li></ol>"#,
            r##"<td class="slug"><a class="pick" href="#ws-billing-retry">billing-retry</a>"##,
            "<td>session billing-retry</td>",
            "<td><code>refs/heads/worktree-billing-retry</code></td>",
            r#"<td><span class="state state-integrated">integrated</span><ol class="track""#,
            r#"<td><span class="gate gate-merged">merged<svg class="ico ico-tick""#,
            // A full SHA is shortened, the whole kept on hover.
            r#"<td><code title="89abcdef0123456789abcdef0123456789abcdef">89abcdef0123</code></td>"#,
            "<td>none</td>",
            r#"<b>Merged:</b> <code title="fedcba9876543210fedcba9876543210fedcba98">fedcba987654</code>"#,
            r#"<td class="more"><span class="pair"><b>Worktree:</b> <code>/abs/.claude/worktrees/billing-retry</code></span> · <span class="pair"><b>Merged:</b>"#,
            // The wrapped entry is one row.
            r#"<tr id="ws-login-timeout" data-slug="login-timeout" data-state="checked" data-gate="ready" style="view-transition-name: row-login-timeout">"#,
            "<b>Worktree:</b> <code>/abs/.claude/worktrees/login-timeout</code>",
            r##"<td><a class="after" href="#ws-billing-retry">billing-retry</a></td>"##,
            // Unknown values stay `—`, and text from the board is escaped.
            r#"<tr id="ws-audit-log" data-slug="audit-log" data-state="dispatched" data-gate="pending" style="view-transition-name: row-audit-log">"#,
            "<td>session audit-log (yours) &lt;i&gt;late&lt;/i&gt;</td>",
            "<td>—</td>",
            // Links only to the files that exist.
            r#"<a href="billing-retry/brief.md">brief</a>"#,
            r#"<a href="billing-retry/report.md">report</a>"#,
            r#"<a href="login-timeout/brief.md">brief</a>"#,
            // The other sections, as written.
            // Every section is a pane that folds, its state remembered.
            r#"<section class="sec" data-sec="differs"><details class="pane" data-pane="differs" open><summary><h2>Differs</h2></summary>"#,
            "<li>login-timeout: src/auth/mod.rs — a shared helper it needed</li>",
            r#"<section class="sec" data-sec="log"><details class="pane" data-pane="log" open><summary><h2>Log</h2></summary>"#,
            "<li>2026-10-24T10:00:00Z billing-retry reported: Check pass, Confidence 97%</li>",
            // The Markdown as written, escaped, behind the source view.
            r#"id="view-source""#,
            r#"<pre class="source" id="markdown" hidden><code># Workstreams — Ship 0.9.7"#,
            "&lt;script&gt;alert(1)&lt;/script&gt;",
            "workstreams board, read-only; edit the .md file",
            // What moved since the tab last showed the board is highlighted.
            "ocgen-board:",
            "@keyframes board-flash",
            "prefers-reduced-motion",
        ],
    );
    for never in [
        r#"href="login-timeout/report.md""#,
        r#"href="audit-log/"#,
        // The entries are rows, not list items or paragraphs.
        "<li>Workstream:",
        "<p>Worktree:",
        "<script>alert",
        // A file page: no live view, no list, no language switch, no debt.
        "EventSource",
        r#"class="all""#,
        r#"class="langs""#,
        r#"class="offline""#,
        r#"class="version-note""#,
    ] {
        assert!(!page.contains(never), "{never} in:\n{page}");
    }
    assert_eq!(page.matches("<script").count(), 1);
    assert_eq!(
        read(&dir.path().join(".claude/workstreams/.gitignore")),
        "*\n"
    );
    // Rendering again writes the same bytes, and the Markdown is untouched.
    render_file(&md).unwrap();
    assert_eq!(read(&html), page);
    assert_eq!(read(&md), BOARD);
}

#[test]
fn worktrees_under_the_project_are_shown_relative_to_it() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join(".claude/workstreams");
    fs::create_dir_all(&ws).unwrap();
    let md = ws.join("board.md");
    let worktree = dir.path().join(".claude/worktrees/a");
    fs::write(
        &md,
        format!(
            "Status: DRAFT\n- Workstream: a — Worktree: {} — Gate: pending\n",
            worktree.display()
        ),
    )
    .unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[&format!(
            r#"<b>Worktree:</b> <code title="{}">.claude/worktrees/a</code>"#,
            worktree.display()
        )],
    );
}

/// The agents panel: each worker, what it works on, where it is, and the
/// issues its report, the board and the loop guard raise.
#[test]
fn the_agents_panel_shows_each_worker_its_work_state_and_issues() {
    let (dir, md) = board_dir();
    let ws = dir.path().join(".claude/workstreams");
    // The board: audit-log reported with its worktree gone.
    fs::write(
        &md,
        BOARD.replace(
            "Branch: — — Worktree: — — Tip: — — State: dispatched",
            "Branch: — — Worktree: none — Tip: — — State: reported",
        ),
    )
    .unwrap();
    fs::write(
        ws.join("billing-retry/brief.md"),
        "# Workstream: billing-retry\nCheck: cargo test\n\n## Goal\nRetry failed charges.\nOnce.\n\n## Files\nsrc/billing\n",
    )
    .unwrap();
    fs::write(
        ws.join("billing-retry/report.md"),
        "Workstream: billing-retry\nWorktree: /abs/.claude/worktrees/billing-retry\n\
         Branch: refs/heads/worktree-billing-retry\nTip: 89abcdef0123456789abcdef0123456789abcdef\n\
         Check: pass\nChanged: src/billing/retry.rs\nStatus: done\nConfidence: 97%\n",
    )
    .unwrap();
    fs::write(
        ws.join("login-timeout/report.md"),
        "Workstream: login-timeout\nTip: ffffffffffffffffffffffffffffffffffffffff\n\
         Check: fail: 2 tests in <b>auth</b>\nChanged: src/auth/mod.rs\n\
         Status: partial: tests missing\nConfidence: 80%\n",
    )
    .unwrap();
    // What audit-log's worker posted in its worktree, not yet collected: it
    // is the latest word, and the manager has a copy to make.
    let scratch = dir
        .path()
        .join(".claude/worktrees/audit-log/.claude/scratch");
    fs::create_dir_all(&scratch).unwrap();
    fs::write(
        scratch.join("workstream-report.md"),
        "Workstream: audit-log\nTip: uncommitted\nCheck: not run: no tests yet\nStatus: blocked: waiting on the schema\nConfidence: 70%\n",
    )
    .unwrap();
    let guard = dir.path().join(".claude/loop-guard");
    fs::create_dir_all(&guard).unwrap();
    fs::write(
        guard.join("escalations.md"),
        "- 2026-10-24T11:00:00Z [subagent-confidence] a1b2: confidence stopped rising (3 blocks)\n\
         - 2026-10-24T11:30:00Z [workstreams-gate] refs/heads/other: halted after repeated refused merges\n",
    )
    .unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[
            r#"<section class="sec" data-sec="agents"><details class="pane" data-pane="agents" open><summary><h2>Agents</h2> <span class="count">3 workers · 12 issues</span></summary>"#,
            // Selecting a workstream narrows the panel to its agents.
            r#"<p class="selection" hidden>Showing <b class="which"></b> <button type="button" id="show-all">"#,
            // A session with a clean report: its work, its facts, no issues — a
            // card folded by default, with an icon for its kind and a tick.
            r#"<details class="agent agent-session" id="agent-billing-retry" data-slug="billing-retry" style="view-transition-name: agent-billing-retry"><summary class="agent-head"><svg class="ico ico-session""#,
            r##"<span class="kind">session</span> <b class="name">billing-retry</b> <span class="on">on <a href="#ws-billing-retry">billing-retry</a></span> <span class="state state-integrated">integrated</span> <span class="gate gate-merged">merged<svg class="ico ico-tick""##,
            // The report and the brief fold out in full.
            r#"<summary>Report (collected)</summary><pre>Workstream: billing-retry"#,
            r#"<details class="text" data-pane="brief-billing-retry"><summary>Brief</summary><pre># Workstream: billing-retry"#,
            r#"<p class="goal">Retry failed charges. Once.</p>"#,
            "<dt>Check</dt><dd>pass</dd>",
            "<dt>Status</dt><dd>done</dd>",
            "<dt>Confidence</dt><dd>97%</dd>",
            r#"<dt>Tip</dt><dd><code title="89abcdef0123456789abcdef0123456789abcdef">89abcdef0123</code></dd>"#,
            "<dt>Changed</dt><dd><code>src/billing/retry.rs</code></dd>",
            r#"<p class="ok">No issues</p>"#,
            // A subagent whose report, board line and loop guard raise issues.
            r#"<details class="agent agent-subagent has-issues" id="agent-login-timeout" data-slug="login-timeout" style="view-transition-name: agent-login-timeout" open><summary class="agent-head"><svg class="ico ico-subagent""#,
            r#"<span class="kind">subagent</span> <b class="name">a1b2</b>"#,
            r#"<svg class="ico ico-warn""#,
            r#"<ul class="issues">"#,
            "<li>partial: tests missing</li>",
            "<li>check failed: 2 tests in &lt;b&gt;auth&lt;/b&gt;</li>",
            "<li>confidence 80% is below the bar of 95%</li>",
            "<li>the branch moved since the report: board tip 001122334455, report ffffffffffff</li>",
            "<li>differs: src/auth/mod.rs — a shared helper it needed</li>",
            "<li>loop guard: 2026-10-24T11:00:00Z [subagent-confidence] a1b2: confidence stopped rising (3 blocks)</li>",
            // A session that reported with no report collected and no worktree
            // on the board — but a report posted in a worktree, shown live.
            r#"<details class="agent agent-session has-issues" id="agent-audit-log" data-slug="audit-log" style="view-transition-name: agent-audit-log" open>"#,
            r#"<p class="posted">Posted from its worktree, not yet collected</p>"#,
            "<dt>Status</dt><dd>blocked: waiting on the schema</dd>",
            "<li>blocked: waiting on the schema</li>",
            "<li>check not run: no tests yet</li>",
            "<li>confidence 70% is below the bar of 95%</li>",
            "<li>uncommitted work in the worktree</li>",
            "<li>a newer report in its worktree, not yet collected</li>",
            "<li>the worktree is gone</li>",
            r#"<details class="text" data-pane="posted-audit-log"><summary>Report (posted)</summary><pre>Workstream: audit-log"#,
            // The count of issues joins the summary; escalations tied to no
            // workstream are listed for the manager.
            r#"<li class="issues"><b>12</b> issues</li>"#,
            r#"<ul class="escalations"><li>2026-10-24T11:30:00Z [workstreams-gate] refs/heads/other: halted after repeated refused merges</li></ul>"#,
        ],
    );
    assert!(!page.contains("<b>auth</b>"), "{page}");
    assert!(!page.contains("reported, but no report on file"), "{page}");
    // A collected report is labelled so, and shown beside the posted one.
    assert_has(
        &page,
        &[
            r#"<details class="text" data-pane="report-billing-retry"><summary>Report (collected)</summary>"#,
        ],
    );
    // The agents come first on the page.
    let agents = page.find(r#"data-sec="agents""#).unwrap();
    let table = page.find(r#"data-sec="workstreams""#).unwrap();
    assert!(agents < table);
    // Back to the plain board: a worker still waiting shows so; what is left
    // is login-timeout's differs and a checked workstream with no report on
    // file; nothing is said of escalations.
    fs::write(&md, BOARD).unwrap();
    fs::remove_file(ws.join("login-timeout/report.md")).unwrap();
    fs::remove_file(guard.join("escalations.md")).unwrap();
    fs::remove_file(scratch.join("workstream-report.md")).unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[
            // A worker still at it turns its icon.
            r#"<details class="agent agent-session" id="agent-audit-log" data-slug="audit-log" style="view-transition-name: agent-audit-log"><summary class="agent-head"><svg class="ico ico-session""#,
            r#"<svg class="ico ico-working""#,
            r#"<p class="facts none">No report yet</p>"#,
            r#"<details class="agent agent-subagent has-issues" id="agent-login-timeout" data-slug="login-timeout" style="view-transition-name: agent-login-timeout" open>"#,
            "<li>checked, but no report on file</li>",
            r#"<li class="issues"><b>2</b> issues</li>"#,
        ],
    );
    assert!(!page.contains(r#"class="escalations""#), "{page}");
    // No differs, and login-timeout only dispatched: nothing to count; with
    // one issue the tile counts in the singular.
    let quiet = BOARD
        .replace(
            "## Differs\n- login-timeout: src/auth/mod.rs — a shared helper it needed\n",
            "",
        )
        .replace(
            "State: checked — Gate: ready",
            "State: dispatched — Gate: pending",
        );
    fs::write(&md, &quiet).unwrap();
    let page = read(&render_file(&md).unwrap());
    assert!(!page.contains(r#"class="issues""#), "{page}");
    // (The style sheet names the class; the attribute form is what counts.)
    assert!(!page.contains(r#"has-issues""#), "{page}");
    fs::write(
        &md,
        quiet.replace(
            "Worktree: — — Tip: — — State: dispatched",
            "Worktree: none — Tip: — — State: dispatched",
        ),
    )
    .unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[
            r#"<li class="issues"><b>1</b> issue</li>"#,
            "<li>the worktree is gone</li>",
        ],
    );
}

/// The next steps, read off the board the way the skill's steps go.
#[test]
fn next_steps_follow_the_board() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join(".claude/workstreams");
    fs::create_dir_all(&ws).unwrap();
    let md = ws.join("board.md");
    let steps = |text: &str| -> Vec<String> {
        fs::write(&md, text).unwrap();
        let page = read(&render_file(&md).unwrap());
        let list = page
            .split(r#"<ol class="next">"#)
            .nth(1)
            .and_then(|r| r.split("</ol>").next())
            .unwrap_or_default();
        list.lines()
            .filter(|l| l.starts_with("<li"))
            .map(|l| {
                l.replace(r##"<a class="pick" href="#ws-"##, "")
                    .replace("</a>", "")
                    .replace("</li>", "")
            })
            .collect()
    };
    // A draft: approve it first; nothing planned: plan.
    let s = steps("# Workstreams — x\nStatus: DRAFT\n");
    assert!(s[0].contains("approve the board"), "{s:?}");
    assert!(s[1].contains("plan the workstreams"), "{s:?}");
    // Each state's move, with dependencies honoured.
    let s = steps(
        "Status: APPROVED session=s\n\n## Workstreams\n\
         - Workstream: a — Worker: — — Branch: — — Worktree: — — Tip: — — State: planned — Gate: pending — After: none\n\
         - Workstream: b — Worker: — — Branch: — — Worktree: — — Tip: — — State: planned — Gate: pending — After: a\n\
         - Workstream: c — Worker: session c — Branch: refs/heads/worktree-c — Worktree: — — Tip: — — State: dispatched — Gate: pending — After: none\n\
         - Workstream: d — Worker: subagent d1 — Branch: refs/heads/worktree-d — Worktree: — — Tip: abc1234 — State: reported — Gate: pending — After: none\n\
         - Workstream: e — Worker: session e — Branch: refs/heads/worktree-e — Worktree: — — Tip: abc1234 — State: checked — Gate: ready — After: none\n\
         - Workstream: f — Worker: session f — Branch: refs/heads/worktree-f — Worktree: — — Tip: abc1234 — State: checked — Gate: ready — After: d\n\
         - Workstream: g — Worker: session g — Branch: refs/heads/worktree-g — Worktree: — — Tip: abc1234 — State: integrated — Gate: merged — After: none\n\
         \n## Differs\n- e: src/x.rs — needed\n",
    );
    let want = [
        ("a", "dispatch it (session or subagent worker)"),
        ("b", "hold it until a is integrated"),
        ("c", "wait for its worker (c)"),
        ("d", "check it (step 4)"),
        (
            "e",
            "merge it (step 5): `git merge --no-ff --no-edit refs/heads/worktree-e`",
        ),
        ("f", "wait for d to be integrated before merging it"),
    ];
    for (i, (slug, text)) in want.iter().enumerate() {
        assert!(
            s[i].starts_with(&format!(r#"<li data-slug="{slug}">"#)) && s[i].contains(text),
            "step {i}: {:?}\nall: {s:#?}",
            s.get(i)
        );
    }
    assert!(s[6].contains("decide on the 1 differs"), "{s:?}");
    assert_eq!(s.len(), 7, "{s:#?}");
    // A posted report asks to be collected; a partial one sends the worker back.
    let scratch = dir.path().join(".claude/worktrees/c/.claude/scratch");
    fs::create_dir_all(&scratch).unwrap();
    fs::write(
        scratch.join("workstream-report.md"),
        "Workstream: c\nStatus: done\nConfidence: 97%\n",
    )
    .unwrap();
    fs::create_dir_all(ws.join("d")).unwrap();
    fs::write(
        ws.join("d/report.md"),
        "Workstream: d\nStatus: partial: no tests\nConfidence: 97%\n",
    )
    .unwrap();
    let s = steps(
        "Status: APPROVED session=s\n\n## Workstreams\n\
         - Workstream: c — Worker: session c — Branch: refs/heads/worktree-c — Worktree: — — Tip: — — State: dispatched — Gate: pending — After: none\n\
         - Workstream: d — Worker: subagent d1 — Branch: refs/heads/worktree-d — Worktree: — — Tip: abc1234 — State: reported — Gate: pending — After: none\n",
    );
    assert!(
        s[0].contains(
            "collect the report its worker posted: copy it to .claude/workstreams/c/report.md"
        ),
        "{s:?}"
    );
    assert!(
        s[1].contains("send its worker back once with what is missing (partial: no tests)"),
        "{s:?}"
    );
    // All integrated: check the whole and close; closed: nothing but that.
    let s = steps(
        "Status: APPROVED session=s\n- Workstream: g — Branch: refs/heads/g — Tip: abc1234 — State: integrated — Gate: merged — After: none\n",
    );
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(
        s[0].contains("check the whole (step 6), then close the board"),
        "{s:?}"
    );
    let s =
        steps("Status: CLOSED 2026-10-24\n- Workstream: g — State: integrated — Gate: merged\n");
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("the board is closed"), "{s:?}");
}

#[test]
fn a_board_without_a_workstreams_heading_or_entries_still_renders() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join(".claude/workstreams");
    fs::create_dir_all(&ws).unwrap();
    let md = ws.join("board.md");
    fs::write(&md, "# Workstreams — nothing yet\nStatus: DRAFT\n").unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &["<li><b>Status:</b> DRAFT</li>", "Workstreams board"],
    );
    assert!(!page.contains("<table"), "{page}");
    // Entries anywhere are rows; a closed board is a page like any other.
    fs::write(
        &md,
        "Status: CLOSED 2026-10-24\n- Workstream: a — Branch: refs/heads/a — Tip: — — State: integrated — Gate: merged — After: none\n",
    )
    .unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[
            "<li><b>Status:</b> CLOSED 2026-10-24</li>",
            r#"<tr id="ws-a" data-slug="a" data-state="integrated" data-gate="merged" style="view-transition-name: row-a">"#,
            r##"<td class="slug"><a class="pick" href="#ws-a">a</a>"##,
            r#"<span class="gate gate-merged">merged<svg class="ico ico-tick""#,
            r#"aria-label="1 of 1 integrated""#,
            "<td>none</td>",
        ],
    );
    // No section heading: no wrapper around the table (the agents panel has
    // its own); a state off the known way: no track.
    assert!(!page.contains(r#"data-sec="workstreams""#), "{page}");
    assert!(page.contains(r#"data-sec="agents""#), "{page}");
    fs::write(
        &md,
        "Status: DRAFT\n- Workstream: ../x — State: stuck — Gate: pending — After: b\n",
    )
    .unwrap();
    let page = read(&render_file(&md).unwrap());
    assert_has(
        &page,
        &[
            r#"<tr data-slug="" data-state="stuck" data-gate="pending">"#,
            r#"<span class="state state-stuck">stuck</span></td>"#,
            r#"<li class="state-stuck"><b>1</b> stuck</li>"#,
            r#"aria-label="0 of 1 integrated""#,
            // A dependency that names no row on the board is text.
            "<td>b</td>",
        ],
    );
    assert!(!page.contains(r#"id="ws-"#), "{page}");
    assert!(!page.contains(r#"class="track""#), "{page}");
}

#[test]
fn only_the_board_itself_is_a_document_in_its_folder() {
    let (dir, md) = board_dir();
    let ws = dir.path().join(".claude/workstreams");
    assert!(check_renderable(&md).is_ok());
    let doc = versions::classify(&md).expect("the board is a document");
    assert_eq!(doc.family.kind, versions::Kind::Board);
    assert_eq!(doc.language, "English");
    assert!(!doc.orphan);
    assert_eq!(doc.family.languages(), ["English"]);
    assert!(!doc.family.bilingual());
    // A relative path classifies the same.
    let rel = Path::new(".claude/workstreams/board.md");
    assert_eq!(
        versions::classify(rel).map(|d| d.family.kind),
        Some(versions::Kind::Board)
    );
    for other in [
        "board.ukrainian.md",
        "board.english.md",
        "board-2026-10-23.md",
        "notes.md",
        "billing-retry/brief.md",
        "billing-retry/report.md",
        "board.html",
    ] {
        let p = ws.join(other);
        if !p.exists() {
            fs::write(&p, "# x\n").unwrap();
        }
        assert_eq!(versions::classify(&p), None, "{other}");
        let refused = check_renderable(&p).unwrap_err().to_string();
        assert!(
            refused.contains("not an /inquire ledger") || refused.contains("no HTML view"),
            "{other}: {refused}"
        );
        assert!(render_file(&p).is_err(), "{other}");
    }
    assert!(!ws.join("notes.html").exists());
    assert!(!ws.join("board.ukrainian.html").exists());
    // No version records are ever kept for it.
    assert!(versions::touch(&doc, Some("s1")).is_empty());
    assert!(!ws.join(versions::RECORDS).exists());
    let v = versions::status(&doc.family);
    assert_eq!(v.len(), 1);
    assert!(v[0].exists && !v[0].stale);
}

#[test]
fn the_word_and_the_path_find_the_board() {
    let (dir, md) = board_dir();
    assert!(board::wanted("workstreams"));
    assert!(board::wanted(" Workstreams "));
    assert!(board::wanted(".claude/workstreams/board.md"));
    assert!(board::wanted(r"C:\p\.claude\workstreams\board.md"));
    for not in ["board", "workstream", "flow", ".claude/notes/flow.md", ""] {
        assert!(!board::wanted(not), "{not}");
    }
    assert_eq!(
        board::find(&dir.path().join(".claude/workstreams/billing-retry")),
        Some(md.clone())
    );
    assert_eq!(board::find(dir.path()), Some(md));
    assert_eq!(board::find(tempdir().unwrap().path()), None);
}

#[test]
fn notes_commands_render_and_open_the_board() {
    let (dir, md) = board_dir();
    let html = dir.path().join(".claude/workstreams/board.html");
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "render"])
        .arg(&md)
        .assert()
        .success()
        .stdout(contains("board.html"));
    assert!(html.is_file());
    fs::remove_file(&html).unwrap();
    // The word, from anywhere in the project; the browser switched off.
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "open", "workstreams", "-p"])
        .arg(dir.path().join(".claude/workstreams/billing-retry"))
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .success()
        .stdout(contains("Rendered").and(contains("board.html")));
    assert!(html.is_file());
    fs::remove_file(&html).unwrap();
    // The path.
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "open"])
        .arg(&md)
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .success()
        .stdout(contains("board.html"));
    assert!(html.is_file());
    // Opened again on each request (through the viewer, or as a file).
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
    for _ in 0..2 {
        assert_cmd::Command::cargo_bin("ocgen")
            .unwrap()
            .args(["notes", "open", "workstreams", "-p"])
            .arg(dir.path())
            .env("OCGEN_NOTES_OPEN", "1")
            .env("OCGEN_NOTES_BROWSER", &browser)
            .env("OCGEN_NOTES_BROWSER_WAIT", "1")
            .assert()
            .success()
            .stdout(contains("Opened"));
    }
    let opened = fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(opened.lines().count(), 2, "{opened}");
    assert!(
        opened.lines().all(|l| l.ends_with("board.html")),
        "{opened}"
    );
    // The binary started a viewer for the board's folder: stop it.
    if let Some(info) = ocgen::notes::viewer::read_info(&dir.path().join(".claude/workstreams")) {
        ocgen::notes::viewer::quit(&info);
    }
    // Without a board: a clear refusal.
    let empty = tempdir().unwrap();
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "open", "workstreams", "-p"])
        .arg(empty.path())
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .failure()
        .stderr(contains(".claude/workstreams/board.md"));
}

#[test]
fn the_hook_renders_the_board_after_a_write_and_opens_it_once() {
    let (dir, md) = board_dir();
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
    let env: std::collections::HashMap<String, String> = [
        ("CLAUDE_PROJECT_DIR", ocgen::paths::for_shell(dir.path())),
        ("OCGEN_NOTES_OPEN", "1".to_string()),
        (
            "OCGEN_NOTES_BROWSER",
            format!("sh '{}'", ocgen::paths::for_shell(&fake)),
        ),
        ("OCGEN_NOTES_BROWSER_WAIT", "1".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&md) }
    })
    .to_string();
    for _ in 0..2 {
        let o = ocgen::hooks::run("inquire-notes", &payload, &env);
        assert_eq!(
            (o.code, o.stdout.as_str(), o.stderr.as_str()),
            (0, "", ""),
            "{o:?}"
        );
    }
    let html = dir.path().join(".claude/workstreams/board.html");
    assert!(html.is_file());
    // A relative path (as Claude Code sends one) renders the same page.
    fs::remove_file(&html).unwrap();
    let relative = serde_json::json!({
        "tool_name": "Edit", "tool_input": { "file_path": ".claude/workstreams/board.md" }
    })
    .to_string();
    let o = ocgen::hooks::run("inquire-notes", &relative, &env);
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    assert!(html.is_file());
    // In-process there is no ocgen binary to run a viewer: the page is opened
    // once as a file, and later writes only re-render it.
    let opened = fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(opened.lines().count(), 1, "{opened}");
    assert!(opened.trim().ends_with("board.html"), "{opened}");
    assert!(dir
        .path()
        .join(".claude/workstreams/.board.opened")
        .is_file());
    // The Markdown is never written by ocgen.
    assert_eq!(read(&md), BOARD);
}
