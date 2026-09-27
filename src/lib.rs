//! ocgen — scaffold OpenCode multi-agent projects from editable, embedded templates.
//!
//! The library half holds everything deterministic: template loading (embedded
//! defaults overridable from `~/.config/ocgen/templates`), the manifest/archetype
//! data model, and rendering a [`Project`] into files. The binary half (`main.rs`)
//! is just the interactive wizard that builds a `Project` and calls [`Project::scaffold`].

pub mod agent;
pub mod archetype;
pub mod claude;
pub mod manifest;
pub mod preset;
pub mod render;
pub mod seeds;
pub mod target;
pub mod templates;
pub mod validate;

pub use agent::Agent;
pub use archetype::Archetype;
pub use claude::{ClaudeConfig, Skill};
pub use manifest::Manifest;
pub use preset::Preset;
pub use render::Project;
pub use seeds::Seeds;
pub use target::Target;
