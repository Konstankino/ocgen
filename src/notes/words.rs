//! The pages' own words — labels, counts, the `lang` attribute, and the ledger's
//! fixed English keys (lens names, evidence tags, footnote prefixes) shown
//! translated — in the project's answer language. The Markdown keeps the keys in
//! English, since the parser and the page's colours rely on them; only what is
//! displayed changes. A language without its own words reads English.

/// Everything a page says by itself, in one language.
pub struct Words {
    /// The `<html lang>` value.
    pub lang: &'static str,
    /// The banner shown when the live view stops (HTML: it names a command).
    pub offline: &'static str,
    pub commit: &'static str,
    pub ledger: &'static str,
    pub overview: &'static str,
    pub qa_log: &'static str,
    pub where_we_left_off: &'static str,
    pub mental_model: &'static str,
    pub nothing_yet: &'static str,
    pub lens_coverage: &'static str,
    pub lens_lead: &'static str,
    pub lens_aria: &'static str,
    pub map: &'static str,
    pub open_questions: &'static str,
    pub none_recorded: &'static str,
    pub glossary: &'static str,
    pub term: &'static str,
    pub meaning: &'static str,
    pub verified: &'static str,
    pub inferred: &'static str,
    pub corrected: &'static str,
    pub stale: &'static str,
    pub stale_reverify: &'static str,
    pub hint: &'static str,
    /// A phase heading's key (`## Phase 1 · …`).
    pub phase: &'static str,
    pub no_answers: &'static str,
    pub footer: &'static str,
    /// The /intent reading copy's page.
    pub intent_original: &'static str,
    pub intent_footer: &'static str,
    /// The issue draft's editor page.
    pub draft: DraftWords,
    /// Lens keys → shown names.
    lenses: [(&'static str, &'static str); 7],
    /// Footnote prefixes → shown prefixes.
    footnotes: [(&'static str, &'static str); 6],
    /// The counts line: questions, verified, inferred, stale (0 = left out).
    counts: fn(usize, usize, usize, usize) -> String,
}

/// What the issue draft's editor page says by itself.
pub struct DraftWords {
    pub title: &'static str,
    /// The formatted editor's name for screen readers.
    pub editor: &'static str,
    /// The Markdown source view.
    pub source: &'static str,
    /// The preview drawn the way GitHub draws the issue.
    pub preview: &'static str,
    /// The Focus button: full screen, without the browser's toolbar. (The
    /// dimming — all but the block being edited and the one before and after
    /// it — is on by default either way.)
    pub focus: &'static str,
    /// The Focus button's tooltip.
    pub focus_title: &'static str,
    /// The page's two palettes, black (the default) and white, and their group.
    pub palette: &'static str,
    pub black: &'static str,
    pub white: &'static str,
    /// The keys, for the footer (HTML: it marks them up as `<kbd>`).
    pub keys: &'static str,
    /// Asked for by Ctrl/⌘+K.
    pub link_prompt: &'static str,
    /// The formatted editor couldn't open the draft: the Source view shows it.
    pub source_only: &'static str,
    pub save: &'static str,
    pub copy_markdown: &'static str,
    pub copy_formatted: &'static str,
    pub saved: &'static str,
    pub unsaved: &'static str,
    pub saving: &'static str,
    pub save_failed: &'static str,
    pub copied: &'static str,
    pub copy_failed: &'static str,
    /// The file changed on disk while the page has no unsaved edits.
    pub updated: &'static str,
    /// The file changed on disk while the page has unsaved edits.
    pub changed: &'static str,
    /// A save refused because the file changed since it was loaded.
    pub conflict: &'static str,
    pub load_disk: &'static str,
    pub keep_mine: &'static str,
    pub overwrite: &'static str,
    /// The banner shown when the editor stops (HTML: it names a command).
    pub offline: &'static str,
    /// Before the approvers the draft doesn't @mention.
    pub missing: &'static str,
    /// The draft still says "No approvers configured".
    pub says_none: &'static str,
    /// Before the count of lines wrapped by hand mid-sentence.
    pub wrapped: &'static str,
    /// The button that joins them.
    pub join_lines: &'static str,
    pub footer: &'static str,
    /// The editor's link to the list of drafts.
    pub all_drafts: &'static str,
    /// The list of drafts: its title, its pager (`{page}`, `{pages}`) and its
    /// count (`{count}`).
    pub list_title: &'static str,
    pub newer: &'static str,
    pub older: &'static str,
    pub page_of: &'static str,
    pub count: &'static str,
    /// Before the approvers a listed draft doesn't @mention.
    pub misses: &'static str,
    /// A listed draft still says "No approvers configured".
    pub says_none_short: &'static str,
    /// The list's keys (HTML: `<kbd>`), its footer, its offline banner (HTML: it
    /// names a command), and what it shows with no drafts.
    pub list_keys: &'static str,
    pub list_footer: &'static str,
    pub list_offline: &'static str,
    pub none_yet: &'static str,
    /// Before the `.md` files in the folder whose names can't name a draft.
    pub skipped: &'static str,
}

