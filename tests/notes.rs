//! The /inquire ledger's HTML view: parsing the Markdown ledger, rendering it to a
//! self-contained page, and deciding which writes are ledgers.

use std::collections::HashMap;
use std::fs;

use ocgen::notes::browser::decide;
use ocgen::notes::ledger::{parse, Evidence};
use ocgen::notes::{html, ledger_target, render_file, Target};

const LEDGER: &str = r#"Topic: Request flow from CLI to renderer
Updated: 2026-10-01   Commit: 3f2a9c1
## Resume point
Last question: How does `ocgen new` reach Project::scaffold?
Hint: (Failure) the override-template error path in src/templates.rs:24 is only Inferred.
## Mental model
- main.rs only dispatches; the logic lives in the **library** crate
- templates are embedded, overridable from `~/.config/ocgen`
## Map
- `src/cli.rs` parses, `src/main.rs` routes
```mermaid
flowchart LR
  cli["src/cli.rs"] --> main["src/main.rs"] --> render["src/render.rs"]
```
## Q&A log
### Q1 · Flow · Verified
Q: How does `ocgen new` reach Project::scaffold?
A: main.rs matches Command::New and calls wizard::run_new.
Cites: src/main.rs:28, src/wizard/mod.rs:120
Hint: (Contract) what scaffold guarantees about files that already exist.
### Q2 · Failure · Inferred 70% · Stale
Q: What happens when an override template is broken?
A: The render fails with context.
Cites: src/templates.rs:24
Note: README says it falls back, the code does not (src/templates.rs:30).
## Open questions
- Does doctor re-render plugin hooks?
## Glossary
- **Project** — the in-memory model rendered to files (src/render.rs).
- **Override**: a template under ~/.config/ocgen/templates.
"#;

#[test]
fn parses_the_structured_ledger() {
    let l = parse(LEDGER);
    assert_eq!(l.topic, "Request flow from CLI to renderer");
    assert_eq!(l.updated, "2026-10-01");
    assert_eq!(l.commit, "3f2a9c1");
    assert_eq!(
        l.resume.question,
        "How does `ocgen new` reach Project::scaffold?"
    );
    assert_eq!(l.resume.hint_lens, "Failure");
    assert!(l.resume.hint.starts_with("the override-template"));
    assert_eq!(l.mental_model.len(), 2);
    assert_eq!(l.map_mermaid.len(), 1);
    assert!(l.map_mermaid[0].contains("flowchart LR"));
    assert!(l.map_text.iter().any(|s| s.contains("src/cli.rs")));

    assert_eq!(l.entries.len(), 2);
    let q1 = &l.entries[0];
    assert_eq!(q1.n, 1);
    assert_eq!(q1.lens, "Flow");
    assert_eq!(q1.evidence, Evidence::Verified);
    assert!(!q1.stale);
    assert_eq!(q1.cites, ["src/main.rs:28", "src/wizard/mod.rs:120"]);
    assert_eq!(q1.hint_lens, "Contract");
    let q2 = &l.entries[1];
    assert_eq!(q2.evidence, Evidence::Inferred(Some(70)));
    assert!(q2.stale);
    assert_eq!(q2.notes.len(), 1);
    assert!(q2.hint.is_empty());

    assert_eq!(l.open_questions, ["Does doctor re-render plugin hooks?"]);
    assert_eq!(l.glossary.len(), 2);
    assert_eq!(l.glossary[0].0, "Project");
    assert_eq!(l.glossary[1].0, "Override");
    let lens = |name: &str| {
        l.lens_coverage()
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
    };
    assert_eq!(lens("Flow"), 1);
    assert_eq!(lens("Failure"), 1);
    assert_eq!(lens("Rationale"), 0);
}

#[test]
fn tolerates_format_variants() {
    let md = "topic: Variants\nupdated: 2026-10-01\ncommit: abc1234\n\
## Q&A log\n\
### Q7 | structure | Inferred (Confidence: 65%)\n\
q: Who owns the map?\n\
a: The renderer.\n\
### Q8 · Telepathy · Verified · Stale — re-verify\n\
- Q: Unknown lens?\n\
- A: Counted as Other.\n\
## Glossary\n\
| Term | Meaning |\n|---|---|\n| Ledger | the notes file |\n";
    let l = parse(md);
    assert_eq!(l.topic, "Variants");
    assert_eq!(l.commit, "abc1234");
    assert_eq!(l.entries.len(), 2);
    assert_eq!(l.entries[0].n, 7);
    assert_eq!(l.entries[0].lens, "Structure");
    assert_eq!(l.entries[0].evidence, Evidence::Inferred(Some(65)));
    assert_eq!(l.entries[0].q, "Who owns the map?");
    assert!(l.entries[0].cites.is_empty());
    assert_eq!(l.entries[1].lens, "Other");
    assert!(l.entries[1].stale);
    assert_eq!(l.entries[1].a, "Counted as Other.");
    assert_eq!(
        l.glossary,
        [("Ledger".to_string(), "the notes file".to_string())]
    );
}

