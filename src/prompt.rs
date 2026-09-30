//! Prompt helpers for the wizard. Each prints its help line first.
//!
//! Every interactive question goes through these functions, so the wizard can be
//! driven without a terminal: a test loads a script of answers with [`script`],
//! and each helper consumes the next answer instead of prompting. Validators still
//! run on scripted answers, so a test can also assert that bad input is rejected.
//!
//! Scripted answer forms: `""` = accept the default (Enter), text = type it;
//! selects take an index or part of the item's label; confirms take `y`/`n`;
//! multi-selects take `""` (keep defaults), `none`, or comma-separated indices or
//! labels; optional fields take `-` to clear.

use std::cell::RefCell;
use std::collections::VecDeque;

use anyhow::{anyhow, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Editor, Input, MultiSelect, Select};

use ocgen::validate;

use crate::ui;

thread_local! {
    static SCRIPT: RefCell<Option<VecDeque<String>>> = const { RefCell::new(None) };
}

/// Drive the wizard from `answers` (tests only), in order.
#[cfg(test)]
pub(crate) fn script(answers: &[&str]) {
    SCRIPT.with(|s| *s.borrow_mut() = Some(answers.iter().map(|a| a.to_string()).collect()));
}

/// Scripted answers not consumed yet (tests assert a flow used exactly its script).
#[cfg(test)]
pub(crate) fn script_remaining() -> usize {
    SCRIPT.with(|s| s.borrow().as_ref().map_or(0, |q| q.len()))
}

/// Whether prompts are being answered — from a terminal, or from a test script.
/// When neither (e.g. piped or in CI), callers skip optional confirmations.
pub(crate) fn can_ask() -> bool {
    use std::io::IsTerminal;
    SCRIPT.with(|s| s.borrow().is_some()) || std::io::stdin().is_terminal()
}

/// The next scripted answer, or `None` when prompting interactively.
fn next(prompt: &str) -> Option<String> {
    SCRIPT.with(|s| {
        s.borrow_mut().as_mut().map(|q| {
            q.pop_front().unwrap_or_else(|| {
                panic!("scripted wizard ran out of answers at: {}", prompt.trim())
            })
        })
    })
}

pub(crate) fn hint(help: &str) {
    if !help.is_empty() {
        println!("  {} {}", style("›").dim(), style(help).dim());
    }
}

pub(crate) fn ask(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    default: Option<&str>,
    allow_empty: bool,
) -> Result<String> {
    hint(help);
    if let Some(a) = next(prompt) {
        let v = if a.is_empty() {
            default.unwrap_or("").to_string()
        } else {
            a
        };
        if v.is_empty() && !allow_empty {
            return Err(anyhow!("{}: a value is required", prompt.trim()));
        }
        return Ok(v);
    }
    let mut b = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(allow_empty);
    if let Some(d) = default {
        b = b.default(d.to_string());
    }
    Ok(b.interact_text()?)
}

