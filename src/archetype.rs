//! Agent role archetypes (`archetypes/*.toml`): reusable presets that supply an
//! agent's mode, permissions, colour, temperature, and language-keyed text.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use crate::templates;

type LangMap = HashMap<String, String>;

#[derive(Debug, Clone, Deserialize)]
pub struct Archetype {
    pub mode: String,
    pub default_model: String,
    pub temperature: String,
    pub color: String,
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub prompt_file: bool,
    /// Raw YAML permission block, already indented two spaces.
    pub permissions: String,
    pub description: LangMap,
    pub body: LangMap,
    /// External prompt-file content (only meaningful when `prompt_file` is true).
    #[serde(default)]
    pub prompt: Option<LangMap>,
    /// Claude Code model alias default for this role (opus/sonnet/haiku).
    #[serde(default)]
    pub claude_model: Option<String>,
    /// Claude Code tool allow-list default for this role.
    #[serde(default)]
    pub tools: Option<String>,
}

impl Archetype {
    pub fn load(name: &str) -> Result<Self> {
        let src = templates::load(&format!("archetypes/{name}.toml"))?;
        Ok(toml::from_str(&src)?)
    }

    pub fn description_for(&self, lang: &str) -> String {
        pick(&self.description, lang)
    }

    pub fn body_for(&self, lang: &str) -> String {
        pick(&self.body, lang)
    }

    pub fn prompt_for(&self, lang: &str) -> Option<String> {
        self.prompt.as_ref().map(|m| pick(m, lang))
    }

    /// Claude model alias for this role, defaulting to `sonnet`.
    pub fn claude_model(&self) -> String {
        self.claude_model
            .clone()
            .unwrap_or_else(|| "sonnet".to_string())
    }

    /// Claude tool allow-list for this role (empty = inherit all).
    pub fn tools(&self) -> String {
        self.tools.clone().unwrap_or_default()
    }
}

/// Resolve a language-keyed value, falling back to English then empty.
pub(crate) fn pick(map: &LangMap, lang: &str) -> String {
    map.get(lang)
        .or_else(|| map.get("English"))
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_accessors_default_when_absent() {
        // A minimal archetype without claude_model/tools falls back to sonnet / empty.
        let src = r#"
mode = "subagent"
default_model = "m"
temperature = "0.2"
color = "accent"
permissions = "  edit: ask"
[description]
English = "d"
[body]
English = "b"
"#;
        let a: Archetype = toml::from_str(src).unwrap();
        assert_eq!(a.claude_model(), "sonnet");
        assert_eq!(a.tools(), "");

        // And when present, they are returned verbatim.
        let src2 = r#"
mode = "subagent"
default_model = "m"
temperature = "0.2"
color = "accent"
claude_model = "haiku"
tools = "Read"
permissions = "  edit: ask"
[description]
English = "d"
[body]
English = "b"
"#;
        let b: Archetype = toml::from_str(src2).unwrap();
        assert_eq!(b.claude_model(), "haiku");
        assert_eq!(b.tools(), "Read");
    }
}
