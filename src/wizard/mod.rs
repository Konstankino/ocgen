//! The interactive wizard — the whole user-facing experience.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Editor};

use ocgen::agent::{self, Agent};
use ocgen::claude::{Output, Powerups, Team};
use ocgen::manifest::{Manifest, Model, Provider};
use ocgen::render::{ChangeKind, FileChange, Project};
use ocgen::target::Target;
use ocgen::templates;
use ocgen::validate::{self, unique_ident};

use crate::cli::{IntentCli, OutputArg, PermissionsCli, TargetArg, TeamCli};
use crate::prompt::{
    ask, ask_color, ask_confirm, ask_multi, ask_optional_v, ask_select, ask_v, edit_multiline,
};
use crate::ui;

mod mcp;
mod skill;
#[cfg(test)]
mod tests;

pub use mcp::{run_add_mcp, run_edit_mcp};
pub use skill::{run_add_skill, run_edit_skill};

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
const HELP_SKILL_BODY: &str = "Numbered steps Claude follows ($ARGUMENTS = the input). Keep it short: long reference goes in reference.md, deterministic logic in scripts/.";
const HELP_PROMPTFILE: &str = "Keep the prompt in prompts/<name>.txt (good for long coordinators).";
const HELP_PROMPTBODY: &str =
    "External prompt content; may reference {{ subagents }} to list the team.";
const HELP_PKEY: &str = "Short id used in model refs, e.g. <key>/<model>.";
const HELP_PNAME: &str = "Human-readable provider name shown in opencode.json.";
const HELP_PNPM: &str = "The @ai-sdk adapter package for this endpoint.";
const HELP_PURL: &str = "The provider's OpenAI-compatible API base URL.";
const HELP_MID: &str = "The provider's model identifier (used in model refs).";
const HELP_MNAME: &str = "Human-readable model name.";

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
    target: Option<TargetArg>,
    base_url: Option<String>,
    output: OutputArg,
    repo: Option<String>,
    team: TeamCli,
) -> Result<()> {
    let theme = ColorfulTheme::default();
    let manifest = Manifest::load().context("loading manifest.toml")?;

    // Validate --base-url up front (it only applies to OpenCode) so a bad value
    // fails before prompts.
    if !matches!(target, Some(TargetArg::Claude)) {
        if let Some(u) = &base_url {
            validate::url(u).map_err(|e| anyhow!("--base-url {u}: {e}"))?;
        }
    }

    ui::banner("LLM project bootstrapper");
    println!(
        "  {}",
        style("Answer the prompts — Enter accepts the default.").dim()
    );
    if let Some(t) = target {
        ui::kv("target", target_label(t));
    }
    let target = pick_target(&theme, target)?;

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

    write_with_review(&theme, &project, Path::new(&target_dir))
}

/// The tools ocgen can generate for, in the order the wizard offers them.
const TARGETS: [TargetArg; 2] = [TargetArg::Claude, TargetArg::Opencode];

fn target_label(t: TargetArg) -> &'static str {
    match t {
        TargetArg::Opencode => "OpenCode",
        TargetArg::Claude => "Claude Code",
    }
}

/// The tool to generate for: `--target` when given, otherwise the wizard asks.
fn pick_target(theme: &ColorfulTheme, given: Option<TargetArg>) -> Result<TargetArg> {
    if let Some(t) = given {
        return Ok(t);
    }
    let items: Vec<String> = TARGETS
        .iter()
        .map(|t| target_label(*t).to_string())
        .collect();
    let idx = ask_select(
        theme,
        "Which tool is this project for?",
        "ocgen generates the agents and config for one tool per project (--target skips this question).",
        &items,
        0,
    )?;
    Ok(TARGETS[idx])
}

/// Write the project under `target`. Existing files that differ from what ocgen
/// generates are listed first and the user decides what happens to each; whatever
/// gets overwritten is backed up. Identical files and the user-owned `CLAUDE.md`
/// are not conflicts.
fn write_with_review(theme: &ColorfulTheme, project: &Project, target: &Path) -> Result<()> {
    let plan = project.plan_changes(target)?;
    let conflicts: Vec<&FileChange> = plan
        .iter()
        .filter(|c| c.kind == ChangeKind::Modified)
        .collect();
    let keep = if conflicts.is_empty() {
        BTreeSet::new()
    } else {
        let unchanged = plan
            .iter()
            .filter(|c| c.kind == ChangeKind::Unchanged)
            .count();
        match resolve_conflicts(theme, &conflicts, unchanged)? {
            Some(keep) => keep,
            None => {
                println!("Aborted — nothing written.");
                return Ok(());
            }
        }
    };

    let applied: Vec<FileChange> = plan
        .into_iter()
        .filter(|c| !keep.contains(&c.rel))
        .collect();
    let backup = Project::backup(target, &applied)?;
    let written = project.scaffold_keeping(target, &keep)?;
    report_written(&written);
    report_backup(target, backup.as_deref());
    report_kept(&keep);
    Ok(())
}