pub static ENGLISH: Words = Words {
    lang: "en",
    offline: "Live view disconnected — this page won't refresh by itself. Run <code>ocgen notes open</code> to reconnect.",
    commit: "Commit",
    ledger: "Ledger:",
    overview: "Overview",
    qa_log: "Q&A log",
    where_we_left_off: "Where we left off",
    mental_model: "Mental model",
    nothing_yet: "Nothing yet.",
    lens_coverage: "Lens coverage",
    lens_lead: "Which lenses your questions have used — a gap is a good place to look next.",
    lens_aria: "Questions per lens",
    map: "Map",
    open_questions: "Open questions",
    none_recorded: "None recorded.",
    glossary: "Glossary",
    term: "Term",
    meaning: "Meaning",
    verified: "Verified",
    inferred: "Inferred",
    corrected: "Corrected",
    stale: "Stale",
    stale_reverify: "Stale — re-verify",
    hint: "Hint:",
    phase: "Phase",
    no_answers: "No questions answered yet.",
    footer: "Rendered by ocgen from the Markdown ledger next to this file — the .md is the source of truth.",
    intent_original: "Original:",
    intent_footer: "A reading copy rendered by ocgen from the Markdown next to this file. The intent file it renders is the record.",
    draft: DraftWords {
        title: "Issue draft",
        editor: "Issue text",
        source: "Source",
        preview: "GitHub preview",
        focus: "Focus",
        focus_title: "Full screen: hides the browser’s toolbar (Esc leaves)",
        palette: "Palette",
        black: "Black",
        white: "White",
        keys: "<kbd class=\"mod\">Ctrl</kbd>+<kbd>S</kbd> saves · <kbd>Esc</kbd> shows or hides the controls · <kbd class=\"mod\">Ctrl</kbd>+<kbd>/</kbd> shows the Markdown source · <kbd class=\"mod\">Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>.</kbd> turns the dimming off or on",
        link_prompt: "Link address",
        source_only: "The formatted editor couldn't open this draft, so it shows as Markdown source.",
        save: "Save",
        copy_markdown: "Copy Markdown",
        copy_formatted: "Copy formatted",
        saved: "Saved",
        unsaved: "Unsaved changes",
        saving: "Saving…",
        save_failed: "Not saved — your edits are still here.",
        copied: "Copied",
        copy_failed: "Couldn't copy — select the text and copy it by hand.",
        updated: "Updated from the file",
        changed: "The file changed while you were editing.",
        conflict: "The file changed since you opened it — saving now would overwrite that change.",
        load_disk: "Load the file (drop my edits)",
        keep_mine: "Keep my edits",
        overwrite: "Save mine anyway",
        offline: "Editor disconnected — saving won't work. Run <code>ocgen draft</code> to reconnect.",
        missing: "GitHub notifies only the people an issue @mentions, and this draft doesn't mention these approvers — add a pending sign-off line for each under “Needs from”:",
        says_none: "It still says “No approvers configured”, but this project has approvers — remove that line.",
        wrapped: "GitHub shows every newline in an issue as a line break. Lines wrapped by hand mid-sentence:",
        join_lines: "Join them",
        footer: "Save writes the file named above — the one gh issue create --body-file files on GitHub.",
        all_drafts: "All drafts",
        list_title: "Issue drafts",
        newer: "‹ Newer",
        older: "Older ›",
        page_of: "Page {page} of {pages}",
        count: "{count} drafts · newest first",
        misses: "Doesn’t @mention:",
        says_none_short: "Says “No approvers configured”",
        list_keys: "<kbd>↑</kbd> <kbd>↓</kbd> choose · <kbd>Enter</kbd> opens · <kbd>←</kbd> <kbd>→</kbd> newer or older",
        list_footer: "Open a draft to edit it. The list follows the folder: it refreshes when a draft changes.",
        list_offline: "Editor disconnected — this list won’t refresh. Run <code>ocgen draft</code> to reconnect.",
        none_yet: "No issue drafts yet.",
        skipped: "Not listed, because ocgen can’t open these names — rename them to letters, digits, - and _ (at most 200 characters):",
    },
    lenses: [
        ("Structure", "Structure"),
        ("Flow", "Flow"),
        ("Contract", "Contract"),
        ("Rationale", "Rationale"),
        ("Change impact", "Change impact"),
        ("Failure", "Failure"),
        ("Other", "Other"),
    ],
    footnotes: [
        ("Source:", "Source:"),
        ("Sources:", "Sources:"),
        ("Note:", "Note:"),
        ("Evidence:", "Evidence:"),
        ("Example:", "Example:"),
        ("Why:", "Why:"),
    ],
    counts: |q, v, i, s| {
        let stale = if s > 0 { format!(" · {s} stale") } else { String::new() };
        let plural = if q == 1 { "" } else { "s" };
        format!("{q} question{plural} · {v} verified · {i} inferred{stale}")
    },
};

