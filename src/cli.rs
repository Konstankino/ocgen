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
    },
    /// Show the full configuration of one agent.
    Show {
        #[command(subcommand)]
        what: ShowWhat,
    },
    /// Explain every configurable agent/provider field in detail.
    #[command(visible_alias = "reference")]
    Fields,
    /// Manage the editable template set.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
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
