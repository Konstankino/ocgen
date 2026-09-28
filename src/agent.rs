//! A fully-resolved agent. Every field the agent file needs lives here, so
//! rendering never has to consult an archetype and the saved project state is
//! self-describing. Archetypes are only used (in the wizard) to seed these values.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::archetype::Archetype;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)] // tolerate state files from older versions that lack newer fields
pub struct Agent {
    pub name: String,
    /// Free-text role label (preset name or user-typed). Informational; not emitted.
    pub role: String,
    /// `primary`, `subagent`, or `all`.
    pub mode: String,
    /// Provider key this agent's model belongs to.
    pub provider: String,
    pub model: String,
    /// Provider model variant (empty = omit).
    pub variant: String,
    pub temperature: String,
    /// Nucleus sampling, alternative to temperature (empty = omit).
    pub top_p: String,
    pub steps: Option<u32>,
    pub color: String,
    /// Disable the agent without deleting it.
    pub disable: bool,
    /// Hide a subagent from the `@` autocomplete menu.
    pub hidden: bool,
    pub description: String,
    /// Raw YAML permission block, two-space indented, no task section.
    pub permissions: String,
    /// Claude Code tool allow-list, e.g. `Read, Grep, Edit` (Claude target only;
    /// OpenCode ignores it). Empty = inherit all tools.
    pub tools: String,
    /// Raw YAML block of provider-specific model options (empty = omit).
    pub options: String,
    /// Claude Code isolation mode (Claude target only). `worktree` runs the
    /// subagent in its own enforced git worktree; empty = no isolation.
    pub isolation: String,
    /// Markdown system-prompt body.
    pub body: String,
    /// Whether to emit an external `.opencode/prompts/<name>.txt` file.
    pub prompt_file: bool,
    /// External prompt content (used when `prompt_file` is true).
    pub prompt_body: Option<String>,
}

impl Agent {
    /// Seed a fully-populated agent from an archetype preset, for the given language
    /// and default provider. Every field is copied so later edits are independent.
    pub fn from_archetype(name: &str, archetype: &str, lang: &str, provider: &str) -> Result<Self> {
        let arch = Archetype::load(archetype)?;
        Ok(Agent {
            name: name.to_string(),
            role: archetype.to_string(),
            mode: arch.mode.clone(),
            provider: provider.to_string(),
            model: arch.default_model.clone(),
            temperature: arch.temperature.clone(),
            steps: arch.steps,
            color: arch.color.clone(),
            description: arch.description_for(lang),
            permissions: arch.permissions.trim_end().to_string(),
            body: arch.body_for(lang),
            prompt_file: arch.prompt_file,
            prompt_body: arch.prompt_for(lang),
            ..Default::default()
        })
    }

    /// A blank agent with neutral defaults, for building a custom role from scratch.
    pub fn blank(name: &str, role: &str, provider: &str) -> Self {
        Agent {
            name: name.to_string(),
            role: role.to_string(),
            mode: "subagent".to_string(),
            provider: provider.to_string(),
            temperature: "0.2".to_string(),
            color: "accent".to_string(),
            permissions: "  edit: ask\n  bash:\n    \"*\": ask".to_string(),
            ..Default::default()
        }
    }

    /// Seed a Claude Code agent from an archetype: model becomes a Claude alias,
    /// permissions become a `tools` allow-list, and a coordinator's rich prompt
    /// becomes the body (it renders into CLAUDE.md).
    pub fn from_archetype_claude(name: &str, archetype: &str, lang: &str) -> Result<Self> {
        let arch = Archetype::load(archetype)?;
        let body = if arch.mode == "primary" {
            arch.prompt_for(lang).unwrap_or_else(|| arch.body_for(lang))
        } else {
            arch.body_for(lang)
        };
        Ok(Agent {
            name: name.to_string(),
            role: archetype.to_string(),
            mode: arch.mode.clone(),
            provider: String::new(),
            model: arch.claude_model(),
            tools: arch.tools(),
            color: claude_color(&arch.color),
            description: arch.description_for(lang),
            body,
            isolation: arch.claude_isolation(),
            ..Default::default()
        })
    }
}

/// The default team the wizard seeds, read from the manifest's `[[pipeline]]`
/// entries (each names an agent and the archetype it starts from). Fully
/// data-driven, so the default team is not tied to any particular domain.
pub fn default_pipeline(lang: &str, provider: &str) -> Result<Vec<Agent>> {
    let manifest = crate::manifest::Manifest::load()?;
    manifest
        .pipeline
        .iter()
        .map(|p| Agent::from_archetype(&p.name, &p.archetype, lang, provider))
        .collect()
}

/// The default team for the Claude Code target (aliases + tools from archetypes).
pub fn claude_default_pipeline(lang: &str) -> Result<Vec<Agent>> {
    let manifest = crate::manifest::Manifest::load()?;
    manifest
        .pipeline
        .iter()
        .map(|p| Agent::from_archetype_claude(&p.name, &p.archetype, lang))
        .collect()
}

/// Map an OpenCode archetype colour to a Claude Code named colour.
fn claude_color(c: &str) -> String {
    match c {
        "accent" => "blue",
        "warning" => "yellow",
        "success" => "green",
        "error" => "red",
        other if other.starts_with('#') => "cyan",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_color_maps_all_branches() {
        assert_eq!(claude_color("accent"), "blue");
        assert_eq!(claude_color("warning"), "yellow");
        assert_eq!(claude_color("success"), "green");
        assert_eq!(claude_color("error"), "red");
        assert_eq!(claude_color("#4ec9b0"), "cyan");
        assert_eq!(claude_color("purple"), "purple"); // passthrough
    }

    #[test]
    fn from_archetype_claude_maps_role_to_alias_and_tools() {
        // Subagent role: alias + tools from the archetype, provider cleared.
        let rev = Agent::from_archetype_claude("explorer", "reviewer", "English").unwrap();
        assert_eq!(rev.mode, "subagent");
        assert_eq!(rev.model, "opus");
        assert_eq!(rev.tools, "Read, Grep, Glob");
        assert_eq!(rev.provider, "");
        assert!(!rev.description.is_empty());

        // Coordinator role: uses the archetype's rich prompt as its body.
        let boss = Agent::from_archetype_claude("coordinator", "coordinator", "English").unwrap();
        assert_eq!(boss.mode, "primary");
        assert_eq!(boss.model, "opus");
        assert!(!boss.body.is_empty());
    }

    #[test]
    fn from_archetype_claude_sets_worktree_isolation_for_the_writer() {
        // The implementer edits, so it runs in its own worktree.
        let imp = Agent::from_archetype_claude("impl", "implementer", "English").unwrap();
        assert_eq!(imp.isolation, "worktree");
        // Read-only roles are not isolated.
        let rev = Agent::from_archetype_claude("rev", "reviewer", "English").unwrap();
        assert_eq!(rev.isolation, "");
    }

    #[test]
    fn claude_default_pipeline_uses_manifest() {
        let team = claude_default_pipeline("English").unwrap();
        let names: Vec<&str> = team.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["coordinator", "explorer", "implementer", "reviewer"]
        );
        assert!(team.iter().any(|a| a.mode == "primary"));
    }
}
