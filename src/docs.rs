//! The project's trusted documentation sites (`ocgen edit docs`): the one list
//! every way an agent reaches the web follows — the WebFetch guard, the
//! `WebFetch(domain:…)` permission rules, and the rules', skills' and agents'
//! guidance, for both targets.

use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// Official documentation sites a new project trusts.
pub const DEFAULT_TRUSTED_DOMAINS: [&str; 25] = [
    "docs.github.com",
    "git-scm.com",
    "docs.rs",
    "doc.rust-lang.org",
    "docs.python.org",
    "nodejs.org",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "kubernetes.io",
    "developer.hashicorp.com",
    "registry.terraform.io",
    "docs.aws.amazon.com",
    "aws.amazon.com",
    "repost.aws",
    // Older AWS SDK and CLI docs hosts: they redirect to docs.aws.amazon.com,
    // but the guard checks the host that was asked for.
    "boto3.amazonaws.com",
    "awscli.amazonaws.com",
    "sdk.amazonaws.com",
    "docs.powertools.aws.dev",
    "cloud.google.com",
    "learn.microsoft.com",
    "datatracker.ietf.org",
    "www.rfc-editor.org",
    "code.claude.com",
    "docs.anthropic.com",
];

/// The trusted documentation hosts, each exact or a `*.` wildcard (checked by
/// [`crate::validate::trusted_domain`]). Empty trusts nothing: every fetch is
/// blocked. Its default is [`DEFAULT_TRUSTED_DOMAINS`], so every way a project
/// is made — new, loaded from an old state, built in a test — starts with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrustedDocs(Vec<String>);

impl Default for TrustedDocs {
    fn default() -> Self {
        Self(DEFAULT_TRUSTED_DOMAINS.map(String::from).to_vec())
    }
}

impl From<Vec<String>> for TrustedDocs {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl Deref for TrustedDocs {
    type Target = Vec<String>;
    fn deref(&self) -> &Vec<String> {
        &self.0
    }
}

impl DerefMut for TrustedDocs {
    fn deref_mut(&mut self) -> &mut Vec<String> {
        &mut self.0
    }
}

impl TrustedDocs {
    /// A permission rule per site: fetching it never asks.
    pub fn webfetch_rules(&self) -> Vec<String> {
        self.iter()
            .map(|d| format!("WebFetch(domain:{d})"))
            .collect()
    }
}
