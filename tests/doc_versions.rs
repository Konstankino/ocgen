//! Every local document — an /inquire ledger, a /recap report, an /intent
//! reading copy — is kept in both project languages: the unsuffixed file in the
//! answer language, the instruction language's as `<name>.<language>.md` (a
//! reading copy's English version is the intent file itself). Each version gets
//! its own page with a switch to the others; the notes hook tells Claude which
//! version a write left behind, and holds the end of a turn once for it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::notes::{render_family, render_file, versions};
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;
use tempfile::{tempdir, TempDir};

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

/// A scaffolded Claude project with these languages and workflows.
fn project(prompts: &str, answers: &str, inquire: bool, intent: bool, recap: bool) -> TempDir {
    let dir = tempdir().unwrap();
    let mut p = Project::from_manifest(&manifest(), prompts);
    p.target = Target::ClaudeCode;
    p.project_name = "versions".into();
    p.providers.clear();
    p.language = prompts.into();
    p.response_language = answers.into();
    p.agents = vec![Agent::from_archetype_claude("explorer", "explorer", prompts).unwrap()];
    p.claude.workflow.inquire = inquire;
    p.claude.workflow.intent = intent;
    p.claude.workflow.recap = recap;
    p.scaffold(dir.path(), false).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

const UK_LEDGER: &str =
    "Topic: Потік запиту\n## Q&A log\n### Q1 · Flow · Verified\nQ: Як?\nA: Так.\n";
const EN_LEDGER: &str =
    "Topic: Request flow\n## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\n";

fn assert_has(page: &str, all: &[&str]) {
    for must in all {
        assert!(page.contains(must), "missing {must:?} in:\n{page}");
    }
}

#[test]
fn pages_link_their_counterpart_and_flag_a_pending_one() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    let uk = write(root, ".claude/notes/flow.md", UK_LEDGER);
    render_file(&uk).unwrap();
    let page = read(root, ".claude/notes/flow.html");
    assert_has(
        &page,
        &[
            r#"<html lang="uk">"#,
            r#"<span class="lang current" aria-current="page" lang="uk">Українська</span>"#,
            // Not written yet: named, not linked.
            r#"<span class="lang" lang="en">English</span><span class="state">· ще не написано</span>"#,
        ],
    );

    let en = write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    let doc = versions::classify(&en).unwrap();
    assert_eq!(doc.language, "English");
    assert!(!doc.orphan);
    let pages = render_family(&doc);
    assert_eq!(pages.len(), 2, "{pages:?}");
    let english = read(root, ".claude/notes/flow.english.html");
    assert_has(
        &english,
        &[
            r#"<html lang="en">"#,
            "Request flow",
            r#"<a class="lang" href="flow.html" hreflang="uk" lang="uk">Українська</a>"#,
            ".claude/notes/flow.english.md",
        ],
    );
    assert!(read(root, ".claude/notes/flow.html").contains(
        r#"<a class="lang" href="flow.english.html" hreflang="en" lang="en">English</a>"#
    ));

    // In step once both are seen; then the English one changes: the Ukrainian
    // one falls behind, and both pages say so.
    versions::touch(&versions::classify(&uk).unwrap(), Some("s1"));
    versions::touch(&doc, Some("s1"));
    write(
        root,
        ".claude/notes/flow.english.md",
        &format!("{EN_LEDGER}### Q2 · Flow · Verified\nQ: And?\nA: Yes.\n"),
    );
    assert_eq!(versions::touch(&doc, Some("s1")), ["Ukrainian"]);
    render_family(&doc);
    assert_has(
        &read(root, ".claude/notes/flow.html"),
        &[r#"<p class="version-note" role="note">Ця версія може бути застарілою"#],
    );
    assert_has(
        &read(root, ".claude/notes/flow.english.html"),
        &[r#"Українська</a><span class="state">· out of date</span>"#],
    );

    // Lists show each topic once.
    assert_eq!(ocgen::notes::ledgers(&root.join(".claude/notes")), [uk]);
}

#[test]
fn ukrainian_prompts_with_english_answers_swap_the_roles() {
    let dir = project("Ukrainian", "English", true, true, false);
    let root = dir.path();
    let en = write(root, ".claude/notes/flow.md", EN_LEDGER);
    let uk = write(root, ".claude/notes/flow.ukrainian.md", UK_LEDGER);
    assert_eq!(versions::classify(&en).unwrap().language, "English");
    assert_eq!(versions::classify(&uk).unwrap().language, "Ukrainian");
    render_file(&uk).unwrap();
    assert_has(
        &read(root, ".claude/notes/flow.ukrainian.html"),
        &[r#"<html lang="uk">"#, r#"href="flow.html" hreflang="en""#],
    );
    // The reading copy is Ukrainian, unsuffixed; the intent file is its English.
    write(
        root,
        "docs/adr/ADR-0001-cache.md",
        "# ADR-0001: Cache\n\nStatus: Proposed\n",
    );
    let copy = write(
        root,
        ".claude/intent/view/adr-0001-cache.md",
        "# ADR-0001: Кеш\n",
    );
    let doc = versions::classify(&copy).unwrap();
    assert_eq!(doc.language, "Ukrainian");
    assert_eq!(doc.family.languages(), ["English", "Ukrainian"]);
    assert_eq!(
        doc.family.path("English").unwrap(),
        root.join("docs/adr/ADR-0001-cache.md")
    );
    // A language ocgen offers that the project doesn't keep (left by a change of
    // languages): an orphan, rendered but never owed. One it doesn't know isn't
    // a version at all.
    let dir = project("English", "English", true, false, false);
    let old = write(dir.path(), ".claude/notes/flow.ukrainian.md", UK_LEDGER);
    assert!(versions::classify(&old).unwrap().orphan);
    let other = write(root, ".claude/notes/flow.polish.md", EN_LEDGER);
    assert_eq!(versions::classify(&other), None);
}

#[test]
fn recap_reports_get_a_static_zen_page_in_every_project() {
    let dir = project("English", "English", false, false, true);
    let root = dir.path();
    let md = write(
        root,
        ".claude/notes/recap/2026-10-06.md",
        "# Recap — 2026-10-06\n\n## main\n- merged <b>#12</b>\n",
    );
    let html = render_file(&md).unwrap();
    assert_eq!(html, root.join(".claude/notes/recap/2026-10-06.html"));
    let page = fs::read_to_string(&html).unwrap();
    assert_has(
        &page,
        &[
            r#"<html lang="en" data-palette="black">"#,
            "/recap · Daily recap",
            ".claude/notes/recap/2026-10-06.md",
            r#"id="view-source""#,
            // The Markdown as written, escaped.
            r#"<pre class="source" id="markdown" hidden><code># Recap — 2026-10-06"#,
            "merged &lt;b&gt;#12&lt;/b&gt;",
        ],
    );
    // A file page: no live view, no list to go back to, no switch.
    for never in [
        "EventSource",
        r#"class="all""#,
        r#"class="langs""#,
        r#"class="offline""#,
    ] {
        assert!(!page.contains(never), "{never} in:\n{page}");
    }
    assert_eq!(page.matches("<script").count(), 1);
    assert_eq!(read(root, ".claude/notes/recap/.gitignore"), "*\n");
    // Rendering again writes the same bytes.
    render_file(&md).unwrap();
    assert_eq!(fs::read_to_string(&html).unwrap(), page);

    // Two languages: each report links the other.
    let dir = project("English", "Ukrainian", false, false, true);
    let root = dir.path();
    write(
        root,
        ".claude/notes/recap/2026-10-06.english.md",
        "# Recap\n",
    );
    let uk = write(root, ".claude/notes/recap/2026-10-06.md", "# Підсумок\n");
    render_family(&versions::classify(&uk).unwrap());
    assert_has(
        &read(root, ".claude/notes/recap/2026-10-06.html"),
        &[
            "/recap · Щоденний підсумок",
            r#"<a class="lang" href="2026-10-06.english.html" hreflang="en" lang="en">English</a>"#,
        ],
    );
    assert_has(
        &read(root, ".claude/notes/recap/2026-10-06.english.html"),
        &[
            r#"<html lang="en" data-palette="black">"#,
            r#"href="2026-10-06.html""#,
        ],
    );
}

#[test]
fn a_reading_copy_page_links_its_other_copies_and_names_its_intent_file() {
    let dir = project("English", "Ukrainian", false, true, false);
    let root = dir.path();
    write(root, "docs/adr/ADR-0001-cache.md", "# ADR-0001: Cache\n");
    let copy = write(
        root,
        ".claude/intent/view/adr-0001-cache.md",
        "# ADR-0001: Кеш\n\n## Контекст\nХолодний.\n",
    );
    let html = render_file(&copy).unwrap();
    let page = fs::read_to_string(html).unwrap();
    assert_has(
        &page,
        &[
            "/intent · Переклад для читання",
            "<b>Оригінал англійською:</b> <code>docs/adr/ADR-0001-cache.md</code>",
            // The intent file has no page beside the copy: named, not linked.
            r#"<span class="lang" lang="en">English</span>"#,
            r#"<span class="lang current" aria-current="page" lang="uk">Українська</span>"#,
        ],
    );
}

#[test]
fn notes_render_takes_a_version_and_a_recap() {
    let dir = project("English", "Ukrainian", true, false, true);
    let root = dir.path();
    let en = write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    let recap = write(root, ".claude/notes/recap/2026-10-06.md", "# Підсумок\n");
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "render"])
        .arg(&en)
        .arg(&recap)
        .assert()
        .success();
    assert!(root.join(".claude/notes/flow.english.html").is_file());
    assert!(root.join(".claude/notes/recap/2026-10-06.html").is_file());
    // A suffix that can't name a language isn't a version: refused before
    // anything is written.
    let bad = write(root, ".claude/notes/flow.en9.md", EN_LEDGER);
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "render"])
        .arg(&bad)
        .assert()
        .failure()
        .stderr(predicates::str::contains("lowercase-hyphen slug"));
    assert!(!root.join(".claude/notes/flow.en9.html").exists());
}

