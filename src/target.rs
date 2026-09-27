//! The output target a project is generated for. OpenCode is the original and
//! remains the default; ClaudeCode renders a Claude Code project/plugin instead.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Target {
    #[default]
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "claude")]
    ClaudeCode,
}

impl Target {
    /// Relative path of the state file this target writes/reads.
    pub fn state_file(self) -> &'static str {
        match self {
            Target::OpenCode => ".opencode/.ocgen-state.json",
            Target::ClaudeCode => ".claude/.ocgen-state.json",
        }
    }

    /// Short human/CLI label.
    pub fn label(self) -> &'static str {
        match self {
            Target::OpenCode => "opencode",
            Target::ClaudeCode => "claude",
        }
    }

    /// Every known state-file path, in the order `discover` probes them.
    pub fn state_files() -> [&'static str; 2] {
        [
            Target::ClaudeCode.state_file(),
            Target::OpenCode.state_file(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_labels_and_paths() {
        assert_eq!(Target::default(), Target::OpenCode);
        assert_eq!(Target::OpenCode.label(), "opencode");
        assert_eq!(Target::ClaudeCode.label(), "claude");
        assert_eq!(Target::OpenCode.state_file(), ".opencode/.ocgen-state.json");
        assert_eq!(Target::ClaudeCode.state_file(), ".claude/.ocgen-state.json");
        // Claude is probed before OpenCode.
        assert_eq!(
            Target::state_files(),
            [".claude/.ocgen-state.json", ".opencode/.ocgen-state.json"]
        );
    }
}