#[test]
fn an_old_ledger_still_renders() {
    let md = "Topic: Old style\nUpdated: 2026-09-01   Commit: 1111111\n\
## Resume point\nLast question: what?\nHint: (Flow) look at main.\n\
## Mental model\n- one\n\
## Q&A log\n- Q: how? — A: like this (src/a.rs:1) Verified\n\
## Notes\nFree text with **bold**.\n";
    let l = parse(md);
    assert!(l.entries.is_empty());
    let page = html::render(&l).unwrap();
    assert!(page.contains("how?"), "free-form Q&A is kept");
    assert!(page.contains("<strong>bold</strong>"));
    assert!(page.contains("Notes"), "unknown sections are kept");
}

#[test]
fn html_escapes_everything_from_the_ledger() {
    let md = "Topic: <script>alert(1)</script>\n\
## Map\n```mermaid\nflowchart LR\n  a[\"<img src=x onerror=alert(2)>\"]\n```\n\
## Q&A log\n### Q1 · Flow · Verified\nQ: <b>bold?</b>\nA: \"quoted\" & <i>x</i>\nCites: <evil>.rs:1\n\
## Glossary\n- **<script>** — <script>alert(3)</script>\n";
    let page = html::render(&parse(md)).unwrap();
    for raw in [
        "<script>alert",
        "<img src=x",
        "<b>bold",
        "<i>x</i>",
        "<evil>",
    ] {
        assert!(!page.contains(raw), "{raw} leaked into the page");
    }
    assert!(page.contains("&lt;script&gt;alert(1)"));
}

#[test]
fn html_has_every_visual_section() {
    let page = html::render(&parse(LEDGER)).unwrap();
    for needle in [
        "<!doctype html>",
        "<title>Request flow from CLI to renderer",
        "class=\"card resume\"",
        "<pre class=\"mermaid\">",
        "mermaid@11",
        "integrity=\"sha384-",
        "ev verified",
        "ev inferred",
        "70%",
        "ev stale",
        "class=\"lens-bar\"",
        "Does doctor re-render plugin hooks?",
        "<table",
        "prefers-color-scheme: dark",
        "name=\"referrer\" content=\"no-referrer\"",
        "<code>src/main.rs:28</code>",
        "3f2a9c1",
    ] {
        assert!(page.contains(needle), "missing {needle}");
    }
    assert!(!page.contains("{{") && !page.contains("{%"));
}

#[test]
fn rendering_is_deterministic() {
    let l = parse(LEDGER);
    assert_eq!(html::render(&l).unwrap(), html::render(&l).unwrap());
}

#[test]
fn ledger_target_matches_only_notes_markdown() {
    let ok = |p: &str| match ledger_target(p) {
        Target::Ledger { slug } => slug,
        other => panic!("{p}: {other:?}"),
    };
    assert_eq!(
        ok("/home/x/proj/.claude/notes/request-flow.md"),
        "request-flow"
    );
    assert_eq!(ok(r"C:\Users\x\proj\.claude\notes\flow.md"), "flow");
    assert_eq!(ok("C:/Users/x/proj/.claude/notes/q2.md"), "q2");
    assert_eq!(ok(".claude/notes/flow.md"), "flow");
    for p in [
        "/p/.claude/notes/flow.html",
        "/p/.claude/notes/.viewer.json",
        "/p/.claude/notes/sub/flow.md",
        "/p/docs/notes/flow.md",
        "/p/.claude/notes.md",
        "",
    ] {
        assert_eq!(ledger_target(p), Target::NotLedger, "{p}");
    }
    assert_eq!(
        ledger_target("/p/.claude/notes/Bad Name.md"),
        Target::BadSlug
    );
}