// ---------------------------------------------------------------- the hook --

/// The notes hook in-process, for the project at `root`, with no browser.
fn hook(root: &Path, payload: serde_json::Value) -> ocgen::hooks::Outcome {
    let env: HashMap<String, String> = [
        ("OCGEN_NOTES_OPEN", "0".to_string()),
        ("CLAUDE_PROJECT_DIR", ocgen::paths::for_shell(root)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    ocgen::hooks::run("inquire-notes", &payload.to_string(), &env)
}

/// A write of `rel` by session `session` (and subagent `agent`, if any).
fn wrote(root: &Path, session: &str, agent: &str, rel: &str) -> ocgen::hooks::Outcome {
    let mut p = serde_json::json!({
        "hook_event_name": "PostToolUse", "session_id": session, "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&root.join(rel)) }
    });
    if !agent.is_empty() {
        p["agent_id"] = agent.into();
    }
    hook(root, p)
}

fn stop(
    root: &Path,
    event: &str,
    session: &str,
    agent: &str,
    active: bool,
) -> ocgen::hooks::Outcome {
    hook(
        root,
        serde_json::json!({
            "hook_event_name": event, "session_id": session, "agent_id": agent,
            "stop_hook_active": active
        }),
    )
}

/// What the hook told Claude after a write, if anything.
fn context(o: &ocgen::hooks::Outcome) -> Option<String> {
    assert_eq!(o.code, 0, "{o:?}");
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).ok()?;
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .map(str::to_string)
}

/// The reason a Stop hook held the turn, if it did.
fn held(o: &ocgen::hooks::Outcome) -> Option<String> {
    assert_eq!(o.code, 0, "{o:?}");
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).ok()?;
    assert_eq!(v["decision"], "block");
    v["reason"].as_str().map(str::to_string)
}

