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
use crate::templates;

pub const TEMPLATE: &str = "claude/notes/ledger.html.j2";

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
/// tables and fenced code (a `mermaid` fence becomes a diagram).
pub fn blocks(lines: &[String]) -> String {
    let mut out = String::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Option<(&str, Vec<String>)> = None;
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut fence: Option<(String, Vec<String>)> = None;

    fn flush(
        out: &mut String,
        para: &mut Vec<String>,
        list: &mut Option<(&str, Vec<String>)>,
        table: &mut Vec<Vec<String>>,
    ) {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline(&para.join(" "))));
            para.clear();
        }
        if let Some((tag, items)) = list.take() {
            out.push_str(&format!("<{tag}>"));
            for i in items {
                out.push_str(&format!("<li>{}</li>", inline(&i)));
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        if !table.is_empty() {
            out.push_str("<table>");
            for (r, row) in table.iter().enumerate() {
                let cell = if r == 0 { "th" } else { "td" };
                out.push_str("<tr>");
                for c in row {
                    out.push_str(&format!("<{cell}>{}</{cell}>", inline(c)));
                }
                out.push_str("</tr>");
            }
            out.push_str("</table>\n");
            table.clear();
        }
    }

    for line in lines {
        let t = line.trim();
        if let Some((lang, body)) = &mut fence {
            if t.starts_with("```") {
                if lang == "mermaid" {
                    out.push_str(&format!(
                        "<pre class=\"mermaid\">{}</pre>\n",
                        escape(&body.join("\n"))
                    ));
                } else {
                    out.push_str(&format!(
                        "<pre><code>{}</code></pre>\n",
                        escape(&body.join("\n"))
                    ));
                }
                fence = None;
            } else {
                body.push(line.clone());
            }
            continue;
        }
        if let Some(lang) = t.strip_prefix("```") {
            flush(&mut out, &mut para, &mut list, &mut table);
            fence = Some((lang.trim().to_string(), Vec::new()));
            continue;
        }
        if t.is_empty() {
            flush(&mut out, &mut para, &mut list, &mut table);
            continue;
        }
        if t.starts_with('|') {
            if !para.is_empty() || list.is_some() {
                flush(&mut out, &mut para, &mut list, &mut table);
            }
            let cells: Vec<String> = t
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            let rule = cells
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| "-: ".contains(ch)));
            if !rule {
                table.push(cells);
            }
            continue;
        }
        if let Some(h) = t.strip_prefix('#') {
            flush(&mut out, &mut para, &mut list, &mut table);
            out.push_str(&format!(
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
                if !para.is_empty()
                    || !table.is_empty()
                    || list.as_ref().is_some_and(|l| l.0 != tag)
                {
                    flush(&mut out, &mut para, &mut list, &mut table);
                }
                list.get_or_insert((tag, Vec::new()))
                    .1
                    .push(text.to_string());
            }
            None if list.is_some() && line.starts_with([' ', '\t']) => {
                // A continuation of the previous list item.
                if let Some(last) = list.as_mut().and_then(|l| l.1.last_mut()) {
                    last.push(' ');
                    last.push_str(t);
                }
            }
            None => {
                if list.is_some() || !table.is_empty() {
                    flush(&mut out, &mut para, &mut list, &mut table);
                }
                para.push(t.to_string());
            }
        }
    }
    if let Some((_, body)) = fence {
        // An unclosed fence: show what there is.
        out.push_str(&format!(
            "<pre><code>{}</code></pre>\n",
            escape(&body.join("\n"))
        ));
    }
    flush(&mut out, &mut para, &mut list, &mut table);
    out
}

fn safe(s: String) -> Value {
    Value::from_safe_string(s)
}

/// A short, stable id for a topic-local anchor.
fn anchor(n: u32) -> String {
    format!("q{n}")
}

fn entry_value(e: &Entry) -> Value {
    let (evidence, confidence) = match e.evidence {
        Evidence::Verified => ("verified", String::new()),
        Evidence::Inferred(c) => ("inferred", c.map(|c| format!("{c}%")).unwrap_or_default()),
        Evidence::Unknown => ("", String::new()),
    };
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("n", Value::from(e.n));
    m.insert("id", Value::from(anchor(e.n)));
    m.insert("lens", Value::from(e.lens.clone()));
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
    m.insert(
        "notes",
        Value::from(e.notes.iter().map(|n| safe(inline(n))).collect::<Vec<_>>()),
    );
    m.insert("extra", safe(blocks(&e.extra)));
    Value::from(m)
}

/// The page for `ledger`.
pub fn render(ledger: &Ledger) -> Result<String> {
    let src = templates::load(TEMPLATE)?;
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
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
            m.insert("name", Value::from(name.clone()));
            m.insert(
                "class",
                Value::from(name.to_ascii_lowercase().replace(' ', "-")),
            );
            m.insert("count", Value::from(*count));
            m.insert("pct", Value::from(count * 100 / total));
            Value::from(m)
        })
        .collect();
    let extra: Vec<Value> = l
        .extra
        .iter()
        .map(|s| {
            let mut m: BTreeMap<&str, Value> = BTreeMap::new();
            m.insert("title", safe(inline(&s.title)));
            m.insert("html", safe(blocks(&s.lines)));
            Value::from(m)
        })
        .collect();
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

    let map_html = blocks(&l.map_text);
    let qa_html = blocks(&l.qa_text);
    let has_mermaid = !l.map_mermaid.is_empty()
        || [&map_html, &qa_html]
            .iter()
            .any(|h| h.contains("class=\"mermaid\""))
        || extra.iter().any(|s| {
            s.get_attr("html")
                .is_ok_and(|h| h.to_string().contains("class=\"mermaid\""))
        });
    let ctx = minijinja::context! {
        topic => l.topic.clone(),
        updated => l.updated.clone(),
        commit => l.commit.clone(),
        resume_question => safe(inline(&l.resume.question)),
        resume_hint => safe(inline(&l.resume.hint)),
        resume_hint_lens => l.resume.hint_lens.clone(),
        mental_model => l.mental_model.iter().map(|m| safe(inline(m))).collect::<Vec<_>>(),
        map_html => safe(map_html),
        mermaid => l.map_mermaid.clone(),
        entries => l.entries.iter().map(entry_value).collect::<Vec<_>>(),
        qa_html => safe(qa_html),
        has_mermaid => has_mermaid,
        open_questions => l.open_questions.iter().map(|q| safe(inline(q))).collect::<Vec<_>>(),
        glossary => glossary,
        extra => extra,
        lenses => lenses,
        count_questions => l.entries.len(),
        count_verified => verified,
        count_inferred => inferred,
        count_stale => stale,
        count_open => l.open_questions.len(),
    };
    let mut page = tmpl.render(ctx).context("rendering the ledger HTML")?;
    if !page.ends_with('\n') {
        page.push('\n');
    }
    Ok(page)
}
