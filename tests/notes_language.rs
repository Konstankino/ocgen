//! The pages people read follow the project's answer language; the Markdown stays
//! as written. The /inquire page translates its own words (labels, counts, the
//! `lang` attribute) and the ledger's fixed keys on display. /intent writes its
//! intent file in English and, when the answers aren't English, a translated
//! reading copy that ocgen renders as a page.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::notes::ledger::parse;
use ocgen::notes::{html, intent_view_target, render_file};
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;
use tempfile::tempdir;

const LEDGER: &str = r#"Topic: Потік запиту
Updated: 2026-10-03   Commit: abc1234
Status: Етап 1
## Resume point
Last question: Як `ocgen new` доходить до Project::scaffold?
Hint: (Failure) шлях помилки в src/templates.rs:24 лише припущення.
## Mental model
- main.rs лише перенаправляє
## Map
- `src/cli.rs` розбирає аргументи
## Phase 1 · Огляд
Як влаштовано запуск.
### Висновки
```claims
Verified | Майстер викликає scaffold | src/main.rs:28
Inferred 70% | Шаблони кешуються | src/templates.rs:30
Corrected | Резервна копія є завжди | src/render.rs:2190
```
Source: src/main.rs:28
## Q&A log
### Q1 · Structure · Verified
Q: Де точка входу?
A: У main.rs.
Cites: src/main.rs:1
Hint: (Flow) простежте один запит.
### Q2 · Failure · Inferred 70% · Stale
Q: Що буде, якщо шаблон зламано?
A: Помилка з контекстом.
Cites: src/templates.rs:24
### Q3 · Flow · Verified
Q: Куди йде результат?
A: На диск.
Cites: src/render.rs:1
## Open questions
- Чи перебудовує doctor хуки плагіна?
## Glossary
- **Project** — модель проєкту (src/render.rs).
"#;

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

/// A scaffolded Claude project at `dir` with these answers and workflows.
fn project(dir: &Path, answers: &str, inquire: bool, intent: bool) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "lang".into();
    p.providers.clear();
    p.response_language = answers.into();
    p.agents = vec![Agent::from_archetype_claude("explorer", "explorer", "English").unwrap()];
    p.claude.workflow.inquire = inquire;
    p.claude.workflow.intent = intent;
    p.scaffold(dir, false).unwrap();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

// ------------------------------------------------------------ /inquire --