#[test]
fn monolingual_projects_ask_for_nothing() {
    let dir = project("English", "English", true, false, true);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", EN_LEDGER);
    let o = wrote(root, "s1", "", ".claude/notes/flow.md");
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    assert!(root.join(".claude/notes/flow.html").is_file());
    assert!(!root.join(".claude/notes/.versions").exists());
    write(root, ".claude/notes/recap/2026-10-06.md", "# Recap\n");
    assert_eq!(
        context(&wrote(root, "s1", "", ".claude/notes/recap/2026-10-06.md")),
        None
    );
    assert!(root.join(".claude/notes/recap/2026-10-06.html").is_file());
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
}

#[test]
fn writing_one_version_asks_for_the_other_once() {
    let dir = project("English", "Ukrainian", true, false, true);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    let note = context(&wrote(root, "s1", "", ".claude/notes/flow.md")).expect("a reminder");
    assert_has(
        &note,
        &[
            ".claude/notes/flow.md (Ukrainian) changed",
            "- .claude/notes/flow.english.md (English): it isn't written yet",
            "lens names, evidence labels and block names stay in English",
            "never the .html",
        ],
    );
    // Another edit of the same version: already owed, nothing new to say.
    write(root, ".claude/notes/flow.md", &format!("{UK_LEDGER}\n"));
    assert_eq!(
        context(&wrote(root, "s1", "", ".claude/notes/flow.md")),
        None
    );
    // Writing the owed version asks for nothing back.
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    assert_eq!(
        context(&wrote(root, "s1", "", ".claude/notes/flow.english.md")),
        None
    );
    assert!(root.join(".claude/notes/flow.english.html").is_file());
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);

    // A recap report, the same way.
    write(
        root,
        ".claude/notes/recap/2026-10-06.english.md",
        "# Recap\n",
    );
    let note = context(&wrote(
        root,
        "s1",
        "",
        ".claude/notes/recap/2026-10-06.english.md",
    ))
    .unwrap();
    assert!(
        note.contains("- .claude/notes/recap/2026-10-06.md (Ukrainian): it isn't written yet"),
        "{note}"
    );
}

