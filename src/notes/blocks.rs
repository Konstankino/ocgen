//! The report's visual blocks: fenced blocks in a ledger phase whose lines are
//! `|`-separated cells, so the Markdown stays readable on its own.
//!
//! | Block     | Line                                         | Draws                         |
//! |-----------|----------------------------------------------|-------------------------------|
//! | `stats`   | `value \| caption`                           | big-number tiles              |
//! | `claims`  | `label \| claim \| detail`                   | evidence-tagged findings      |
//! | `steps`   | `n [#color] \| title \| detail [\| ref]`     | a numbered sequence           |
//! | `bars`    | `label \| number [\| hover detail]`          | horizontal bar chart          |
//! | `cards`   | `title \| body`                              | small side-by-side verdicts   |
//! | `compare` | `title [#color] \| subtitle`, then `key \| value`; `---` starts the next column | columns with arrows |
//! | `stack`   | `title [#color] \| subtitle`, then `layer [!\|~] \| detail [\| tag]`; `---` next column | layered stores |
//!
//! A `legend: #blue Setup · #orange Build` line adds a colour legend to any
//! block. Labels and titles take an optional `#color` (blue, orange, green,
//! red, purple, teal, yellow, gray). Everything is escaped through
//! [`inline`]/[`escape`].

use super::html::{escape, inline};
use super::ledger::Evidence;

const COLORS: [&str; 8] = [
    "blue", "orange", "green", "red", "purple", "teal", "yellow", "gray",
];

/// Split a line into cells on `|`, except inside `code`.
pub fn cells(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut in_code = false;
    for c in line.chars() {
        match c {
            '`' => {
                in_code = !in_code;
                out.last_mut().unwrap().push(c);
            }
            '|' if !in_code => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|c| c.trim().to_string()).collect()
}

/// `text #color` → (`text`, Some(color)) for a known colour.
fn split_color(cell: &str) -> (String, Option<&'static str>) {
    let t = cell.trim();
    if let Some((head, tag)) = t.rsplit_once('#') {
        let tag = tag.trim().to_ascii_lowercase();
        if let Some(c) = COLORS.iter().copied().find(|c| *c == tag) {
            if head.is_empty() || head.ends_with(char::is_whitespace) {
                return (head.trim().to_string(), Some(c));
            }
        }
    }
    (t.to_string(), None)
}

fn color_class(c: Option<&str>) -> String {
    c.map(|c| format!(" c-{c}")).unwrap_or_default()
}

/// A trailing `!` (highlight) or `~` (dashed / not stored) marker.
fn split_marker(cell: &str) -> (String, &'static str) {
    let t = cell.trim();
    if let Some(rest) = t.strip_suffix('!') {
        (rest.trim().to_string(), " highlight")
    } else if let Some(rest) = t.strip_suffix('~') {
        (rest.trim().to_string(), " dashed")
    } else {
        (t.to_string(), "")
    }
}

fn cell(cells: &[String], i: usize) -> &str {
    cells.get(i).map(String::as_str).unwrap_or("")
}

/// `legend: #blue Setup · #orange dbt build` → the legend's HTML.
fn legend(spec: &str) -> String {
    let mut out = String::from(r#"<div class="legend">"#);
    for item in spec.split('·') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (color, label) = match item.strip_prefix('#') {
            Some(rest) => match rest.split_once(char::is_whitespace) {
                Some((c, label)) => (
                    COLORS.iter().copied().find(|k| k.eq_ignore_ascii_case(c)),
                    label.trim(),
                ),
                None => (None, item),
            },
            None => (None, item),
        };
        out.push_str(&format!(
            r#"<span class="legend-item"><span class="swatch{}"></span>{}</span>"#,
            color_class(color),
            inline(label)
        ));
    }
    out.push_str("</div>");
    out
}

/// The block's lines minus blanks, with a `legend:` line split off.
fn split_legend(lines: &[String]) -> (String, Vec<String>) {
    let mut legend_html = String::new();
    let mut rest = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        match t
            .split_once(':')
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case("legend"))
        {
            Some((_, spec)) => legend_html = legend(spec),
            None => rest.push(t.to_string()),
        }
    }
    (legend_html, rest)
}