/// Where the previous versions went, if anything was backed up.
fn report_backup(target: &Path, backup: Option<&Path>) {
    if let Some(b) = backup {
        // Forward slashes on every platform (it's shown, and matched by tests).
        let shown = ocgen::paths::for_shell(b.strip_prefix(target).unwrap_or(b));
        println!(
            "  {} {shown}/  {}",
            style("backup:").bold(),
            ui::muted("(previous versions; copy a file back to restore it)")
        );
    }
}

/// The existing files the user chose to keep instead of regenerating.
fn report_kept(keep: &BTreeSet<String>) {
    if keep.is_empty() {
        return;
    }
    println!();
    ui::warning(&format!(
        "kept {} existing file(s) as they were:",
        keep.len()
    ));
    for rel in keep {
        println!("    {}", ui::muted(rel));
    }
    ui::tip("The generated files may rely on things the kept ones lack. `ocgen doctor --dry-run` shows how they differ; `ocgen doctor` replaces them (with a backup).");
}

/// Show the existing files that differ and ask what to do with them. Returns the
/// files to keep as they are (relative paths), or `None` to write nothing at all.
fn resolve_conflicts(
    theme: &ColorfulTheme,
    conflicts: &[&FileChange],
    unchanged: usize,
) -> Result<Option<BTreeSet<String>>> {
    ui::section(&format!("Existing files ({})", conflicts.len()));
    for c in conflicts {
        print_conflict(c, 1, 12);
    }
    if unchanged > 0 {
        println!(
            "  {}",
            ui::muted(&format!("{unchanged} file(s) already up to date"))
        );
    }

    choose_keep(
        theme,
        conflicts,
        "These files already exist and differ. What now?",
        [
            format!("Overwrite all ({})", conflicts.len()),
            "Keep all existing — write only the new files".to_string(),
        ],
        2,
    )
}

/// Ask what happens to files that would be overwritten: all of them, none, one
/// by one, or cancel. `labels` names the first two choices. Returns the files to
/// keep as they are (relative paths), or `None` to write nothing at all.
fn choose_keep(
    theme: &ColorfulTheme,
    conflicts: &[&FileChange],
    question: &str,
    labels: [String; 2],
    default: usize,
) -> Result<Option<BTreeSet<String>>> {
    let [overwrite_all, keep_all] = labels;
    let choices = [
        overwrite_all,
        keep_all,
        "Decide file by file".to_string(),
        "Cancel — write nothing".to_string(),
    ];
    let choice = ask_select(
        theme,
        question,
        "Anything overwritten is backed up to .ocgen-backup/ first.",
        &choices,
        default,
    )?;
    let all = || conflicts.iter().map(|c| c.rel.clone()).collect();
    match choice {
        0 => return Ok(Some(BTreeSet::new())),
        1 => return Ok(Some(all())),
        2 => {}
        _ => return Ok(None),
    }

    let per_file = [
        "Overwrite".to_string(),
        "Keep mine".to_string(),
        "Show full diff".to_string(),
    ];
    let mut keep = BTreeSet::new();
    for c in conflicts {
        loop {
            match ask_select(theme, &c.rel, "", &per_file, 0)? {
                0 => break,
                1 => {
                    keep.insert(c.rel.clone());
                    break;
                }
                _ => print_conflict(c, 3, usize::MAX),
            }
        }
    }
    Ok(Some(keep))
}

