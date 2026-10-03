//! Render a parsed [`Ledger`] to its HTML view with the `claude/notes/ledger.html.j2`
//! template (overridable like every other template). Everything that comes from
//! the ledger is escaped: inline Markdown is turned into HTML here, after
//! escaping, and handed to the template as already-safe strings; the template
//! itself auto-escapes the rest. The output is deterministic (no timestamps), so
//! an unchanged ledger renders to the same bytes.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use minijinja::{AutoEscape, Environment, Value};
use regex::Regex;

use super::ledger::{Entry, Evidence, Ledger};
use super::words::{self, Words};
use crate::templates;

pub const TEMPLATE: &str = "claude/notes/ledger.html.j2";
/// The /intent reading copy's page.
pub const INTENT_TEMPLATE: &str = "claude/notes/intent.html.j2";
/// The style both pages share.
pub const STYLE: &str = "claude/notes/page.css";

/// HTML-escape text (including quotes, so it is safe inside attributes).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// One line of inline Markdown → HTML: `code`, **bold**, *em* and
/// [links](https://…). The text is escaped before any markup is added.
pub fn inline(s: &str) -> String {
    let bold = Regex::new(r"\*\*([^*]+?)\*\*").unwrap();
    let em = Regex::new(r"(^|[\s(])\*([^*\s][^*]*?)\*").unwrap();
    let link = Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap();
    let mut out = String::new();
    // Odd segments are inside backticks.
    for (i, seg) in s.split('`').enumerate() {
        if i % 2 == 1 {
            out.push_str("<code>");
            out.push_str(&escape(seg));
            out.push_str("</code>");
            continue;
        }
        let e = escape(seg);
        let e = link.replace_all(&e, r#"<a href="$2" rel="noreferrer">$1</a>"#);
        let e = bold.replace_all(&e, "<strong>$1</strong>");
        let e = em.replace_all(&e, "$1<em>$2</em>");
        out.push_str(&e);
    }
    out
}

/// Free-form Markdown lines → HTML blocks: paragraphs, lists, `####` headings,
/// tables, `>` callouts, fenced code, and the report's visual blocks (see
/// [`super::blocks`]; a `mermaid` fence becomes a diagram).
pub fn blocks(lines: &[String]) -> String {
    blocks_in(lines, &words::ENGLISH)
}

/// [`blocks`], with the ledger's fixed keys (evidence tags, footnote prefixes)
/// shown in `words`.
pub fn blocks_in(lines: &[String], words: &'static Words) -> String {
    Blocks::new(words).run(lines)
}

#[derive(Default)]
struct Blocks {
    /// How the ledger's fixed keys are shown (English when unset).
    words: Option<&'static Words>,
    out: String,
    para: Vec<String>,
    list: Option<(&'static str, Vec<String>)>,
    table: Vec<Vec<String>>,
    quote: Vec<String>,
    /// Mark the first paragraph as the lead (a card's or a phase's intro).
    lead: bool,
    emitted: bool,
}

impl Blocks {
    fn new(words: &'static Words) -> Self {
        Self {
            words: Some(words),
            ..Self::default()
        }
    }

    fn lead(words: &'static Words) -> Self {
        Self {
            lead: true,
            ..Self::new(words)
        }
    }

    fn words(&self) -> &'static Words {
        self.words.unwrap_or(&words::ENGLISH)
    }

    fn push(&mut self, html: &str) {
        self.out.push_str(html);
        self.emitted = true;
    }

    fn flush(&mut self) {
        if !self.para.is_empty() {
            let mut text = self.para.join(" ");
            let class = if Words::is_footnote(&text) {
                text = self.words().footnote(&text);
                r#" class="footnote""#
            } else if self.lead && !self.emitted {
                r#" class="lead""#
            } else {
                ""
            };
            self.para.clear();
            self.push(&format!("<p{class}>{}</p>\n", inline(&text)));
        }
        if let Some((tag, items)) = self.list.take() {
            let mut h = format!("<{tag}>");
            for i in items {
                h.push_str(&format!("<li>{}</li>", inline(&i)));
            }
            h.push_str(&format!("</{tag}>\n"));
            self.push(&h);
        }
        if !self.table.is_empty() {
            let mut h = String::from(r#"<div class="table-wrap"><table>"#);
            for (r, row) in self.table.iter().enumerate() {
                let cell = if r == 0 { "th" } else { "td" };
                h.push_str("<tr>");
                for c in row {
                    h.push_str(&format!("<{cell}>{}</{cell}>", inline(c)));
                }
                h.push_str("</tr>");
            }
            h.push_str("</table></div>\n");
            self.table.clear();
            self.push(&h);
        }
        if !self.quote.is_empty() {
            let text = self.quote.join(" ");
            self.quote.clear();
            self.push(&format!(
                r#"<div class="callout"><p>{}</p></div>"#,
                inline(&text)
            ));
            self.out.push('\n');
        }
    }

    fn fence(&mut self, lang: &str, body: &[String]) {
        let html = if lang == "mermaid" {
            format!(
                "<pre class=\"mermaid\">{}</pre>\n",
                escape(&body.join("\n"))
            )
        } else if let Some(h) = super::blocks::render(lang, body, self.words()) {
            h
        } else {
            format!("<pre><code>{}</code></pre>\n", escape(&body.join("\n")))
        };
        self.push(&html);
    }

    fn run(mut self, lines: &[String]) -> String {
        let mut fence: Option<(String, Vec<String>)> = None;
        for line in lines {
            let t = line.trim();
            if let Some((lang, body)) = &mut fence {
                if t.starts_with("```") {
                    let (lang, body) = (lang.clone(), std::mem::take(body));
                    fence = None;
                    self.fence(&lang, &body);
                } else {
                    body.push(line.clone());
                }
                continue;
            }
            if let Some(lang) = t.strip_prefix("```") {
                self.flush();
                fence = Some((lang.trim().to_ascii_lowercase(), Vec::new()));
                continue;
            }
            if t.is_empty() {
                self.flush();
                continue;
            }
            if let Some(q) = t.strip_prefix('>') {
                if !self.para.is_empty() || self.list.is_some() || !self.table.is_empty() {
                    self.flush();
                }
                self.quote.push(q.trim().to_string());
                continue;
            }
            if !self.quote.is_empty() {
                self.flush();
            }
            if t.starts_with('|') {
                if !self.para.is_empty() || self.list.is_some() {
                    self.flush();
                }
                let cells = super::blocks::cells(t.trim_matches('|'));
                let rule = cells
                    .iter()
                    .all(|c| !c.is_empty() && c.chars().all(|ch| "-: ".contains(ch)));
                if !rule {
                    self.table.push(cells);
                }
                continue;
            }
            if let Some(h) = t.strip_prefix('#') {
                self.flush();
                self.push(&format!(
                    "<h4>{}</h4>\n",
                    inline(h.trim_start_matches('#').trim())
                ));
                continue;
            }
            let ordered = t.chars().take_while(char::is_ascii_digit).count();
            let item = if ["- ", "* ", "+ "].iter().any(|p| t.starts_with(p)) {
                Some(("ul", t[2..].trim()))
            } else if ordered > 0 && t[ordered..].starts_with(". ") {
                Some(("ol", t[ordered + 2..].trim()))
            } else {
                None
            };
            match item {
                Some((tag, text)) => {
                    if !self.para.is_empty()
                        || !self.table.is_empty()
                        || self.list.as_ref().is_some_and(|l| l.0 != tag)
                    {
                        self.flush();
                    }
                    self.list
                        .get_or_insert((tag, Vec::new()))
                        .1
                        .push(text.to_string());
                }
                None if self.list.is_some() && line.starts_with([' ', '\t']) => {
                    // A continuation of the previous list item.
                    if let Some(last) = self.list.as_mut().and_then(|l| l.1.last_mut()) {
                        last.push(' ');
                        last.push_str(t);
                    }
                }
                None => {
                    if self.list.is_some() || !self.table.is_empty() {
                        self.flush();
                    }
                    self.para.push(t.to_string());
                }
            }
        }
        if let Some((_, body)) = fence {
            // An unclosed fence: show what there is.
            self.flush();
            let html = format!("<pre><code>{}</code></pre>\n", escape(&body.join("\n")));
            self.push(&html);
        }
        self.flush();
        self.out
    }
}

/// A phase (one tab of the report): an intro, then `### Title` cards. A title
/// ending in `[half]` makes a half-width card; neighbouring halves share a row.
pub fn phase(lines: &[String]) -> String {
    phase_in(lines, &words::ENGLISH)
}

/// [`phase`], with the ledger's fixed keys shown in `words`.
pub fn phase_in(lines: &[String], words: &'static Words) -> String {
    // (title, half, lines); the intro has no title.
    let mut parts: Vec<(Option<String>, bool, Vec<String>)> = vec![(None, false, Vec::new())];
    let mut in_fence = false;
    for line in lines {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence {
            // `---` ends the current card: what follows sits on the phase itself.
            if t.len() >= 3 && t.chars().all(|c| c == '-') {
                parts.push((None, false, Vec::new()));
                continue;
            }
            if let Some(h) = t.strip_prefix("### ") {
                let h = h.trim();
                let (title, half) = match h.strip_suffix("[half]") {
                    Some(rest) => (rest.trim().to_string(), true),
                    None => (h.to_string(), false),
                };
                parts.push((Some(title), half, Vec::new()));
                continue;
            }
        }
        parts.last_mut().unwrap().2.push(line.clone());
    }
    let mut out = String::new();
    let mut row_open = false;
    for (title, half, body) in parts {
        let Some(title) = title else {
            if row_open {
                out.push_str("</div>\n");
                row_open = false;
            }
            out.push_str(&Blocks::lead(words).run(&body));
            continue;
        };
        if half && !row_open {
            out.push_str(r#"<div class="row">"#);
            row_open = true;
        } else if !half && row_open {
            out.push_str("</div>\n");
            row_open = false;
        }
        out.push_str(&format!(
            r#"<section class="card{}"><h3>{}</h3>"#,
            if half { " half" } else { "" },
            inline(&title)
        ));
        out.push('\n');
        out.push_str(&Blocks::lead(words).run(&body));
        out.push_str("</section>\n");
    }
    if row_open {
        out.push_str("</div>\n");
    }
    out
}

fn safe(s: String) -> Value {
    Value::from_safe_string(s)
}

/// A short, stable id for a topic-local anchor.
fn anchor(n: u32) -> String {
    format!("q{n}")
}

fn entry_value(e: &Entry, w: &'static Words) -> Value {
    let (evidence, confidence) = match e.evidence {
        Evidence::Verified => ("verified", String::new()),
        Evidence::Inferred(c) => ("inferred", c.map(|c| format!("{c}%")).unwrap_or_default()),
        Evidence::Unknown => ("", String::new()),
    };
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("n", Value::from(e.n));
    m.insert("id", Value::from(anchor(e.n)));
    m.insert("lens", Value::from(w.lens(&e.lens)));
    m.insert(
        "lens_class",
        Value::from(e.lens.to_ascii_lowercase().replace(' ', "-")),
    );
    m.insert("evidence", Value::from(evidence));
    m.insert("confidence", Value::from(confidence));
    m.insert("stale", Value::from(e.stale));
    m.insert("q", safe(inline(&e.q)));
    m.insert("a", safe(inline(&e.a)));
    m.insert(
        "cites",
        Value::from(
            e.cites
                .iter()
                .map(|c| safe(format!("<code>{}</code>", escape(c))))
                .collect::<Vec<_>>(),
        ),
    );
    m.insert("hint", safe(inline(&e.hint)));
    m.insert("hint_lens", Value::from(e.hint_lens.clone()));
    m.insert("hint_lens_label", Value::from(w.lens(&e.hint_lens)));
    m.insert(
        "notes",
        Value::from(e.notes.iter().map(|n| safe(inline(n))).collect::<Vec<_>>()),
    );
    m.insert("extra", safe(blocks_in(&e.extra, w)));
    Value::from(m)
}

/// The page for an /intent reading copy (`md`, already translated): its `# `
/// title, its leading `Key: value` lines as pills, anything else before the
/// first `## ` heading, then one card per `## ` section. `source` is the copy's
/// path and `original` the English intent file it translates.
pub fn render_intent(
    md: &str,
    source: &str,
    original: Option<&str>,
    language: &str,
) -> Result<String> {
    let w = words::for_language(language);
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template_owned("page.css", templates::load(STYLE)?)
        .context("parsing the notes page style")?;
    env.add_template_owned("intent.html", templates::load(INTENT_TEMPLATE)?)
        .context("parsing the intent HTML template")?;

    let field = Regex::new(r"^([^:`|#>*\-][^:`|]{0,30}):\s+(\S.*)$").unwrap();
    let mut title = String::new();
    let mut fields: Vec<Value> = Vec::new();
    let mut preface: Vec<String> = Vec::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let (mut in_fence, mut in_comment) = (false, false);
    for line in md.lines() {
        let t = line.trim();
        // A template's guidance comment is not part of the copy.
        if !in_fence && (in_comment || t.starts_with("<!--")) {
            in_comment = !t.contains("-->");
            continue;
        }
        if t.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence {
            if title.is_empty() && sections.is_empty() {
                if let Some(h) = t.strip_prefix("# ") {
                    title = h.trim().to_string();
                    continue;
                }
            }
            if let Some(h) = t.strip_prefix("## ") {
                sections.push((h.trim().to_string(), Vec::new()));
                continue;
            }
        }
        match sections.last_mut() {
            Some((_, body)) => body.push(line.to_string()),
            None => match field
                .captures(t)
                .filter(|_| preface.iter().all(|l| l.trim().is_empty()))
            {
                Some(c) => {
                    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
                    m.insert("key", Value::from(c[1].trim().to_string()));
                    m.insert("value", safe(inline(&c[2])));
                    fields.push(Value::from(m));
                }
                None => preface.push(line.to_string()),
            },
        }
    }
    if title.is_empty() {
        title = source
            .rsplit('/')
            .next()
            .unwrap_or(source)
            .trim_end_matches(".md")
            .to_string();
    }
    let sections: Vec<Value> = sections
        .iter()
        .map(|(t, body)| {
            let mut m: BTreeMap<&str, Value> = BTreeMap::new();
            m.insert("title", safe(inline(t)));
            m.insert("html", safe(blocks_in(body, w)));
            Value::from(m)
        })
        .collect();
    let ctx = minijinja::context! {
        w => words_value(w),
        title => title.replace('`', ""),
        title_html => safe(inline(&title)),
        fields => fields,
        preface => safe(blocks_in(&preface, w)),
        sections => sections,
        original => safe(escape(original.unwrap_or(""))),
        source => safe(escape(source)),
    };
    let mut page = env
        .get_template("intent.html")?
        .render(ctx)
        .context("rendering the intent HTML")?;
    if !page.ends_with('\n') {
        page.push('\n');
    }
    Ok(page)
}

/// The page for `ledger`.
pub fn render(ledger: &Ledger) -> Result<String> {
    render_page(ledger, None)
}

/// The page for `ledger`; `source` is the ledger's path, shown in the header.
pub fn render_page(ledger: &Ledger, source: Option<&str>) -> Result<String> {
    render_page_in(ledger, source, "English")
}

/// The page's own words for the template, as one `w` map.
fn words_value(w: &'static Words) -> Value {
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    for (k, v) in [
        ("lang", w.lang),
        ("commit", w.commit),
        ("ledger", w.ledger),
        ("overview", w.overview),
        ("qa_log", w.qa_log),
        ("where_we_left_off", w.where_we_left_off),
        ("mental_model", w.mental_model),
        ("nothing_yet", w.nothing_yet),
        ("lens_coverage", w.lens_coverage),
        ("lens_lead", w.lens_lead),
        ("lens_aria", w.lens_aria),
        ("map", w.map),
        ("open_questions", w.open_questions),
        ("none_recorded", w.none_recorded),
        ("glossary", w.glossary),
        ("term", w.term),
        ("meaning", w.meaning),
        ("verified", w.verified),
        ("inferred", w.inferred),
        ("stale_reverify", w.stale_reverify),
        ("hint", w.hint),
        ("no_answers", w.no_answers),
        ("footer", w.footer),
        ("intent_original", w.intent_original),
        ("intent_footer", w.intent_footer),
    ] {
        m.insert(k, Value::from(v));
    }
    // Ours, not the ledger's: it names a command in markup.
    m.insert("offline", safe(w.offline.to_string()));
    Value::from(m)
}

/// [`render_page`] in an answer language (e.g. `Ukrainian`): the page's own
/// words, the counts and the ledger's fixed keys are shown in it.
pub fn render_page_in(ledger: &Ledger, source: Option<&str>, language: &str) -> Result<String> {
    let w = words::for_language(language);
    let src = templates::load(TEMPLATE)?;
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template_owned("page.css", templates::load(STYLE)?)
        .context("parsing the notes page style")?;
    env.add_template_owned("ledger.html", src)
        .context("parsing the ledger HTML template")?;
    let tmpl = env.get_template("ledger.html")?;

    let l = ledger;
    let verified = l
        .entries
        .iter()
        .filter(|e| e.evidence == Evidence::Verified)
        .count();
    let inferred = l
        .entries
        .iter()
        .filter(|e| matches!(e.evidence, Evidence::Inferred(_)))
        .count();
    let stale = l.entries.iter().filter(|e| e.stale).count();
    let coverage = l.lens_coverage();
    let total = l.entries.len().max(1);
    let lenses: Vec<Value> = coverage
        .iter()
        .map(|(name, count)| {
            let mut m: BTreeMap<&str, Value> = BTreeMap::new();
            m.insert("name", Value::from(w.lens(name)));
            m.insert(
                "class",
                Value::from(name.to_ascii_lowercase().replace(' ', "-")),
            );
            m.insert("count", Value::from(*count));
            m.insert("pct", Value::from(count * 100 / total));
            Value::from(m)
        })
        .collect();
    let mut htmls: Vec<String> = Vec::new();
    let phases: Vec<Value> = l
        .phases
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let html = phase_in(&s.lines, w);
            htmls.push(html.clone());
            let mut m: BTreeMap<&str, Value> = BTreeMap::new();
            m.insert("id", Value::from(format!("phase-{}", i + 1)));
            m.insert("title", safe(inline(&w.phase_title(&s.title))));
            m.insert("html", safe(html));
            Value::from(m)
        })
        .collect();
    let default_tab = match l.phases.len() {
        0 => "overview".to_string(),
        n => format!("phase-{n}"),
    };
    let glossary: Vec<Value> = l
        .glossary
        .iter()
        .map(|(t, d)| {
            let mut m: BTreeMap<&str, Value> = BTreeMap::new();
            m.insert("term", safe(inline(t)));
            m.insert("def", safe(inline(d)));
            Value::from(m)
        })
        .collect();

    let map_html = blocks_in(&l.map_text, w);
    let qa_html = blocks_in(&l.qa_text, w);
    htmls.push(map_html.clone());
    htmls.push(qa_html.clone());
    let has_mermaid =
        !l.map_mermaid.is_empty() || htmls.iter().any(|h| h.contains("class=\"mermaid\""));
    let ctx = minijinja::context! {
        topic => l.topic.replace('`', ""),
        topic_html => safe(inline(&l.topic)),
        summary => safe(inline(&l.summary)),
        status => l.status.clone(),
        source => safe(escape(source.unwrap_or(""))),
        updated => l.updated.clone(),
        commit => l.commit.clone(),
        resume_question => safe(inline(&l.resume.question)),
        resume_hint => safe(inline(&l.resume.hint)),
        resume_hint_lens => l.resume.hint_lens.clone(),
        resume_hint_lens_label => w.lens(&l.resume.hint_lens),
        mental_model => l.mental_model.iter().map(|m| safe(inline(m))).collect::<Vec<_>>(),
        map_html => safe(map_html),
        mermaid => l.map_mermaid.clone(),
        entries => l.entries.iter().map(|e| entry_value(e, w)).collect::<Vec<_>>(),
        qa_html => safe(qa_html),
        has_mermaid => has_mermaid,
        open_questions => l.open_questions.iter().map(|q| safe(inline(q))).collect::<Vec<_>>(),
        glossary => glossary,
        phases => phases,
        default_tab => default_tab,
        lenses => lenses,
        count_questions => l.entries.len(),
        count_verified => verified,
        count_inferred => inferred,
        count_stale => stale,
        count_open => l.open_questions.len(),
        counts => w.counts(l.entries.len(), verified, inferred, stale),
        w => words_value(w),
    };
    let mut page = tmpl.render(ctx).context("rendering the ledger HTML")?;
    if !page.ends_with('\n') {
        page.push('\n');
    }
    Ok(page)
}