#[test]
fn an_edit_after_sync_marks_the_counterpart_out_of_date() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.md");
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.english.md");
    write(
        root,
        ".claude/notes/flow.english.md",
        &format!("{EN_LEDGER}\nOpen: more\n"),
    );
    let note = context(&wrote(root, "s2", "", ".claude/notes/flow.english.md")).unwrap();
    assert!(
        note.contains("- .claude/notes/flow.md (Ukrainian): update it to match"),
        "{note}"
    );
    assert!(
        note.contains("If this change concerned one language only"),
        "{note}"
    );
    // Both pages say which one is behind.
    assert!(read(root, ".claude/notes/flow.html").contains(r#"class="version-note""#));
    assert!(read(root, ".claude/notes/flow.english.html").contains("· out of date"));
}

#[test]
fn stop_blocks_once_for_this_sessions_pending_versions() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.md");
    // Another session owes nothing.
    assert_eq!(held(&stop(root, "Stop", "s2", "", false)), None);
    let reason = held(&stop(root, "Stop", "s1", "", false)).expect("held once");
    assert_has(
        &reason,
        &[
            "- .claude/notes/flow.english.md (English): not written yet",
            "wait for it",
            "say so in one line and finish",
        ],
    );
    // Stopping again anyway: let go, and the version stays behind on its page.
    assert_eq!(held(&stop(root, "Stop", "s1", "", true)), None);
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
    assert!(read(root, ".claude/notes/flow.html").contains("· ще не написано"));
}

#[test]
fn a_version_written_through_bash_counts_at_stop() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.md");
    // No hook saw this write.
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
}

#[test]
fn a_subagent_answers_for_what_it_left_pending() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "helper", ".claude/notes/flow.md");
    // The main agent's own end isn't held for the helper's write…
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
    // …the helper's is, once; then what it owes passes to its session.
    assert!(held(&stop(root, "SubagentStop", "s1", "helper", false)).is_some());
    assert_eq!(
        held(&stop(root, "SubagentStop", "s1", "helper", true)),
        None
    );
    assert!(held(&stop(root, "Stop", "s1", "", false)).is_some());
}