/// One existing file that differs from what would be generated, with its diff.
fn print_conflict(c: &FileChange, context: usize, max: usize) {
    println!(
        "  {} {}  {}",
        style("~").yellow(),
        style(&c.rel).bold(),
        ui::muted("already exists — differs")
    );
    if let (Some(old), Some(new)) = (&c.old, &c.new) {
        print_diff(old, new, context, max);
    }
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
        "Writes a settings.json tuned for CLI power users. Its statusline replaces your personal one in this project — keep yours by setting statusLine in .claude/settings.local.json.",
        true,
    )? {
        p.claude.powerups = Powerups {
            permissions: false,
            hooks: false,
            output_style: false,
            statusline: false,
        };
    }
    configure_hooks_extra(theme, &mut p)?;
    p.claude.sandbox.enabled = ask_confirm(
        theme,
        "Enable the sandbox (OS-level containment of shell commands)?",
        "Bash runs with limited file and network access (Seatbelt on macOS, bubblewrap on Linux). Safer, but new network hosts prompt until allowed. Off by default.",
        false,
    )?;
    if p.claude.sandbox.enabled {
        let extra = ask(
            theme,
            "  Extra allowed domains (comma-separated; optional)",
            "GitHub and the npm/crates/PyPI/Go/Terraform registries are already allowed.",
            Some(&p.claude.sandbox.extra_domains.join(", ")),
            true,
        )?;
        p.claude.sandbox.extra_domains = ocgen::claude::split_list(&extra);
        p.claude.sandbox.allow_credentials = ask_confirm(
            theme,
            "  Let sandboxed commands use your push/deploy credentials?",
            "No (recommended): ~/.ssh, the gh token, AWS and kube config are withheld, so an agent can't push or deploy however the command is phrased — you do those steps.",
            false,
        )?;
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
        "Sharpens your questions, answers with file:line evidence, ends with one hint toward the next, keeps a git-ignored ledger in .claude/notes/.",
        true,
    )?;
    let (intent, intent_settings) =
        configure_intent(theme, true, &ocgen::claude::IntentSettings::default())?;
    p.claude.workflow.intent = intent;
    p.claude.intent = intent_settings;
    p.claude.workflow.subagent_confidence = ask_v(
        theme,
        "Enforce a minimum confidence on subagents that write files (0–100, 0 = off)",
        "SubagentStop hook: a worker that edited files must state Confidence ≥ this % before finishing (pairs with /fanout isolation).",
        Some("96"),
        validate::confidence_threshold,
    )?
    .parse::<u8>()
    .unwrap_or(96);
    p.claude.workflow.check_cmd = ask(
        theme,
        "Check command that must pass before a worker finishes (optional)",
        "An objective gate next to self-reported confidence, e.g. cargo test, npm test, terraform validate. It runs in the worker's own directory; keep it reasonably fast.",
        Some(&p.claude.workflow.check_cmd),
        true,
    )?;
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
        "Blocks ssh, cloud mutations, git push/merge, deploys & publishes until a human runs \
         `ocgen approve` in their own terminal (time-limited; agents can't approve).",
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

/// `ocgen edit permissions` — the user's own permission rules, added to the
/// generated ones in settings.json. Flags apply directly; otherwise interactive.
pub fn run_edit_permissions(path: String, changes: PermissionsCli) -> Result<()> {
    use ocgen::claude::RuleList;
    let (root, mut project) = Project::discover(Path::new(&path))?;
    if project.target != Target::ClaudeCode {
        bail!("permission rules are a Claude Code setting; OpenCode permissions are per agent — use `ocgen edit agent`.");
    }
    if changes.list {
        print_all_permissions(&project);
        return Ok(());
    }
    if project.claude.output.plugin && !project.claude.output.project {
        bail!("this project emits only a plugin, and a plugin can't set permissions — enable the project output first.");
    }
    let before = project.claude.permissions.clone();

    if changes.is_empty() {
        if !crate::prompt::can_ask() {
            bail!("no terminal to ask in — pass the changes as flags: --allow/--ask/--deny <RULE> or --remove <RULE>");
        }
        ui::banner("edit permissions");
        edit_permissions_interactively(&ColorfulTheme::default(), &mut project)?;
    } else {
        let lists = [
            (RuleList::Allow, &changes.allow),
            (RuleList::Ask, &changes.ask),
            (RuleList::Deny, &changes.deny),
        ];
        for rule in &changes.remove {
            if !project.claude.permissions.remove(rule) {
                bail!("{rule} is not one of your rules (see `ocgen edit permissions`; generated rules can't be removed)");
            }
        }
        for (list, rules) in lists {
            for rule in rules {
                project.claude.permissions.add(list, rule)?;
                warn_if_shadowed(&project, list, rule);
            }
        }
    }

    if project.claude.permissions == before {
        println!("No changes.");
        return Ok(());
    }
    let written = project.scaffold(&root, true)?;
    report_written(&written);
    print_extra_permissions(&project);
    Ok(())
}

fn edit_permissions_interactively(theme: &ColorfulTheme, project: &mut Project) -> Result<()> {
    use ocgen::claude::RuleList;
    let generated = project.generated_permissions();
    ui::kv(
        "generated by ocgen",
        &ui::muted(&format!(
            "{} allow, {} ask, {} deny (always kept)",
            generated.allow.len(),
            generated.ask.len(),
            generated.deny.len()
        )),
    );
    print_extra_permissions(project);
    let actions = [
        "Add a rule".to_string(),
        "Remove a rule".to_string(),
        "List all rules".to_string(),
        "Done".to_string(),
    ];
    let lists = [
        "allow — run without a prompt".to_string(),
        "ask — always confirm first, even in auto mode".to_string(),
        "deny — never run".to_string(),
    ];
    loop {
        match ask_select(theme, "What now?", "", &actions, 0)? {
            0 => {
                let list = RuleList::ALL[ask_select(
                    theme,
                    "Which list?",
                    "Claude Code checks deny first, then ask, then allow.",
                    &lists,
                    0,
                )?];
                let rule = ask_v(
                    theme,
                    "Rule",
                    "Tool or Tool(specifier), e.g. Bash(gh run view:*), Read(./docs/**), WebFetch(domain:example.com), mcp__server__tool.",
                    None,
                    validate::permission_rule,
                )?;
                match project.claude.permissions.add(list, &rule) {
                    Ok(true) => warn_if_shadowed(project, list, &rule),
                    Ok(false) => println!("  {}", ui::muted("already there")),
                    Err(e) => ui::warning(&e.to_string()),
                }
            }
            1 => {
                let entries = project.claude.permissions.entries();
                if entries.is_empty() {
                    println!("  {}", ui::muted("no rules of your own yet"));
                    continue;
                }
                let items: Vec<String> = entries
                    .iter()
                    .map(|(l, r)| format!("{:<5}  {r}", l.key()))
                    .collect();
                let idx = ask_select(theme, "Remove which rule?", "", &items, 0)?;
                project.claude.permissions.remove(&entries[idx].1);
            }
            2 => print_all_permissions(project),
            _ => return Ok(()),
        }
    }
}