#[test]
fn render_file_writes_the_sibling_and_a_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join(".claude/notes");
    fs::create_dir_all(&notes).unwrap();
    let md = notes.join("flow.md");
    fs::write(&md, LEDGER).unwrap();
    let html = render_file(&md).unwrap();
    assert_eq!(html, notes.join("flow.html"));
    let page = fs::read_to_string(&html).unwrap();
    assert!(page.contains("Request flow from CLI to renderer"));
    assert_eq!(fs::read_to_string(notes.join(".gitignore")).unwrap(), "*\n");
    // Re-rendering an unchanged ledger leaves the file alone.
    let before = fs::metadata(&html).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    render_file(&md).unwrap();
    assert_eq!(fs::metadata(&html).unwrap().modified().unwrap(), before);
    assert!(fs::read_dir(&notes).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".tmp")));
}

#[test]
fn open_policy() {
    let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    // Desktops open by default.
    assert!(decide(&env(&[]), "macos", false));
    assert!(decide(&env(&[]), "windows", false));
    assert!(decide(&env(&[("DISPLAY", ":0")]), "linux", false));
    assert!(decide(
        &env(&[("WAYLAND_DISPLAY", "wayland-0")]),
        "linux",
        false
    ));
    // Headless and CI don't, unless asked explicitly.
    assert!(!decide(&env(&[]), "linux", false));
    assert!(!decide(&env(&[("CI", "true")]), "macos", false));
    assert!(decide(&env(&[("CI", "true")]), "macos", true));
    // The opt-out always wins; the opt-in overrides the heuristics.
    assert!(!decide(&env(&[("OCGEN_NOTES_OPEN", "0")]), "macos", true));
    assert!(decide(
        &env(&[("OCGEN_NOTES_OPEN", "1"), ("CI", "1")]),
        "linux",
        false
    ));
}

// ------------------------------------------------------- the visual report --

const REPORT: &str = r#"Topic: From CI dbt-build to Redshift
Summary: A phased deep dive. Phase 1: the CI job, which never touches Redshift.
Updated: 2026-09-30   Commit: 6ebfa72c
Status: Phase 2 in progress
## Resume point
Last question: What does CI run?
Hint: (Failure) nothing shows what a Spectrum scan sees mid-write.
## Mental model
- CI never touches Redshift
## Phase 1 · CI dbt-build
Where it stands and how it works, end to end. Source: `.github/workflows/ci.yml:23-128`.

```stats
10 | Steps, strictly serial, no `continue-on-error`
Whole DAG | Built each run despite the "modified models" name
```

### Step flow [half]
Each row is a step, top to bottom.

```steps
legend: #blue Setup · #orange dbt build · #green Downstream gate
0 | Postgres 16 service | Backs only tests/test_app_store.py | :32-44
5 #orange | dbt seed (use_mock_data) | 109 CSVs incl. 64 mocks | :68-73
7 #green | Gold row-count regression | Counts vs the baseline — hard fail | :84-94
```

### What the job exercises [half]
```bars
pytest test files | 240 | tests/**/*.py
dbt models | 119
```
Source: counts from the working tree at 6ebfa72c.

> **Latest hint (Failure):** only the vault keeps TIP-owned history.

## Phase 2 · Triggers into Redshift
### Exceptions to the simple picture
```claims
Inferred 70% | ADaM reads a study-named schema | Source `incb161734_101` (`_sources.yml:4-11`)
Verified | ClinView starts from Fabric SQL | Its Bronze reader queries a Fabric endpoint
Corrected | Data rows reach Redshift only through dbt | Earlier I said Dagster had no connection
Source #green | Source landing — the vendor's system of record | ICON pushes Archer tables
```

### Is Bronze "validation"?
```cards
✓ Bronze: right, with three more jobs | It checks that the expected **columns** arrived.
✓ Silver: right, and more | It's normalization in the sense of standardizing values.
```

### One Archer row through three tiers
```compare
Bronze #blue | archer_core_myeloid_raw
subject_id | "AU001001"
---
Silver #orange | genomic_snv
**usubjid** | INCA033989-101-AU001-001
---
Gold #green | fct_genomic_snv · marts
variant_key | gene + HGVS identity
```

### Where each layer physically lives
```stack
S3 + Glue catalog #blue | Written by Dagster · PyIceberg `type: glue`
1 · Vendor file | e.g. a Caris SNV file | files
3 · Silver | Glue DB `tip_dev_silver` | Iceberg
---
Redshift #orange | Built by dbt · connects as `dagster_svc`
6 · Intermediate ~ | Ephemeral: inlined as CTEs | CTE
7 · Gold marts ! | `CREATE TABLE AS` | table
```