#[test]
fn the_adr_marks_its_reading_copies_pending() {
    let dir = project("English", "Ukrainian", false, true, false);
    let root = dir.path();
    write(
        root,
        "docs/adr/ADR-0001-cache.md",
        "# ADR-0001: Cache\n\nStatus: Proposed\n",
    );
    let note = context(&wrote(root, "s1", "", "docs/adr/ADR-0001-cache.md")).expect("a reminder");
    assert!(
        note.contains("- .claude/intent/view/adr-0001-cache.md (Ukrainian): it isn't written yet"),
        "{note}"
    );
    write(
        root,
        ".claude/intent/view/adr-0001-cache.md",
        "# ADR-0001: Кеш\n",
    );
    assert_eq!(
        context(&wrote(
            root,
            "s1",
            "",
            ".claude/intent/view/adr-0001-cache.md"
        )),
        None
    );
    assert!(root
        .join(".claude/intent/view/adr-0001-cache.html")
        .is_file());
    // A fix in the copy alone asks nothing of the intent file.
    write(
        root,
        ".claude/intent/view/adr-0001-cache.md",
        "# ADR-0001: Кеш (виправлено)\n",
    );
    assert_eq!(
        context(&wrote(
            root,
            "s1",
            "",
            ".claude/intent/view/adr-0001-cache.md"
        )),
        None
    );
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
    // Nothing is asked of a project without copies.
    let dir = project("English", "English", false, true, false);
    write(
        dir.path(),
        "docs/adr/ADR-0001-cache.md",
        "# ADR-0001: Cache\n",
    );
    assert_eq!(
        context(&wrote(dir.path(), "s1", "", "docs/adr/ADR-0001-cache.md")),
        None
    );
}