/// Ask whether to include `/intent` and, if so, its settings (seeded with `current`).
fn configure_intent(
    theme: &ColorfulTheme,
    enabled: bool,
    current: &ocgen::claude::IntentSettings,
) -> Result<(bool, ocgen::claude::IntentSettings)> {
    let on = ask_confirm(
        theme,
        "Include the /intent command (plan → numbered intent file → GitHub issue draft)?",
        "Improves your prompt, investigates, agrees a plan with you, writes an intent file (e.g. ADR-0007) and drafts an issue for you to file — Claude never files it.",
        enabled,
    )?;
    if !on {
        return Ok((false, current.clone()));
    }
    let mut s = current.clone();
    s.prefix = ask_v(
        theme,
        "Intent file prefix",
        "Starts every intent file name, e.g. ADR → ADR-0007-cache-invalidation.md.",
        Some(&current.prefix),
        validate::intent_prefix,
    )?;
    s.digits = ask_v(
        theme,
        "Number width (digits)",
        "Zero-padding of the number: 4 → 0007.",
        Some(&current.digits.to_string()),
        validate::intent_digits,
    )?
    .trim()
    .parse()
    .unwrap_or(current.digits);
    s.dir = ask_v(
        theme,
        "Directory for intent files",
        "Relative to the project root. Numbers already used here or on the remote main branch are never reused.",
        Some(&current.dir),
        validate::intent_dir,
    )?;
    s.max_words = ask_v(
        theme,
        "Max words in the GitHub issue description (50–1000)",
        "The issue body /intent drafts stays within this.",
        Some(&current.max_words.to_string()),
        validate::intent_max_words,
    )?
    .trim()
    .parse()
    .unwrap_or(current.max_words);
    s.branch = ask_optional_v(
        theme,
        "Remote branch to check for taken numbers (optional)",
        "Empty = the remote's default branch (origin/HEAD). Enter keeps, '-' clears.",
        &current.branch,
        validate::git_branch,
    )?;
    Ok((true, s))
}

/// `ocgen edit intent` — /intent's numbering, issue word limit and templates.
/// Flags apply directly; otherwise interactive.
pub fn run_edit_intent(path: String, changes: IntentCli) -> Result<()> {
    use ocgen::claude::{INTENT_FILE_TEMPLATE, INTENT_ISSUE_TEMPLATE};
    let (root, mut project) = Project::discover(Path::new(&path))?;
    if project.target != Target::ClaudeCode {
        bail!("/intent is a Claude Code workflow; this project targets OpenCode.");
    }
    if changes.show {
        print_intent(&project, &root);
        return Ok(());
    }
    let was = (
        project.claude.workflow.intent,
        project.claude.intent.clone(),
    );

    if changes.is_empty() {
        if !crate::prompt::can_ask() {
            bail!("no terminal to ask in — pass the changes as flags: --prefix, --digits, --dir, --max-words, --branch, --enable/--disable, --issue-template, --intent-template, --reset-issue-template, --reset-intent-template (see --help)");
        }
        ui::banner("edit /intent");
        let (on, s) = configure_intent(
            &ColorfulTheme::default(),
            project.claude.workflow.intent,
            &project.claude.intent,
        )?;
        project.claude.workflow.intent = on;
        project.claude.intent = s;
    } else {
        let s = &mut project.claude.intent;
        if let Some(v) = &changes.prefix {
            s.prefix = v.trim().to_string();
        }
        if let Some(v) = changes.digits {
            s.digits = v;
        }
        if let Some(v) = &changes.dir {
            s.dir = v.trim().trim_end_matches('/').to_string();
        }
        if let Some(v) = changes.max_words {
            s.max_words = v;
        }
        if let Some(v) = &changes.branch {
            s.branch = v.trim().to_string();
        }
        if changes.enable {
            project.claude.workflow.intent = true;
        }
        if changes.disable {
            project.claude.workflow.intent = false;
        }
    }
    // Check everything before writing anything.
    project.claude.intent.validate()?;

    if (
        project.claude.workflow.intent,
        project.claude.intent.clone(),
    ) != was
    {
        let written = project.scaffold(&root, true)?;
        if !project.claude.workflow.intent {
            remove_intent_skill(&project, &root)?;
        }
        report_written(&written);
    }
    for (reset, edit, rel, default) in [
        (
            changes.reset_issue_template,
            changes.issue_template,
            INTENT_ISSUE_TEMPLATE,
            "claude/intent/issue.md",
        ),
        (
            changes.reset_intent_template,
            changes.intent_template,
            INTENT_FILE_TEMPLATE,
            "claude/intent/intent.md",
        ),
    ] {
        if !reset && !edit {
            continue;
        }
        let path = root.join(rel);
        let current = if reset {
            templates::load(default)?
        } else {
            std::fs::read_to_string(&path).or_else(|_| templates::load(default))?
        };
        let new = if edit {
            match Editor::new().edit(&current)? {
                Some(text) => text,
                None => {
                    println!("{rel}: editor closed without saving — unchanged.");
                    continue;
                }
            }
        } else {
            current
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, new).with_context(|| format!("writing {rel}"))?;
        ui::success(&format!("{} {rel}", if reset { "reset" } else { "saved" }));
    }
    print_intent(&project, &root);
    Ok(())
}

