//! Claude Code–specific project configuration and the Skill data model.
//!
//! These fields live on [`crate::render::Project`] and are only meaningful when
//! its `target` is [`crate::target::Target::ClaudeCode`]. Everything is
//! `#[serde(default)]` so OpenCode state files (which omit them) still load.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::archetype::pick;
use crate::templates;

/// Built-in Claude Code tools that can appear in a skill's `allowed-tools`.
pub const CLAUDE_TOOLS: [&str; 12] = [
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "Bash",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "Task",
    "NotebookEdit",
    "Skill",
];

/// Entries in a comma-separated tool list that are not a known built-in, a
/// `Bash(...)`/`Tool(...)` pattern on a known tool, or an `mcp__…` server tool.
/// Used to *warn* (not reject) on likely typos.
pub fn unknown_tools(list: &str) -> Vec<String> {
    list.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter(|t| {
            !t.starts_with("mcp__")
                && !CLAUDE_TOOLS.contains(&t.split('(').next().unwrap_or(t).trim())
        })
        .map(|t| t.to_string())
        .collect()
}

/// A reusable skill preset (`claude/skill-presets.toml`) that seeds a [`Skill`]
/// with sensible frontmatter and a numbered-step body.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillPreset {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub allowed_tools: String,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default)]
    pub context_fork: bool,
    #[serde(default)]
    pub agent: String,
    /// Language-keyed numbered-step body.
    pub body: HashMap<String, String>,
}

impl SkillPreset {
    /// Turn this preset into a seed [`Skill`] with the given name and language.
    pub fn to_skill(&self, name: &str, lang: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: self.description.clone(),
            allowed_tools: self.allowed_tools.clone(),
            body: pick(&self.body, lang),
            disable_model_invocation: self.disable_model_invocation,
            context_fork: self.context_fork,
            agent: self.agent.clone(),
            ..Default::default()
        }
    }
}

/// Load the skill presets (embedded, override-able).
pub fn skill_presets() -> Result<Vec<SkillPreset>> {
    #[derive(Deserialize)]
    struct Presets {
        #[serde(default, rename = "preset")]
        presets: Vec<SkillPreset>,
    }
    let src = templates::load("claude/skill-presets.toml")?;
    Ok(toml::from_str::<Presets>(&src)?.presets)
}

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
    /// Emit the `/team-plan` command and gate task creation on an approved plan
    /// (`.claude/team/plan.md` must contain `Status: APPROVED`).
    pub plan_gate: bool,
    /// Minimum self-assessed confidence (0–100) a teammate must record before a
    /// task may be completed. `0` disables the gate. Defaults to 96 in the wizard.
    pub confidence_threshold: u8,
    /// Require a mitigation round for every risk in the plan's risk register
    /// before teammates may go idle.
    pub risk_rounds: bool,
    /// Gate high-impact external / substantial-side-effect actions (ssh, cloud
    /// mutations, git push/merge, deploys, publishes) behind a human-created
    /// approval marker, enforced deterministically by a PreToolUse hook. This is
    /// emitted independently of `hooks` so the safety line is never silently off.
    pub approval_gate: bool,
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
    /// Emit the `/improve-prompt` command (improves a prompt in-session using
    /// Anthropic's prompt-engineering technique). The matching skill preset is
    /// available via `ocgen add skill`.
    pub improve_prompt: bool,
    /// Emit the `/fanout` command + `.worktreeinclude` + CLAUDE.md protocol for
    /// fanning work out to worktree-isolated subagents.
    pub fanout: bool,
    /// Emit CLAUDE.md guidance to build self-checking/verification into the todo
    /// list for complex prompts (definition-of-done, per-task verify, self-review,
    /// confidence self-rating).
    pub verify_todos: bool,
    /// Emit the `/deliver` end-to-end pipeline command + delivery/multi-session
    /// guidance.
    pub deliver: bool,
    /// Emit the `/inquire` codebase-understanding loop (sharpen each question,
    /// answer with file:line evidence, suggest next questions, keep a git-ignored
    /// ledger under `.claude/notes/`) and route `/deliver` "understand" goals to it.
    pub inquire: bool,
    /// Minimum confidence (0–100) a subagent that wrote files must state before it
    /// may stop, enforced by a `SubagentStop` hook. `0` disables. This pairs with
    /// worktree isolation to give isolated writes + enforced per-worker confidence.
    pub subagent_confidence: u8,
    /// How many times one gate may block the same agent on the same thing before
    /// the loop guard escalates (quality gates release the agent, marked
    /// UNRESOLVED; approval gates stay closed and halt it). `0` = unbounded.
    pub loop_guard_max: u8,
}

impl Default for Workflow {
    fn default() -> Self {
        Self {
            intake: true,
            refine: true,
            improve_prompt: true,
            fanout: true,
            verify_todos: true,
            deliver: true,
            inquire: true,
            subagent_confidence: 96,
            loop_guard_max: 3,
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
    /// Extra trigger context appended to the description (`when_to_use`).
    pub when_to_use: String,
    /// Autocomplete hint for arguments (`argument-hint`).
    pub argument_hint: String,
    /// `disable-model-invocation: true` — a user-run-only task skill.
    pub disable_model_invocation: bool,
    /// Emit `user-invocable: false` (Claude-only background knowledge).
    pub hidden_from_menu: bool,
    /// Run in a forked subagent context (`context: fork`).
    pub context_fork: bool,
    /// Subagent type to fork into (`agent`, only with `context_fork`).
    pub agent: String,
    /// Model alias to pin for this skill's turn (empty = inherit).
    pub model: String,
}
