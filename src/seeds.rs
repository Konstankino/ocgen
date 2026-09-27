//! Wizard seed text (`seeds.toml`): the language-keyed defaults the wizard
//! pre-fills for a "blank (custom role)" agent's system-prompt body and its
//! optional external prompt file. Embedded by default, override-able like every
//! other template. Kept in the library so its resolution logic stays testable.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use crate::archetype::pick;
use crate::templates;

type LangMap = HashMap<String, String>;

#[derive(Debug, Clone, Deserialize)]
pub struct Seeds {
    #[serde(default)]
    pub body: LangMap,
    #[serde(default)]
    pub prompt: LangMap,
}

impl Seeds {
    pub fn load() -> Result<Self> {
        let src = templates::load("seeds.toml")?;
        Ok(toml::from_str(&src)?)
    }

    /// Inline system-prompt body seed for `lang` (English fallback).
    pub fn body_for(&self, lang: &str) -> String {
        pick(&self.body, lang)
    }

    /// External prompt-file seed for `lang` (English fallback).
    pub fn prompt_for(&self, lang: &str) -> String {
        pick(&self.prompt, lang)
    }
}
