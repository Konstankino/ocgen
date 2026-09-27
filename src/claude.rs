//! Claude Code–specific project configuration and the Skill data model.
//!
//! These fields live on [`crate::render::Project`] and are only meaningful when
//! its `target` is [`crate::target::Target::ClaudeCode`]. Everything is
//! `#[serde(default)]` so OpenCode state files (which omit them) still load.

use serde::{Deserialize, Serialize};

/// Claude Code generation settings: the global default model alias, the editable
/// `CLAUDE.md` body, what to emit, which power-user defaults to include, which
/// workflow assets to bake in, and plugin/marketplace metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    /// Global default model alias written to settings.json (opus/sonnet/haiku/…).
    pub model: String,
    /// The CLAUDE.md body (project instructions), editable in the wizard.
    pub instructions: String,
    pub output: Output,
    pub powerups: Powerups,
    pub workflow: Workflow,
    pub plugin: PluginMeta,
    pub team: Team,
}

/// Claude Code Agent Teams settings (experimental, opt-in). When enabled, the
/// generated settings.json sets `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` and a
/// `teammateMode`, and a `/team` command + CLAUDE.md guidance are emitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Team {
    pub enabled: bool,
    /// Teammate display mode: in-process (default) / auto / tmux / iterm2.
    pub mode: String,
    /// Emit commented TeammateIdle/TaskCreated/TaskCompleted hook stubs.
    pub hooks: bool,
}

/// Which artifact trees to write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub project: bool,
    pub plugin: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            project: true,
            plugin: false,
        }
    }
}

/// Power-user defaults to fold into the generated settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Powerups {
    pub permissions: bool,
    pub hooks: bool,
    pub output_style: bool,
    pub statusline: bool,
}

impl Default for Powerups {
    fn default() -> Self {
        Self {
            permissions: true,
            hooks: true,
            output_style: true,
            statusline: true,
        }
    }
}

/// Which reusable workflow commands to emit into the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Workflow {
    pub intake: bool,
    pub refine: bool,
}

impl Default for Workflow {
    fn default() -> Self {
        Self {
            intake: true,
            refine: true,
        }
    }
}

/// Plugin/marketplace metadata for the distributable output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMeta {
    pub repo_owner: String,
    pub repo_name: String,
    pub version: String,
    pub display_name: String,
}

/// A Claude Code skill (`.claude/skills/<name>/SKILL.md`). May be authored
/// standalone or derived from an agent role (`role` names the archetype).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Comma-separated allow-list, e.g. `Read, Grep, Bash(git *)`.
    pub allowed_tools: String,
    pub body: String,
    /// The archetype this skill was derived from, if any.
    pub role: Option<String>,
}