/// Remove the generated /intent skill after it is turned off (only ocgen's own
/// copy; the user's templates and intent files stay).
fn remove_intent_skill(project: &Project, root: &Path) -> Result<()> {
    let mut dirs = vec![root.join(".claude/skills/intent")];
    if project.claude.output.plugin {
        if let Some(name) = std::fs::read_dir(root.join("plugin"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.join("skills/intent/SKILL.md").exists())
        {
            dirs.push(name.join("skills/intent"));
        }
    }
    for dir in dirs {
        let skill = dir.join("SKILL.md");
        if std::fs::read_to_string(&skill).is_ok_and(|s| s.starts_with("---\nname: intent\n")) {
            std::fs::remove_file(&skill)?;
            if std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none()) {
                let _ = std::fs::remove_dir(&dir);
            }
        }
    }
    Ok(())
}

fn print_intent(project: &Project, root: &Path) {
    use ocgen::claude::{INTENT_FILE_TEMPLATE, INTENT_ISSUE_TEMPLATE};
    let s = &project.claude.intent;
    ui::section("/intent");
    ui::kv(
        "status",
        if project.claude.workflow.intent {
            "on"
        } else {
            "off"
        },
    );
    ui::kv(
        "intent files",
        &format!("{}/{}-<slug>.md", s.dir, s.first_id()),
    );
    ui::kv("prefix", &s.prefix);
    ui::kv("digits", &s.digits.to_string());
    ui::kv("issue words", &format!("at most {}", s.max_words));
    ui::kv(
        "numbers checked on",
        &if s.branch.is_empty() {
            "the remote's default branch (origin/HEAD)".to_string()
        } else {
            format!("origin/{}", s.branch)
        },
    );
    for (label, rel) in [
        ("issue template", INTENT_ISSUE_TEMPLATE),
        ("intent template", INTENT_FILE_TEMPLATE),
    ] {
        let state = if root.join(rel).exists() {
            ui::muted("(yours)")
        } else {
            ui::muted("(missing — `ocgen doctor` recreates the default)")
        };
        ui::kv(label, &format!("{rel}  {state}"));
    }
    println!(
        "  {}",
        ui::muted(
            "Edit the templates with `ocgen edit intent --issue-template` / `--intent-template`."
        )
    );
}

/// Tell the user when a generated deny/ask rule makes theirs pointless.
fn warn_if_shadowed(project: &Project, list: ocgen::claude::RuleList, rule: &str) {
    if let Some(by) = project.shadowing_rule(list, rule) {
        ui::warning(&format!(
            "{rule} ({}) has no effect: ocgen's {} list holds it, and {} is checked first",
            list.key(),
            by.key(),
            by.key()
        ));
    }
}

/// Every rule settings.json gets, list by list in the order Claude Code checks
/// them, each marked as ocgen's or yours (and yours flagged when it has no effect).
fn print_all_permissions(project: &Project) {
    use ocgen::claude::RuleList;
    let generated = project.generated_permissions();
    let yours = &project.claude.permissions;
    ui::section("Permission rules (.claude/settings.json)");
    println!(
        "  {}",
        ui::muted("Claude Code checks deny, then ask, then allow; the first match wins.")
    );
    let width = generated
        .entries()
        .iter()
        .chain(yours.entries().iter())
        .map(|(_, r)| r.chars().count())
        .max()
        .unwrap_or(0)
        .min(48);
    for list in [RuleList::Deny, RuleList::Ask, RuleList::Allow] {
        let own: Vec<&String> = yours
            .list(list)
            .iter()
            .filter(|r| !generated.list(list).contains(r))
            .collect();
        let total = generated.list(list).len() + own.len();
        println!();
        println!("  {}", style(format!("{} ({total})", list.key())).bold());
        if total == 0 {
            println!("    {}", ui::muted("none"));
        }
        for rule in generated.list(list) {
            println!("    {rule:<width$}  {}", ui::muted("ocgen"));
        }
        for rule in own {
            let note = match project.shadowing_rule(list, rule) {
                Some(by) => style(format!("yours — no effect: ocgen's {} rule wins", by.key()))
                    .yellow()
                    .to_string(),
                None => style("yours").cyan().to_string(),
            };
            println!("    {rule:<width$}  {note}");
        }
    }
    println!();
    println!(
        "  {}",
        ui::muted("Change yours with `ocgen edit permissions --allow/--ask/--deny/--remove <RULE>`; ocgen's always stay.")
    );
}

