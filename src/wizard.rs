//! The interactive wizard — the whole user-facing experience.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Editor, Input, MultiSelect, Select};

use ocgen::agent::{self, Agent};
use ocgen::claude::{Output, Powerups, Skill, Team};
use ocgen::manifest::{Manifest, Model, Provider};
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;
use ocgen::validate::{self, unique_ident};

use crate::cli::{OutputArg, TargetArg, TeamCli};
use crate::ui;

// Per-field help, shown dimmed above each prompt so the user is reminded what it means.
const HELP_NAME: &str = "Identifier → file name and @mention. Lowercase, no spaces (e.g. editor).";
const HELP_PRESET: &str = "A starting point that fills the fields below; 'blank' starts empty.";
const HELP_ROLE: &str =
    "Free-text job label (e.g. reviewer). Informational; not written to the file.";
const HELP_MODE: &str =
    "primary = invoked directly & delegates; subagent = only called by others; all = both.";
const HELP_TOPP: &str =
    "Nucleus sampling 0.0–1.0; alternative to temperature. Enter keeps, '-' clears.";
const HELP_VARIANT: &str =
    "Model variant when the provider offers one (e.g. thinking). Enter keeps, '-' clears.";
const HELP_DISABLE: &str = "Turn the agent off without deleting it (it won't be loaded).";
const HELP_HIDDEN: &str = "Hide this subagent from the @ autocomplete menu (subagents only).";
const HELP_OPTIONS: &str =
    "Provider-specific model options as YAML (e.g. reasoningEffort: high). Blank = none.";
const HELP_PROVIDER: &str =
    "Which provider hosts this model → reference becomes <provider>/<model>.";
const HELP_MODEL: &str = "The model id to run on, from the chosen provider.";
const HELP_TEMP: &str =
    "Randomness 0.0–2.0: lower = focused/repeatable, higher = creative. Coding ~0.2.";
const HELP_STEPS: &str =
    "Cap on tool-call cycles (safety limit). Enter keeps, '-' = unlimited. e.g. 30.";
const HELP_COLOR: &str = "Cosmetic UI accent: a name (accent, warning) or a hex like #4ec9b0.";
const HELP_DESC: &str =
    "One-line summary; shown in lists and used by the coordinator to route work.";
const HELP_PERMS: &str =
    "What it may do (YAML): edit, bash rules, webfetch. Primary agents also get a task block.";
const HELP_BODY: &str = "The agent's system prompt/persona; $ARGUMENTS = the user's task.";
const HELP_PROMPTFILE: &str = "Keep the prompt in prompts/<name>.txt (good for long coordinators).";
const HELP_PROMPTBODY: &str =
    "External prompt content; may reference {{ subagents }} to list the team.";
const HELP_PKEY: &str = "Short id used in model refs, e.g. <key>/<model>.";
const HELP_PNAME: &str = "Human-readable provider name shown in opencode.json.";
const HELP_PNPM: &str = "The @ai-sdk adapter package for this endpoint.";
const HELP_PURL: &str = "The provider's OpenAI-compatible API base URL.";
const HELP_MID: &str = "The provider's model identifier (used in model refs).";
const HELP_MNAME: &str = "Human-readable model name.";

// ---------- prompt helpers (each prints its help line first) ----------

fn hint(help: &str) {
    if !help.is_empty() {
        println!("  {} {}", style("›").dim(), style(help).dim());
    }
}

fn ask(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    default: Option<&str>,
    allow_empty: bool,
) -> Result<String> {
    hint(help);
    let mut b = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(allow_empty);
    if let Some(d) = default {
        b = b.default(d.to_string());
    }
    Ok(b.interact_text()?)
}

