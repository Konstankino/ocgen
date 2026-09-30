//! Command-line surface. The whole tool is interactive, so flags are minimal:
//! subcommand routing plus an optional target directory for `new`.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "ocgen",
    version,
    about = "Interactively scaffold OpenCode multi-agent projects"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the scaffolding wizard (default when no subcommand is given).
    New {
        /// Directory to create the project in (otherwise the wizard asks).
        path: Option<String>,
        /// Which platform to generate for.
        #[arg(long, value_enum, default_value_t = TargetArg::Opencode)]
        target: TargetArg,
        /// Base URL for the seeded provider(s) (OpenCode target only).
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        /// What to emit for the Claude target.
        #[arg(long, value_enum, default_value_t = OutputArg::Project)]
        output: OutputArg,
        /// GitHub owner/repo for the Claude plugin distribution.
        #[arg(long, value_name = "OWNER/REPO")]
        repo: Option<String>,
        /// Enable Claude Code Agent Teams (experimental) in the generated project.
        #[arg(long)]
        team: bool,
        /// Default minimum teammate confidence (0–100, 0 = off) for the governance gate.
        #[arg(long, value_name = "N")]
        team_confidence: Option<u8>,
        /// Default the plan-approval gate (/team-plan) to off in the wizard.
        #[arg(long)]
        no_plan_gate: bool,
        /// Default risk-mitigation rounds to off in the wizard.
        #[arg(long)]
        no_risk_rounds: bool,
        /// Default the execution approval gate (high-impact external actions) to off.
        #[arg(long)]
        no_approval_gate: bool,
    },
    /// Add something to an existing project.
    Add {
        #[command(subcommand)]
        what: AddWhat,
    },
    /// Edit something in an existing project.
    Edit {
        #[command(subcommand)]
        what: EditWhat,
    },
    /// Show an overview of all agents and providers and how they fit together.
    #[command(visible_alias = "horizon")]
    Landscape {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Repair a project's config (and upgrade an old state file), then rewrite files.
    Doctor {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// Show what would change (with diffs) and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Apply without asking for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Show the full configuration of one agent.
    Show {
        #[command(subcommand)]
        what: ShowWhat,
    },
    /// Run a generated project's hook (called by Claude Code, not by hand).
    ///
    /// Reads the event JSON on stdin and the gate settings from the environment,
    /// exactly like the `.claude/hooks/*.sh` scripts it replaces.
    Hook {
        /// Hook name, e.g. team-approval-gate.
        name: Option<String>,
        /// Print the hook protocol version and exit (hook commands probe this
        /// before using the binary, falling back to the scripts otherwise).
        #[arg(long)]
        check: bool,
    },
    /// Approve high-impact actions (pushes, deploys…) for a limited time.
    ///
    /// For a human, from their own terminal: it refuses to run under Claude Code
    /// or without a terminal. The approval lives outside the project and expires
    /// by itself.
    Approve {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// How long the approval lasts.
        #[arg(long, default_value_t = 30)]
        minutes: u64,
        /// Re-lock now.
        #[arg(long)]
        revoke: bool,
        /// Show whether execution is approved, and for how long.
        #[arg(long)]
        status: bool,
    },
    /// Check that a generated project works: up to date, valid settings, hook
    /// commands run, the approval gate blocks, the statusline renders, a
    /// compatible ocgen is on PATH, and Claude Code's validator passes.
    /// Exits 1 if any check fails (usable in CI).
    Verify {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// Skip `claude plugin validate` (no Claude Code CLI needed).
        #[arg(long)]
        no_claude: bool,
        /// Also run the project's check command (usually its tests; may be slow).
        #[arg(long)]
        run_check: bool,
    },
    /// Print a recommended organisation policy (managed-settings.json) that
    /// projects can't override.
    ManagedSettings,
    /// Explain every configurable agent/provider field in detail.
    #[command(visible_alias = "reference")]
    Fields,
    /// Manage the editable template set.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
}

/// Team governance defaults gathered from CLI flags, used to seed the wizard's
/// Agent Teams prompts (the wizard stays interactive; these set the defaults).
pub struct TeamCli {
    pub enabled: bool,
    pub confidence: Option<u8>,
    pub plan_gate: bool,
    pub risk_rounds: bool,
    pub approval_gate: bool,
}

/// Which platform `ocgen new` generates for.
#[derive(Clone, Copy, ValueEnum)]
pub enum TargetArg {
    Opencode,
    Claude,
}

/// What the Claude target emits.
#[derive(Clone, Copy, ValueEnum)]
pub enum OutputArg {
    Project,
    Plugin,
    Both,
}

#[derive(Subcommand)]
pub enum AddWhat {
    /// Interactively add one agent to an existing project.
    Agent {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Interactively add one provider to an existing project.
    Provider {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Author a new Claude Code skill (Claude projects only).
    Skill {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Add an MCP server to `.mcp.json` (Claude projects only).
    Mcp {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ShowWhat {
    /// Show one agent's configuration (you pick from a list if no name is given).
    Agent {
        /// Name of the agent to show.
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

#[derive(Subcommand)]
pub enum EditWhat {
    /// Interactively edit the fields of an existing agent.
    Agent {
        /// Name of the agent to edit (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Interactively edit an existing provider (and its models).
    Provider {
        /// Key of the provider to edit (otherwise you pick from a list).
        key: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Edit an existing Claude Code skill.
    Skill {
        /// Name of the skill to edit (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Edit or remove an MCP server (Claude projects only).
    Mcp {
        /// Name of the server (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Enable or adjust Claude Code Agent Teams (and its governance gates).
    Team {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

#[derive(Subcommand)]
pub enum TemplatesAction {
    /// Copy the embedded templates into ~/.config/ocgen/templates for editing.
    Init,
    /// Print the template override directory.
    Path,
    /// List resolved templates, marking overridden ones.
    List,
    /// Edit a template in $EDITOR, saving your changes to the override dir.
    Edit {
        /// Template path to edit, e.g. seeds.toml or archetypes/reviewer.toml
        /// (otherwise you pick one from a list).
        path: Option<String>,
    },
}