#[test]
fn the_ledger_page_speaks_ukrainian() {
    let page =
        html::render_page_in(&parse(LEDGER), Some(".claude/notes/flow.md"), "Ukrainian").unwrap();
    for must in [
        r#"<html lang="uk">"#,
        "Огляд",
        "Запитання й відповіді",
        "На чому ми зупинилися",
        "Ментальна модель",
        "Охоплення ракурсів",
        "Карта",
        "Відкриті запитання",
        "Словник",
        "Підказка:",
        "Нотатки: .claude/notes/flow.md",
        "Коміт abc1234",
        // Counts read as Ukrainian UI counters.
        "Запитань: 3 · перевірених: 2 · припущень: 1 · застарілих: 1",
        // Lens names and evidence tags are the ledger's English keys, shown translated.
        "Структура",
        "Збої",
        "Перевірено",
        "Припущення 70%",
        "Застаріло — перевірте ще раз",
        "Живий перегляд від’єднано",
        // A phase heading's key, shown translated on its tab and title.
        "Етап 1 · Огляд",
        "ocgen створив цю сторінку",
    ] {
        assert!(page.contains(must), "missing {must:?}");
    }
    for gone in [
        "Where we left off",
        "Open questions",
        "Mental model",
        "Hint:",
        ">Verified<",
        "Phase 1",
    ] {
        assert!(!page.contains(gone), "still English: {gone:?}");
    }
    // The classes stay the keys', so the colours still apply.
    assert!(page.contains(r#"class="lens-tag structure""#), "lens class");
    assert!(page.contains(r#"class="ev verified""#), "evidence class");
}

#[test]
fn claim_badges_and_footnotes_are_translated_but_keep_their_class() {
    let page = html::render_page_in(&parse(LEDGER), None, "Ukrainian").unwrap();
    assert!(
        page.contains(r#"<span class="badge ev-verified">Перевірено</span>"#),
        "verified badge"
    );
    assert!(
        page.contains(r#"<span class="badge ev-inferred">Припущення 70%</span>"#),
        "inferred badge"
    );
    assert!(
        page.contains(r#"<span class="badge ev-corrected">Виправлено</span>"#),
        "corrected badge"
    );
    assert!(
        page.contains(r#"<p class="footnote">Джерело: src/main.rs:28</p>"#),
        "footnote"
    );
}

#[test]
fn an_english_page_is_unchanged() {
    let l = parse(LEDGER);
    let english = html::render_page_in(&l, Some(".claude/notes/flow.md"), "English").unwrap();
    assert_eq!(
        english,
        html::render_page(&l, Some(".claude/notes/flow.md")).unwrap()
    );
    // Any language without its own words falls back to English.
    assert_eq!(
        english,
        html::render_page_in(&l, Some(".claude/notes/flow.md"), "Polish").unwrap()
    );
    assert!(english.contains(r#"<html lang="en">"#));
    assert!(english.contains("Where we left off") && english.contains("3 questions · 2 verified"));
}

#[test]
fn render_file_takes_the_language_from_the_project() {
    for (answers, lang) in [("Ukrainian", "uk"), ("", "en")] {
        let dir = tempdir().unwrap();
        project(dir.path(), answers, true, false);
        let md = dir.path().join(".claude/notes/flow.md");
        fs::create_dir_all(md.parent().unwrap()).unwrap();
        fs::write(&md, LEDGER).unwrap();
        let page = fs::read_to_string(render_file(&md).unwrap()).unwrap();
        assert!(
            page.contains(&format!(r#"<html lang="{lang}">"#)),
            "{answers:?}"
        );
    }
    // Outside any ocgen project: English.
    let dir = tempdir().unwrap();
    let md = dir.path().join(".claude/notes/flow.md");
    fs::create_dir_all(md.parent().unwrap()).unwrap();
    fs::write(&md, LEDGER).unwrap();
    let page = fs::read_to_string(render_file(&md).unwrap()).unwrap();
    assert!(page.contains(r#"<html lang="en">"#));
}

#[test]
fn the_inquire_skill_asks_for_ledger_text_in_the_answer_language() {
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", true, false);
    let skill = read(dir.path(), ".claude/skills/inquire/SKILL.md");
    assert!(
        skill.contains("Write the ledger's text in Ukrainian"),
        "{skill}"
    );
    assert!(skill.contains("keep its headings, keys and labels exactly as in the layout below"));

    let dir = tempdir().unwrap();
    project(dir.path(), "", true, false);
    let skill = read(dir.path(), ".claude/skills/inquire/SKILL.md");
    assert!(!skill.contains("Write the ledger's text in"), "{skill}");
}

// -------------------------------------------------------------- /intent --

const ORIGINAL: &str =
    "# ADR-0007: Cache invalidation\n\nStatus: Proposed\n\n## Context\nThe cache is not cleared.\n";
const COPY: &str = "# ADR-0007: Інвалідація кешу\n\nStatus: Proposed\nSeverity: Medium\n\n\
## Контекст\nКеш не очищується після оновлення шаблону (`src/cache.rs:42`).\n\n\
## Рішення\n- Очищати кеш під час `ocgen doctor`.\n\n\
| Варіант | Наслідок |\n|---|---|\n| A | швидко |\n";

fn write_intent(dir: &Path) -> std::path::PathBuf {
    let adr = dir.join("docs/adr");
    fs::create_dir_all(&adr).unwrap();
    fs::write(adr.join("ADR-0007-cache-invalidation.md"), ORIGINAL).unwrap();
    let view = dir.join(".claude/intent/view");
    fs::create_dir_all(&view).unwrap();
    let md = view.join("adr-0007-cache-invalidation.md");
    fs::write(&md, COPY).unwrap();
    md
}

#[test]
fn intent_reading_copies_are_recognised_by_their_place_and_name() {
    for (path, want) in [
        (
            ".claude/intent/view/adr-0007-cache.md",
            Some("adr-0007-cache"),
        ),
        (
            "/abs/repo/.claude/intent/view/rfc-001-x.md",
            Some("rfc-001-x"),
        ),
        (
            r"C:\repo\.claude\intent\view\adr-0007-cache.md",
            Some("adr-0007-cache"),
        ),
        (".claude/intent/view/ADR-0007-cache.md", None),
        (".claude/intent/view/adr-0007-cache.html", None),
        (".claude/intent/adr-0007-cache.md", None),
        (".claude/notes/adr-0007-cache.md", None),
        ("docs/adr/ADR-0007-cache.md", None),
        (".claude/intent/view/../x.md", None),
    ] {
        assert_eq!(intent_view_target(path).as_deref(), want, "{path}");
    }
}

#[test]
fn an_intent_reading_copy_renders_as_a_page_in_the_answer_language() {
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", false, true);
    let md = write_intent(dir.path());
    let html = render_file(&md).unwrap();
    assert_eq!(
        html,
        dir.path()
            .join(".claude/intent/view/adr-0007-cache-invalidation.html")
    );
    let page = fs::read_to_string(&html).unwrap();
    for must in [
        r#"<html lang="uk">"#,
        "/intent",
        "ADR-0007: Інвалідація кешу",
        "<h3>Контекст</h3>",
        "<h3>Рішення</h3>",
        "<table>",
        "src/cache.rs:42",
        // The record it translates, which stays English.
        "Оригінал англійською",
        "docs/adr/ADR-0007-cache-invalidation.md",
        "Переклад для читання",
    ] {
        assert!(page.contains(must), "missing {must:?} in:\n{page}");
    }
    // The view folder keeps itself out of git.
    assert_eq!(read(dir.path(), ".claude/intent/view/.gitignore"), "*\n");
}

#[test]
fn the_notes_hook_renders_an_intent_reading_copy() {
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", false, true);
    let md = write_intent(dir.path());
    let env: HashMap<String, String> = [("OCGEN_NOTES_OPEN", "0")]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let payload = serde_json::json!({
        "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&md) }
    })
    .to_string();
    let o = ocgen::hooks::run("inquire-notes", &payload, &env);
    assert_eq!(
        (o.code, o.stdout.as_str(), o.stderr.as_str()),
        (0, "", ""),
        "{o:?}"
    );
    assert!(dir
        .path()
        .join(".claude/intent/view/adr-0007-cache-invalidation.html")
        .is_file());
}

#[test]
fn the_intent_skill_keeps_markdown_english_and_asks_for_a_reading_copy() {
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", false, true);
    let skill = read(dir.path(), ".claude/skills/intent/SKILL.md");
    for must in [
        "Write the intent file and the GitHub issue draft in English",
        ".claude/intent/view/",
        "translated into Ukrainian",
        "never write the `.html` yourself",
    ] {
        assert!(skill.contains(must), "missing {must:?} in:\n{skill}");
    }

    let dir = tempdir().unwrap();
    project(dir.path(), "", false, true);
    let skill = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(!skill.contains(".claude/intent/view/"), "{skill}");
}

#[test]
fn the_notes_hook_is_wired_for_intent_reading_copies() {
    let has_hook = |dir: &Path| {
        let s: serde_json::Value =
            serde_json::from_str(&read(dir, ".claude/settings.json")).unwrap();
        s["hooks"]["PostToolUse"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|h| h.to_string().contains("inquire-notes"))
    };
    // /intent alone, answers in Ukrainian: the reading copy needs the hook.
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", false, true);
    assert!(has_hook(dir.path()));
    assert!(dir.path().join(".claude/hooks/inquire-notes.sh").is_file());
    // English answers write no copy: nothing to render.
    let dir = tempdir().unwrap();
    project(dir.path(), "", false, true);
    assert!(!has_hook(dir.path()));
}

#[test]
fn notes_render_accepts_an_intent_reading_copy() {
    let dir = tempdir().unwrap();
    project(dir.path(), "Ukrainian", false, true);
    let md = write_intent(dir.path());
    assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .args(["notes", "render"])
        .arg(&md)
        .assert()
        .success();
    assert!(dir
        .path()
        .join(".claude/intent/view/adr-0007-cache-invalidation.html")
        .is_file());
}
