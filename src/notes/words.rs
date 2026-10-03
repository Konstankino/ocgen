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
    /// Lens keys → shown names.
    lenses: [(&'static str, &'static str); 7],
    /// Footnote prefixes → shown prefixes.
    footnotes: [(&'static str, &'static str); 6],
    /// The counts line: questions, verified, inferred, stale (0 = left out).
    counts: fn(usize, usize, usize, usize) -> String,
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
