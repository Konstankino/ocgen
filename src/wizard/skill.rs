//! `ocgen add skill` / `ocgen edit skill`: author a Claude Code skill, check it
//! against "what makes a skill good", and offer supporting-file stubs.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use console::style;
use dialoguer::theme::ColorfulTheme;

use ocgen::claude::Skill;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::validate::{self, unique_ident};

use super::{report_written, HELP_SKILL_BODY};
use crate::prompt::{ask, ask_confirm, ask_multi, ask_select, ask_v, edit_multiline, hint};
use crate::ui;

/// `ocgen add skill` — author a new Claude Code skill and re-scaffold.
pub fn run_add_skill(path_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let start = path_arg.unwrap_or_else(|| ".".to_string());
    let (target, mut project) = Project::discover(Path::new(&start))?;
    if project.target != Target::ClaudeCode {
        bail!("skills are a Claude Code feature — open a Claude project (`ocgen new --target claude`)");
    }
    println!(
        "Adding a skill to {}.\n",
        style(&project.project_name).bold()
    );

    // Step 0: is a skill the right tool at all?
    let kinds = vec![
        "A capability Claude uses when relevant, or a procedure I run by name → skill".to_string(),
        "Guidance Claude should follow in every session → a rule, not a skill".to_string(),
        "A worker with its own context and tools → a subagent, not a skill".to_string(),
    ];
    match ask_select(
        &theme,
        "What do you need?",
        "A skill costs almost nothing until it's needed: only its description sits in context.",
        &kinds,
        0,
    )? {
        1 => {
            ui::tip("put always-on guidance in CLAUDE.md (yours) or a .claude/rules/*.md file — it loads every session, no trigger needed");
            return Ok(());
        }
        2 => {
            ui::tip("run `ocgen add agent` — a subagent works in its own context, so its file reads don't fill yours");
            return Ok(());
        }
        _ => {}
    }

    let taken: Vec<String> = project.skills.iter().map(|s| s.name.clone()).collect();
    let presets = ocgen::claude::skill_presets()?;
    let mut labels: Vec<String> = presets
        .iter()
        .map(|p| {
            let when = if p.choose_when.is_empty() {
                &p.description
            } else {
                &p.choose_when
            };
            format!("{} — {}", p.name, when)
        })
        .collect();
    labels.push("blank — anything else; you fill every field".to_string());
    let idx = ask_select(
        &theme,
        "Start from preset",
        "Pick by when you'd use it; presets seed the tools, frontmatter and a numbered-step body.",
        &labels,
        0,
    )?;
    let seed = if idx < presets.len() {
        presets[idx].to_skill("", &project.language)
    } else {
        Skill {
            body: "Describe the steps this skill performs.\n".to_string(),
            ..Default::default()
        }
    };

    let skill = check_and_revise(
        &theme,
        configure_skill(&theme, seed, false, &taken)?,
        false,
        &taken,
    )?;
    let name = skill.name.clone();

    ui::section("Supporting files (optional)");
    hint("Keep SKILL.md short: detail in reference.md loads only when needed. ocgen never rewrites these.");
    let reference = ask_confirm(
        &theme,
        "  Create a reference.md stub?",
        "For checklists, schemas and examples SKILL.md points to.",
        false,
    )?;
    let scripts = ask_confirm(
        &theme,
        "  Create a scripts/ stub?",
        "For deterministic helpers SKILL.md runs, instead of re-deriving logic each time.",
        false,
    )?;

    project.skills.push(skill);
    let mut written = project.scaffold(&target, true)?;
    written.extend(Project::scaffold_skill_extras(
        &target, &name, reference, scripts,
    )?);
    report_written(&written);
    if let Some(skill) = project.skills.last() {
        report_file_issues(&target, skill);
    }
    skill_next_steps(&name);
    Ok(())
}

/// The rules for the skill's own files (reference files, `scripts/`). They're
/// reported, not re-prompted: the files are yours, and no prompt can fix them.
fn report_file_issues(target: &Path, skill: &Skill) {
    let dir = target.join(".claude/skills").join(&skill.name);
    let issues = ocgen::claude::skill_file_issues(&dir, &skill.body);
    if issues.is_empty() {
        return;
    }
    ui::section(&format!("Supporting-file checks ({})", issues.len()));
    for i in &issues {
        ui::warning(i);
    }
}

