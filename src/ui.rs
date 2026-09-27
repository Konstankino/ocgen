//! Shared terminal styling: colours and box-drawing helpers so every command's
//! output looks consistent. Uses `console`, which auto-disables ANSI when the
//! output is not a terminal (and honours `NO_COLOR`).

use console::{measure_text_width, style, Style};

/// Dim/secondary text, used throughout for hints and de-emphasised values.
pub fn muted(s: &str) -> String {
    style(s).dim().to_string()
}

/// OpenCode named theme colours, each mapped to an approximate xterm-256 colour
/// so we can render a representative swatch in the terminal.
pub const THEME_COLORS: &[(&str, u8)] = &[
    ("primary", 170),
    ("secondary", 245),
    ("accent", 44),
    ("success", 42),
    ("warning", 214),
    ("error", 196),
    ("info", 39),
];

/// Approximate an RGB colour with the xterm-256 colour cube.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let q = |v: u8| -> u16 { (v as u16 * 5 + 127) / 255 };
    (16 + 36 * q(r) + 6 * q(g) + q(b)) as u8
}

/// A filled swatch (`■`) coloured to represent `color` — a `#RGB`/`#RRGGBB` hex or
/// a named theme colour. Returns `None` when the colour isn't recognised.
pub fn color_swatch(color: &str) -> Option<String> {
    if let Some(hex) = color.strip_prefix('#') {
        let expanded = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => hex.to_string(),
            _ => return None,
        };
        let rgb = u32::from_str_radix(&expanded, 16).ok()?;
        let code = rgb_to_256((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
        return Some(style("■").color256(code).to_string());
    }
    THEME_COLORS
        .iter()
        .find(|(name, _)| *name == color)
        .map(|(_, code)| style("■").color256(*code).to_string())
}

/// A styled wordmark + subtitle, shown at the top of the wizard.
pub fn banner(subtitle: &str) {
    println!();
    println!("  {} {}", style("◆").cyan().bold(), style("ocgen").bold());
    println!("  {}", muted(subtitle));
    println!("  {}", muted(&"╌".repeat(subtitle.len().clamp(12, 48))));
}

/// A section header: a coloured bar and a bold title.
pub fn section(title: &str) {
    println!();
    println!("{} {}", style("▍").cyan().bold(), style(title).bold());
}

/// A dim key/value line, e.g. `label: value`.
pub fn kv(label: &str, value: &str) {
    println!("  {} {}", muted(&format!("{label}:")), value);
}

/// A green success line with a check mark.
pub fn success(msg: &str) {
    println!("{} {}", style("✔").green().bold(), msg);
}

/// A yellow warning line.
pub fn warning(msg: &str) {
    println!("  {} {}", style("▲").yellow().bold(), msg);
}

/// A dim tip line prefixed with a lightbulb.
pub fn tip(msg: &str) {
    println!("\n  {} {}", style("✦").cyan(), muted(msg));
}

/// Print a list of written files as a dim tree.
pub fn file_tree(paths: &[std::path::PathBuf]) {
    success(&format!("Wrote {} file(s)", paths.len()));
    for (i, p) in paths.iter().enumerate() {
        let branch = if i + 1 == paths.len() {
            "╰─"
        } else {
            "├─"
        };
        println!("  {} {}", muted(branch), muted(&p.display().to_string()));
    }
}

/// A box-drawing table. Cells may already contain ANSI styling — widths are
/// measured with `measure_text_width` so colours don't break alignment.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let cols = headers.len();
    let mut w = vec![0usize; cols];
    for (i, h) in headers.iter().enumerate() {
        w[i] = measure_text_width(h);
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < cols {
                w[i] = w[i].max(measure_text_width(cell));
            }
        }
    }

    let border = Style::new().dim();
    let rule = |left: &str, mid: &str, right: &str| {
        let mut s = String::new();
        for (i, width) in w.iter().enumerate() {
            s.push_str(if i == 0 { left } else { mid });
            s.push_str(&"─".repeat(width + 2));
        }
        s.push_str(right);
        border.apply_to(s).to_string()
    };

    let bar = border.apply_to("│").to_string();
    let print_row = |cells: Vec<String>| {
        let mut line = bar.clone();
        for (i, cell) in cells.iter().enumerate() {
            line.push_str(&format!(" {} {}", pad(cell, w[i]), bar));
        }
        println!("{line}");
    };

    println!("{}", rule("╭", "┬", "╮"));
    print_row(
        headers
            .iter()
            .map(|h| style(h.to_string()).bold().cyan().to_string())
            .collect(),
    );
    println!("{}", rule("├", "┼", "┤"));
    for row in rows {
        print_row(row.clone());
    }
    println!("{}", rule("╰", "┴", "╯"));
}

/// Right-pad `s` to display width `w`, accounting for any ANSI escapes it holds.
fn pad(s: &str, w: usize) -> String {
    let len = measure_text_width(s);
    if len >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - len))
    }
}

/// First line of `s`, truncated to `max` display columns with an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > max {
        let t: String = first.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_256_endpoints() {
        assert_eq!(rgb_to_256(0, 0, 0), 16); // black
        assert_eq!(rgb_to_256(255, 255, 255), 231); // white
    }

    #[test]
    fn swatch_recognises_hex_and_names() {
        assert!(color_swatch("#4ec9b0").is_some());
        assert!(color_swatch("#abc").is_some()); // short hex expands
        assert!(color_swatch("accent").is_some()); // theme name
        assert!(color_swatch("warning").is_some());
        assert!(color_swatch("#zz").is_none()); // bad hex
        assert!(color_swatch("not-a-colour").is_none());
    }

    #[test]
    fn pad_and_truncate() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcde", 3), "abcde"); // never shrinks
        assert_eq!(truncate("hello world", 20), "hello world");
        assert_eq!(truncate("line one\nline two", 20), "line one"); // first line only
        assert!(truncate("a very long description here", 10).ends_with('…'));
    }
}
