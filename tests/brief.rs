//! The `/brief` brief's library side: the lenient parser, the coverage strip's
//! rules per target, the hand-off command, where briefs live and how they are
//! told from drafts and workstream briefs, what a session remembers, that no
//! static page is ever written for one, and the page words' Ukrainian.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ocgen::agent;
use ocgen::manifest::Manifest;
use ocgen::notes::brief::{self, Counts, Fill, Status, Target};
use ocgen::notes::{check_renderable, draft, render_family, render_file, versions, words};
use ocgen::render::Project;
use tempfile::{tempdir, TempDir};

const BRIEF: &str = "# Brief: billing-retry\nTarget: deliver\nStatus: draft\n\n## Goal\nRetry the billing client.\n";

fn fixture(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/briefs")
        .join(name);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A project-shaped temp directory with a `.claude/briefs` folder.
fn briefs_dir() -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let briefs = dir.path().join(".claude/briefs");
    fs::create_dir_all(&briefs).unwrap();
    (dir, briefs)
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

fn names(v: &[PathBuf]) -> Vec<String> {
    v.iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn fills(md: &str) -> Vec<(&'static str, Fill)> {
    brief::coverage(&brief::parse(md))
        .cells
        .iter()
        .map(|c| (c.key, c.fill))
        .collect()
}

/// The brief as `/brief`'s skill writes it before the first answer: the fenced
/// template of its "The brief" section, rendered with these targets on.
fn skeleton(deliver: bool, inquire: bool, workstreams: bool) -> String {
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.target = ocgen::target::Target::ClaudeCode;
    p.project_name = "br".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.workflow.deliver = deliver;
    p.claude.workflow.inquire = inquire;
    p.claude.workflow.workstreams = workstreams;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = fs::read_to_string(dir.path().join(".claude/skills/brief/SKILL.md")).unwrap();
    let (_, rest) = md
        .split_once("````markdown\n")
        .expect("the skill holds the template in a four-backtick fence");
    let (block, _) = rest.split_once("\n````").expect("the fence closes");
    block.to_string()
}

/// `md` with its `Target:` line naming `target`.
fn targeted(md: &str, target: Target) -> String {
    md.lines()
        .map(|l| {
            if l.starts_with("Target:") {
                format!("Target: {}", target.word())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_brief_parses_leniently() {
    // Carriage returns, a comment block, every header key in one of its forms,
    // a heading with a note after its key, wrapped and placeholder bullets, a
    // fenced block whose lines are neither bullets nor headings.
    let text = [
        "# Brief: billing-retry",
        "<!-- written by /brief; the page follows this file -->",
        "Target: /Deliver",
        "Status: READY",
        "Updated: 2026-10-24 · Commit: `abc1234`",
        "Handoff: `/deliver .claude/briefs/billing-retry.md`",
        "",
        "## Goal",
        "Add retries",
        "to the billing client.",
        "",
        "## Success criteria (deliver)",
        "- retries-on-timeout: a timeout is retried",
        "  up to three times.",
        "- —",
        "- none",
        "- TBD",
        "-",
        "<!--",
        "- commented-out: not a criterion",
        "-->",
        "* no-double-charge: one charge per key.",
        "1. gives-up-cleanly: the last error says how many tries.",
        "",
        "## Context",
        "```",
        "- not a bullet",
        "## not a heading",
        "```",
        "Status: not a header line either",
        "",
        "## Open",
        "- <what the dry run still asks; empty when ready>",
        "",
        "## Inferred",
        "- backoff-shape: exponential, from src/net/retry.rs:9",
        "",
    ]
    .join("\r\n");
    let b = brief::parse(&text);
    assert_eq!(b.title, "billing-retry");
    assert_eq!(
        (b.target, b.target_raw.as_str()),
        (Some(Target::Deliver), "/Deliver")
    );
    assert_eq!((b.status, b.status_raw.as_str()), (Status::Ready, "READY"));
    assert_eq!(
        (b.updated.as_str(), b.commit.as_str()),
        ("2026-10-24", "abc1234")
    );
    assert_eq!(b.handoff, "/deliver .claude/briefs/billing-retry.md");
    assert_eq!(
        b.sections
            .iter()
            .map(|s| s.heading.as_str())
            .collect::<Vec<_>>(),
        [
            "Goal",
            "Success criteria (deliver)",
            "Context",
            "Open",
            "Inferred"
        ]
    );
    assert_eq!(
        b.section("goal").unwrap().text(),
        "Add retries\nto the billing client."
    );
    let criteria = b
        .section("Success criteria")
        .expect("matched by its first words");
    assert_eq!(
        criteria.bullets(),
        [
            "retries-on-timeout: a timeout is retried up to three times.",
            "no-double-charge: one charge per key.",
            "gives-up-cleanly: the last error says how many tries.",
        ]
    );
    let context = b.section("Context").unwrap();
    assert!(context.bullets().is_empty(), "{:?}", context.bullets());
    assert!(context.text().contains("- not a bullet"));
    assert_eq!(
        b.status_raw, "READY",
        "a Status: line inside a section is body"
    );
    assert!(
        b.open().is_empty(),
        "an unfilled placeholder is no open item"
    );
    assert_eq!(
        b.inferred(),
        ["backoff-shape: exponential, from src/net/retry.rs:9"]
    );
    assert_eq!(b.section("Risks"), None);
    assert_eq!(b.section("Scope"), None, "Success criteria isn't Scope");

    // The other spellings and the defaults.
    let b = brief::parse(
        "# Brief: x\nTarget: deliver | inquire | workstreams\nStatus: draft | ready\n\
         Updated: 2026-10-24\nCommit: abc1234\nHand off: /inquire .claude/briefs/x.md\n\n\
         ## Goal\nwhy\n",
    );
    assert_eq!(b.target, None);
    assert_eq!(b.status, Status::Other);
    assert_eq!(
        (b.updated.as_str(), b.commit.as_str()),
        ("2026-10-24", "abc1234")
    );
    assert_eq!(b.handoff, "/inquire .claude/briefs/x.md");
    let b = brief::parse("# Something else\nTarget: WORKSTREAMS\nStatus: Draft\n");
    assert_eq!(b.title, "Something else");
    assert_eq!(
        (b.target, b.status),
        (Some(Target::Workstreams), Status::Draft)
    );
    assert!(b.sections.is_empty());
    assert_eq!(brief::parse(""), brief::Brief::default());
    for (raw, target) in [
        ("inquire", Some(Target::Inquire)),
        (" /Inquire ", Some(Target::Inquire)),
        ("Deliver", Some(Target::Deliver)),
        ("delivery", None),
        ("", None),
    ] {
        assert_eq!(Target::parse(raw), target, "{raw:?}");
    }
    assert_eq!(Target::Workstreams.word(), "workstreams");
}

#[test]
fn coverage_per_target_on_fixture_briefs() {
    for (file, target) in [
        ("deliver.md", Target::Deliver),
        ("inquire.md", Target::Inquire),
        ("workstreams.md", Target::Workstreams),
    ] {
        let b = brief::parse(&fixture(file));
        assert_eq!(b.target, Some(target), "{file}");
        let c = brief::coverage(&b);
        assert_eq!(c.target, Some(target));
        let keys: Vec<&str> = c.cells.iter().map(|c| c.key).collect();
        assert_eq!(keys, target.sections(), "{file}");
        for cell in &c.cells {
            assert_eq!(cell.fill, Fill::Covered, "{file}: {}", cell.key);
        }
        let n = target.sections().len();
        assert_eq!(
            c.counts(),
            Counts {
                covered: n,
                thin: 0,
                missing: 0
            },
            "{file}"
        );
        assert_eq!(c.inferred, 1, "{file}");
        // The inquire brief has one thing left open; the others are ready.
        assert_eq!(c.open, usize::from(target == Target::Inquire), "{file}");
        assert_eq!(c.complete(), target != Target::Inquire, "{file}");
    }
    assert_eq!(Target::Deliver.sections().len(), 7);
    assert_eq!(Target::Inquire.sections().len(), 7);
    assert_eq!(Target::Workstreams.sections().len(), 8);
    assert_eq!(brief::CORE.len(), 5);

    // Thin: a 12-character Goal, criteria without slugs, and no Risks at all.
    let thin = brief::parse(&fixture("thin.md"));
    assert_eq!(thin.section("Goal").unwrap().prose_chars(), 12);
    assert_eq!(brief::PROSE_CHARS, 40);
    let c = brief::coverage(&thin);
    assert_eq!(
        c.cells.iter().map(|c| (c.key, c.fill)).collect::<Vec<_>>(),
        [
            ("Goal", Fill::Thin),
            ("Context", Fill::Covered),
            ("Scope", Fill::Covered),
            ("Success criteria", Fill::Thin),
            ("Constraints", Fill::Covered),
            ("Out of scope", Fill::Covered),
            ("Risks", Fill::Missing),
        ]
    );
    assert_eq!(
        c.counts(),
        Counts {
            covered: 4,
            thin: 2,
            missing: 1
        }
    );
    assert_eq!((c.open, c.inferred), (2, 0));
    assert!(!c.complete());
    assert_eq!(
        (thin.updated.as_str(), thin.commit.as_str()),
        ("2026-10-24", "")
    );

    // No or an unknown target: the core five.
    let none = brief::parse("# Brief: x\nTarget: deliver | inquire | workstreams\n\n## Goal\nx\n");
    assert_eq!(
        brief::coverage(&none)
            .cells
            .iter()
            .map(|c| c.key)
            .collect::<Vec<_>>(),
        brief::CORE
    );
    assert_eq!(
        fills("# Brief: x\n\n## Goal\nx\n"),
        [
            ("Goal", Fill::Thin),
            ("Context", Fill::Missing),
            ("Success criteria", Fill::Missing),
            ("Constraints", Fill::Missing),
            ("Out of scope", Fill::Missing),
        ]
    );
    // Empty is missing; a placeholder alone is thin, as a short answer is: a
    // hint, never a block.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Risks\n\n## Out of scope\nnothing\n\n## Success criteria\n- none\n");
    assert!(f.contains(&("Risks", Fill::Missing)), "{f:?}");
    assert!(f.contains(&("Out of scope", Fill::Thin)), "{f:?}");
    assert!(f.contains(&("Success criteria", Fill::Thin)), "{f:?}");
    // Forty non-whitespace characters cover a prose section, whatever the script.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Goal\nДодати повтори до клієнта оплати, щоб один тайм-аут не ламав оплату.\n");
    assert!(f.contains(&("Goal", Fill::Covered)), "{f:?}");
    // One slugged criterion among plain ones covers; a slug in bold counts.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Success criteria\n- A timeout is retried.\n- **no-double-charge**: one charge per key.\n");
    assert!(f.contains(&("Success criteria", Fill::Covered)), "{f:?}");
    // A Workstreams entry without its Files: field is thin; the board shape covers.
    let f = fills("# Brief: x\nTarget: workstreams\n\n## Workstreams\n- Workstream: a — After: none — Worker: session\n");
    assert!(f.contains(&("Workstreams", Fill::Thin)), "{f:?}");
    let f = fills("# Brief: x\nTarget: workstreams\n\n## Workstreams\n- Workstream: a — Files: src/a.rs — After: none — Worker: session — Check: cargo test — Criteria: a-works\n");
    assert!(f.contains(&("Workstreams", Fill::Covered)), "{f:?}");
    // What I know is a list: one real bullet covers it.
    let f =
        fills("# Brief: x\nTarget: inquire\n\n## What I know\n- assumed: one pool per process.\n");
    assert!(f.contains(&("What I know", Fill::Covered)), "{f:?}");
    assert_eq!(Fill::Thin.word(), "thin");
}

#[test]
fn the_unfilled_template_covers_nothing() {
    // The skill's first write is commonly the template itself, its `<…>`
    // placeholders and all; the strip must call every cell missing, not covered
    // by the placeholders' own words — with /inquire on (every section and the
    // `; inquire:` suffixes) and off.
    for (deliver, inquire, workstreams) in [(true, true, true), (true, false, true)] {
        let skeleton = skeleton(deliver, inquire, workstreams);
        assert!(skeleton.starts_with("# Brief: <slug>\n"), "{skeleton}");
        assert!(
            skeleton.contains("\n## Inferred\n- <slug>: <"),
            "{skeleton}"
        );
        let mut targets: Vec<Option<Target>> = vec![None];
        targets.extend(
            Target::ALL
                .into_iter()
                .filter(|t| match t {
                    Target::Deliver => deliver,
                    Target::Inquire => inquire,
                    Target::Workstreams => workstreams,
                })
                .map(Some),
        );
        for target in targets {
            let md = match target {
                Some(t) => targeted(&skeleton, t),
                None => skeleton.clone(),
            };
            let b = brief::parse(&md);
            assert_eq!(b.target, target, "{md}");
            assert_eq!(b.status, Status::Other);
            let c = brief::coverage(&b);
            let n = target.map_or(brief::CORE.len(), |t| t.sections().len());
            assert_eq!(c.cells.len(), n, "{target:?}");
            for cell in &c.cells {
                assert_eq!(
                    cell.fill,
                    Fill::Missing,
                    "inquire {inquire}, {target:?}: {} in\n{md}",
                    cell.key
                );
            }
            assert_eq!(
                c.counts(),
                Counts {
                    covered: 0,
                    thin: 0,
                    missing: n
                }
            );
            assert_eq!((c.open, c.inferred), (0, 0), "{target:?}");
            assert!(!c.complete());
        }
    }

    // A placeholder beside an answer counts for nothing: twelve characters, thin.
    let b = brief::parse(
        "# Brief: x\nTarget: deliver\n\n## Goal\n<what and why, two to five sentences> Retry billing\n",
    );
    assert_eq!(b.section("Goal").unwrap().prose_chars(), 12);
    assert_eq!(brief::coverage(&b).cells[0].fill, Fill::Thin);
    // A slug whose text is still the placeholder says nothing yet; a real
    // criterion beside the template's own line covers.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Success criteria\n- retries-on-timeout: <what must be true, and how to see it>\n");
    assert!(f.contains(&("Success criteria", Fill::Missing)), "{f:?}");
    let f = fills("# Brief: x\nTarget: deliver\n\n## Success criteria\n- <criterion-slug>: <what must be true, and how to see it>\n- retries-on-timeout: a timeout is retried.\n");
    assert!(f.contains(&("Success criteria", Fill::Covered)), "{f:?}");
    assert_eq!(
        brief::parse("## Risks\n- <risk-slug>: <what could go wrong>\n- budget-overrun: the test with a slow fake provider shows it.\n")
            .section("Risks")
            .unwrap()
            .bullets(),
        ["budget-overrun: the test with a slow fake provider shows it."]
    );
    // A Workstreams entry whose slug or files are still the placeholder is thin.
    for entry in [
        "- Workstream: <slug> — Files: src/a.rs — After: none — Worker: session — Check: cargo test — Criteria: a-works",
        "- Workstream: a — Files: <paths> — After: none — Worker: session — Check: cargo test — Criteria: a-works",
    ] {
        let f = fills(&format!("# Brief: x\nTarget: workstreams\n\n## Workstreams\n{entry}\n"));
        assert!(f.contains(&("Workstreams", Fill::Thin)), "{entry}: {f:?}");
    }
    // The What I know choice unmade is nothing written.
    let f = fills("# Brief: x\nTarget: inquire\n\n## What I know\n- assumed | verified: <inquire only: one bullet of my mental model>\n");
    assert!(f.contains(&("What I know", Fill::Missing)), "{f:?}");
    // A placeholder wrapped over two lines is still one; angle brackets paragraphs
    // apart are prose.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Scope\n<what changes and what doesn't; the refactors, renames or dependency changes\nthe goal needs>\n");
    assert!(f.contains(&("Scope", Fill::Missing)), "{f:?}");
    let f = fills("# Brief: x\nTarget: deliver\n\n## Context\nRetry when a < b.\n\nStop when c > d, after the cap the config sets.\n");
    assert!(f.contains(&("Context", Fill::Covered)), "{f:?}");
    // Generics in a criterion are not placeholders either.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Success criteria\n- returns-option: `charge()` returns `Option<Receipt>`, never panics.\n");
    assert!(f.contains(&("Success criteria", Fill::Covered)), "{f:?}");
}

#[test]
fn a_pipe_in_a_written_value_is_text() {
    // `a | b` is the brief's own idiom for alternatives, so a written line may
    // hold one; only the template's unmade choices (`session | subagent`,
    // `none |` once its `<slug>` is gone, the `assumed | verified` label) say
    // nothing. An Open item with a pipe counts as open: the strip must not say
    // the dry run has nothing left while the file still asks.
    let open = "worker-kind: session | subagent for the pins piece?";
    let b = brief::parse(&format!(
        "# Brief: x\nTarget: deliver\n\n## Open\n- <what the dry run still asks; empty when ready>\n- {open}\n"
    ));
    assert_eq!(b.open(), [open]);
    assert_eq!(brief::coverage(&b).open, 1);
    assert!(!brief::coverage(&b).complete());
    // An Inferred item with a pipe stays in the list the user contests.
    let inferred = "retry-shape: exponential | jitter, from src/net/retry.rs:9";
    let b = brief::parse(&format!("# Brief: x\n\n## Inferred\n- {inferred}\n"));
    assert_eq!(b.inferred(), [inferred]);
    // A criterion, a risk and a Constraints line with a pipe are written.
    let f = fills("# Brief: x\nTarget: deliver\n\n## Success criteria\n- exit-code: `ocgen verify` exits 0 | 1 as the README says.\n");
    assert!(f.contains(&("Success criteria", Fill::Covered)), "{f:?}");
    let f = fills("# Brief: x\nTarget: deliver\n\n## Risks\n- alike-errors: a 5xx | a timeout look the same to the client; the log shows which.\n");
    assert!(f.contains(&("Risks", Fill::Covered)), "{f:?}");
    let f = fills("# Brief: x\nTarget: deliver\n\n## Constraints\nCheck: `cargo test 2>&1 | tail -20` must stay green on every commit.\n");
    assert!(f.contains(&("Constraints", Fill::Covered)), "{f:?}");
    // A Workstreams entry whose Worker is chosen in words is a real entry.
    let f = fills("# Brief: x\nTarget: workstreams\n\n## Workstreams\n- Workstream: pins — Files: src/pins.rs — After: none — Worker: session | subagent, whichever is free — Check: cargo test — Criteria: pins-work\n");
    assert!(f.contains(&("Workstreams", Fill::Covered)), "{f:?}");
    // The template's own choices, unmade, are still nothing written.
    let f = fills("# Brief: x\nTarget: workstreams\n\n## Workstreams\n- Workstream: <slug> — Files: <paths> — After: none | <slug> — Worker: session | subagent — Check: <command> — Criteria: <criterion-slug>, <criterion-slug>\n");
    assert!(f.contains(&("Workstreams", Fill::Missing)), "{f:?}");
    let f = fills("# Brief: x\nTarget: inquire\n\n## What I know\n- assumed | verified: <inquire only: one bullet of my mental model>\n");
    assert!(f.contains(&("What I know", Fill::Missing)), "{f:?}");
}

#[test]
fn handoff_command_is_the_line_or_computed() {
    let b = brief::parse(&fixture("inquire.md"));
    assert_eq!(
        b.handoff_command("request-flow"),
        "/inquire .claude/briefs/request-flow.md"
    );
    let b = brief::parse("# Brief: x\nTarget: inquire\n\n## Goal\nwhy\n");
    assert_eq!(b.handoff_command("x"), "/inquire .claude/briefs/x.md");
    let b = brief::parse("# Brief: x\nTarget: workstreams\n");
    assert_eq!(b.handoff_command("x"), "/workstreams .claude/briefs/x.md");
    // No target: /deliver, the default route.
    let b = brief::parse("# Brief: x\n\n## Goal\nwhy\n");
    assert_eq!(b.handoff_command("x"), "/deliver .claude/briefs/x.md");
    // The line wins over the target, backticks dropped.
    let b =
        brief::parse("# Brief: x\nTarget: deliver\nHand-off: `/deliver .claude/briefs/other.md`\n");
    assert_eq!(b.handoff_command("x"), "/deliver .claude/briefs/other.md");
    assert_eq!(brief::DIR, ".claude/briefs");
}

#[test]
fn brief_names_and_places() {
    for ok in ["billing-retry", "a", "2026-10-24-x", &"a".repeat(80)] {
        assert!(brief::is_name(ok), "{ok}");
    }
    for bad in [
        "Billing",
        "a_b",
        "-a",
        "a b",
        "",
        &"a".repeat(81),
        "a.b",
        "бриф",
    ] {
        assert!(!brief::is_name(bad), "{bad:?}");
    }
    assert_eq!(
        brief::brief_target("/p/.claude/briefs/billing-retry.md"),
        Some("billing-retry".to_string())
    );
    assert_eq!(
        brief::brief_target(r"C:\p\.claude\briefs\x.md"),
        Some("x".to_string())
    );
    assert_eq!(
        brief::brief_target(".claude/briefs/a.md"),
        Some("a".to_string())
    );
    for other in [
        "/p/.claude/intent/drafts/a.md",
        "/p/.claude/workstreams/a/brief.md",
        "/p/.claude/briefs/X Y.md",
        "/p/.claude/briefs/a.ukrainian.md",
        "/p/.claude/briefs/a.txt",
        "/p/.claude/briefs/a.html",
        "/p/briefs/a.md",
        "/p/.claude/briefs/sub/a.md",
        "/p/.claude/notes/a.md",
    ] {
        assert_eq!(brief::brief_target(other), None, "{other}");
    }
    // The drafts' and the ledgers' lookups don't claim a brief.
    assert_eq!(draft::draft_target(".claude/briefs/a.md"), None);
    assert_eq!(
        ocgen::notes::ledger_target(".claude/briefs/a.md"),
        ocgen::notes::Target::NotLedger
    );
    for yes in ["brief", "Brief", " BRIEF "] {
        assert!(brief::is_prompt(yes), "{yes:?}");
    }
    for no in ["briefs", "the brief", "/brief", "", "draft"] {
        assert!(!brief::is_prompt(no), "{no:?}");
    }
    assert_eq!(brief::WORD, "brief");
    assert_eq!(brief::LIST, "_briefs");

    // Kinds, by directory and name.
    let (dir, briefs) = briefs_dir();
    let a = briefs.join("a.md");
    fs::write(&a, BRIEF).unwrap();
    let doc = versions::classify(&a).expect("a brief is a document");
    assert_eq!(doc.family.kind, versions::Kind::Brief);
    assert_eq!(doc.language, "English");
    assert!(!doc.orphan);
    assert_eq!(doc.family.languages(), ["English"]);
    assert!(!doc.family.bilingual());
    assert_eq!(versions::Kind::Brief.dir(), ".claude/briefs");
    for other in ["a.ukrainian.md", "a.english.md", "A.md", "a b.md", "a.html"] {
        let p = briefs.join(other);
        fs::write(&p, BRIEF).unwrap();
        assert_eq!(versions::classify(&p), None, "{other}");
    }
    let ws = dir.path().join(".claude/workstreams/a");
    fs::create_dir_all(&ws).unwrap();
    let ws_brief = ws.join("brief.md");
    fs::write(&ws_brief, BRIEF).unwrap();
    assert_eq!(versions::classify(&ws_brief), None);
    assert!(!versions::Kind::VERSIONED.contains(&versions::Kind::Brief));

    assert!(brief::is_briefs_dir(&briefs));
    assert!(!brief::is_briefs_dir(&a));
    assert!(!brief::is_briefs_dir(&ws));
    assert!(!brief::is_briefs_dir(&dir.path().join(".claude/notes")));
    fs::create_dir_all(dir.path().join(".claude/notes")).unwrap();
    assert!(!brief::is_briefs_dir(&dir.path().join(".claude/notes")));
    assert!(!brief::is_briefs_dir(&dir.path().join("briefs")));
    assert_eq!(
        brief::briefs_dir(&ws),
        Some(ocgen::paths::plain(&briefs.canonicalize().unwrap()))
    );
    assert_eq!(brief::briefs_dir(Path::new("/")), None);
}

#[test]
fn skipped_names_the_files_whose_names_are_no_slug() {
    let (_dir, briefs) = briefs_dir();
    for name in [
        "billing-retry.md",
        "Not A Slug.md",
        "x.ukrainian.md",
        "Billing.md",
        "notes.html",
        "notes.txt",
        ".gitignore",
    ] {
        fs::write(briefs.join(name), BRIEF).unwrap();
    }
    fs::create_dir_all(briefs.join("Folder.md")).unwrap();
    assert_eq!(
        brief::skipped(&briefs),
        ["Billing.md", "Not A Slug.md", "x.ukrainian.md"]
    );
    assert_eq!(names(&brief::briefs(&briefs)), ["billing-retry.md"]);
    assert!(brief::skipped(Path::new("/nonexistent/briefs")).is_empty());
}

#[test]
fn check_renderable_and_render_file_refuse_a_brief() {
    let (_dir, briefs) = briefs_dir();
    let md = briefs.join("billing-retry.md");
    fs::write(&md, BRIEF).unwrap();
    let err = check_renderable(&md).unwrap_err().to_string();
    assert!(
        err.contains("billing-retry.md is a /brief brief — it has no static page; type `brief` in Claude Code to open its editor"),
        "{err}"
    );
    assert_eq!(render_file(&md).unwrap_err().to_string(), err);
    let doc = versions::classify(&md).unwrap();
    assert!(render_family(&doc).is_empty());
    // Nothing was written beside it: no page, no .gitignore, no records.
    let mut left: Vec<String> = fs::read_dir(&briefs)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["billing-retry.md"]);
    // A file in the folder that is no brief gets the ledgers' refusal, as before.
    let other = briefs.join("Not A Slug.md");
    fs::write(&other, BRIEF).unwrap();
    let err = check_renderable(&other).unwrap_err().to_string();
    assert!(err.contains("not an /inquire ledger"), "{err}");
}

#[test]
fn briefs_are_found_newest_first() {
    let (_dir, briefs) = briefs_dir();
    assert!(brief::briefs(&briefs).is_empty(), "none yet");
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (i, name) in ["billing-retry.md", "search-rewrite.md", "request-flow.md"]
        .iter()
        .enumerate()
    {
        let p = briefs.join(name);
        fs::write(&p, BRIEF).unwrap();
        touch(&p, base + Duration::from_secs(60 * i as u64));
    }
    fs::write(briefs.join("Not A Slug.md"), BRIEF).unwrap();
    fs::write(briefs.join(".gitignore"), "*\n").unwrap();
    fs::write(briefs.join("search-rewrite.ukrainian.md"), BRIEF).unwrap();
    assert_eq!(
        names(&brief::briefs(&briefs)),
        ["request-flow.md", "search-rewrite.md", "billing-retry.md"]
    );
    // A change moves a brief to the front.
    touch(&briefs.join("billing-retry.md"), SystemTime::now());
    assert_eq!(names(&brief::briefs(&briefs))[0], "billing-retry.md");
    assert_eq!(
        brief::skipped(&briefs),
        ["Not A Slug.md", "search-rewrite.ukrainian.md"]
    );
}

#[test]
fn a_session_remembers_the_brief_it_wrote_last() {
    let (_dir, briefs) = briefs_dir();
    fs::write(briefs.join("billing-retry.md"), BRIEF).unwrap();
    fs::write(briefs.join("search-rewrite.md"), BRIEF).unwrap();
    assert_eq!(brief::session_brief(&briefs, "s-1"), None);

    assert!(brief::remember(&briefs, "s-1", "billing-retry"));
    assert!(brief::remember(&briefs, "s-1", "search-rewrite"));
    assert!(brief::remember(&briefs, "s-2", "billing-retry"));
    assert_eq!(
        brief::session_brief(&briefs, "s-1").as_deref(),
        Some("search-rewrite")
    );
    assert_eq!(
        brief::session_brief(&briefs, "s-2").as_deref(),
        Some("billing-retry")
    );
    // The records are neither briefs nor in git (the folder ignores itself).
    assert_eq!(brief::briefs(&briefs).len(), 2);
    assert!(brief::skipped(&briefs).is_empty());
    assert_eq!(
        fs::read_to_string(briefs.join(".gitignore")).unwrap(),
        "*\n"
    );

    // A brief that is gone is no session's.
    fs::remove_file(briefs.join("search-rewrite.md")).unwrap();
    assert_eq!(brief::session_brief(&briefs, "s-1"), None);

    // Only a plain session id and a slug make a record.
    for bad in ["", "../x", "a/b", "x y", "s\\1"] {
        assert!(!brief::remember(&briefs, bad, "billing-retry"), "{bad:?}");
    }
    for bad in ["../../etc/passwd", "Billing", "a_b", "a.ukrainian"] {
        assert!(!brief::remember(&briefs, "s-3", bad), "{bad:?}");
    }
    assert_eq!(brief::session_brief(&briefs, "../s-1"), None);

    // A record older than a month is dropped on the next write.
    let old = briefs.join(brief::SESSIONS).join("s-old");
    fs::write(&old, "billing-retry").unwrap();
    touch(
        &old,
        SystemTime::now() - Duration::from_secs(31 * 24 * 3600),
    );
    assert!(brief::remember(&briefs, "s-1", "billing-retry"));
    assert!(!old.exists(), "pruned");
    assert!(briefs.join(brief::SESSIONS).join("s-2").exists(), "kept");
}

#[test]
fn words_in_ukrainian_have_no_russian_letters() {
    let uk = words::for_language("Ukrainian");
    assert_eq!(uk.lang, "uk");
    let all = uk.brief.all();
    assert!(all.len() >= 36, "{}", all.len());
    for s in &all {
        assert!(!s.trim().is_empty());
        for letter in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
            assert!(!s.contains(letter), "{letter:?} in {s:?}");
        }
    }
    // Every section the strip can show has its own name, translated.
    let en = words::for_language("English");
    for key in [
        brief::GOAL,
        brief::CONTEXT,
        brief::SCOPE,
        brief::SUCCESS_CRITERIA,
        brief::CONSTRAINTS,
        brief::OUT_OF_SCOPE,
        brief::RISKS,
        brief::WORKSTREAMS,
        brief::WHAT_I_KNOW,
        brief::FIRST_QUESTION,
        brief::OPEN,
        brief::INFERRED,
    ] {
        assert_eq!(en.brief.section(key), key);
        assert_ne!(uk.brief.section(key), key, "{key} untranslated");
        assert!(uk
            .brief
            .section(key)
            .chars()
            .any(|c| ('а'..='я').contains(&c)));
    }
    assert_eq!(uk.brief.section("Goal"), "Мета");
    assert_eq!(uk.brief.section("Open"), "Відкриті питання");
    assert_eq!(uk.brief.section("Inferred"), "Виведено");
    assert_eq!(uk.brief.section("Workstreams (board)"), "Робочі потоки");
    assert_eq!(uk.brief.section("Something else"), "Something else");
    // The terms the wizard guide shares, and the word rather than a command.
    assert_eq!(uk.brief.coverage, "Охоплення розділів");
    assert!(
        uk.brief.open.contains("пробний запуск"),
        "{}",
        uk.brief.open
    );
    for s in [
        uk.brief.offline,
        uk.brief.list_offline,
        en.brief.offline,
        en.brief.list_offline,
    ] {
        assert!(s.contains("<code>brief</code>"), "{s}");
        assert!(!s.contains("ocgen"), "{s}");
    }
    for (fill, en_word, uk_word) in [
        (Fill::Missing, en.brief.missing, uk.brief.missing),
        (Fill::Thin, en.brief.thin, uk.brief.thin),
        (Fill::Covered, en.brief.covered, uk.brief.covered),
    ] {
        assert_eq!(fill.word(), en_word);
        assert_ne!(uk_word, en_word);
    }
}

#[test]
fn a_written_handoff_wins_over_the_template_line_and_bare_choices_stay_open() {
    use ocgen::notes::brief::{coverage, parse};
    // The template's own Hand-off line still holds a placeholder: computed.
    let unwritten =
        parse("# Brief: pins\nTarget: deliver\nHand-off: /<target> .claude/briefs/<slug>.md\n");
    assert_eq!(
        unwritten.handoff_command("pins"),
        "/deliver .claude/briefs/pins.md"
    );
    let written =
        parse("# Brief: pins\nTarget: deliver\nHand-off: /deliver .claude/briefs/pins.md\n");
    assert_eq!(
        written.handoff_command("other"),
        "/deliver .claude/briefs/pins.md"
    );
    // An Open or Inferred item with a bare choice is a real question.
    let b = parse(
        "# Brief: pins\nTarget: deliver\n\n## Open\n- worker-kind: session | subagent\n- <what the dry run still asks; empty when ready>\n\n## Inferred\n- retry-shape: exponential | jitter\n- none\n",
    );
    assert_eq!(
        b.open(),
        vec!["worker-kind: session | subagent".to_string()]
    );
    assert_eq!(
        b.inferred(),
        vec!["retry-shape: exponential | jitter".to_string()]
    );
    let c = coverage(&b);
    assert_eq!(c.open, 1);
    assert!(!c.complete());
    // A byte-order mark is not the title's.
    let bom = parse("\u{feff}# Brief: pins\nTarget: deliver\n");
    assert_eq!(bom.title, "pins");
}

// ----------------------------------------------------------------- page --

/// Stands in for the inlined Milkdown bundle in the page fixtures: the bundle
/// is pinned by its own test, and half a megabyte twice over would bury the
/// markup the fixtures are for.
const EDITOR_MARK: &str = "/* EDITOR_JS */";

/// A page fixture under `tests/fixtures/`.
fn page_fixture(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The draft editor's page for the fixture draft, in `language`, the bundle
/// replaced by [`EDITOR_MARK`].
fn draft_page(language: &str) -> String {
    let page = draft::page(
        &page_fixture("draft-fixture.md"),
        ".claude/intent/drafts/issue-cache-cold.md",
        "r1",
        "n1",
        language,
        &["alice".to_string()],
    )
    .unwrap();
    assert_eq!(page.matches(draft::EDITOR_JS).count(), 1, "{language}");
    page.replace(draft::EDITOR_JS, EDITOR_MARK)
}

#[test]
fn the_draft_editor_is_unchanged_by_brief_mode() {
    // The brief page is the draft editor's template in another mode; a draft's
    // page must not change by a byte for it. The fixtures were captured before
    // the mode existed.
    for (language, file) in [
        ("English", "draft-page-en.html"),
        ("Ukrainian", "draft-page-uk.html"),
    ] {
        let got = draft_page(language);
        let want = page_fixture(file);
        if got != want {
            let at = got
                .lines()
                .zip(want.lines())
                .position(|(g, w)| g != w)
                .unwrap_or_else(|| got.lines().count().min(want.lines().count()));
            panic!(
                "{file} drifted at line {}:\n  got:  {:?}\n  want: {:?}",
                at + 1,
                got.lines().nth(at).unwrap_or(""),
                want.lines().nth(at).unwrap_or("")
            );
        }
    }
}

/// The page without its one script (the editor bundle and the page's code).
fn markup(page: &str) -> String {
    let start = page.find("<script").expect("a script");
    let end = page[start..].find("</script>").expect("its end") + start;
    format!("{}{}", &page[..start], &page[end..])
}

/// The page's own script, after the editor bundle.
fn script(page: &str) -> &str {
    let start = page.find("<script").expect("a script");
    let end = page[start..].find("</script>").expect("its end") + start;
    let s = &page[start..end];
    match s.find(draft::EDITOR_JS) {
        Some(i) => &s[i + draft::EDITOR_JS.len()..],
        None => s,
    }
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

/// The brief page for fixture `file`, in `language`.
fn brief_page(file: &str, language: &str) -> String {
    let slug = file.trim_end_matches(".md");
    brief::page(
        &fixture(file),
        &format!(".claude/briefs/{slug}.md"),
        "r1",
        "n0nce",
        language,
    )
    .unwrap()
}

#[test]
fn the_page_holds_the_strip_the_open_list_the_badge_and_the_copy_buttons() {
    let page = brief_page("thin.md", "English");
    // The strip is the first thing in the column: the badges, one cell per
    // section /deliver needs in its state, the Open list open, Inferred folded.
    assert!(
        page.contains(
            "<main class=\"draft brief\">\n  <div id=\"brief-strip\"><div class=\"brief-head\""
        ),
        "{}",
        markup(&page)
    );
    for part in [
        r#"data-handoff="/deliver .claude/briefs/thin.md""#,
        r#"data-status="draft""#,
        r#"data-target="deliver""#,
        r#"<span class="badge target">For /deliver</span>"#,
        r#"<span class="badge status">Draft</span>"#,
        r#"<ol class="coverage" aria-label="Section coverage">"#,
        r#"<li class="cell thin" title="Goal: thin" aria-label="Goal: thin">Goal</li>"#,
        r#"<li class="cell covered" title="Context: covered" aria-label="Context: covered">Context</li>"#,
        r#"<li class="cell thin" title="Success criteria: thin" aria-label="Success criteria: thin">Success criteria</li>"#,
        r#"<li class="cell missing" title="Risks: missing" aria-label="Risks: missing">Risks</li>"#,
        r#"<details class="brief-open" open><summary>What the dry run still asks (2)</summary><ul><li>How many retries, and with what cap.</li><li>Whether a 5xx answer counts as a timeout.</li></ul></details>"#,
        r#"<details class="brief-inferred"><summary>Inferred without asking — contest an item by its slug (0)</summary></details>"#,
        // The eyebrow, the list link and the buttons.
        r#"<a class="all" href="_briefs/thin">← All briefs</a> <span>/brief · Brief</span> <code>.claude/briefs/thin.md</code>"#,
        r#"<button type="button" id="copy-md">Copy brief</button>"#,
        r#"<button type="button" id="copy-html">Copy formatted</button>"#,
        r#"<button type="button" id="copy-handoff">Copy hand-off</button>"#,
        ">Save<",
        ">Source<",
        ">GitHub preview<",
        ">Focus<",
        r#"id="palette-black" aria-pressed="true">Black<"#,
        r#"id="palette-white" aria-pressed="false">White<"#,
        // The banners both modes share, the footer and the offline text.
        r#"id="banner" role="alert" hidden"#,
        r#"id="source-only""#,
        "the brief the target skill reads instead of interviewing you again",
        "Type <code>brief</code> in Claude Code to reconnect.",
        r#"<title>thin.md · Brief</title>"#,
        r#"<meta name="generator" content="ocgen — the /brief brief editor; Save writes the .md file">"#,
        // One nonce-tagged script, the editor in it, the strip refreshed from it.
        r#"<script nonce="n0nce">"#,
        "window.OcgenDraft",
        r#"fetch("coverage?slug=" + encodeURIComponent(slug)"#,
        "refresh = function () { baseRefresh(); coverage(text()); };",
        "take = function (t, r) { baseTake(t, r); coverage(saved); };",
        r#"$("copy-handoff").addEventListener"#,
        "function checkApprovers() {}",
        "function checkWraps() {}",
        r#"data-rev="r1""#,
    ] {
        assert!(page.contains(part), "the brief page misses {part}");
    }
    assert_eq!(page.matches("<script").count(), 1);
    assert_eq!(page.matches(draft::EDITOR_JS).count(), 1);
    // Nothing of what only a draft has: no approvers, no wrapped-lines banner
    // and no /intent in the eyebrow.
    let html = markup(&page);
    for gone in [
        r#"id="approvers""#,
        r#"id="wraps""#,
        r#"id="join-lines""#,
        r#"id="missing-list""#,
        "/intent",
        "Issue draft",
        "Copy Markdown",
        "GitHub notifies",
    ] {
        assert!(!html.contains(gone), "{gone} in the brief page:\n{html}");
    }
    for gone in [
        "join-lines",
        "wraps-count",
        "missing-list",
        "data-approvers\") || \"\")",
    ] {
        assert!(!script(&page).contains(gone), "{gone} in the brief script");
    }
    // The Markdown is in the Source view verbatim, and the preview reads a
    // newline as a space, the way GitHub shows a file (a draft's would break).
    assert_eq!(source_text(&page), fixture("thin.md"));
    let preview = &page[page.find(r#"id="pane-preview""#).unwrap()..];
    let preview = &preview[..preview.find("</section>").unwrap()];
    assert!(
        preview.contains("maps any error to\n<code>ChargeError::Provider</code>"),
        "{preview}"
    );
    assert!(!preview.contains("<br"), "{preview}");

    // A ready brief with nothing open: the sentence instead of the list, the
    // ready badge, every cell covered, the inferred item.
    let page = brief_page("deliver.md", "English");
    for part in [
        r#"data-handoff="/deliver .claude/briefs/billing-retry.md" data-status="ready" data-target="deliver""#,
        r#"<span class="badge status ready">Ready</span>"#,
        r#"<details class="brief-open" open><summary>What the dry run still asks (0)</summary><p>The dry run has nothing left to ask.</p></details>"#,
        r#"<details class="brief-inferred"><summary>Inferred without asking — contest an item by its slug (1)</summary><ul><li>backoff-shape: exponential with jitter, from the `retry` helper the fetcher already uses at `src/net/retry.rs:9`.</li></ul></details>"#,
    ] {
        assert!(page.contains(part), "the ready page misses {part}");
    }
    assert_eq!(markup(&page).matches(r#"class="cell covered""#).count(), 7);
    assert!(!markup(&page).contains(r#"class="cell thin""#));

    // The strip on its own: an inquire brief's cells, no target, and text that
    // can't run anything.
    let en = words::for_language("English");
    let strip = brief::strip_html(&fixture("inquire.md"), "request-flow", en);
    assert!(
        strip.contains(r#"aria-label="What I know: covered""#),
        "{strip}"
    );
    assert!(
        strip.contains(r#"aria-label="First question: covered""#),
        "{strip}"
    );
    assert!(strip.contains(r#"data-handoff="/inquire .claude/briefs/request-flow.md""#));
    let strip = brief::strip_html(
        "# Brief: x\n\n## Goal\nwhy\n\n## Open\n- <script>alert(1)</script> & \"quotes\"\n",
        "x",
        en,
    );
    assert!(
        strip.contains(r#"<span class="badge target">No target yet</span>"#),
        "{strip}"
    );
    assert!(
        strip.contains(r#"data-target="" "#) || strip.contains(r#"data-target="">"#),
        "{strip}"
    );
    assert!(
        strip.contains(r#"data-handoff="/deliver .claude/briefs/x.md""#),
        "{strip}"
    );
    assert!(
        strip.contains("<li>&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quotes&quot;</li>"),
        "{strip}"
    );
    assert!(!strip.contains("<script"), "{strip}");
    assert_eq!(
        strip.matches(r#"<li class="cell"#).count(),
        brief::CORE.len()
    );
    // The page's own summary of a brief, for the list.
    assert_eq!(
        brief::summary(&brief::parse(&fixture("thin.md"))),
        "Retry billing"
    );
    assert_eq!(
        brief::summary(&brief::parse("# Brief: x\n\n## Goal\n<what and why>\n")),
        ""
    );
    let long = format!("# Brief: x\n\n## Goal\n{}\n", "a".repeat(130));
    let s = brief::summary(&brief::parse(&long));
    assert_eq!(s.chars().count(), 121);
    assert!(s.ends_with('…'));
}

#[test]
fn every_script_id_exists_in_the_brief_page() {
    // The brief page is the draft editor's template without the draft-only
    // elements: every element its script reaches must still be there, or the
    // script dies before the editor comes up.
    let id = regex::Regex::new(r#"(?:\$|getElementById)\("([^"]+)"\)"#).unwrap();
    let english = brief_page("thin.md", "English");
    let ukrainian = brief_page("workstreams.md", "Ukrainian");
    let draft = draft::page(
        "Text\n",
        ".claude/intent/drafts/x.md",
        "r",
        "n",
        "English",
        &[],
    )
    .unwrap();
    for (what, page) in [
        ("the English brief page", &english),
        ("the Ukrainian brief page", &ukrainian),
        ("the draft page", &draft),
    ] {
        let html = markup(page);
        let ids: Vec<&str> = id
            .captures_iter(script(page))
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        assert!(ids.len() >= 20, "{what}: {ids:?}");
        for i in &ids {
            assert!(
                html.contains(&format!(r#"id="{i}""#)),
                "{what}: the script reaches #{i}, which its markup lacks"
            );
        }
        // The ids built at run time.
        for i in ["palette-black", "palette-white", "words"] {
            assert!(html.contains(&format!(r#"id="{i}""#)), "{what}: #{i}");
        }
    }
    let brief_ids: Vec<&str> = id
        .captures_iter(script(&english))
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    assert!(brief_ids.contains(&"brief-strip") && brief_ids.contains(&"copy-handoff"));
    assert!(!brief_ids.contains(&"join-lines") && !brief_ids.contains(&"wraps-count"));
    let draft_ids: Vec<&str> = id
        .captures_iter(script(&draft))
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    assert!(draft_ids.contains(&"join-lines") && !draft_ids.contains(&"brief-strip"));
}

#[test]
fn the_page_speaks_ukrainian_without_russian_letters() {
    let page = brief_page("thin.md", "Ukrainian");
    for part in [
        r#"<html lang="uk" data-palette="black">"#,
        "Критерії успіху",
        "← Усі брифи",
        r#"<span>/brief · Бриф</span>"#,
        ">Копіювати бриф<",
        ">Копіювати команду передачі<",
        r#"aria-label="Охоплення розділів""#,
        r#"title="Мета: неповний" aria-label="Мета: неповний">Мета</li>"#,
        r#"aria-label="Ризики: відсутній""#,
        r#"aria-label="Контекст: заповнений""#,
        "Про що пробний запуск ще запитує (2)",
        r#"<span class="badge target">Для /deliver</span>"#,
        r#"<span class="badge status">Чернетка</span>"#,
        "введіть <code>brief</code> у Claude Code",
        ">Зберегти<",
    ] {
        assert!(page.contains(part), "the Ukrainian page misses {part}");
    }
    let text = page.replace(draft::EDITOR_JS, "");
    for letter in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
        assert!(!text.contains(letter), "{letter} in the Ukrainian page");
    }

    // The list of briefs, in a project answering in Ukrainian: the title, the
    // count, each brief's Goal, target, status and date, and the files left out.
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.response_language = "Ukrainian".into();
    p.target = ocgen::target::Target::ClaudeCode;
    p.project_name = "br".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let briefs = dir.path().join(brief::DIR);
    fs::create_dir_all(&briefs).unwrap();
    let list = brief::list_page(&briefs, None, None, "./", "n0nce").unwrap();
    assert!(list.contains(r#"<html lang="uk""#), "{list}");
    assert!(list.contains("Брифів ще немає — їх пише /brief."), "{list}");
    assert!(list.contains("Брифи /brief"), "{list}");
    for name in ["thin.md", "deliver.md"] {
        fs::write(briefs.join(name), fixture(name)).unwrap();
    }
    fs::write(briefs.join("Not A Slug.md"), BRIEF).unwrap();
    let list = brief::list_page(&briefs, None, Some("thin"), "./", "n0nce").unwrap();
    for part in [
        "Брифів: 2 · найновіші вгорі",
        r#"data-list="_briefs""#,
        r#"href="./thin.html""#,
        r#"href="./deliver.html""#,
        r#"data-slug="thin""#,
        r#"aria-current="true""#,
        "Retry billing",
        "Add retries with backoff to the billing client",
        r#"<span class="tag">deliver</span>"#,
        r#"<span class="tag">Чернетка</span>"#,
        r#"<span class="tag">Готово</span>"#,
        r#"<span class="tag">Оновлено 2026-10-24</span>"#,
        "назва брифу має бути слагом",
        "<code>Not A Slug.md</code>",
        r#"<code>.claude/briefs/</code>"#,
        r#"<script nonce="n0nce">"#,
        "введіть <code>brief</code> у Claude Code",
    ] {
        assert!(
            list.contains(part),
            "the Ukrainian list misses {part}:\n{list}"
        );
    }
    for letter in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
        assert!(!list.contains(letter), "{letter} in the Ukrainian list");
    }
    assert!(
        !list.contains("Intent files") && !list.contains("Файли намірів"),
        "{list}"
    );
}