/// Run the "what makes a skill good" checks; while any fail, show them and offer
/// to revise (re-running the prompts seeded with the current values).
fn check_and_revise(
    theme: &ColorfulTheme,
    mut s: Skill,
    allow_rename: bool,
    taken: &[String],
) -> Result<Skill> {
    loop {
        let issues = ocgen::claude::skill_issues(&s);
        if issues.is_empty() {
            ui::success("skill checks passed");
            return Ok(s);
        }
        ui::section(&format!("Skill checks ({})", issues.len()));
        for i in &issues {
            ui::warning(i);
        }
        if !ask_confirm(
            theme,
            "Revise now?",
            "Re-walks the prompts with your current values (Enter keeps each).",
            true,
        )? {
            return Ok(s);
        }
        s = configure_skill(theme, s, allow_rename, taken)?;
    }
}

fn skill_next_steps(name: &str) {
    ui::tip("test that it triggers: in Claude Code, run the skill-creator skill on it — most skills that \"don't work\" never trigger");
    ui::tip("test it on each model you'll run it with: smaller models need explicit numbered steps, the strongest do worse when over-prescribed");
    ui::tip(&format!(
        "change it with `ocgen edit skill {name}` — `ocgen doctor` rewrites SKILL.md from saved state; reference.md and scripts/ are yours"
    ));
}

/// `ocgen edit skill` — modify an existing skill and re-scaffold.
pub fn run_edit_skill(path: String, name_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let (target, mut project) = Project::discover(Path::new(&path))?;
    if project.target != Target::ClaudeCode {
        bail!("skills are a Claude Code feature");
    }
    if project.skills.is_empty() {
        bail!("this project has no skills to edit (add one with `ocgen add skill`)");
    }
    let idx = match name_arg {
        Some(n) => project
            .skills
            .iter()
            .position(|s| s.name == n)
            .ok_or_else(|| {
                let have = project
                    .skills
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!("no skill named '{n}' (have: {have})")
            })?,
        None => {
            let labels: Vec<String> = project
                .skills
                .iter()
                .map(|s| format!("{} — {}", s.name, s.description))
                .collect();
            ask_select(&theme, "Which skill to edit?", "", &labels, 0)?
        }
    };
    let old_name = project.skills[idx].name.clone();
    println!(
        "\nEditing skill {} — Enter keeps each value.\n",
        style(&old_name).bold()
    );
    let taken: Vec<String> = project
        .skills
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, s)| s.name.clone())
        .collect();
    let edited = configure_skill(&theme, project.skills[idx].clone(), true, &taken)?;
    let edited = check_and_revise(&theme, edited, true, &taken)?;
    let new_name = edited.name.clone();
    project.skills[idx] = edited;
    let written = project.scaffold(&target, true)?;
    if new_name != old_name {
        // Carries reference.md / scripts/ over instead of deleting them.
        Project::rename_claude_skill_dir(&target, &old_name, &new_name)?;
        println!("Renamed skill {old_name} → {new_name}");
    }
    report_written(&written);
    report_file_issues(&target, &project.skills[idx]);
    skill_next_steps(&new_name);
    Ok(())
}

