//! Parse an `/inquire` ledger (`.claude/notes/<topic>.md`) into the parts its HTML
//! view shows. The format is defined in the inquire skill; the parser is lenient
//! (separators, key case, confidence phrasing, bullets) and never fails — anything
//! it doesn't recognise is kept as free-form Markdown, so older ledgers still render.

use regex::Regex;

/// The six lenses the inquire skill hints with, in display order.
pub const LENSES: [&str; 6] = [
    "Structure",
    "Flow",
    "Contract",
    "Rationale",
    "Change impact",
    "Failure",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Evidence {
    #[default]
    Unknown,
    Verified,
    /// With its stated confidence, when there is one.
    Inferred(Option<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct Resume {
    pub question: String,
    pub hint: String,
    pub hint_lens: String,
}

/// One `### Q<n> · <Lens> · <Evidence>` entry of the Q&A log.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub n: u32,
    /// One of [`LENSES`], or `Other`.
    pub lens: String,
    pub evidence: Evidence,
    pub stale: bool,
    pub q: String,
    pub a: String,
    pub cites: Vec<String>,
    pub hint: String,
    pub hint_lens: String,
    pub notes: Vec<String>,
    /// Lines that had no recognised key.
    pub extra: Vec<String>,
}

/// A section the parser has no special view for: its title and raw lines.
#[derive(Debug, Clone, Default)]
pub struct Section {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub topic: String,
    pub updated: String,
    pub commit: String,
    pub resume: Resume,
    pub mental_model: Vec<String>,
    /// The map's Markdown lines, without its mermaid blocks.
    pub map_text: Vec<String>,
    pub map_mermaid: Vec<String>,
    pub entries: Vec<Entry>,
    /// Q&A lines outside any `### Q` entry (an older, free-form log).
    pub qa_text: Vec<String>,
    pub open_questions: Vec<String>,
    pub glossary: Vec<(String, String)>,
    pub extra: Vec<Section>,
}

impl Ledger {
    /// Questions per lens: the six lenses, then `Other` when used.
    pub fn lens_coverage(&self) -> Vec<(String, usize)> {
        let count = |name: &str| self.entries.iter().filter(|e| e.lens == name).count();
        let mut out: Vec<(String, usize)> =
            LENSES.iter().map(|l| (l.to_string(), count(l))).collect();
        let other = count("Other");
        if other > 0 {
            out.push(("Other".into(), other));
        }
        out
    }
}

/// `key: value` with a case-insensitive key from `keys` (bullet markers allowed).
fn keyed<'a>(line: &'a str, keys: &[&str]) -> Option<&'a str> {
    let line = strip_bullet(line.trim());
    let (k, v) = line.split_once(':')?;
    let k = k.trim().trim_matches('*').trim().to_ascii_lowercase();
    keys.contains(&k.as_str()).then(|| v.trim())
}

fn strip_bullet(line: &str) -> &str {
    for p in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            return rest.trim_start();
        }
    }
    line
}

/// A lens name in any case, canonicalised.
fn lens_of(s: &str) -> Option<&'static str> {
    let s = s.trim().trim_matches(|c| c == '(' || c == ')').trim();
    LENSES.iter().copied().find(|l| l.eq_ignore_ascii_case(s))
}

/// `(Lens) text` → (`Lens`, `text`); otherwise no lens.
fn split_hint(hint: &str) -> (String, String) {
    let h = hint.trim();
    if let Some(rest) = h.strip_prefix('(') {
        if let Some((lens, text)) = rest.split_once(')') {
            if let Some(l) = lens_of(lens) {
                return (l.to_string(), text.trim().to_string());
            }
        }
    }
    (String::new(), h.to_string())
}

fn evidence_of(s: &str) -> Option<Evidence> {
    let lower = s.to_ascii_lowercase();
    if lower.contains("unverified") {
        return Some(Evidence::Inferred(None));
    }
    let inferred = Regex::new(r"(?i)inferred\D{0,15}?(\d{1,3})\s*%").unwrap();
    if let Some(c) = inferred.captures(s) {
        let n: u32 = c[1].parse().unwrap_or(0);
        return Some(Evidence::Inferred(Some(n.min(100) as u8)));
    }
    if lower.contains("inferred") {
        return Some(Evidence::Inferred(None));
    }
    lower.contains("verified").then_some(Evidence::Verified)
}

fn is_stale(s: &str) -> bool {
    s.to_ascii_lowercase().contains("stale")
}

fn parse_entry_heading(text: &str, fallback_n: u32) -> Entry {
    let mut e = Entry {
        n: fallback_n,
        lens: "Other".into(),
        ..Entry::default()
    };
    let qn = Regex::new(r"(?i)^q\s*(\d+)$").unwrap();
    for part in text.split(['·', '|']).map(str::trim) {
        if let Some(c) = qn.captures(part) {
            e.n = c[1].parse().unwrap_or(fallback_n);
        } else if let Some(l) = lens_of(part) {
            e.lens = l.to_string();
        } else {
            if let Some(ev) = evidence_of(part) {
                e.evidence = ev;
            }
            if is_stale(part) {
                e.stale = true;
            }
        }
    }
    e
}