| Tier | Owner | History? |
|---|---|---|
| Raw | vendor | in the source buckets |
| Vault | TIP | yes, versioned + KMS |

```mermaid
flowchart LR
  src["Source landing"]:::green --> bronze["Bronze"]:::blue
```
## Q&A log
### Q1 · Flow · Verified
Q: What does CI run?
A: Ten serial steps on DuckDB.
## Open questions
- What does a Spectrum scan see mid-write?
"#;

#[test]
fn phases_become_tabs_after_the_overview() {
    let l = parse(REPORT);
    assert_eq!(
        l.summary,
        "A phased deep dive. Phase 1: the CI job, which never touches Redshift."
    );
    assert_eq!(l.status, "Phase 2 in progress");
    let names: Vec<&str> = l.phases.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        names,
        ["Phase 1 · CI dbt-build", "Phase 2 · Triggers into Redshift"]
    );
    let page = html::render_page(&l, Some(".claude/notes/ci-dbt-build.md")).unwrap();
    // Tabs: overview, each phase, the log — the latest phase opens by default.
    for needle in [
        r#"data-tab="overview""#,
        r#"data-tab="phase-1""#,
        r#"data-tab="phase-2""#,
        r#"data-tab="log""#,
        r#"data-default="phase-2""#,
        ">Phase 1 · CI dbt-build</button>",
        "Ledger: .claude/notes/ci-dbt-build.md",
        "Phase 2 in progress",
        "6ebfa72c",
        "A phased deep dive.",
    ] {
        assert!(page.contains(needle), "missing {needle}");
    }
}

#[test]
fn every_visual_block_renders() {
    let page = html::render_page(&parse(REPORT), None).unwrap();
    for needle in [
        // stats
        r#"<div class="stats">"#,
        r#"<div class="stat-value">Whole DAG</div>"#,
        "<code>continue-on-error</code>",
        // cards, half width, lead and footnote
        r#"<section class="card half">"#,
        "<h3>Step flow</h3>",
        r#"<p class="lead">Each row is a step, top to bottom.</p>"#,
        r#"<p class="footnote">Source: counts from the working tree at 6ebfa72c.</p>"#,
        // steps with colours, refs and a legend
        r#"<ol class="steps">"#,
        r#"<span class="step-n c-orange">5</span>"#,
        r#"<span class="step-ref">:84-94</span>"#,
        r#"<span class="swatch c-green"></span>Downstream gate"#,
        // bars with hover detail
        r#"<div class="bars">"#,
        r#"title="tests/**/*.py""#,
        r#"style="width: 100%""#,
        // callout
        r#"<div class="callout"><p><strong>Latest hint (Failure):</strong>"#,
        // claims
        r#"<span class="badge ev-inferred">Inferred 70%</span>"#,
        r#"<span class="badge ev-verified">Verified</span>"#,
        r#"<span class="badge ev-corrected">Corrected</span>"#,
        r#"<span class="badge c-green">Source</span>"#,
        // verdict cards
        r#"<div class="mini-cards">"#,
        // compare columns
        r#"<div class="compare">"#,
        r#"<div class="col c-orange">"#,
        r#"<dt>usubjid</dt><dd class="changed">INCA033989-101-AU001-001</dd>"#,
        // stack
        r#"<div class="stack">"#,
        r#"<div class="layer dashed">"#,
        r#"<div class="layer highlight">"#,
        r#"<span class="tag">table</span>"#,
        // tables and diagrams
        "<th>History?</th>",
        r#"<pre class="mermaid">"#,
    ] {
        assert!(page.contains(needle), "missing {needle}");
    }
    assert!(!page.contains("```"), "no raw fences left");
}

#[test]
fn report_blocks_escape_their_content() {
    let md = "Topic: x\n## Phase 1\n```stats\n<b>1</b> | <script>x</script>\n```\n\
```claims\n<i>Verified</i> | <img src=x> | ok\n```\n```bars\n<u>a</u> | 3 | \"><script>\n```\n\
```compare\n<s>T</s> #blue | sub\n<k> | <v>\n```\n```stack\n<p>S #red | sub\n<a> ! | <b> | <c>\n```\n";
    let page = html::render_page(&parse(md), None).unwrap();
    for raw in [
        "<b>1",
        "<script>x",
        "<i>Verified",
        "<img src=x",
        "<u>a",
        "\"><script>",
        "<s>T",
        "<k>",
        "<v>",
        "<p>S",
        "<a>",
        "<c>",
    ] {
        assert!(!page.contains(raw), "{raw} leaked");
    }
}