pub static UKRAINIAN: Words = Words {
    lang: "uk",
    offline: "Живий перегляд від’єднано — сторінка сама не оновиться. Щоб під’єднатися знову, виконайте <code>ocgen notes open</code>.",
    commit: "Коміт",
    ledger: "Нотатки:",
    overview: "Огляд",
    qa_log: "Запитання й відповіді",
    where_we_left_off: "На чому ми зупинилися",
    mental_model: "Ментальна модель",
    nothing_yet: "Поки нічого.",
    lens_coverage: "Охоплення ракурсів",
    lens_lead: "Які ракурси використовували ваші запитання — прогалина підказує, куди дивитися далі.",
    lens_aria: "Запитання за ракурсами",
    map: "Карта",
    open_questions: "Відкриті запитання",
    none_recorded: "Поки немає.",
    glossary: "Словник",
    term: "Термін",
    meaning: "Значення",
    verified: "Перевірено",
    inferred: "Припущення",
    corrected: "Виправлено",
    stale: "Застаріло",
    stale_reverify: "Застаріло — перевірте ще раз",
    hint: "Підказка:",
    phase: "Етап",
    no_answers: "Ще немає відповідей.",
    footer: "ocgen створив цю сторінку з Markdown-нотаток, що лежать поруч. Основний файл — .md.",
    intent_original: "Оригінал англійською:",
    intent_footer: "Переклад для читання, який ocgen створив із Markdown-файлу поруч. Чинний запис — англійський файл наміру.",
    draft: DraftWords {
        title: "Чернетка GitHub issue",
        editor: "Текст задачі",
        source: "Код Markdown",
        preview: "Як на GitHub",
        focus: "Фокус",
        focus_title: "На весь екран: ховає панель браузера (Esc — вийти)",
        palette: "Палітра",
        black: "Чорна",
        white: "Біла",
        keys: "<kbd class=\"mod\">Ctrl</kbd>+<kbd>S</kbd> — зберегти · <kbd>Esc</kbd> — показати або сховати кнопки · <kbd class=\"mod\">Ctrl</kbd>+<kbd>/</kbd> — код Markdown · <kbd class=\"mod\">Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>.</kbd> — вимкнути або ввімкнути приглушення",
        link_prompt: "Адреса посилання",
        source_only: "Форматований редактор не зміг відкрити цю чернетку, тож її показано як код Markdown.",
        save: "Зберегти",
        copy_markdown: "Копіювати Markdown",
        copy_formatted: "Копіювати з форматуванням",
        saved: "Збережено",
        unsaved: "Є незбережені зміни",
        saving: "Зберігаю…",
        save_failed: "Не збережено — ваші зміни нікуди не зникли.",
        copied: "Скопійовано",
        copy_failed: "Не вдалося скопіювати — виділіть текст і скопіюйте вручну.",
        updated: "Оновлено з файлу",
        changed: "Файл змінився, поки ви редагували.",
        conflict: "Файл змінився відтоді, як ви його відкрили, — збереження зараз перезапише цю зміну.",
        load_disk: "Завантажити файл (мої зміни буде втрачено)",
        keep_mine: "Залишити мої зміни",
        overwrite: "Усе одно зберегти мої",
        offline: "Редактор від’єднано — зберегти не вийде. Щоб під’єднатися знову, виконайте <code>ocgen draft</code>.",
        missing: "GitHub сповіщає лише тих, кого згадано через @ у задачі, а ця чернетка не згадує цих затверджувачів — додайте для кожного рядок погодження в розділі «Needs from»:",
        says_none: "У чернетці досі написано «No approvers configured», але в проєкті є затверджувачі — приберіть цей рядок.",
        wrapped: "GitHub показує кожен перенос рядка в задачі як розрив рядка. Рядків, перенесених вручну посеред речення:",
        join_lines: "Об’єднати їх",
        footer: "Кнопка «Зберегти» записує файл, указаний вище, — саме його подає на GitHub команда gh issue create --body-file.",
        all_drafts: "Усі чернетки",
        list_title: "Чернетки GitHub issue",
        newer: "‹ Новіші",
        older: "Старіші ›",
        page_of: "Сторінка {page} з {pages}",
        count: "Чернеток: {count} · найновіші вгорі",
        misses: "Не згадує через @:",
        says_none_short: "Досі написано «No approvers configured»",
        list_keys: "<kbd>↑</kbd> <kbd>↓</kbd> — вибрати · <kbd>Enter</kbd> — відкрити · <kbd>←</kbd> <kbd>→</kbd> — новіші або старіші",
        list_footer: "Відкрийте чернетку, щоб редагувати її. Список стежить за текою й оновлюється, коли чернетка змінюється.",
        list_offline: "Редактор від’єднано — список не оновлюватиметься. Щоб під’єднатися знову, виконайте <code>ocgen draft</code>.",
        none_yet: "Чернеток ще немає.",
        skipped: "Не в списку, бо ocgen не може відкрити файли з такими назвами — перейменуйте їх: лише букви, цифри, - і _ (до 200 символів):",
    },
    lenses: [
        ("Structure", "Структура"),
        ("Flow", "Потік"),
        ("Contract", "Контракт"),
        ("Rationale", "Обґрунтування"),
        ("Change impact", "Вплив змін"),
        ("Failure", "Збої"),
        ("Other", "Інше"),
    ],
    footnotes: [
        ("Source:", "Джерело:"),
        ("Sources:", "Джерела:"),
        ("Note:", "Примітка:"),
        ("Evidence:", "Доказ:"),
        ("Example:", "Приклад:"),
        ("Why:", "Чому:"),
    ],
    // Ukrainian UI counters: a genitive plural and the number, whatever it is.
    counts: |q, v, i, s| {
        let stale = if s > 0 { format!(" · застарілих: {s}") } else { String::new() };
        format!("Запитань: {q} · перевірених: {v} · припущень: {i}{stale}")
    },
};