/// Required input that re-prompts until `validator` accepts it.
fn ask_v(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    default: Option<&str>,
    validator: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String> {
    hint(help);
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
fn ask_optional_v(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    current: &str,
    validator: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String> {
    hint(help);
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

fn ask_select(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    items: &[String],
    default_idx: usize,
) -> Result<usize> {
    hint(help);
    Ok(Select::with_theme(theme)
        .with_prompt(prompt)
        .items(items)
        .default(default_idx.min(items.len().saturating_sub(1)))
        .interact()?)
}

fn ask_confirm(theme: &ColorfulTheme, prompt: &str, help: &str, default: bool) -> Result<bool> {
    hint(help);
    Ok(Confirm::with_theme(theme)
        .with_prompt(prompt)
        .default(default)
        .interact()?)
}

/// Pick an agent colour from the OpenCode theme palette (each shown with a swatch
/// of how it looks), or choose a custom hex colour.
fn ask_color(theme: &ColorfulTheme, help: &str, current: &str) -> Result<String> {
    hint(help);
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

    let idx = Select::with_theme(theme)
        .with_prompt("  Color")
        .items(&labels)
        .default(default_idx)
        .interact()?;

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
fn edit_multiline(theme: &ColorfulTheme, label: &str, help: &str, seeded: &str) -> Result<String> {
    hint(help);
    println!("  {} (current default):", style(label).bold());
    for line in seeded.lines() {
        println!("    {}", style(line).dim());
    }
    if ask_confirm(theme, &format!("Edit {label} in $EDITOR?"), "", false)? {
        if let Some(edited) = Editor::new().edit(seeded)? {
            return Ok(edited);
        }
    }
    Ok(seeded.to_string())
}

// ---------- top-level commands ----------

/// `ocgen templates edit [path]` — open a template in $EDITOR pre-filled with its
/// current content (the override if present, else the embedded default) and save
/// the result into the override dir. Lets you update any existing template — or a
/// user-added one — without hunting for files.
pub fn run_templates_edit(path_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let available = templates::editable_paths();

    let path = match path_arg {
        Some(p) => {
            if !available.iter().any(|a| a == &p) {
                bail!(
                    "unknown template: {p}\n\
                     Run `ocgen templates list` to see the available templates."
                );
            }
            p
        }
        None => {
            let idx = ask_select(
                &theme,
                "Template to edit",
                "Your edits are saved into the override dir; the embedded default stays intact.",
                &available,
                0,
            )?;
            available[idx].clone()
        }
    };

    let current = templates::load(&path).with_context(|| format!("loading template {path}"))?;

    // Give the editor the file's real extension so it highlights sensibly.
    let ext = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".txt".to_string());

    match Editor::new().extension(&ext).edit(&current)? {
        Some(edited) if edited != current => {
            if path.ends_with(".toml") {
                if let Err(e) = toml::from_str::<toml::Value>(&edited) {
                    eprintln!(
                        "  {} edited {path} is not valid TOML: {e}",
                        style("warning:").yellow()
                    );
                    eprintln!("  Saving anyway — fix it before ocgen reads this template.");
                }
            }
            let dest = templates::save_override(&path, &edited)?;
            println!("Updated {}", dest.display());
            println!("ocgen now prefers this override over the built-in default.");
        }
        _ => println!("No changes — {path} left as-is."),
    }
    Ok(())
}

/// `ocgen new` — ask everything, then scaffold.
pub fn run_new(
    path_arg: Option<String>,
    target: TargetArg,
    base_url: Option<String>,
    output: OutputArg,
    repo: Option<String>,
    team: TeamCli,
) -> Result<()> {
    let theme = ColorfulTheme::default();
    let manifest = Manifest::load().context("loading manifest.toml")?;

    // Validate --base-url up front (OpenCode only) so a bad value fails before prompts.
    if let (TargetArg::Opencode, Some(u)) = (target, &base_url) {
        validate::url(u).map_err(|e| anyhow!("--base-url {u}: {e}"))?;
    }

    let kind = match target {
        TargetArg::Opencode => "OpenCode",
        TargetArg::Claude => "Claude Code",
    };
    ui::banner(&format!("new {kind} project"));
    println!(
        "  {}",
        style("Answer the prompts — Enter accepts the default.").dim()
    );

    // 1. Project basics, driven by the manifest's declared variables.
    let mut answers: HashMap<String, String> = HashMap::new();
    for var in &manifest.variables {
        let value = match var.r#type.as_str() {
            "select" => {
                let default_idx = var
                    .choices
                    .iter()
                    .position(|c| c == &var.default)
                    .unwrap_or(0);
                let idx = ask_select(&theme, &var.prompt, &var.help, &var.choices, default_idx)?;
                var.choices[idx].clone()
            }
            "bool" => ask_confirm(
                &theme,
                &var.prompt,
                &var.help,
                var.default.eq_ignore_ascii_case("true"),
            )?
            .to_string(),
            _ => ask(&theme, &var.prompt, &var.help, Some(&var.default), false)?,
        };
        answers.insert(var.key.clone(), value);
    }
    let language = answers
        .get("language")
        .cloned()
        .unwrap_or_else(|| "English".to_string());
    let project_name = answers.get("project_name").cloned().unwrap_or_default();

    let target_dir = match path_arg {
        Some(p) => p,
        None => ask(&theme, "Create in directory", "", Some("."), false)?,
    };

    let project = match target {
        TargetArg::Opencode => {
            ui::section("Providers");
            let providers = collect_providers(&theme, &manifest, base_url.as_deref())?;
            let (utility_provider, utility_model) = pick_utility(
                &theme,
                &providers,
                &manifest.defaults.utility_provider,
                &manifest.defaults.utility_model,
            )?;
            ui::section("Agents");
            let agents = collect_agents(&theme, &manifest, &providers, &language)?;
            let mut p = Project::from_manifest(&manifest, &language);
            p.project_name = project_name;
            p.providers = providers;
            p.utility_provider = utility_provider;
            p.utility_model = utility_model;
            p.agents = agents;
            p
        }
        TargetArg::Claude => build_claude_project(
            &theme,
            &manifest,
            &language,
            &project_name,
            output,
            repo,
            team,
        )?,
    };

    match project.target {
        Target::OpenCode => print_summary(&project, &target_dir),
        Target::ClaudeCode => print_claude_summary(&project, &target_dir),
    }
    if !ask_confirm(&theme, "Create these files?", "", true)? {
        println!("Aborted — nothing written.");
        return Ok(());
    }

    let target_path = Path::new(&target_dir);
    let files = project.render_all()?;
    let conflict = files.iter().any(|(rel, _)| target_path.join(rel).exists());
    let force = if conflict {
        ask_confirm(
            &theme,
            "Some files already exist. Overwrite them?",
            "",
            false,
        )?
    } else {
        false
    };
    if conflict && !force {
        println!("Aborted — existing files kept.");
        return Ok(());
    }

    let written = project.scaffold(target_path, force)?;
    report_written(&written);
    Ok(())
}

/// Build the Claude Code project: agents (aliases + tools), CLAUDE.md, power-ups,
/// workflow commands, and output/plugin selection.
fn build_claude_project(
    theme: &ColorfulTheme,
    manifest: &Manifest,
    language: &str,
    project_name: &str,
    output: OutputArg,
    repo: Option<String>,
    team: TeamCli,
) -> Result<Project> {
    let mut p = Project::from_manifest(manifest, language);
    p.target = Target::ClaudeCode;
    p.project_name = project_name.to_string();
    p.providers.clear();
    p.claude.model = "opus".to_string();

    ui::section("Agents");
    p.agents = collect_claude_agents(theme, manifest, language)?;

    ui::section("Project instructions (CLAUDE.md)");
    // Capture the project's intent up front so the generic team steers toward it.
    let purpose = ask_v(
        theme,
        "What is this project for?",
        "One or two lines describing the goal; steers the generic team. Enter to skip.",
        Some(""),
        |_s: &str| -> Result<(), String> { Ok(()) },
    )?;
    let mut seed = String::new();
    if !purpose.trim().is_empty() {
        seed.push_str(&format!("## Purpose\n{}\n\n", purpose.trim()));
    }
    seed.push_str("Conventions:\n- Write tests before fixes or features.\n- Keep changes small and focused.\n- Explain non-obvious decisions.\n");
    p.claude.instructions = edit_multiline(
        theme,
        "CLAUDE.md",
        "Project-wide instructions Claude Code always loads.",
        &seed,
    )?;

    ui::section("Options");
    if !ask_confirm(
        theme,
        "Include power-user defaults (permissions, hooks, output style, statusline)?",
        "Writes a settings.json tuned for CLI power users.",
        true,
    )? {
        p.claude.powerups = Powerups {
            permissions: false,
            hooks: false,
            output_style: false,
            statusline: false,
        };
    }
    if !ask_confirm(
        theme,
        "Include the /intake + /refine workflow commands?",
        "A structured interview and a push-back/refine loop.",
        true,
    )? {
        p.claude.workflow.intake = false;
        p.claude.workflow.refine = false;
    }
    p.claude.workflow.improve_prompt = ask_confirm(
        theme,
        "Include the /improve-prompt command?",
        "Improves a prompt (or an agent's system prompt) in-session using Anthropic's technique.",
        true,
    )?;
    p.claude.workflow.fanout = ask_confirm(
        theme,
        "Include the /fanout worktree command?",
        "Fans work out to worktree-isolated subagents (parallel edits never collide); adds .worktreeinclude.",
        true,
    )?;
    p.claude.workflow.verify_todos = ask_confirm(
        theme,
        "Include verification-first todo guidance?",
        "CLAUDE.md guidance to build definition-of-done, per-task verification, self-review, and confidence into todos.",
        true,
    )?;
    p.claude.workflow.deliver = ask_confirm(
        theme,
        "Include the /deliver pipeline command?",
        "One command: sharpen → requirements → plan+approve → parallel research → gated execution; plus multi-session guidance.",
        true,
    )?;
    p.claude.workflow.inquire = ask_confirm(
        theme,
        "Include the /inquire codebase Q&A command?",
        "Sharpens your questions, answers with file:line evidence, suggests smarter next questions, keeps a git-ignored ledger in .claude/notes/.",
        true,
    )?;
    p.claude.workflow.subagent_confidence = ask_v(
        theme,
        "Enforce a minimum confidence on subagents that write files (0–100, 0 = off)",
        "SubagentStop hook: a worker that edited files must state Confidence ≥ this % before finishing (pairs with /fanout isolation).",
        Some("96"),
        validate::confidence_threshold,
    )?
    .parse::<u8>()
    .unwrap_or(96);
    p.claude.workflow.loop_guard_max = ask_v(
        theme,
        "Max times a gate may block the same agent before escalating (0–20, 0 = unlimited)",
        "Loop guard: after this many blocks (or when confidence stops rising) quality gates release the agent marked UNRESOLVED; approval gates stay closed and halt it.",
        Some("3"),
        validate::loop_budget,
    )?
    .trim()
    .parse::<u8>()
    .unwrap_or(3);
    // Seed the team prompts from the CLI flags, then reuse the shared editor.
    let team_seed = Team {
        enabled: team.enabled,
        mode: "in-process".to_string(),
        hooks: true,
        plan_gate: team.plan_gate,
        confidence_threshold: team.confidence.unwrap_or(96),
        risk_rounds: team.risk_rounds,
        approval_gate: team.approval_gate,
    };
    p.claude.team = configure_claude_team(theme, &team_seed)?;

    let (project_out, plugin_out) = match output {
        OutputArg::Project => (true, false),
        OutputArg::Plugin => (false, true),
        OutputArg::Both => (true, true),
    };
    p.claude.output = Output {
        project: project_out,
        plugin: plugin_out,
    };
    if plugin_out {
        let slug = match repo {
            Some(r) => {
                validate::owner_repo(&r).map_err(|e| anyhow!("--repo {r}: {e}"))?;
                r
            }
            None => ask_v(
                theme,
                "GitHub owner/repo",
                "For the plugin marketplace and release workflow.",
                None,
                validate::owner_repo,
            )?,
        };
        let (owner, name) = slug.split_once('/').unwrap();
        p.claude.plugin.repo_owner = owner.to_string();
        p.claude.plugin.repo_name = name.to_string();
        p.claude.plugin.version = "0.1.0".to_string();
        p.claude.plugin.display_name = project_name.to_string();
    }

    Ok(p)
}

/// Interactively configure Agent Teams + its governance gates, seeded with the
/// current values. Shared by `new` and `edit team`. Returns an all-off `Team`
/// when the user declines.
fn configure_claude_team(theme: &ColorfulTheme, seed: &Team) -> Result<Team> {
    if !ask_confirm(
        theme,
        "Enable Agent Teams (experimental)?",
        "Adds CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1, a /team command, and .claude/rules guidance.",
        seed.enabled,
    )? {
        return Ok(Team::default());
    }
    let hooks = ask_confirm(
        theme,
        "Include team quality-gate hooks?",
        "TeammateIdle/TaskCreated/TaskCompleted scripts that enforce the governance gates.",
        if seed.enabled { seed.hooks } else { true },
    )?;
    let modes: Vec<String> = ["in-process", "auto", "tmux", "iterm2"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let default_idx = modes.iter().position(|m| m == &seed.mode).unwrap_or(0);
    let mode = modes[ask_select(
        theme,
        "Teammate display mode",
        "in-process (default) / auto / tmux / iterm2.",
        &modes,
        default_idx,
    )?]
    .clone();
    let plan_gate = ask_confirm(
        theme,
        "Require an approved plan before tasks start (/team-plan gate)?",
        "Emits /team-plan; TaskCreated is blocked until .claude/team/plan.md is APPROVED.",
        seed.plan_gate,
    )?;
    let default_conf = if seed.confidence_threshold > 0 {
        seed.confidence_threshold.to_string()
    } else {
        "96".to_string()
    };
    let confidence_threshold = ask_v(
        theme,
        "Minimum teammate confidence before the next task (0–100, 0 = off)",
        "A teammate must be ≥ this % confident before completing a task; enforced by TaskCompleted.",
        Some(&default_conf),
        validate::confidence_threshold,
    )?
    .parse::<u8>()
    .unwrap_or(96);
    let risk_rounds = ask_confirm(
        theme,
        "Require a mitigation round for every identified risk?",
        "Teammates stay busy until each risk in the plan's register is mitigated or accepted.",
        seed.risk_rounds,
    )?;
    let approval_gate = ask_confirm(
        theme,
        "Gate high-impact external actions behind human approval?",
        "Deterministic PreToolUse hook blocks ssh, cloud mutations, git push/merge, deploys \
         & publishes until a human creates .claude/team/execution-approved.",
        seed.approval_gate,
    )?;
    Ok(Team {
        enabled: true,
        mode,
        hooks,
        plan_gate,
        confidence_threshold,
        risk_rounds,
        approval_gate,
    })
}

/// `ocgen edit team <dir>`: enable or adjust Agent Teams for an existing Claude
/// project, then re-render. Preserves the user's CLAUDE.md (create-once).
pub fn run_edit_team(path: String) -> Result<()> {
    let (root, mut project) = Project::discover(Path::new(&path))?;
    if project.target != Target::ClaudeCode {
        bail!("Agent Teams is a Claude Code feature; this project targets OpenCode.");
    }
    ui::banner("edit agent teams");
    let theme = ColorfulTheme::default();
    project.claude.team = configure_claude_team(&theme, &project.claude.team)?;
    let written = project.scaffold(&root, true)?;
    report_written(&written);
    Ok(())
}

fn collect_claude_agents(
    theme: &ColorfulTheme,
    manifest: &Manifest,
    language: &str,
) -> Result<Vec<Agent>> {
    let mut agents = if manifest.pipeline.is_empty() {
        Vec::new()
    } else {
        let names = manifest
            .pipeline
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join("/");
        if ask_confirm(
            theme,
            &format!(
                "Start from the default {}-agent pipeline?",
                manifest.pipeline.len()
            ),
            &format!("Loads {names}, which you can then extend."),
            true,
        )? {
            let a = agent::claude_default_pipeline(language)?;
            println!(
                "  Loaded: {}",
                a.iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            a
        } else {
            Vec::new()
        }
    };

    loop {
        let prompt = if agents.is_empty() {
            "Add an agent?"
        } else {
            "Add another agent?"
        };
        if !ask_confirm(theme, prompt, "", agents.is_empty())? {
            if agents.is_empty() {
                println!("  A project needs at least one agent.");
                continue;
            }
            break;
        }
        let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        agents.push(prompt_claude_agent(theme, language, &names)?);
    }
    Ok(agents)
}

fn prompt_claude_agent(theme: &ColorfulTheme, language: &str, taken: &[String]) -> Result<Agent> {
    // Preset first, so the (generic) role name seeds the agent name.
    let mut presets = templates::archetype_names();
    let blank = "blank (custom role)".to_string();
    presets.push(blank.clone());
    let pidx = ask_select(theme, "  Start from preset", HELP_PRESET, &presets, 0)?;
    let default_name = if presets[pidx] == blank {
        "agent".to_string()
    } else {
        presets[pidx].clone()
    };
    let name = ask_v(
        theme,
        "  Agent name",
        HELP_NAME,
        Some(&default_name),
        unique_ident(taken.to_vec(), "agent name"),
    )?;
    let mut seed = if presets[pidx] == blank {
        let mut b = Agent::blank(&name, "custom", "");
        b.color = "blue".to_string();
        b.model = "opus".to_string();
        b
    } else {
        Agent::from_archetype_claude(&name, &presets[pidx], language)?
    };
    seed.name = name;
    configure_claude_agent(theme, language, seed, false, taken)
}

/// Walk a Claude agent's fields (mode, alias, tools, description, body), seeded
/// with its current values. Shared by add (from a preset) and edit.
fn configure_claude_agent(
    theme: &ColorfulTheme,
    language: &str,
    mut a: Agent,
    allow_rename: bool,
    taken: &[String],
) -> Result<Agent> {
    if allow_rename {
        a.name = ask_v(
            theme,
            "  Agent name",
            HELP_NAME,
            Some(&a.name),
            unique_ident(taken.to_vec(), "agent name"),
        )?;
    }
    let modes = vec!["subagent".to_string(), "primary".to_string()];
    let midx = ask_select(
        theme,
        "  Mode",
        "primary = coordinator (becomes CLAUDE.md); subagent = a team member.",
        &modes,
        if a.mode == "primary" { 1 } else { 0 },
    )?;
    a.mode = modes[midx].clone();
    a.model = ask_v(
        theme,
        "  Model alias",
        "opus / sonnet / haiku / inherit",
        Some(if a.model.trim().is_empty() {
            "opus"
        } else {
            a.model.trim()
        }),
        validate::claude_model,
    )?;
    a.tools = ask(
        theme,
        "  Tools (comma-separated allow-list; empty = all)",
        "e.g. Read, Grep, Edit, Write",
        Some(&a.tools),
        true,
    )?;
    if a.mode != "primary" {
        let current = a.steps.map(|s| s.to_string()).unwrap_or_default();
        let turns = ask_optional_v(
            theme,
            "  Max turns (- = unlimited)",
            "maxTurns: a hard ceiling on this subagent's agentic turns; at the limit its output returns marked partial.",
            &current,
            validate::steps,
        )?;
        a.steps = turns.parse().ok();
    }
    a.description = ask(
        theme,
        "  Description",
        HELP_DESC,
        Some(&a.description),
        true,
    )?;
    let body_seed = if a.body.is_empty() {
        ocgen::seeds::Seeds::load()?.body_for(language)
    } else {
        a.body.clone()
    };
    a.body = edit_multiline(theme, "System prompt body", HELP_BODY, &body_seed)?;
    Ok(a)
}

fn print_claude_summary(project: &Project, target_dir: &str) {
    ui::section("Summary");
    ui::kv("project", &project.project_name);
    ui::kv("target", &format!("{} (Claude Code)", target_dir));
    ui::kv("language", &project.language);
    ui::kv("default model", &project.claude.model);
    let agents = project
        .agents
        .iter()
        .map(|a| {
            let m = if a.mode == "primary" {
                "coordinator"
            } else {
                a.model.as_str()
            };
            format!("{} [{}]", a.name, m)
        })
        .collect::<Vec<_>>()
        .join(", ");
    ui::kv("agents", &agents);
    if !project.skills.is_empty() {
        ui::kv(
            "skills",
            &project
                .skills
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let mut out = Vec::new();
    if project.claude.output.project {
        out.push("project (.claude/)");
    }
    if project.claude.output.plugin {
        out.push("plugin");
    }
    ui::kv("output", &out.join(" + "));
}

/// `ocgen add agent` — reload the saved project, append one agent, re-render.
pub fn run_add_agent(path_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let start = path_arg.unwrap_or_else(|| ".".to_string());
    let (target, mut project) = Project::discover(Path::new(&start))?;

    println!(
        "Adding an agent to {} ({} existing agent(s)).\n",
        style(&project.project_name).bold(),
        project.agents.len()
    );

    let taken: Vec<String> = project.agents.iter().map(|a| a.name.clone()).collect();
    let agent = if project.target == Target::ClaudeCode {
        prompt_claude_agent(&theme, &project.language, &taken)?
    } else {
        prompt_agent(&theme, &project.providers, &project.language, &taken)?
    };
    project.agents.push(agent);

    // Re-render everything so cross-references (task perms, multi command) update.
    let written = project.scaffold(&target, true)?;
    report_written(&written);
    Ok(())
}

/// `ocgen edit agent` — reload the project, pick an agent, edit its fields, re-render.
pub fn run_edit_agent(path: String, name_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let (target, mut project) = Project::discover(Path::new(&path))?;

    if project.agents.is_empty() {
        bail!("this project has no agents to edit");
    }

    let idx = match name_arg {
        Some(n) => project
            .agents
            .iter()
            .position(|a| a.name == n)
            .ok_or_else(|| {
                let have = project
                    .agents
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!("no agent named '{n}' (have: {have})")
            })?,
        None => {
            let labels: Vec<String> = project
                .agents
                .iter()
                .map(|a| format!("{} — {}, {}/{}", a.name, a.mode, a.provider, a.model))
                .collect();
            ask_select(&theme, "Which agent to edit?", "", &labels, 0)?
        }
    };

    let old_name = project.agents[idx].name.clone();
    println!(
        "\nEditing {} — press Enter to keep each current value.\n",
        style(&old_name).bold()
    );

    let taken: Vec<String> = project
        .agents
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, a)| a.name.clone())
        .collect();
    let edited = if project.target == Target::ClaudeCode {
        configure_claude_agent(
            &theme,
            &project.language,
            project.agents[idx].clone(),
            true,
            &taken,
        )?
    } else {
        configure_agent(
            &theme,
            &project.providers,
            &project.language,
            project.agents[idx].clone(),
            true,
            &taken,
        )?
    };
    let new_name = edited.name.clone();
    project.agents[idx] = edited;

    // Re-render so cross-references reflect the change; clean up on rename.
    let written = project.scaffold(&target, true)?;
    if new_name != old_name {
        if project.target == Target::ClaudeCode {
            Project::remove_claude_agent_file(&target, &old_name)?;
        } else {
            Project::remove_agent_artifacts(&target, &old_name)?;
        }
        println!("Renamed {old_name} → {new_name}");
    }
    report_written(&written);
    Ok(())
}

/// `ocgen add provider` — reload the project, define a provider, re-render.
pub fn run_add_provider(path_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let start = path_arg.unwrap_or_else(|| ".".to_string());
    let (target, mut project) = Project::discover(Path::new(&start))?;

    println!(
        "Adding a provider to {}.\n",
        style(&project.project_name).bold()
    );

    let taken: Vec<String> = project.providers.iter().map(|p| p.key.clone()).collect();
    let provider = prompt_provider(&theme, &taken)?;
    project.providers.push(provider);

    let written = project.scaffold(&target, true)?;
    report_written(&written);
    Ok(())
}

/// `ocgen edit provider` — reload the project, pick a provider, edit it, re-render.
/// A key rename propagates to every agent and the utility provider.
pub fn run_edit_provider(path: String, key_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let (target, mut project) = Project::discover(Path::new(&path))?;

    if project.providers.is_empty() {
        bail!("this project has no providers to edit");
    }

    let idx = match key_arg {
        Some(k) => project
            .providers
            .iter()
            .position(|p| p.key == k)
            .ok_or_else(|| {
                let have = project
                    .providers
                    .iter()
                    .map(|p| p.key.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!("no provider with key '{k}' (have: {have})")
            })?,
        None => {
            let labels: Vec<String> = project
                .providers
                .iter()
                .map(|p| format!("{} — {} ({} model(s))", p.key, p.name, p.models.len()))
                .collect();
            ask_select(&theme, "Which provider to edit?", "", &labels, 0)?
        }
    };

    let old_key = project.providers[idx].key.clone();
    println!(
        "\nEditing provider {} — press Enter to keep each current value.\n",
        style(&old_key).bold()
    );

    let taken: Vec<String> = project
        .providers
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, p)| p.key.clone())
        .collect();
    let edited = configure_provider(&theme, project.providers[idx].clone(), &taken)?;
    let new_key = edited.key.clone();
    if new_key != old_key
        && project
            .providers
            .iter()
            .enumerate()
            .any(|(i, p)| i != idx && p.key == new_key)
    {
        bail!("another provider already uses key '{new_key}'");
    }
    project.providers[idx] = edited;

    if new_key != old_key {
        let mut updated = 0;
        for a in &mut project.agents {
            if a.provider == old_key {
                a.provider = new_key.clone();
                updated += 1;
            }
        }
        if project.utility_provider == old_key {
            project.utility_provider = new_key.clone();
        }
        println!("Renamed provider {old_key} → {new_key} (updated {updated} agent ref(s))");
    }

    let written = project.scaffold(&target, true)?;
    report_written(&written);
    Ok(())
}

/// `ocgen doctor` — repair a project's config and rewrite its files. Also upgrades
/// an old-format state file to the current schema.
pub fn run_doctor(path: String) -> Result<()> {
    let (target, mut project) = Project::discover(Path::new(&path))?;

    ui::banner("doctor");
    ui::kv("project", &style(&project.project_name).bold().to_string());
    ui::kv("path", &ui::muted(&target.display().to_string()));

    let fixes = project.doctor();
    if fixes.is_empty() {
        ui::section("Checks");
        ui::success("no problems found");
    } else {
        ui::section(&format!("Fixes ({})", fixes.len()));
        for f in &fixes {
            ui::success(f);
        }
    }

    // Rewrite everything (regenerates files and upgrades the state file schema).
    let written = project.scaffold(&target, true)?;
    report_written(&written);
    // Not auto-fixable: the project's .gitignore belongs to the user.
    if project.target == Target::ClaudeCode
        && ocgen::gitcheck::claude_config_ignored(&target) == Some(true)
    {
        ui::warning(ocgen::gitcheck::IGNORED_CONFIG_WARNING);
    }
    Ok(())
}

// ---------- providers ----------

fn collect_providers(
    theme: &ColorfulTheme,
    manifest: &Manifest,
    base_url: Option<&str>,
) -> Result<Vec<Provider>> {
    let mut providers: Vec<Provider> = Vec::new();

    // Offer each seeded provider from the manifest. A --base-url override replaces
    // the seeded base URL so it flows through as the (still editable) prompt default.
    for seeded in &manifest.providers {
        if ask_confirm(
            theme,
            &format!("Use seeded provider '{}' ({})?", seeded.key, seeded.name),
            "An OpenAI-compatible endpoint that serves models.",
            true,
        )? {
            let p = match base_url {
                Some(u) => seeded.clone().with_base_url(u),
                None => seeded.clone(),
            };
            providers.push(p);
        }
    }

    loop {
        let prompt = if providers.is_empty() {
            "Add a provider?"
        } else {
            "Add another provider?"
        };
        if !ask_confirm(theme, prompt, "", providers.is_empty())? {
            if providers.is_empty() {
                println!("  At least one provider is required.");
                continue;
            }
            break;
        }
        let keys: Vec<String> = providers.iter().map(|p| p.key.clone()).collect();
        providers.push(prompt_provider(theme, &keys)?);
    }

    Ok(providers)
}

/// Add flow: configure a brand-new provider (empty seed).
/// `taken_keys` are provider keys already in use (for uniqueness).
fn prompt_provider(theme: &ColorfulTheme, taken_keys: &[String]) -> Result<Provider> {
    configure_provider(
        theme,
        Provider {
            key: String::new(),
            name: String::new(),
            npm: String::new(),
            base_url: String::new(),
            models: Vec::new(),
        },
        taken_keys,
    )
}

/// Walk every field of a provider, seeded with its current values (Enter keeps each).
/// Shared by add (empty seed) and edit (seeded from the existing provider).
/// `taken_keys` are the keys of *other* providers (for uniqueness).
fn configure_provider(
    theme: &ColorfulTheme,
    seed: Provider,
    taken_keys: &[String],
) -> Result<Provider> {
    let key = ask_v(
        theme,
        "  Provider key",
        HELP_PKEY,
        if seed.key.is_empty() {
            None
        } else {
            Some(seed.key.as_str())
        },
        unique_ident(taken_keys.to_vec(), "provider key"),
    )?;
    let name_default = if seed.name.is_empty() {
        key.clone()
    } else {
        seed.name.clone()
    };
    let name = ask(
        theme,
        "  Provider display name",
        HELP_PNAME,
        Some(&name_default),
        false,
    )?;
    let npm_default = if seed.npm.is_empty() {
        "@ai-sdk/openai-compatible".to_string()
    } else {
        seed.npm.clone()
    };
    let npm = ask_v(
        theme,
        "  AI SDK npm package",
        HELP_PNPM,
        Some(&npm_default),
        validate::nonempty_nospace,
    )?;
    let url_default = if seed.base_url.is_empty() {
        "http://localhost:8080/v1".to_string()
    } else {
        seed.base_url.clone()
    };
    let base_url = ask_v(
        theme,
        "  Base URL",
        HELP_PURL,
        Some(&url_default),
        validate::url,
    )?;

    let models = if seed.models.is_empty() {
        add_models(theme, Vec::new())?
    } else {
        edit_models(theme, seed.models)?
    };

    Ok(Provider {
        key,
        name,
        npm,
        base_url,
        models,
    })
}

/// Review each existing model (keep/edit/remove), then append any new ones.
fn edit_models(theme: &ColorfulTheme, existing: Vec<Model>) -> Result<Vec<Model>> {
    let mut result: Vec<Model> = Vec::new();
    for m in existing {
        let actions = vec!["keep".to_string(), "edit".to_string(), "remove".to_string()];
        let idx = ask_select(
            theme,
            &format!("  Model '{}' ({})", m.name, m.id),
            "",
            &actions,
            0,
        )?;
        match idx {
            1 => {
                let taken: Vec<String> = result.iter().map(|x| x.id.clone()).collect();
                let id = ask_v(
                    theme,
                    "    Model id",
                    HELP_MID,
                    Some(&m.id),
                    unique_ident(taken, "model id"),
                )?;
                let name = ask(
                    theme,
                    "    Model display name",
                    HELP_MNAME,
                    Some(&m.name),
                    false,
                )?;
                result.push(Model { id, name });
            }
            2 => {} // remove
            _ => result.push(m),
        }
    }
    add_models(theme, result)
}

/// Loop asking for additional models, appended to `models`. Ensures at least one.
fn add_models(theme: &ColorfulTheme, mut models: Vec<Model>) -> Result<Vec<Model>> {
    loop {
        let prompt = if models.is_empty() {
            "  Add a model?"
        } else {
            "  Add another model?"
        };
        if !ask_confirm(
            theme,
            prompt,
            "Models this provider serves.",
            models.is_empty(),
        )? {
            if models.is_empty() {
                println!("    A provider needs at least one model.");
                continue;
            }
            break;
        }
        let taken: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
        let id = ask_v(
            theme,
            "    Model id",
            HELP_MID,
            None,
            unique_ident(taken, "model id"),
        )?;
        let name = ask(
            theme,
            "    Model display name",
            HELP_MNAME,
            Some(&id),
            false,
        )?;
        models.push(Model { id, name });
    }
    Ok(models)
}

fn pick_utility(
    theme: &ColorfulTheme,
    providers: &[Provider],
    default_provider: &str,
    default_model: &str,
) -> Result<(String, String)> {
    let keys: Vec<String> = providers.iter().map(|p| p.key.clone()).collect();
    let pidx = ask_select(
        theme,
        "Utility provider (compaction/title/summary)",
        "The built-in helper agents run on this provider + model.",
        &keys,
        keys.iter().position(|k| k == default_provider).unwrap_or(0),
    )?;
    let provider = &providers[pidx];
    let labels: Vec<String> = provider
        .models
        .iter()
        .map(|m| format!("{} ({})", m.name, m.id))
        .collect();
    let midx = ask_select(
        theme,
        "Utility model",
        "",
        &labels,
        provider.model_index(default_model),
    )?;
    Ok((provider.key.clone(), provider.models[midx].id.clone()))
}

// ---------- agents ----------

fn collect_agents(
    theme: &ColorfulTheme,
    manifest: &Manifest,
    providers: &[Provider],
    language: &str,
) -> Result<Vec<Agent>> {
    let default_provider = providers[0].key.clone();

    // Offer the manifest-declared default team (skip the question if there is none).
    let mut agents = if manifest.pipeline.is_empty() {
        Vec::new()
    } else {
        let names = manifest
            .pipeline
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join("/");
        let question = format!(
            "Start from the default {}-agent pipeline?",
            manifest.pipeline.len()
        );
        let help = format!("Loads {names}, which you can then extend.");
        if ask_confirm(theme, &question, &help, true)? {
            let a = agent::default_pipeline(language, &default_provider)?;
            println!(
                "  Loaded: {}",
                a.iter()
                    .map(|x| x.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            a
        } else {
            Vec::new()
        }
    };

    loop {
        let prompt = if agents.is_empty() {
            "Add an agent?"
        } else {
            "Add another agent?"
        };
        if !ask_confirm(theme, prompt, "", agents.is_empty())? {
            if agents.is_empty() {
                println!("  A project needs at least one agent.");
                continue;
            }
            break;
        }
        let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        agents.push(prompt_agent(theme, providers, language, &names)?);
    }

    Ok(agents)
}

/// Add flow: ask a name, pick a preset (or blank), then configure every field.
/// `taken_names` are agent names already used (for uniqueness).
fn prompt_agent(
    theme: &ColorfulTheme,
    providers: &[Provider],
    language: &str,
    taken_names: &[String],
) -> Result<Agent> {
    let default_provider = providers[0].key.clone();

    let name = ask_v(
        theme,
        "  Agent name",
        HELP_NAME,
        None,
        unique_ident(taken_names.to_vec(), "agent name"),
    )?;

    let mut presets = templates::archetype_names();
    let blank_label = "blank (custom role)".to_string();
    presets.push(blank_label.clone());
    let pidx = ask_select(theme, "  Start from preset", HELP_PRESET, &presets, 0)?;

    let seed = if presets[pidx] == blank_label {
        Agent::blank(&name, "custom", &default_provider)
    } else {
        Agent::from_archetype(&name, &presets[pidx], language, &default_provider)?
    };

    // Name is already set; don't ask again.
    configure_agent(theme, providers, language, seed, false, taken_names)
}

/// Walk every field of `agent`, seeded with its current values (Enter keeps each).
/// Shared by add (seeded from a preset) and edit (seeded from the existing agent).
/// `taken_names` are the names of *other* agents (for rename uniqueness).
fn configure_agent(
    theme: &ColorfulTheme,
    providers: &[Provider],
    language: &str,
    mut agent: Agent,
    allow_rename: bool,
    taken_names: &[String],
) -> Result<Agent> {
    if allow_rename {
        agent.name = ask_v(
            theme,
            "  Agent name",
            HELP_NAME,
            Some(&agent.name),
            unique_ident(taken_names.to_vec(), "agent name"),
        )?;
    }
    agent.role = ask(theme, "  Role name", HELP_ROLE, Some(&agent.role), true)?;

    let modes = vec![
        "subagent".to_string(),
        "primary".to_string(),
        "all".to_string(),
    ];
    let mode_default = modes.iter().position(|m| m == &agent.mode).unwrap_or(0);
    let mode_idx = ask_select(theme, "  Mode", HELP_MODE, &modes, mode_default)?;
    agent.mode = modes[mode_idx].clone();

    // Provider, then model within it.
    let keys: Vec<String> = providers.iter().map(|p| p.key.clone()).collect();
    let prov_idx = ask_select(
        theme,
        "  Provider",
        HELP_PROVIDER,
        &keys,
        keys.iter().position(|k| k == &agent.provider).unwrap_or(0),
    )?;
    let provider = &providers[prov_idx];
    agent.provider = provider.key.clone();

    let model_labels: Vec<String> = provider
        .models
        .iter()
        .map(|m| format!("{} ({})", m.name, m.id))
        .collect();
    let model_idx = ask_select(
        theme,
        "  Model",
        HELP_MODEL,
        &model_labels,
        provider.model_index(&agent.model),
    )?;
    agent.model = provider.models[model_idx].id.clone();
    agent.variant = ask_optional_v(
        theme,
        "  Model variant (- = default)",
        HELP_VARIANT,
        &agent.variant,
        validate::nonempty_nospace,
    )?;

    // Sampling.
    agent.temperature = ask_v(
        theme,
        "  Temperature",
        HELP_TEMP,
        Some(&agent.temperature),
        |s| validate::number(s, 0.0, 2.0),
    )?;
    agent.top_p = ask_optional_v(
        theme,
        "  Top-p (- = model default)",
        HELP_TOPP,
        &agent.top_p,
        |s| validate::number(s, 0.0, 1.0),
    )?;

    let current_steps = agent.steps.map(|s| s.to_string()).unwrap_or_default();
    let steps_str = ask_optional_v(
        theme,
        "  Max steps (- = unlimited)",
        HELP_STEPS,
        &current_steps,
        validate::steps,
    )?;
    agent.steps = if steps_str.trim().is_empty() {
        None
    } else {
        steps_str.trim().parse().ok()
    };

    agent.color = ask_color(theme, HELP_COLOR, &agent.color)?;

    // Visibility.
    agent.disable = ask_confirm(theme, "  Disable this agent?", HELP_DISABLE, agent.disable)?;
    agent.hidden = if agent.mode == "subagent" {
        ask_confirm(
            theme,
            "  Hide from @ autocomplete?",
            HELP_HIDDEN,
            agent.hidden,
        )?
    } else {
        false
    };

    agent.description = ask(
        theme,
        "  Description",
        HELP_DESC,
        Some(&agent.description),
        true,
    )?;

    // Multi-line fields (hybrid $EDITOR).
    agent.permissions = edit_multiline(theme, "Permissions", HELP_PERMS, &agent.permissions)?;
    let body_seed = if agent.body.is_empty() {
        ocgen::seeds::Seeds::load()?.body_for(language)
    } else {
        agent.body.clone()
    };
    agent.body = edit_multiline(theme, "System prompt body", HELP_BODY, &body_seed)?;

    // Provider-specific model options (optional).
    let want_options = ask_confirm(
        theme,
        "  Set provider-specific model options?",
        HELP_OPTIONS,
        !agent.options.trim().is_empty(),
    )?;
    agent.options = if want_options {
        let seed = if agent.options.trim().is_empty() {
            "  reasoningEffort: high".to_string()
        } else {
            agent.options.clone()
        };
        edit_multiline(theme, "Model options", HELP_OPTIONS, &seed)?
    } else {
        String::new()
    };

    // Optional external prompt file.
    agent.prompt_file = ask_confirm(
        theme,
        "  Use an external prompt file?",
        HELP_PROMPTFILE,
        agent.prompt_file || agent.mode == "primary",
    )?;
    if agent.prompt_file {
        let seed = match agent.prompt_body.clone() {
            Some(existing) => existing,
            None => ocgen::seeds::Seeds::load()?.prompt_for(language),
        };
        agent.prompt_body = Some(edit_multiline(
            theme,
            "External prompt",
            HELP_PROMPTBODY,
            &seed,
        )?);
    } else {
        agent.prompt_body = None;
    }

    Ok(agent)
}

// ---------- output ----------

fn print_summary(project: &Project, target_dir: &str) {
    ui::section("Summary");
    ui::kv("project", &project.project_name);
    ui::kv("target", target_dir);
    ui::kv("language", &project.language);
    ui::kv(
        "providers",
        &project
            .providers
            .iter()
            .map(|p| p.key.clone())
            .collect::<Vec<_>>()
            .join(", "),
    );
    ui::kv(
        "utility",
        &format!("{}/{}", project.utility_provider, project.utility_model),
    );
    let rows: Vec<Vec<String>> = project
        .agents
        .iter()
        .map(|a| {
            let mode = if a.mode == "primary" {
                style("primary").magenta().bold().to_string()
            } else {
                ui::muted("subagent")
            };
            vec![
                style(&a.name).bold().to_string(),
                mode,
                format!("{}/{}", style(&a.provider).cyan(), a.model),
            ]
        })
        .collect();
    ui::table(&["AGENT", "MODE", "MODEL"], &rows);
}

fn report_written(written: &[std::path::PathBuf]) {
    println!();
    ui::file_tree(written);
}

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
    let taken: Vec<String> = project.skills.iter().map(|s| s.name.clone()).collect();

    let presets = ocgen::claude::skill_presets()?;
    let mut labels: Vec<String> = presets
        .iter()
        .map(|p| format!("{} — {}", p.name, p.description))
        .collect();
    labels.push("blank (from scratch)".to_string());
    let idx = ask_select(
        &theme,
        "Start from preset",
        "Presets seed the tools, frontmatter and a numbered-step body.",
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

    let skill = configure_skill(&theme, seed, false, &taken)?;
    project.skills.push(skill);
    let written = project.scaffold(&target, true)?;
    report_written(&written);
    Ok(())
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
    let new_name = edited.name.clone();
    project.skills[idx] = edited;
    let written = project.scaffold(&target, true)?;
    if new_name != old_name {
        Project::remove_claude_skill_dir(&target, &old_name)?;
        println!("Renamed skill {old_name} → {new_name}");
    }
    report_written(&written);
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
            "Identifier → /skill-name and its folder.",
            if s.name.is_empty() {
                None
            } else {
                Some(s.name.as_str())
            },
            unique_ident(taken.to_vec(), "skill name"),
        )?;
    }
    s.description = ask(
        theme,
        "  Description",
        "Guides when Claude should invoke the skill.",
        if s.description.is_empty() {
            None
        } else {
            Some(&s.description)
        },
        false,
    )?;
    s.when_to_use = ask(
        theme,
        "  When to use (extra trigger context; optional)",
        "Appended to the description to help Claude auto-invoke it.",
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
        "A task you trigger with /name; Claude won't run it on its own.",
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
        "Runs the skill in a forked context; pick which agent next.",
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
    } else {
        s.agent = String::new();
    }
    s.model = ask(
        theme,
        "  Model alias for this skill (optional)",
        "opus / sonnet / haiku / inherit; empty = session model.",
        Some(&s.model),
        true,
    )?;
    s.body = edit_multiline(theme, "Skill body", HELP_BODY, &s.body)?;
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
    hint("Space toggles, Enter confirms. Add Bash(...)/mcp__ patterns at the next prompt.");
    let chosen = MultiSelect::with_theme(theme)
        .with_prompt("  Allowed tools (pre-approved during the skill's turn)")
        .items(&tools)
        .defaults(&defaults)
        .interact()?;
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
