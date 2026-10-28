//! The `manifest.toml` data model: the project-level questions the wizard asks,
//! the providers/models available, and default primary/utility models.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::templates;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default, rename = "variable")]
    pub variables: Vec<Variable>,
    #[serde(default, rename = "provider")]
    pub providers: Vec<Provider>,
    /// The default team the wizard seeds: (agent name -> archetype preset). Empty
    /// means "no default team" and the wizard goes straight to adding agents.
    #[serde(default, rename = "pipeline")]
    pub pipeline: Vec<PipelineAgent>,
    #[serde(default)]
    pub defaults: Defaults,
}

/// One entry in the default pipeline: an agent name and the archetype it starts from.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineAgent {
    pub name: String,
    pub archetype: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Variable {
    pub key: String,
    pub prompt: String,
    /// One-line reminder of what the field is, shown above the prompt.
    #[serde(default)]
    pub help: String,
    /// `input` (default), `select`, or `bool`.
    #[serde(default = "default_type")]
    pub r#type: String,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub default: String,
}

/// A model provider (a `provider.<key>` entry in opencode.json) plus its models.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Provider {
    pub key: String,
    pub name: String,
    pub npm: String,
    pub base_url: String,
    #[serde(default, alias = "model")]
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub primary_model: String,
    #[serde(default)]
    pub utility_provider: String,
    #[serde(default)]
    pub utility_model: String,
    /// The OpenCode model research runs on (empty = `primary_model`): the most
    /// capable of the provider's models.
    #[serde(default)]
    pub research_model: String,
    /// The Claude model research runs on (empty = `opus`).
    #[serde(default)]
    pub claude_research_model: String,
}

impl Defaults {
    /// The OpenCode research model: `research_model`, else `primary_model`.
    pub fn research_model(&self) -> String {
        [&self.research_model, &self.primary_model]
            .into_iter()
            .map(|m| m.trim())
            .find(|m| !m.is_empty())
            .unwrap_or_default()
            .to_string()
    }

    /// The Claude research model: `claude_research_model`, else `opus`.
    pub fn claude_research_model(&self) -> String {
        match self.claude_research_model.trim() {
            "" => crate::claude::DEFAULT_RESEARCH_MODEL.to_string(),
            m => m.to_string(),
        }
    }
}

fn default_type() -> String {
    "input".to_string()
}

impl Manifest {
    pub fn load() -> Result<Self> {
        let src = templates::load("manifest.toml")?;
        Ok(toml::from_str(&src)?)
    }
}

impl Provider {
    /// Index of a model id in this provider's models, or 0 if absent (prompt default).
    pub fn model_index(&self, id: &str) -> usize {
        self.models.iter().position(|m| m.id == id).unwrap_or(0)
    }

    /// Return a copy with `base_url` replaced. Used to apply `ocgen new --base-url`.
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.to_string();
        self
    }
}