fn configure_skill(
    theme: &ColorfulTheme,
    mut s: Skill,
    allow_rename: bool,
    taken: &[String],
) -> Result<Skill> {
    if s.name.is_empty() || allow_rename {
        s.name = ask_v(
            theme,
            "  Skill name",
            "Becomes /skill-name and its folder: lowercase letters, digits, hyphens.",
            if s.name.is_empty() {
                None
            } else {
                Some(s.name.as_str())
            },
            {
                let unique = unique_ident(taken.to_vec(), "skill name");
                move |v: &str| validate::skill_name(v).and_then(|_| unique(v))
            },
        )?;
    }
    s.description = ask(
        theme,
        "  Description",
        "This is the trigger: Claude loads the skill from its description alone. Say what it does AND when (\"Use when …\"), in the words you'd type.",
        if s.description.is_empty() {
            None
        } else {
            Some(&s.description)
        },
        false,
    )?;
    s.when_to_use = ask(
        theme,
        "  When to use (extra trigger phrases; optional)",
        "More ways you'd ask for it, e.g. \"Use when asked to check a plan or bucket policy.\"",
        Some(&s.when_to_use),
        true,
    )?;
    s.allowed_tools = pick_tools(theme, &s.allowed_tools)?;
    s.argument_hint = ask(
        theme,
        "  Argument hint (optional)",
        "Shown in autocomplete, e.g. [issue] or [file] [format].",
        Some(&s.argument_hint),
        true,
    )?;
    s.disable_model_invocation = ask_confirm(
        theme,
        "  User-run only (disable auto-invocation)?",
        "Yes for anything with side effects (edits, commits, deploys): only /name starts it.",
        s.disable_model_invocation,
    )?;
    s.hidden_from_menu = ask_confirm(
        theme,
        "  Hide from the / menu (Claude-only knowledge)?",
        "Sets user-invocable: false — only Claude loads it, never you.",
        s.hidden_from_menu,
    )?;
    s.context_fork = ask_confirm(
        theme,
        "  Run in an isolated subagent (context: fork)?",
        "Yes for anything that reads a lot: the reading happens in a subagent and you get a summary.",
        s.context_fork,
    )?;
    if s.context_fork {
        s.agent = ask(
            theme,
            "  Fork into agent",
            "Subagent type, e.g. Explore / Plan / general-purpose.",
            Some(if s.agent.is_empty() {
                "Explore"
            } else {
                &s.agent
            }),
            true,
        )?;
        s.background = ask_confirm(
            theme,
            "  Run the fork in the background?",
            "You keep working while it runs; its summary arrives when done.",
            s.background,
        )?;
    } else {
        s.agent = String::new();
        s.background = false;
    }
    s.model = ask(
        theme,
        "  Model alias for this skill (optional)",
        "opus / sonnet / haiku / fable / inherit or a full ID; empty = session model. Pin the one you tested it on if it only works well there.",
        Some(&s.model),
        true,
    )?;
    let efforts: Vec<String> = std::iter::once("inherit".to_string())
        .chain(ocgen::claude::EFFORT_LEVELS.iter().map(|e| e.to_string()))
        .collect();
    let cur = ocgen::claude::EFFORT_LEVELS
        .iter()
        .position(|e| *e == s.effort)
        .map(|i| i + 1)
        .unwrap_or(0);
    let e = ask_select(
        theme,
        "  Effort for the skill's turn",
        "Higher thinks harder but costs more; inherit keeps the session's effort.",
        &efforts,
        cur,
    )?;
    s.effort = if e == 0 {
        String::new()
    } else {
        efforts[e].clone()
    };
    s.disallowed_tools = ask(
        theme,
        "  Disallowed tools while it runs (optional)",
        "e.g. Write, Edit to keep a review skill read-only.",
        Some(&s.disallowed_tools),
        true,
    )?;
    s.paths = ask(
        theme,
        "  Only activate for these files (optional)",
        "Space-separated globs, e.g. **/*.tf **/*.tfvars — the skill auto-loads only when Claude works on matching files.",
        Some(&s.paths),
        true,
    )?;
    s.arguments = ask(
        theme,
        "  Named arguments (optional)",
        "Space-separated names, e.g. plan_file env → use $plan_file and $env in the body.",
        Some(&s.arguments),
        true,
    )?;
    s.body = edit_multiline(theme, "Skill body", HELP_SKILL_BODY, &s.body)?;
    Ok(s)
}

/// Multi-select the built-in tools, then collect any Bash(...)/mcp__ patterns,
/// warning (not rejecting) on unrecognized names.
fn pick_tools(theme: &ColorfulTheme, current: &str) -> Result<String> {
    let tools = ocgen::claude::CLAUDE_TOOLS;
    let selected_now: Vec<String> = current
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    let defaults: Vec<bool> = tools
        .iter()
        .map(|t| selected_now.iter().any(|x| x == t))
        .collect();
    let chosen = ask_multi(
        theme,
        "  Allowed tools (pre-approved during the skill's turn)",
        "Grant the fewest tools that work — these run without asking. Space toggles, Enter confirms; scope shell access as Bash(cmd:*) at the next prompt.",
        &tools.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
        &defaults,
    )?;
    let mut all: Vec<String> = chosen.iter().map(|&i| tools[i].to_string()).collect();
    let custom_seed = selected_now
        .iter()
        .filter(|x| !tools.contains(&x.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let custom = ask(
        theme,
        "  Extra tool patterns (comma-separated; empty = none)",
        "e.g. Bash(git commit:*), mcp__github__create_pr",
        Some(&custom_seed),
        true,
    )?;
    for c in custom
        .split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
    {
        all.push(c.to_string());
    }
    let joined = all.join(", ");
    let unknown = ocgen::claude::unknown_tools(&joined);
    if !unknown.is_empty() {
        println!(
            "  {} unrecognized: {} (kept — fine for MCP/custom, but check for typos)",
            style("warning:").yellow(),
            unknown.join(", ")
        );
    }
    Ok(joined)
}