fn fill_entry(e: &mut Entry, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    if let Some(v) = keyed(line, &["q", "question"]) {
        e.q = v.into();
    } else if let Some(v) = keyed(line, &["a", "answer"]) {
        e.a = v.into();
    } else if let Some(v) = keyed(line, &["cites", "cite", "citations", "sources"]) {
        e.cites = v
            .split([',', ';'])
            .map(|c| c.trim().trim_matches('`').trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
    } else if let Some(v) = keyed(line, &["hint"]) {
        (e.hint_lens, e.hint) = split_hint(v);
    } else if let Some(v) = keyed(line, &["note", "discrepancy"]) {
        e.notes.push(v.into());
    } else if let Some(v) = keyed(line, &["evidence", "status", "confidence"]) {
        if let Some(ev) = evidence_of(v) {
            e.evidence = ev;
        }
        e.stale |= is_stale(v);
    } else {
        let t = strip_bullet(line.trim());
        if is_stale(t) && t.len() < 40 {
            e.stale = true;
        } else {
            e.extra.push(t.to_string());
        }
    }
}

fn list_items(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| strip_bullet(l.trim()).to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn glossary(lines: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.starts_with('|') {
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            let header = cells
                .first()
                .is_some_and(|c| c.eq_ignore_ascii_case("term"));
            let rule = cells.iter().all(|c| c.chars().all(|ch| "-: ".contains(ch)));
            if cells.len() >= 2 && !header && !rule {
                out.push((
                    cells[0].trim_matches('*').to_string(),
                    cells[1..].join(" | "),
                ));
            }
            continue;
        }
        let item = strip_bullet(t);
        if item.is_empty() {
            continue;
        }
        // The earliest separator wins (a definition may contain dashes or colons).
        let split = [" — ", " – ", " - ", ":"]
            .iter()
            .filter_map(|sep| item.find(sep).map(|i| (i, sep.len())))
            .min()
            .map(|(i, n)| (&item[..i], &item[i + n..]));
        match split {
            Some((term, def)) => out.push((
                term.trim().trim_matches('*').trim().to_string(),
                def.trim().to_string(),
            )),
            None => out.push((item.trim_matches('*').to_string(), String::new())),
        }
    }
    out
}

/// Parse a ledger. Never fails.
pub fn parse(md: &str) -> Ledger {
    let mut l = Ledger::default();
    // Split into the header (before the first `## `) and `## ` sections, keeping
    // fenced code blocks intact (a `## ` inside a fence is not a heading).
    let mut header: Vec<String> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut in_fence = false;
    for line in md.lines() {
        let t = line.trim_end();
        if t.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence {
            if let Some(title) = t.strip_prefix("## ") {
                sections.push(Section {
                    title: title.trim().to_string(),
                    lines: Vec::new(),
                });
                continue;
            }
        }
        match sections.last_mut() {
            Some(s) => s.lines.push(t.to_string()),
            None => header.push(t.to_string()),
        }
    }

    let commit_re = Regex::new(r"(?i)commit:\s*`?([0-9a-zA-Z._-]+)`?").unwrap();
    for line in &header {
        if let Some(v) = keyed(line, &["topic"]) {
            l.topic = v.into();
        } else if let Some(v) = keyed(line, &["updated"]) {
            let date = commit_re.replace(v, "");
            l.updated = date.trim().to_string();
        }
        if let Some(c) = commit_re.captures(line) {
            l.commit = c[1].to_string();
        }
    }
    if l.topic.is_empty() {
        l.topic = header
            .iter()
            .find_map(|h| h.strip_prefix("# "))
            .unwrap_or("Untitled topic")
            .trim()
            .to_string();
    }

    for s in sections {
        let title = s.title.to_ascii_lowercase();
        if title.starts_with("resume") {
            for line in &s.lines {
                if let Some(v) = keyed(line, &["last question", "question"]) {
                    l.resume.question = v.into();
                } else if let Some(v) = keyed(line, &["hint"]) {
                    (l.resume.hint_lens, l.resume.hint) = split_hint(v);
                }
            }
        } else if title.starts_with("mental model") {
            l.mental_model = list_items(&s.lines);
        } else if title == "map" || title.starts_with("map ") || title.starts_with("module map") {
            let mut fence: Option<(bool, Vec<String>)> = None;
            for line in s.lines {
                let t = line.trim_start();
                match &mut fence {
                    None if t.starts_with("```") => {
                        let mermaid = t.trim_start_matches('`').trim() == "mermaid";
                        if !mermaid {
                            l.map_text.push(line.clone());
                        }
                        fence = Some((mermaid, Vec::new()));
                    }
                    None => l.map_text.push(line),
                    Some((mermaid, body)) if t.starts_with("```") => {
                        if *mermaid {
                            l.map_mermaid.push(body.join("\n"));
                        } else {
                            l.map_text.push(line.clone());
                        }
                        fence = None;
                    }
                    Some((mermaid, body)) => {
                        if !*mermaid {
                            l.map_text.push(line.clone());
                        }
                        body.push(line);
                    }
                }
            }
        } else if title.contains("q&a") || title.contains("q & a") || title == "log" {
            let mut current: Option<Entry> = None;
            for line in &s.lines {
                if let Some(h) = line.trim_start().strip_prefix("### ") {
                    if let Some(e) = current.take() {
                        l.entries.push(e);
                    }
                    current = Some(parse_entry_heading(h, l.entries.len() as u32 + 1));
                    continue;
                }
                match &mut current {
                    Some(e) => fill_entry(e, line),
                    None => l.qa_text.push(line.clone()),
                }
            }
            if let Some(e) = current {
                l.entries.push(e);
            }
        } else if title.starts_with("open question") || title.starts_with("open ") {
            l.open_questions = list_items(&s.lines);
        } else if title.starts_with("glossary") {
            l.glossary = glossary(&s.lines);
        } else {
            l.extra.push(s);
        }
    }
    l
}
