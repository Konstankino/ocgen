//! `ocgen edit language`: switch a project's instruction language — re-seeding
//! the preset agents nobody edited — or its answer language.

use anyhow::{bail, Result};

use super::{canonical_language, Project};
use crate::agent::Agent;
use crate::target::Target;
use crate::templates;

/// What [`Project::change_languages`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LanguageChange {
    /// Agents re-seeded from their preset in the new instruction language.
    pub reseeded: Vec<String>,
    /// Agents that keep their text: edited since their preset seeded them,
    /// seeded by an older preset, or not from a preset at all.
    pub kept: Vec<String>,
    /// The answer language, pinned to the old instruction language: a project
    /// from before the setting keeps answering as it did.
    pub pinned_answers: Option<String>,
}

/// `name` as one of `choices`, ignoring case and surrounding spaces.
pub fn match_language(name: &str, choices: &[String]) -> Result<String> {
    let name = name.trim();
    match choices.iter().find(|c| c.eq_ignore_ascii_case(name)) {
        Some(c) => Ok(c.clone()),
        None => bail!(
            "unknown language '{name}' (choose from: {})",
            choices.join(", ")
        ),
    }
}

impl Project {
    /// Set the answer language and/or the instruction language. A new
    /// instruction language re-seeds each preset agent whose text is still what
    /// its preset gave it in the old one; every other agent keeps its text.
    pub fn change_languages(
        &mut self,
        prompts: Option<&str>,
        answers: Option<&str>,
    ) -> Result<LanguageChange> {
        let mut change = LanguageChange::default();
        if let Some(answers) = answers {
            self.response_language = canonical_language(answers);
        }
        let old = canonical_language(&self.language);
        let Some(new) = prompts.map(canonical_language).filter(|n| *n != old) else {
            return Ok(change);
        };
        if self.response_language.trim().is_empty() {
            self.response_language = old.clone();
            change.pinned_answers = Some(old.clone());
        }
        let presets = templates::archetype_names();
        for agent in &mut self.agents {
            if !presets.contains(&agent.role) {
                change.kept.push(agent.name.clone());
                continue;
            }
            if !same_text(agent, &seed(self.target, agent, &old)?) {
                change.kept.push(agent.name.clone());
                continue;
            }
            let fresh = seed(self.target, agent, &new)?;
            agent.description = fresh.description;
            agent.body = fresh.body;
            agent.prompt_body = fresh.prompt_body;
            change.reseeded.push(agent.name.clone());
        }
        self.language = new;
        Ok(change)
    }
}

/// What `agent`'s preset gives it in `lang`.
pub(super) fn seed(target: Target, agent: &Agent, lang: &str) -> Result<Agent> {
    match target {
        Target::ClaudeCode => Agent::from_archetype_claude(&agent.name, &agent.role, lang),
        Target::OpenCode => Agent::from_archetype(&agent.name, &agent.role, lang, &agent.provider),
    }
}

/// Whether two agents carry the same language-dependent text.
pub(super) fn same_text(a: &Agent, b: &Agent) -> bool {
    a.description == b.description && a.body == b.body && a.prompt_body == b.prompt_body
}
