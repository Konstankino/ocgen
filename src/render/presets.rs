//! Bringing preset agents nobody edited up to their current preset. A project
//! keeps each agent's text in its state file, so regenerating it renders what an
//! older preset gave the agent. `ocgen doctor` re-seeds the text of an agent whose
//! every text is one its role's preset has shipped with (a fingerprint in
//! `preset_history.txt`): nobody edited it. Any other text is the user's, and so
//! is every other field.

use super::language::{same_text, seed};
use super::{canonical_language, fingerprint, Project};
use crate::templates;

/// `<role> <fingerprint>` for every description, body and prompt each preset has
/// shipped with.
const HISTORY: &str = include_str!("preset_history.txt");

/// Whether `text` is one that `role`'s preset has shipped with.
pub fn shipped_preset_text(role: &str, text: &str) -> bool {
    let fp = fingerprint(text);
    HISTORY
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(' '))
        .any(|(r, f)| r == role && f == fp)
}

impl Project {
    /// Re-seed the text (description, body, prompt) of each preset agent that
    /// still has an older preset's text, in the instruction language. Returns
    /// what changed, for doctor's fixes.
    pub(super) fn refresh_presets(&mut self) -> Vec<String> {
        let presets = templates::archetype_names();
        let lang = canonical_language(&self.language);
        let target = self.target;
        let mut fixes = Vec::new();
        for agent in &mut self.agents {
            if !presets.contains(&agent.role) {
                continue;
            }
            let Ok(fresh) = seed(target, agent, &lang) else {
                continue;
            };
            if same_text(agent, &fresh) {
                continue;
            }
            let texts: Vec<&String> = [Some(&agent.description), Some(&agent.body)]
                .into_iter()
                .chain([agent.prompt_body.as_ref()])
                .flatten()
                .filter(|t| !t.trim().is_empty())
                .collect();
            if texts.is_empty() || !texts.iter().all(|t| shipped_preset_text(&agent.role, t)) {
                continue;
            }
            agent.description = fresh.description;
            agent.body = fresh.body;
            agent.prompt_body = fresh.prompt_body;
            fixes.push(format!(
                "agent '{}': text from an older '{}' preset → the current one",
                agent.name, agent.role
            ));
        }
        fixes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archetype::Archetype;

    /// Every text a shipped preset has today is fingerprinted, so a project made
    /// with it can be brought up to the next one. A changed preset fails here
    /// with the lines to append to `preset_history.txt`.
    #[test]
    fn every_shipped_preset_text_is_fingerprinted() {
        let mut missing = Vec::new();
        for name in templates::archetype_names() {
            // Overrides are the user's presets, not ocgen's.
            let Ok(src) = templates::load_embedded(&format!("archetypes/{name}.toml")) else {
                continue;
            };
            let arch: Archetype = toml::from_str(&src).unwrap();
            let maps = [
                Some(&arch.description),
                Some(&arch.body),
                arch.prompt.as_ref(),
            ];
            for text in maps.into_iter().flatten().flat_map(|m| m.values()) {
                if !text.trim().is_empty() && !shipped_preset_text(&name, text) {
                    missing.push(format!("{name} {}", fingerprint(text)));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "append to src/render/preset_history.txt:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn the_history_is_well_formed() {
        for line in HISTORY.lines().filter(|l| !l.starts_with('#')) {
            let (role, fp) = line.split_once(' ').unwrap_or_else(|| panic!("{line:?}"));
            assert!(!role.is_empty() && !role.contains(' '), "{line:?}");
            assert!(
                fp.len() == 16 && fp.bytes().all(|b| b.is_ascii_hexdigit()),
                "{line:?}"
            );
        }
    }
}
