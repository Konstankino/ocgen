//! Named project presets (`presets/<name>.toml`): a bundled, editable seed for a
//! whole Claude Code project — its agent pipeline, model, Agent Teams settings,
//! skills, extra commands, and CLAUDE.md instructions. Selected with
//! `ocgen new --target claude --preset <name>`.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use crate::agent::Agent;
use crate::archetype::pick;
use crate::claude::{skill_presets, Team};
use crate::manifest::PipelineAgent;
use crate::render::Project;
use crate::target::Target;
use crate::templates;

/// Agent Teams settings a preset can request (maps onto `claude::Team`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TeamSpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub hooks: bool,
}

/// A whole-project preset.
#[derive(Debug, Clone, Deserialize)]
pub struct Preset {
    pub description: String,
    /// Default Claude model alias for the seeded agents.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub team: TeamSpec,
    /// Agent roster: (name -> archetype). Reuses the manifest pipeline type.
    #[serde(default, rename = "pipeline")]
    pub pipeline: Vec<PipelineAgent>,
    /// Skill-preset names to seed into the project.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Extra command templates to emit (`claude/commands/<name>.md.j2`).
    #[serde(default)]
    pub commands: Vec<String>,
    /// CLAUDE.md body, language-keyed (English fallback).
    #[serde(default)]
    pub instructions: HashMap<String, String>,
}

impl Preset {
    pub fn load(name: &str) -> Result<Self> {
        let src = templates::load(&format!("presets/{name}.toml"))?;
        Ok(toml::from_str(&src)?)
    }

    /// CLAUDE.md instructions for `lang` (English fallback).
    pub fn instructions_for(&self, lang: &str) -> String {
        pick(&self.instructions, lang)
    }

    /// Build a Claude Code [`Project`] seed from this preset (non-interactive).
    /// Agents come from the pipeline archetypes (so their model alias/tools are
    /// per-role); the preset's `model` becomes the settings.json default.
    pub fn build(&self, language: &str) -> Result<Project> {
        let mut p = Project {
            target: Target::ClaudeCode,
            language: language.to_string(),
            ..Default::default()
        };
        p.agents = self
            .pipeline
            .iter()
            .map(|pa| Agent::from_archetype_claude(&pa.name, &pa.archetype, language))
            .collect::<Result<_>>()?;
        p.claude.model = if self.model.trim().is_empty() {
            "sonnet".to_string()
        } else {
            self.model.clone()
        };
        p.claude.team = Team {
            enabled: self.team.enabled,
            mode: self.team.mode.clone(),
            hooks: self.team.hooks,
        };
        p.claude.commands = self.commands.clone();
        p.claude.instructions = self.instructions_for(language);

        let sps = skill_presets()?;
        for name in &self.skills {
            if let Some(sp) = sps.iter().find(|x| &x.name == name) {
                p.skills.push(sp.to_skill(name, language));
            }
        }
        Ok(p)
    }
}

/// Every available preset name (embedded ∪ override), sorted.
pub fn names() -> Vec<String> {
    templates::preset_names()
}
