//! ocgen — scaffold OpenCode multi-agent projects from editable, embedded templates.
//!
//! The library half holds everything deterministic: template loading (embedded
//! defaults overridable from `~/.config/ocgen/templates`), the manifest/archetype
//! data model, and rendering a [`Project`] into files. The binary half (`main.rs`)
//! is just the interactive wizard that builds a `Project` and calls [`Project::scaffold`].

/// The version `ocgen --version` reports. Release builds get the tag's version
/// (`OCGEN_BUILD_VERSION`, set by `.github/workflows/release.yml` from `vX.Y.Z`);
/// any other build falls back to Cargo.toml's.
pub const VERSION: &str = match option_env!("OCGEN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod agent;
pub mod approval;
pub mod archetype;
pub mod claude;
mod clock;
pub mod diff;
pub mod gitcheck;
pub mod hooks;
pub mod manifest;
pub mod notes;
pub mod paths;
pub mod render;
pub mod risk;
pub mod seeds;
pub mod target;
pub mod templates;
pub mod validate;
pub mod verify;

pub use agent::Agent;
pub use archetype::Archetype;
pub use claude::{ClaudeConfig, Skill};
pub use manifest::Manifest;
pub use render::Project;
pub use seeds::Seeds;
pub use target::Target;