/// The words for an answer language (e.g. `Ukrainian`, any case); English otherwise.
pub fn for_language(language: &str) -> &'static Words {
    if language.trim().eq_ignore_ascii_case("ukrainian") {
        &UKRAINIAN
    } else {
        &ENGLISH
    }
}

impl Words {
    /// The shown name of a lens key (`Change impact` → `Вплив змін`).
    pub fn lens(&self, key: &str) -> String {
        self.lenses
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map_or(key, |(_, shown)| shown)
            .to_string()
    }

    /// A phase title as shown: its `Phase` key in these words.
    pub fn phase_title(&self, title: &str) -> String {
        match title.strip_prefix("Phase ") {
            Some(rest) => format!("{} {rest}", self.phase),
            None => title.to_string(),
        }
    }

    /// The counts line of a ledger.
    pub fn counts(
        &self,
        questions: usize,
        verified: usize,
        inferred: usize,
        stale: usize,
    ) -> String {
        (self.counts)(questions, verified, inferred, stale)
    }

    /// A claim's evidence label as shown: a known key (`Verified`, `Inferred 70%`,
    /// `Corrected`, `Stale`) in these words, anything else as written.
    pub fn claim_label(&self, label: &str) -> String {
        let t = label.trim();
        let lower = t.to_ascii_lowercase();
        for (key, shown) in [
            ("verified", self.verified),
            ("inferred", self.inferred),
            ("corrected", self.corrected),
            ("stale", self.stale),
        ] {
            if lower == key || lower.starts_with(&format!("{key} ")) {
                return format!("{shown}{}", &t[key.len()..]);
            }
        }
        t.to_string()
    }

    /// A footnote paragraph with its prefix shown in these words.
    pub fn footnote(&self, text: &str) -> String {
        for (key, shown) in self.footnotes {
            if let Some(rest) = text.strip_prefix(key) {
                return format!("{shown}{rest}");
            }
        }
        text.to_string()
    }

    /// Whether a paragraph is a footnote (by its English key).
    pub fn is_footnote(text: &str) -> bool {
        ENGLISH.footnotes.iter().any(|(k, _)| text.starts_with(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_shown_in_the_language_and_unknown_text_as_written() {
        let uk = for_language("ukrainian");
        assert_eq!(uk.lens("Change impact"), "Вплив змін");
        assert_eq!(uk.lens("Mystery"), "Mystery");
        assert_eq!(uk.claim_label("Inferred 70%"), "Припущення 70%");
        assert_eq!(uk.claim_label("verified"), "Перевірено");
        assert_eq!(uk.claim_label("#blue Risky"), "#blue Risky");
        assert_eq!(uk.footnote("Source: a.rs:1"), "Джерело: a.rs:1");
        assert_eq!(
            uk.counts(1, 1, 0, 0),
            "Запитань: 1 · перевірених: 1 · припущень: 0"
        );
        assert_eq!(for_language("Polish").lang, "en");
        assert_eq!(
            ENGLISH.counts(1, 1, 0, 2),
            "1 question · 1 verified · 0 inferred · 2 stale"
        );
    }
}