fn print_extra_permissions(project: &Project) {
    let entries = project.claude.permissions.entries();
    ui::section(&format!("Your permission rules ({})", entries.len()));
    if entries.is_empty() {
        println!("  {}", ui::muted("none"));
    }
    for (list, rule) in entries {
        println!("  {}  {rule}", style(format!("{:<5}", list.key())).cyan());
    }
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
        agents.push(prompt_claude_agent(
            theme,
            language,
            &names,
            &ClaudeAgentCtx::default(),
        )?);
    }
    Ok(agents)
}

/// Project facts the Claude agent editor offers as choices.
#[derive(Default)]
struct ClaudeAgentCtx {
    skills: Vec<String>,
    mcp_servers: Vec<String>,
}

impl ClaudeAgentCtx {
    fn of(project: &Project) -> Self {
        Self {
            skills: project.skills.iter().map(|s| s.name.clone()).collect(),
            mcp_servers: project
                .claude
                .mcp_servers
                .iter()
                .map(|s| s.name.clone())
                .collect(),
        }
    }
}

fn prompt_claude_agent(
    theme: &ColorfulTheme,
    language: &str,
    taken: &[String],
    ctx: &ClaudeAgentCtx,
) -> Result<Agent> {
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
    configure_claude_agent(theme, language, seed, false, taken, ctx)
}

/// The subagent-only frontmatter: hard tool denies, effort, permission mode,
/// memory, background, preloaded skills and MCP servers. Each prompt says why.
fn configure_claude_agent_extras(
    theme: &ColorfulTheme,
    a: &mut Agent,
    ctx: &ClaudeAgentCtx,
) -> Result<()> {
    use ocgen::claude::{split_list, EFFORT_LEVELS, MEMORY_SCOPES};
    a.disallowed_tools = ask(
        theme,
        "  Disallowed tools (optional)",
        "Removed even if tools would allow them — Edit, Write, NotebookEdit makes a role hard read-only.",
        Some(&a.disallowed_tools),
        true,
    )?;
    let pick = |label: &str, help: &str, opts: &[&str], none: &str, cur: &str| -> Result<String> {
        let mut items = vec![none.to_string()];
        items.extend(opts.iter().map(|s| s.to_string()));
        let idx = opts
            .iter()
            .position(|o| *o == cur)
            .map(|i| i + 1)
            .unwrap_or(0);
        let i = ask_select(theme, label, help, &items, idx)?;
        Ok(if i == 0 {
            String::new()
        } else {
            opts[i - 1].to_string()
        })
    };
    a.effort = pick(
        "  Effort",
        "Higher thinks harder but costs more: high for reviewers, low for quick lookups.",
        &EFFORT_LEVELS,
        "inherit (session effort)",
        &a.effort,
    )?;
    a.permission_mode = pick(
        "  Permission mode",
        "plan = read-only analysis; acceptEdits = edits without asking. Leave inherit unless you need it.",
        &["plan", "acceptEdits", "dontAsk", "default"],
        "inherit",
        &a.permission_mode,
    )?;
    a.memory = pick(
        "  Memory",
        "Lets the agent keep notes across sessions: project = committed .claude/agent-memory/, local = git-ignored, user = all projects.",
        &MEMORY_SCOPES,
        "none",
        &a.memory,
    )?;
    a.background = ask_confirm(
        theme,
        "  Run in the background?",
        "It works while you keep talking to the main session.",
        a.background,
    )?;
    let multi = |label: &str, help: &str, items: &[String], cur: &str| -> Result<String> {
        if items.is_empty() {
            return Ok(cur.to_string());
        }
        let now = split_list(cur);
        let defaults: Vec<bool> = items.iter().map(|i| now.contains(i)).collect();
        let chosen = ask_multi(
            theme,
            label,
            help,
            &items.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            &defaults,
        )?;
        Ok(chosen
            .iter()
            .map(|&i| items[i].clone())
            .collect::<Vec<_>>()
            .join(", "))
    };
    a.preload_skills = multi(
        "  Preload skills",
        "Their full text is loaded when the agent starts — only for skills it always needs.",
        &ctx.skills,
        &a.preload_skills,
    )?;
    a.mcp_servers = multi(
        "  MCP servers it may use",
        "mcp__<server> is added to its tools automatically.",
        &ctx.mcp_servers,
        &a.mcp_servers,
    )?;
    Ok(())
}