/// Render a fenced block of a known `kind`; `None` for any other kind.
pub fn render(kind: &str, lines: &[String]) -> Option<String> {
    let (legend_html, rows) = split_legend(lines);
    let body = match kind {
        "stats" => stats(&rows),
        "claims" => claims(&rows),
        "steps" => steps(&rows),
        "bars" => bars(&rows),
        "cards" => cards(&rows),
        "compare" => compare(&rows),
        "stack" => stack(&rows),
        _ => return None,
    };
    Some(format!("{legend_html}{body}\n"))
}

fn stats(rows: &[String]) -> String {
    let mut out = String::from(r#"<div class="stats">"#);
    for r in rows {
        let c = cells(r);
        out.push_str(&format!(
            r#"<div class="stat"><div class="stat-value">{}</div><div class="stat-label">{}</div></div>"#,
            inline(cell(&c, 0)),
            inline(cell(&c, 1))
        ));
    }
    out.push_str("</div>");
    out
}

/// The badge class for a claim label: evidence, a colour, or neutral.
fn badge_class(label: &str, color: Option<&str>) -> String {
    if let Some(c) = color {
        return format!("c-{c}");
    }
    let lower = label.to_ascii_lowercase();
    if lower.contains("correct") {
        return "ev-corrected".into();
    }
    if lower.contains("stale") {
        return "ev-stale".into();
    }
    match super::ledger::evidence_of(label) {
        Some(Evidence::Verified) => "ev-verified".into(),
        Some(Evidence::Inferred(_)) => "ev-inferred".into(),
        _ => "ev-other".into(),
    }
}

fn claims(rows: &[String]) -> String {
    let mut out = String::from(r#"<ul class="claims">"#);
    for r in rows {
        let c = cells(r);
        let (label, color) = split_color(cell(&c, 0));
        out.push_str(&format!(
            r#"<li><span class="badge {}">{}</span><div class="claim"><div class="claim-title">{}</div>"#,
            badge_class(&label, color),
            escape(&label),
            inline(cell(&c, 1))
        ));
        let detail = c.get(2..).map(|d| d.join(" | ")).unwrap_or_default();
        if !detail.is_empty() {
            out.push_str(&format!(
                r#"<div class="claim-detail">{}</div>"#,
                inline(&detail)
            ));
        }
        out.push_str("</div></li>");
    }
    out.push_str("</ul>");
    out
}

fn steps(rows: &[String]) -> String {
    let mut out = String::from(r#"<ol class="steps">"#);
    for r in rows {
        let c = cells(r);
        let (n, color) = split_color(cell(&c, 0));
        out.push_str(&format!(
            r#"<li><span class="step-n{}">{}</span><div class="step-body"><div class="step-title">{}</div>"#,
            color_class(Some(color.unwrap_or("blue"))),
            escape(&n),
            inline(cell(&c, 1))
        ));
        if !cell(&c, 2).is_empty() {
            out.push_str(&format!(
                r#"<div class="step-detail">{}</div>"#,
                inline(cell(&c, 2))
            ));
        }
        out.push_str("</div>");
        if !cell(&c, 3).is_empty() {
            out.push_str(&format!(
                r#"<span class="step-ref">{}</span>"#,
                escape(cell(&c, 3))
            ));
        }
        out.push_str("</li>");
    }
    out.push_str("</ol>");
    out
}

fn number(s: &str) -> Option<f64> {
    s.replace([',', '_', ' '], "")
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
}

fn bars(rows: &[String]) -> String {
    let parsed: Vec<Vec<String>> = rows.iter().map(|r| cells(r)).collect();
    let max = parsed
        .iter()
        .filter_map(|c| number(cell(c, 1)))
        .fold(0.0_f64, f64::max);
    let mut out = String::from(r#"<div class="bars">"#);
    for c in &parsed {
        let (label, color) = split_color(cell(c, 0));
        let pct = match number(cell(c, 1)) {
            Some(n) if max > 0.0 && n > 0.0 => ((n / max) * 100.0).round().max(1.0) as u32,
            _ => 0,
        };
        let title = match cell(c, 2) {
            "" => String::new(),
            d => format!(r#" title="{}""#, escape(d)),
        };
        out.push_str(&format!(
            r#"<div class="bar-row"{title}><span class="bar-label">{}</span><span class="bar-track"><span class="bar-fill{}" style="width: {pct}%"></span></span><span class="bar-value">{}</span></div>"#,
            inline(&label),
            color_class(color),
            escape(cell(c, 1))
        ));
    }
    out.push_str("</div>");
    out
}

fn cards(rows: &[String]) -> String {
    let mut out = String::from(r#"<div class="mini-cards">"#);
    for r in rows {
        let c = cells(r);
        let (title, color) = split_color(cell(&c, 0));
        out.push_str(&format!(
            r#"<div class="mini-card{}"><h4>{}</h4><p>{}</p></div>"#,
            color_class(color),
            inline(&title),
            inline(&c.get(1..).map(|d| d.join(" | ")).unwrap_or_default())
        ));
    }
    out.push_str("</div>");
    out
}

/// Rows split into columns at `---`.
fn columns(rows: &[String]) -> Vec<&[String]> {
    rows.split(|r| r.trim().chars().all(|c| c == '-') && r.trim().len() >= 3)
        .filter(|c| !c.is_empty())
        .collect()
}

fn compare(rows: &[String]) -> String {
    let mut out = String::from(r#"<div class="compare">"#);
    for (i, col) in columns(rows).into_iter().enumerate() {
        if i > 0 {
            out.push_str(r#"<span class="arrow" aria-hidden="true">→</span>"#);
        }
        let head = cells(&col[0]);
        let (title, color) = split_color(cell(&head, 0));
        out.push_str(&format!(
            r#"<div class="col{}"><div class="col-title">{}</div><div class="col-sub">{}</div><dl>"#,
            color_class(color),
            inline(&title),
            inline(cell(&head, 1))
        ));
        for r in &col[1..] {
            let c = cells(r);
            let key = cell(&c, 0);
            let (key, changed) = match key.strip_prefix("**").and_then(|k| k.strip_suffix("**")) {
                Some(k) => (k, r#" class="changed""#),
                None => (key, ""),
            };
            out.push_str(&format!(
                "<dt>{}</dt><dd{changed}>{}</dd>",
                inline(key),
                inline(&c.get(1..).map(|d| d.join(" | ")).unwrap_or_default())
            ));
        }
        out.push_str("</dl></div>");
    }
    out.push_str("</div>");
    out
}

fn stack(rows: &[String]) -> String {
    let mut out = String::from(r#"<div class="stack">"#);
    for col in columns(rows) {
        let head = cells(&col[0]);
        let (title, color) = split_color(cell(&head, 0));
        out.push_str(&format!(
            r#"<div class="stack-col{}"><div class="stack-title">{}</div><div class="stack-sub">{}</div>"#,
            color_class(color),
            inline(&title),
            inline(cell(&head, 1))
        ));
        for r in &col[1..] {
            let c = cells(r);
            let (name, marker) = split_marker(cell(&c, 0));
            out.push_str(&format!(
                r#"<div class="layer{marker}"><div class="layer-text"><div class="layer-title">{}</div>"#,
                inline(&name)
            ));
            if !cell(&c, 1).is_empty() {
                out.push_str(&format!(
                    r#"<div class="layer-detail">{}</div>"#,
                    inline(cell(&c, 1))
                ));
            }
            out.push_str("</div>");
            if !cell(&c, 2).is_empty() {
                out.push_str(&format!(
                    r#"<span class="tag">{}</span>"#,
                    escape(cell(&c, 2))
                ));
            }
            out.push_str("</div>");
        }
        out.push_str("</div>");
    }
    out.push_str("</div>");
    out
}