/// Required input that re-prompts until `validator` accepts it.
pub(crate) fn ask_v(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    default: Option<&str>,
    validator: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String> {
    hint(help);
    if let Some(a) = next(prompt) {
        let v = if a.is_empty() {
            default.unwrap_or("").to_string()
        } else {
            a
        };
        let v = v.trim().to_string();
        validator(&v).map_err(|e| anyhow!("{}: {e}", prompt.trim()))?;
        return Ok(v);
    }
    let mut b = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .validate_with(move |s: &String| validator(s.trim()));
    if let Some(d) = default {
        b = b.default(d.to_string());
    }
    Ok(b.interact_text()?.trim().to_string())
}

/// Optional editable input, pre-filled with the current value. Enter keeps it,
/// editing changes it, and empty or `-` clears it. Non-empty values are validated.
pub(crate) fn ask_optional_v(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    current: &str,
    validator: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String> {
    hint(help);
    if let Some(a) = next(prompt) {
        let v = match a.trim() {
            "" => current.trim().to_string(),
            "-" => String::new(),
            t => t.to_string(),
        };
        if !v.is_empty() {
            validator(&v).map_err(|e| anyhow!("{}: {e}", prompt.trim()))?;
        }
        return Ok(v);
    }
    let mut input = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .validate_with(move |s: &String| {
            let t = s.trim();
            if t.is_empty() || t == "-" {
                Ok(())
            } else {
                validator(t)
            }
        });
    if !current.is_empty() {
        input = input.with_initial_text(current.to_string());
    }
    let value = input.interact_text()?;
    Ok(if value.trim() == "-" {
        String::new()
    } else {
        value.trim().to_string()
    })
}

/// Resolve a scripted choice — an index or part of an item's label — to an index.
fn pick(prompt: &str, items: &[String], answer: &str) -> Result<usize> {
    if let Ok(i) = answer.trim().parse::<usize>() {
        if i < items.len() {
            return Ok(i);
        }
    }
    let needle = answer.trim().to_lowercase();
    items
        .iter()
        .position(|it| {
            console::strip_ansi_codes(it)
                .to_lowercase()
                .contains(&needle)
        })
        .ok_or_else(|| anyhow!("{}: no choice matches '{answer}'", prompt.trim()))
}

pub(crate) fn ask_select(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    items: &[String],
    default_idx: usize,
) -> Result<usize> {
    hint(help);
    let default_idx = default_idx.min(items.len().saturating_sub(1));
    if let Some(a) = next(prompt) {
        return if a.is_empty() {
            Ok(default_idx)
        } else {
            pick(prompt, items, &a)
        };
    }
    Ok(Select::with_theme(theme)
        .with_prompt(prompt)
        .items(items)
        .default(default_idx)
        .interact()?)
}

pub(crate) fn ask_confirm(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    default: bool,
) -> Result<bool> {
    hint(help);
    if let Some(a) = next(prompt) {
        return match a.trim().to_lowercase().as_str() {
            "" => Ok(default),
            "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            other => Err(anyhow!("{}: answer y or n, not '{other}'", prompt.trim())),
        };
    }
    Ok(Confirm::with_theme(theme)
        .with_prompt(prompt)
        .default(default)
        .interact()?)
}

/// Multi-select: returns the chosen indices.
pub(crate) fn ask_multi(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    items: &[String],
    defaults: &[bool],
) -> Result<Vec<usize>> {
    hint(help);
    if let Some(a) = next(prompt) {
        return match a.trim() {
            "" => Ok((0..items.len())
                .filter(|&i| defaults.get(i) == Some(&true))
                .collect()),
            "none" => Ok(Vec::new()),
            list => list.split(',').map(|x| pick(prompt, items, x)).collect(),
        };
    }
    Ok(MultiSelect::with_theme(theme)
        .with_prompt(prompt)
        .items(items)
        .defaults(defaults)
        .interact()?)
}

/// Pick an agent colour from the OpenCode theme palette (each shown with a swatch
/// of how it looks), or choose a custom hex colour.
pub(crate) fn ask_color(theme: &ColorfulTheme, help: &str, current: &str) -> Result<String> {
    let mut labels: Vec<String> = ui::THEME_COLORS
        .iter()
        .map(|(name, _)| {
            let swatch = ui::color_swatch(name).unwrap_or_default();
            format!("{swatch} {name}")
        })
        .collect();
    let custom_label = format!("{} custom hex…", ui::muted("◇"));
    labels.push(custom_label);

    let default_idx = ui::THEME_COLORS
        .iter()
        .position(|(name, _)| *name == current)
        .unwrap_or(ui::THEME_COLORS.len()); // last item = custom

    let idx = ask_select(theme, "  Color", help, &labels, default_idx)?;

    if idx < ui::THEME_COLORS.len() {
        Ok(ui::THEME_COLORS[idx].0.to_string())
    } else {
        let seed = if current.starts_with('#') {
            current
        } else {
            "#4ec9b0"
        };
        ask_v(
            theme,
            "    Hex colour (rendered as ■ in OpenCode)",
            "",
            Some(seed),
            validate::hex,
        )
    }
}

/// Hybrid multi-line edit: show the seeded default, and open $EDITOR only if asked.
/// Scripted: `""` keeps the seed; any other answer replaces it.
pub(crate) fn edit_multiline(
    theme: &ColorfulTheme,
    label: &str,
    help: &str,
    seeded: &str,
) -> Result<String> {
    hint(help);
    println!("  {} (current default):", style(label).bold());
    for line in seeded.lines() {
        println!("    {}", style(line).dim());
    }
    if let Some(a) = next(label) {
        return Ok(if a.is_empty() { seeded.to_string() } else { a });
    }
    if ask_confirm(theme, &format!("Edit {label} in $EDITOR?"), "", false)? {
        if let Some(edited) = Editor::new().edit(seeded)? {
            return Ok(edited);
        }
    }
    Ok(seeded.to_string())
}