/// Swapping the languages renames each document's versions, so its unsuffixed
/// file stays in the answer language; one whose other file is in the way is
/// left as it was, and said so.
#[test]
fn edit_language_relabels_versions() {
    let dir = project("English", "Ukrainian", true, false, true);
    let root = dir.path();
    let uk = write(root, ".claude/notes/flow.md", UK_LEDGER);
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    render_family(&versions::classify(&uk).unwrap());
    write(root, ".claude/notes/recap/2026-10-06.md", "# Підсумок\n");
    // In the way: a file already named for the old answer language.
    write(root, ".claude/notes/x.md", UK_LEDGER);
    write(root, ".claude/notes/x.ukrainian.md", UK_LEDGER);

    let out = assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["edit", "language", "--path"])
        .arg(root)
        .args(["--prompts", "Ukrainian", "--answers", "English"])
        .env("OCGEN_NOTES_OPEN", "0")
        .assert()
        .success();
    let said = String::from_utf8_lossy(&out.get_output().stdout).into_owned();

    assert_eq!(read(root, ".claude/notes/flow.md"), EN_LEDGER);
    assert_eq!(read(root, ".claude/notes/flow.ukrainian.md"), UK_LEDGER);
    assert!(!root.join(".claude/notes/flow.english.md").exists());
    assert!(!root.join(".claude/notes/flow.english.html").exists());
    assert_has(
        &read(root, ".claude/notes/flow.html"),
        &[r#"<html lang="en">"#, r#"href="flow.ukrainian.html""#],
    );
    assert!(read(root, ".claude/notes/flow.ukrainian.html").contains(r#"<html lang="uk">"#));
    // A report with no English version yet keeps its Ukrainian one, renamed.
    assert!(root
        .join(".claude/notes/recap/2026-10-06.ukrainian.md")
        .is_file());
    assert!(!root.join(".claude/notes/recap/2026-10-06.md").exists());
    assert_has(
        &said,
        &[
            ".claude/notes/flow.md → .claude/notes/flow.ukrainian.md",
            ".claude/notes/flow.english.md → .claude/notes/flow.md",
            ".claude/notes/x.md — its other language's file is in the way",
        ],
    );
    // Each topic is still listed once.
    let listed: Vec<String> = ocgen::notes::ledgers(&root.join(".claude/notes"))
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(listed, ["flow.md", "x.md"]);
}

/// The ledger helper usually writes a version in several edits: once it has
/// caught up, its further edits finish the update instead of putting the other
/// version behind — until its turn ends.
#[test]
fn the_helper_may_write_a_version_in_steps() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    assert!(context(&wrote(root, "s1", "helper", ".claude/notes/flow.md")).is_some());
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    assert_eq!(
        context(&wrote(
            root,
            "s1",
            "helper",
            ".claude/notes/flow.english.md"
        )),
        None
    );
    write(
        root,
        ".claude/notes/flow.english.md",
        &format!("{EN_LEDGER}Hint: (Flow) next.\n"),
    );
    assert_eq!(
        context(&wrote(
            root,
            "s1",
            "helper",
            ".claude/notes/flow.english.md"
        )),
        None
    );
    assert!(!read(root, ".claude/notes/flow.html").contains(r#"class="version-note""#));
    assert_eq!(
        held(&stop(root, "SubagentStop", "s1", "helper", false)),
        None
    );
    // Its turn is over: the next edit of the English version leads again.
    write(
        root,
        ".claude/notes/flow.english.md",
        &format!("{EN_LEDGER}Hint: (Failure) next.\n"),
    );
    let note = context(&wrote(
        root,
        "s1",
        "helper",
        ".claude/notes/flow.english.md",
    ))
    .unwrap();
    assert!(
        note.contains("- .claude/notes/flow.md (Ukrainian): update it to match"),
        "{note}"
    );
}

/// A turn is held once per debt, whatever `stop_hook_active` says — another
/// Stop hook may have set it — and again for a new one.
#[test]
fn a_turn_is_held_once_per_debt() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.md");
    assert!(
        held(&stop(root, "Stop", "s1", "", true)).is_some(),
        "held once, even so"
    );
    assert_eq!(held(&stop(root, "Stop", "s1", "", true)), None);
    // Written after all; then, next turn, the English one changes: a new debt.
    write(root, ".claude/notes/flow.english.md", EN_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.english.md");
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
    write(
        root,
        ".claude/notes/flow.english.md",
        &format!("{EN_LEDGER}\n"),
    );
    assert!(context(&wrote(root, "s1", "", ".claude/notes/flow.english.md")).is_some());
    assert!(held(&stop(root, "Stop", "s1", "", false)).is_some());
}

/// What can't be read right is left alone: a subagent with no id, a document
/// whose files are gone, a project state that doesn't load.
#[test]
fn the_check_lets_go_of_what_it_cant_read() {
    let dir = project("English", "Ukrainian", true, false, false);
    let root = dir.path();
    write(root, ".claude/notes/flow.md", UK_LEDGER);
    wrote(root, "s1", "", ".claude/notes/flow.md");
    // A subagent without an id isn't held for its session's debt.
    assert_eq!(held(&stop(root, "SubagentStop", "s1", "", false)), None);
    // A state file that doesn't load: nothing held, nothing forgotten.
    let state = root.join(".claude/.ocgen-state.json");
    let saved = fs::read_to_string(&state).unwrap();
    fs::write(&state, "{").unwrap();
    assert_eq!(held(&stop(root, "Stop", "s1", "", false)), None);
    fs::write(&state, saved).unwrap();
    assert!(held(&stop(root, "Stop", "s1", "", false)).is_some());
    // Renamed away: nothing is owed for it, and its record goes.
    write(root, ".claude/notes/gone.md", UK_LEDGER);
    wrote(root, "s2", "", ".claude/notes/gone.md");
    fs::remove_file(root.join(".claude/notes/gone.md")).unwrap();
    assert_eq!(held(&stop(root, "Stop", "s2", "", false)), None);
    assert!(!root.join(".claude/notes/.versions/gone.json").exists());
}

/// A dot that names no language isn't a version: `flow.backup.md` is refused
/// as before, never rendered or listed.
#[test]
fn a_suffix_that_names_no_language_is_no_version() {
    for (prompts, answers) in [("English", "English"), ("English", "Ukrainian")] {
        let dir = project(prompts, answers, true, false, false);
        let root = dir.path();
        write(root, ".claude/notes/flow.md", EN_LEDGER);
        write(root, ".claude/notes/flow.backup.md", EN_LEDGER);
        let o = wrote(root, "s1", "", ".claude/notes/flow.backup.md");
        assert!(o.stderr.contains("lowercase-hyphen slug"), "{o:?}");
        assert!(!root.join(".claude/notes/flow.backup.html").exists());
        assert_eq!(
            ocgen::notes::ledgers(&root.join(".claude/notes")),
            [root.join(".claude/notes/flow.md")]
        );
    }
}