/// Walk a Claude agent's fields (mode, alias, tools, description, body), seeded
/// with its current values. Shared by add (from a preset) and edit.
fn configure_claude_agent(
    theme: &ColorfulTheme,
    language: &str,
    mut a: Agent,
    allow_rename: bool,
    taken: &[String],
    ctx: &ClaudeAgentCtx,
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
        "opus / sonnet / haiku / fable / inherit, or a full ID like claude-opus-5-5",
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
        configure_claude_agent_extras(theme, &mut a, ctx)?;
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
        prompt_claude_agent(
            &theme,
            &project.language,
            &taken,
            &ClaudeAgentCtx::of(&project),
        )?
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
            &ClaudeAgentCtx::of(&project),
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
pub fn run_doctor(path: String, dry_run: bool, yes: bool) -> Result<()> {
    let theme = ColorfulTheme::default();
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

    // Permission rules added to settings.json by hand would be dropped by the
    // rewrite: offer to keep them as the user's rules first, so the plan below
    // already includes them.
    if project.target == Target::ClaudeCode {
        adopt_hand_added_permissions(&theme, &mut project, &target, dry_run, yes)?;
    }

    // Show exactly what regenerating would do before touching anything.
    let plan = project.plan_changes(&target)?;
    let changed: Vec<_> = plan
        .iter()
        .filter(|c| c.kind != ChangeKind::Unchanged)
        .collect();
    let unchanged = plan.len() - changed.len();
    ui::section(&format!("Changes ({})", changed.len()));
    if changed.is_empty() {
        ui::success("every generated file is up to date");
    }
    for c in &changed {
        print_change(c);
    }
    if unchanged > 0 {
        println!("  {}", ui::muted(&format!("{unchanged} file(s) unchanged")));
    }
    if changed.iter().any(|c| c.hand_edited == Some(true)) {
        ui::tip("ocgen owns these files and rewrites them. Keep personal settings in .claude/settings.local.json (never touched); change the rest with `ocgen edit …` or `ocgen templates edit`.");
    }

    if dry_run {
        println!("\n{}", style("dry run — nothing written").bold());
        return Ok(());
    }
    let conflicts: Vec<&FileChange> = plan
        .iter()
        .filter(|c| c.kind == ChangeKind::Modified)
        .collect();
    let keep = if changed.is_empty() || yes || !crate::prompt::can_ask() {
        BTreeSet::new()
    } else if conflicts.is_empty() {
        // Only new or stale files: nothing of yours is overwritten.
        if !ask_confirm(
            &theme,
            "Apply these changes?",
            "Stale files are backed up before they're removed.",
            true,
        )? {
            println!("Nothing written.");
            return Ok(());
        }
        BTreeSet::new()
    } else {
        match choose_keep(
            &theme,
            &conflicts,
            "Apply these changes?",
            [
                format!(
                    "Apply all ({}) — previous versions backed up",
                    changed.len()
                ),
                "Keep all changed files — only add new ones".to_string(),
            ],
            0,
        )? {
            Some(keep) => keep,
            None => {
                println!("Nothing written.");
                return Ok(());
            }
        }
    };

    let applied: Vec<FileChange> = plan
        .iter()
        .filter(|c| !keep.contains(&c.rel))
        .cloned()
        .collect();
    let backup = Project::backup(&target, &applied)?;
    // Rewrite everything except what the user kept (also upgrades the state file).
    project.scaffold_keeping(&target, &keep)?;
    match project.install_pre_push(&target)? {
        ocgen::render::PrePush::Installed(_) => {
            println!("  {}", ui::muted("git pre-push hook: installed (blocks unapproved pushes made under Claude Code)"))
        }
        ocgen::render::PrePush::Foreign(_) => ui::warning(
            "another git pre-push hook is in place — add `sh .claude/hooks/git-pre-push.sh || exit 1` to it to gate pushes",
        ),
        ocgen::render::PrePush::NotApplicable => {}
    }
    println!();
    ui::success(&format!("applied {} change(s)", changed.len() - keep.len()));
    report_backup(&target, backup.as_deref());
    report_kept(&keep);
    // Not auto-fixable: the project's .gitignore belongs to the user.
    if project.target == Target::ClaudeCode
        && ocgen::gitcheck::claude_config_ignored(&target) == Some(true)
    {
        ui::warning(ocgen::gitcheck::IGNORED_CONFIG_WARNING);
    }
    Ok(())
}

/// Find permission rules added to settings.json by hand and offer to keep them as
/// the user's own (as `ocgen edit permissions --allow …` would). A dry run or
/// `--yes` keeps them without asking, as does a run without a terminal; a dry run
/// only plans with them (nothing is saved).
fn adopt_hand_added_permissions(
    theme: &ColorfulTheme,
    project: &mut Project,
    target: &Path,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let found = project.hand_added_permissions(target);
    let entries = found.rules.entries();
    if entries.is_empty() && found.invalid.is_empty() {
        return Ok(());
    }
    ui::section(&format!(
        "Permission rules added by hand ({})",
        entries.len()
    ));
    for (list, rule) in &entries {
        println!("  {}  {rule}", style(format!("{:<5}", list.key())).cyan());
    }
    for bad in &found.invalid {
        ui::warning(&format!(
            "{bad}: not a valid permission rule — it will be dropped"
        ));
    }
    if entries.is_empty() {
        return Ok(());
    }
    let keep = dry_run
        || yes
        || !crate::prompt::can_ask()
        || ask_confirm(
            theme,
            "Keep them as your rules?",
            "Saved like `ocgen edit permissions`, so every regeneration keeps them. No = they're dropped (the old settings.json is backed up).",
            true,
        )?;
    if !keep {
        return Ok(());
    }
    let mut kept = 0;
    for (list, rule) in &entries {
        match project.claude.permissions.add(*list, rule) {
            Ok(_) => {
                kept += 1;
                warn_if_shadowed(project, *list, rule);
            }
            Err(e) => ui::warning(&e.to_string()),
        }
    }
    if dry_run {
        println!(
            "  {}",
            ui::muted(&format!(
                "would keep {kept} rule(s) as yours — the plan below includes them"
            ))
        );
    } else {
        ui::success(&format!(
            "kept {kept} rule(s) as yours (see `ocgen edit permissions --list`)"
        ));
    }
    Ok(())
}

/// One planned change: a marker, the path, why, and a short diff.
fn print_change(c: &FileChange) {
    let (mark, note) = match c.kind {
        ChangeKind::Added => (style("+").green(), "new".to_string()),
        ChangeKind::Removed => (style("-").red(), "stale — no longer generated".to_string()),
        ChangeKind::Modified => (
            style("~").yellow(),
            match c.hand_edited {
                Some(true) if c.rel.ends_with("settings.json") => "edited by hand — your edits will be replaced (backed up); personal settings belong in .claude/settings.local.json".to_string(),
                Some(true) => "edited by hand — your edits will be replaced (backed up)".to_string(),
                Some(false) => "updated by ocgen".to_string(),
                None => "differs (this project predates edit tracking)".to_string(),
            },
        ),
        ChangeKind::Unchanged => return,
    };
    println!("  {mark} {}  {}", style(&c.rel).bold(), ui::muted(&note));
    if let (Some(old), Some(new)) = (&c.old, &c.new) {
        print_diff(old, new, 1, 12);
    }
}

/// A coloured line diff: `context` unchanged lines around each change, at most
/// `max` lines (`…` marks a gap or the cut-off).
fn print_diff(old: &str, new: &str, context: usize, max: usize) {
    use ocgen::diff::{hunks, line_diff, DiffLine};
    for line in hunks(&line_diff(old, new), context, max) {
        match line {
            Some(DiffLine::Removed(l)) => println!("      {}", style(format!("- {l}")).red()),
            Some(DiffLine::Added(l)) => println!("      {}", style(format!("+ {l}")).green()),
            Some(DiffLine::Same(l)) => println!("      {}", ui::muted(&format!("  {l}"))),
            None => println!("      {}", ui::muted("…")),
        }
    }
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

/// Opt-in hooks beyond the gates. Declining the power-user defaults turns them
/// all off (a minimal settings.json); otherwise each is offered with its reason.
fn configure_hooks_extra(theme: &ColorfulTheme, p: &mut Project) -> Result<()> {
    if !p.claude.powerups.hooks {
        p.claude.hooks_extra = ocgen::claude::HooksExtra {
            compact_context: false,
            ..Default::default()
        };
        return Ok(());
    }
    let items = [
        "Re-inject context after compaction — re-points Claude at the rules and any /inquire notes",
        "Desktop notification when Claude needs you or a turn fails",
        "Log settings/skills changes made during a session (.claude/audit/, git-ignored)",
    ];
    let x = &p.claude.hooks_extra;
    let defaults = [x.compact_context, x.notify, x.config_audit];
    let chosen = ask_multi(
        theme,
        "Extra hooks",
        "Space toggles, Enter confirms. None of these ever block Claude.",
        &items.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
        &defaults,
    )?;
    p.claude.hooks_extra.compact_context = chosen.contains(&0);
    p.claude.hooks_extra.notify = chosen.contains(&1);
    p.claude.hooks_extra.config_audit = chosen.contains(&2);
    p.claude.hooks_extra.format_cmd = ask(
        theme,
        "Formatter to run after Claude edits a file (optional)",
        "e.g. cargo fmt, terraform fmt -recursive, npx prettier --write . — failures never block.",
        Some(&p.claude.hooks_extra.format_cmd),
        true,
    )?;
    Ok(())
}
