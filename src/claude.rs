//! Claude Code–specific project configuration and the Skill data model.
//!
//! These fields live on [`crate::render::Project`] and are only meaningful when
//! its `target` is [`crate::target::Target::ClaudeCode`]. Everything is
//! `#[serde(default)]` so OpenCode state files (which omit them) still load.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::archetype::pick;
use crate::templates;

/// Built-in Claude Code tools that can appear in a skill's `allowed-tools`.
pub const CLAUDE_TOOLS: [&str; 12] = [
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "Bash",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "Task",
    "NotebookEdit",
    "Skill",
];

/// Entries in a comma-separated tool list that are not a known built-in, a
/// `Bash(...)`/`Tool(...)` pattern on a known tool, or an `mcp__…` server tool.
/// Used to *warn* (not reject) on likely typos.
pub fn unknown_tools(list: &str) -> Vec<String> {
    list.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter(|t| {
            !t.starts_with("mcp__")
                && !CLAUDE_TOOLS.contains(&t.split('(').next().unwrap_or(t).trim())
        })
        .map(|t| t.to_string())
        .collect()
}

/// A reusable skill preset (`claude/skill-presets.toml`) that seeds a [`Skill`]
/// with sensible frontmatter and a numbered-step body.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillPreset {
    pub name: String,
    /// When to pick this preset — shown in the `ocgen add skill` menu.
    #[serde(default)]
    pub choose_when: String,
    /// The seeded SKILL.md description (the skill's trigger).
    pub description: String,
    #[serde(default)]
    pub allowed_tools: String,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default)]
    pub context_fork: bool,
    #[serde(default)]
    pub agent: String,
    /// Language-keyed numbered-step body.
    pub body: HashMap<String, String>,
}

impl SkillPreset {
    /// Turn this preset into a seed [`Skill`] with the given name and language.
    pub fn to_skill(&self, name: &str, lang: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: self.description.clone(),
            allowed_tools: self.allowed_tools.clone(),
            body: pick(&self.body, lang),
            disable_model_invocation: self.disable_model_invocation,
            context_fork: self.context_fork,
            agent: self.agent.clone(),
            ..Default::default()
        }
    }
}

/// Load the skill presets (embedded, override-able).
pub fn skill_presets() -> Result<Vec<SkillPreset>> {
    #[derive(Deserialize)]
    struct Presets {
        #[serde(default, rename = "preset")]
        presets: Vec<SkillPreset>,
    }
    let src = templates::load("claude/skill-presets.toml")?;
    Ok(toml::from_str::<Presets>(&src)?.presets)
}

/// Claude Code generation settings: the global default model alias, the editable
/// `CLAUDE.md` body, what to emit, which power-user defaults to include, which
/// workflow assets to bake in, and plugin/marketplace metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    /// Global default model alias written to settings.json (opus/sonnet/haiku/…).
    pub model: String,
    /// The CLAUDE.md body (project instructions), editable in the wizard.
    pub instructions: String,
    pub output: Output,
    pub powerups: Powerups,
    pub workflow: Workflow,
    pub plugin: PluginMeta,
    pub team: Team,
    /// Project MCP servers, written to `.mcp.json`.
    pub mcp_servers: Vec<McpServer>,
    /// Optional quality-of-life hooks beyond the gates.
    pub hooks_extra: HooksExtra,
    /// Opt-in OS-level sandbox for shell commands.
    pub sandbox: SandboxProfile,
    /// The user's own permission rules, added to the generated ones in
    /// settings.json (`ocgen edit permissions`).
    pub permissions: PermissionRules,
    /// How `/intent` numbers intent files and sizes the GitHub issue draft.
    pub intent: IntentSettings,
    /// Keys this ocgen doesn't know (written by a newer one), kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Where the `/intent` templates live in a project. Written once with ocgen's
/// default and then owned by the user (never overwritten).
pub const INTENT_ISSUE_TEMPLATE: &str = ".claude/intent/issue-template.md";
pub const INTENT_FILE_TEMPLATE: &str = ".claude/intent/intent-template.md";

/// Where GitHub reads a CODEOWNERS file, in its search order (the first one found
/// is used). ocgen links an existing one; it never creates one.
pub const CODEOWNERS_PATHS: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// Where ocgen 0.7.1 and older kept a symbolic link to the linked CODEOWNERS;
/// the next run removes it.
pub const OLD_CODEOWNERS_LINK: &str = ".claude/CODEOWNERS";

/// What ocgen's CODEOWNERS block makes the /intent approvers required reviewers
/// of (once branch protection requires a review from code owners).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CodeownersScope {
    /// The intent directory.
    #[default]
    Intents,
    /// Every file (`*`).
    All,
    /// No block.
    Off,
}

impl CodeownersScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intents => "intents",
            Self::All => "all",
            Self::Off => "off",
        }
    }
}

/// `/intent` settings (`ocgen edit intent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntentSettings {
    /// File-name prefix of an intent file, e.g. `ADR` → `ADR-0007-slug.md`.
    pub prefix: String,
    /// Width of the zero-padded number.
    pub digits: u8,
    /// Directory (relative to the project root) that holds the intent files.
    pub dir: String,
    /// Word limit for the GitHub issue description.
    pub max_words: u16,
    /// Remote branch checked for numbers already taken; empty = the remote's default.
    pub branch: String,
    /// Who must sign off before work starts: GitHub handles (`@login`,
    /// `@org/team`), notified by the @mentions in the issue.
    pub approvers: Vec<String>,
    /// The project's existing CODEOWNERS file ocgen keeps its approvers block in
    /// (relative to the project root; empty = not linked).
    pub codeowners: String,
    /// What that block covers.
    pub codeowners_scope: CodeownersScope,
}

/// Read-only tools /intent may use without asking while it runs (skill
/// `allowed-tools`), on top of `WebFetch` for the trusted domains. Nothing here
/// writes, and `gh issue create` stays denied.
pub const INTENT_READ_TOOLS: [&str; 19] = [
    "Read",
    "Grep",
    "Glob",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git blame:*)",
    "Bash(git grep:*)",
    "Bash(git diff:*)",
    "Bash(git rev-parse:*)",
    "Bash(git fetch:*)",
    "Bash(git ls-tree:*)",
    "Bash(git ls-remote:*)",
    "Bash(gh issue list:*)",
    "Bash(gh issue view:*)",
    "Bash(gh pr list:*)",
    "Bash(gh pr view:*)",
    "Bash(gh pr diff:*)",
    "Bash(gh search issues:*)",
    "Bash(gh search prs:*)",
];

impl Default for IntentSettings {
    fn default() -> Self {
        Self {
            prefix: "ADR".into(),
            digits: 4,
            dir: "docs/adr".into(),
            max_words: 250,
            branch: String::new(),
            approvers: Vec::new(),
            codeowners: String::new(),
            codeowners_scope: CodeownersScope::Intents,
        }
    }
}

impl IntentSettings {
    /// The skill's `allowed-tools`: the read-only tools plus `WebFetch` scoped to
    /// each of the project's trusted documentation sites. settings.json allows
    /// those too, but a plugin can't set permissions, so the skill keeps them.
    pub fn allowed_tools(&self, trusted: &crate::docs::TrustedDocs) -> String {
        INTENT_READ_TOOLS
            .iter()
            .map(|t| t.to_string())
            .chain(trusted.webfetch_rules())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The first file name, e.g. `ADR-0001` (no slug).
    pub fn first_id(&self) -> String {
        format!(
            "{}-{:0width$}",
            self.prefix,
            1,
            width = self.digits as usize
        )
    }

    /// Check every field; the error names the field.
    pub fn validate(&self) -> Result<()> {
        use crate::validate as v;
        let checks = [
            ("prefix", v::intent_prefix(&self.prefix)),
            ("digits", v::intent_digits(&self.digits.to_string())),
            ("dir", v::intent_dir(&self.dir)),
            (
                "max words",
                v::intent_max_words(&self.max_words.to_string()),
            ),
            ("branch", v::git_branch(&self.branch)),
        ];
        for (field, r) in checks {
            if let Err(e) = r {
                anyhow::bail!("intent {field}: {e}");
            }
        }
        for a in &self.approvers {
            v::approver(a).map_err(|e| anyhow::anyhow!("intent approver: {e}"))?;
        }
        if !self.codeowners.is_empty() && !CODEOWNERS_PATHS.contains(&self.codeowners.as_str()) {
            anyhow::bail!(
                "intent CODEOWNERS: {} isn't a file GitHub reads — use one of {}",
                self.codeowners,
                CODEOWNERS_PATHS.join(", ")
            );
        }
        Ok(())
    }

    /// Add an approver as typed (`alice`, `@org/team`), normalized; one already
    /// there (in any letter case) isn't added twice.
    pub fn add_approver(&mut self, typed: &str) -> Result<()> {
        let a = crate::validate::approver(typed)
            .map_err(|e| anyhow::anyhow!("intent approver: {e}"))?;
        if !self.approvers.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            self.approvers.push(a);
        }
        Ok(())
    }

    /// Remove an approver, whatever form or letter case it is typed in.
    pub fn remove_approver(&mut self, typed: &str) {
        let t = typed.trim();
        let a = format!("@{}", t.strip_prefix('@').unwrap_or(t));
        self.approvers.retain(|x| !x.eq_ignore_ascii_case(&a));
    }

    /// ocgen's CODEOWNERS block for these settings (without its markers), or
    /// `None` when there is none: no linked file, scope off, or no approvers.
    pub fn codeowners_rule(&self) -> Option<String> {
        if self.codeowners.is_empty() || self.approvers.is_empty() {
            return None;
        }
        let pattern = match self.codeowners_scope {
            CodeownersScope::Off => return None,
            CodeownersScope::All => "*".to_string(),
            CodeownersScope::Intents => format!("/{}/", self.dir.trim_matches('/')),
        };
        Some(format!("{pattern} {}", self.approvers.join(" ")))
    }
}

/// One of settings.json's permission lists. Claude Code checks deny first, then
/// ask, then allow — so a rule in an earlier list wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleList {
    Allow,
    Ask,
    Deny,
}

impl RuleList {
    pub const ALL: [RuleList; 3] = [RuleList::Allow, RuleList::Ask, RuleList::Deny];

    /// The settings.json key.
    pub fn key(self) -> &'static str {
        match self {
            RuleList::Allow => "allow",
            RuleList::Ask => "ask",
            RuleList::Deny => "deny",
        }
    }

    /// Whether this list wins over `other` for the same rule: deny beats ask
    /// beats allow, the order Claude Code checks them in.
    pub fn stricter_than(self, other: RuleList) -> bool {
        let rank = |l: RuleList| match l {
            RuleList::Allow => 0,
            RuleList::Ask => 1,
            RuleList::Deny => 2,
        };
        rank(self) > rank(other)
    }
}

/// Permission rules by list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionRules {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
}

impl PermissionRules {
    pub fn list(&self, l: RuleList) -> &Vec<String> {
        match l {
            RuleList::Allow => &self.allow,
            RuleList::Ask => &self.ask,
            RuleList::Deny => &self.deny,
        }
    }

    fn list_mut(&mut self, l: RuleList) -> &mut Vec<String> {
        match l {
            RuleList::Allow => &mut self.allow,
            RuleList::Ask => &mut self.ask,
            RuleList::Deny => &mut self.deny,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.ask.is_empty() && self.deny.is_empty()
    }

    /// Every rule with its list, in list order.
    pub fn entries(&self) -> Vec<(RuleList, String)> {
        RuleList::ALL
            .iter()
            .flat_map(|l| self.list(*l).iter().map(|r| (*l, r.clone())))
            .collect()
    }

    /// Add `rule` to list `l`. `Ok(false)` when it is already there; an error when
    /// it is malformed or already in another list (a rule lives in one list).
    pub fn add(&mut self, l: RuleList, rule: &str) -> Result<bool> {
        let rule = rule.trim();
        crate::validate::permission_rule(rule).map_err(anyhow::Error::msg)?;
        if let Some(other) = RuleList::ALL
            .into_iter()
            .find(|o| *o != l && self.list(*o).iter().any(|r| r == rule))
        {
            anyhow::bail!(
                "{rule} is already in the {} list — remove it first",
                other.key()
            );
        }
        let list = self.list_mut(l);
        if list.iter().any(|r| r == rule) {
            return Ok(false);
        }
        list.push(rule.to_string());
        Ok(true)
    }

    /// The list that holds `rule`, if any (the strictest, should several).
    pub fn list_of(&self, rule: &str) -> Option<RuleList> {
        let rule = rule.trim();
        [RuleList::Deny, RuleList::Ask, RuleList::Allow]
            .into_iter()
            .find(|l| self.list(*l).iter().any(|r| r == rule))
    }

    /// Put `rule` in list `l` unless a list at least as strict already holds it;
    /// a looser copy moves out, so the rule still lives in one list. Returns the
    /// list that holds it afterwards. Unlike [`Self::add`], never fails: this is
    /// how settings.json combines ocgen's rules with the user's.
    pub fn tighten(&mut self, l: RuleList, rule: &str) -> RuleList {
        let rule = rule.trim();
        match self.list_of(rule) {
            Some(held) if !l.stricter_than(held) => return held,
            Some(held) => self.list_mut(held).retain(|r| r != rule),
            None => {}
        }
        self.list_mut(l).push(rule.to_string());
        l
    }

    /// Remove `rule` from whichever list holds it. `false` when none does.
    pub fn remove(&mut self, rule: &str) -> bool {
        let rule = rule.trim();
        let mut found = false;
        for l in RuleList::ALL {
            let list = self.list_mut(l);
            let before = list.len();
            list.retain(|r| r != rule);
            found |= list.len() != before;
        }
        found
    }
}

/// Sandbox profile: OS-level containment of Bash (Seatbelt on macOS, bubblewrap
/// on Linux/WSL2), a starter network allowlist, and no unsandboxed retries. The
/// wizard turns it on by default with the approval gate, which is advisory
/// without it. Secret read-denies need no repeat here — permission rules feed
/// the sandbox. Claude Code runs hooks outside its sandbox, so the hooks run
/// the project code they start themselves (the check, the formatter) in an OS
/// sandbox of their own ([`HOOK_DENY_WRITE`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxProfile {
    pub enabled: bool,
    /// Domains allowed on top of [`SANDBOX_DOMAINS`].
    pub extra_domains: Vec<String>,
    /// Let sandboxed commands use your push/deploy credentials. Off by default:
    /// the credentials ocgen knows about ([`CREDENTIAL_PATHS`],
    /// [`CREDENTIAL_ENV`]) are withheld, so a push or publish that needs them
    /// fails however the command is phrased. One an OS keychain holds is out of
    /// the sandbox's reach; git starts with no credential helper to ask it
    /// ([`GIT_HELPER_RESET`]), but a command can name one itself.
    pub allow_credentials: bool,
    /// The user was asked and said no, so "chose off" reads apart from "never
    /// asked" (states written before ocgen asked load as `false`).
    pub declined: bool,
}

/// Whether Claude Code can sandbox shell commands here: macOS (Seatbelt) and
/// Linux/WSL2 (bubblewrap). Native Windows has no sandbox, so there the approval
/// gate is advisory.
pub fn sandbox_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// Credential files withheld from sandboxed commands: SSH keys, cloud and
/// cluster logins, and the tokens git, GitHub and the package registries in
/// [`SANDBOX_DOMAINS`] push and publish with.
pub const CREDENTIAL_PATHS: [&str; 18] = [
    "~/.ssh",
    "~/.aws",
    "~/.config/gh",
    "~/.netrc",
    "~/.docker/config.json",
    "~/.kube",
    "~/.config/gcloud",
    "~/.azure",
    "~/.git-credentials",
    "~/.config/git/credentials",
    "~/.npmrc",
    "~/.yarnrc.yml",
    "~/.cargo/credentials",
    "~/.cargo/credentials.toml",
    "~/.pypirc",
    "~/.gem/credentials",
    "~/.terraform.d/credentials.tfrc.json",
    "~/.pulumi/credentials.json",
];

/// Credential environment variables withheld from sandboxed commands.
pub const CREDENTIAL_ENV: [&str; 23] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_CLIENT_SECRET",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "NPM_TOKEN",
    "NODE_AUTH_TOKEN",
    "YARN_NPM_AUTH_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "TWINE_USERNAME",
    "TWINE_PASSWORD",
    "PYPI_TOKEN",
    "UV_PUBLISH_TOKEN",
    "POETRY_PYPI_TOKEN_PYPI",
    "GEM_HOST_API_KEY",
    "DOCKER_PASSWORD",
    "KUBECONFIG",
    "PULUMI_ACCESS_TOKEN",
];

/// Git config set in the environment of Claude Code's shells and hooks while
/// credentials are withheld. The sandbox hides credential files and variables,
/// not an OS keychain: git asks one through a credential helper (osxkeychain,
/// Git Credential Manager, `gh auth git-credential`), and the keychain answers
/// through a system service no file rule covers. An empty `credential.helper`,
/// read after every config file, resets all the helpers git read before it —
/// the per-URL ones too. An agent can still name a helper in its own command
/// (`git -c credential.helper=…`): a guard-rail, like the approval gate's text
/// match. `ocgen verify` warns when one is configured.
pub const GIT_HELPER_RESET: [(&str, &str); 3] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "credential.helper"),
    ("GIT_CONFIG_VALUE_0", ""),
];

/// What a hook's own sandbox write-protects when it runs project code (the
/// check, the formatter) — Claude Code doesn't sandbox hooks. Written as the
/// sandbox settings write paths: `~/` the home folder, `./` the project (and
/// the folder the command runs in). The approval store and Claude Code's own
/// settings; what the user's shell, git and ssh run by themselves; and in the
/// project what Claude Code's sandbox protects there, plus the status line,
/// ocgen's state and git's hooks and config. Rendered into the hook env, so the
/// `sh` twin needs no copy.
pub const HOOK_DENY_WRITE: [&str; 24] = [
    "~/.claude",
    "~/.claude.json",
    "~/.bashrc",
    "~/.bash_profile",
    "~/.profile",
    "~/.zshrc",
    "~/.zshenv",
    "~/.zprofile",
    "~/.gitconfig",
    "~/.config/git",
    "~/.ssh",
    "~/.local/bin",
    "~/.cargo/bin",
    "./.claude/settings.json",
    "./.claude/settings.local.json",
    "./.claude/hooks",
    "./.claude/skills",
    "./.claude/agents",
    "./.claude/commands",
    "./.claude/statusline.sh",
    "./.claude/.ocgen-state.json",
    "./.mcp.json",
    "./.git/hooks",
    "./.git/config",
];

/// Denied everywhere (with the power-user permissions): reading credentials, and
/// editing or running anything that could self-approve execution. The approval
/// store stays readable: an approval is only an expiry time, and the pre-push
/// hook, which runs inside the sandbox, has to read it (Claude Code turns Read
/// deny rules into sandbox read-denies).
pub const GUARD_DENY: [&str; 9] = [
    "Read(~/.ssh/**)",
    "Read(~/.aws/**)",
    "Read(~/.config/gh/**)",
    "Read(~/.netrc)",
    "Read(~/.docker/config.json)",
    "Read(~/.kube/**)",
    "Read(~/.config/gcloud/**)",
    "Edit(~/.claude/ocgen/**)",
    "Bash(ocgen approve*)",
];

/// Denied whenever the approval gate is on, with or without the permission
/// defaults: the sandbox contains Bash, not the file tools, so file-tool access
/// to the approval store needs rules of its own. An `Edit` rule covers every
/// built-in tool that edits files (Write included; Claude Code never consults a
/// `Write(path)` rule). `ocgen approve` is for a human in their own terminal.
pub const GATE_DENY: [&str; 2] = ["Edit(~/.claude/ocgen/**)", "Bash(ocgen approve*)"];

/// Asked whenever the approval gate is on: edits to what the gate trusts — its
/// hook scripts, the settings that register them, ocgen's state file (which
/// regenerates both), the status line Claude Code runs outside the sandbox, and
/// git hooks. Claude Code protects `.claude` and `.git` on its own, but auto
/// mode hands those writes to its classifier; an ask rule puts a human in front
/// of them. `/path` is the project root and `//**/` any directory (a monorepo's
/// `.git` sits above the project). The same goes for `ocgen edit …` and
/// `ocgen doctor`, which rewrite all of these.
pub const GATE_ASK: [&str; 8] = [
    "Edit(/.claude/hooks/**)",
    "Edit(/.claude/settings.json)",
    "Edit(/.claude/settings.local.json)",
    "Edit(/.claude/.ocgen-state.json)",
    "Edit(/.claude/statusline.sh)",
    "Edit(//**/.git/hooks/**)",
    "Bash(ocgen edit*)",
    "Bash(ocgen doctor*)",
];

/// Asked while ocgen's block in the linked CODEOWNERS makes the /intent
/// approvers code owners: edits to every CODEOWNERS GitHub reads (one it reads
/// earlier than the linked file hides it), and Bash commands that name one — a
/// redirect's target is checked against Edit allow and deny rules, not ask
/// rules. A text match, not a boundary: GitHub's review of the change is.
pub const CODEOWNERS_ASK: [&str; 4] = [
    "Edit(/.github/CODEOWNERS)",
    "Edit(/CODEOWNERS)",
    "Edit(/docs/CODEOWNERS)",
    "Bash(*CODEOWNERS*)",
];

/// Starter network allowlist: source hosting and the common package registries.
pub const SANDBOX_DOMAINS: [&str; 13] = [
    "github.com",
    "api.github.com",
    "*.githubusercontent.com",
    "registry.npmjs.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "pypi.org",
    "files.pythonhosted.org",
    "proxy.golang.org",
    "registry.terraform.io",
    "releases.hashicorp.com",
    "api.anthropic.com",
];

/// Reads denied everywhere: secrets. Example files (.env.example, example.tfvars)
/// are deliberately not matched.
pub const SECRET_READ_DENY: [&str; 6] = [
    "Read(./.env)",
    "Read(./.env.local)",
    "Read(./**/*.pem)",
    "Read(./**/*.key)",
    "Read(./**/*.tfstate)",
    "Read(./**/*.tfstate.*)",
];

/// High-impact commands Claude Code always asks about (even in auto mode) — a
/// second line behind the approval gate's pattern match. A `*` before the
/// subcommand also covers options there (`pnpm -r publish`, `cargo +nightly
/// publish`, `docker image push`).
pub const HIGH_IMPACT_ASK: [&str; 40] = [
    "Bash(git push:*)",
    "Bash(git commit:*)",
    "Bash(git -c alias*)",
    "Bash(gh pr merge*)",
    "Bash(gh release*)",
    "Bash(gh workflow run*)",
    "Bash(terraform apply*)",
    "Bash(terraform destroy*)",
    "Bash(pulumi up*)",
    "Bash(pulumi destroy*)",
    "Bash(kubectl apply*)",
    "Bash(kubectl delete*)",
    "Bash(kubectl rollout restart*)",
    "Bash(kubectl rollout undo*)",
    "Bash(kubectl set *)",
    "Bash(helm install*)",
    "Bash(helm upgrade*)",
    "Bash(helm uninstall*)",
    "Bash(npm publish*)",
    "Bash(npm * publish*)",
    "Bash(pnpm publish*)",
    "Bash(pnpm * publish*)",
    "Bash(yarn publish*)",
    "Bash(yarn * publish*)",
    "Bash(bun publish*)",
    "Bash(cargo publish*)",
    "Bash(cargo * publish*)",
    "Bash(docker push*)",
    "Bash(docker * push*)",
    "Bash(docker *--push*)",
    "Bash(curl *--data*)",
    "Bash(curl *--json*)",
    "Bash(curl *--upload-file*)",
    "Bash(curl *--form*)",
    "Bash(curl *-T *)",
    "Bash(curl *-F *)",
    "Bash(curl *-d *)",
    "Bash(ssh *)",
    "Bash(scp *)",
    "Bash(rsync *)",
];

/// A recommended organisation policy (`managed-settings.json`) that projects can't
/// override: no bypass mode, secrets unreadable, high-impact commands always ask,
/// strict sandbox.
pub fn managed_settings_json() -> String {
    let mut deny = vec!["Bash(rm -rf:*)"];
    deny.extend(SECRET_READ_DENY);
    deny.extend(GUARD_DENY);
    let v = serde_json::json!({
        "$schema": "https://json.schemastore.org/claude-code-settings.json",
        "permissions": {
            "disableBypassPermissionsMode": "disable",
            "deny": deny,
            "ask": &HIGH_IMPACT_ASK[..],
        },
        "sandbox": {
            "enabled": true,
            "allowUnsandboxedCommands": false,
            "network": { "allowedDomains": SANDBOX_DOMAINS },
        },
    });
    format!("{}\n", serde_json::to_string_pretty(&v).unwrap_or_default())
}

/// Opt-in hooks beyond the governance gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksExtra {
    /// After a compaction, re-point Claude at the rules and any `/inquire` ledger.
    pub compact_context: bool,
    /// Desktop notification when Claude needs you or a turn fails.
    pub notify: bool,
    /// Formatter command run after Claude edits a file (empty = off).
    pub format_cmd: String,
    /// Log settings/skills changes made during a session; while the approval gate
    /// or the WebFetch guard is on, also block a change that would weaken it.
    pub config_audit: bool,
    /// Drop a leading `cd` into the folder Claude is already in, so a read-only
    /// command after it isn't asked about under `blockReadsOutsideWorkingDirectories`.
    pub drop_noop_cd: bool,
}

impl Default for HooksExtra {
    fn default() -> Self {
        Self {
            compact_context: true,
            notify: false,
            format_cmd: String::new(),
            config_audit: false,
            drop_noop_cd: true,
        }
    }
}

/// The workflow skills ocgen generates (formerly `.claude/commands/`). A user
/// skill with one of these names would collide with the generated one.
pub const WORKFLOW_SKILLS: [&str; 11] = [
    "multi",
    "intake",
    "refine",
    "improve-prompt",
    "fanout",
    "deliver",
    "inquire",
    "intent",
    "recap",
    "team",
    "team-plan",
];

/// Workflow skills with side effects: only the user starts them (`/name`).
pub const USER_RUN_WORKFLOWS: [&str; 7] = [
    "multi",
    "fanout",
    "deliver",
    "intent",
    "recap",
    "team",
    "team-plan",
];

/// Tools /recap may use without asking while it runs (skill `allowed-tools`).
/// The fetch and the current branch's fast-forward are pinned to one exact
/// command each, so no prefix can carry `+`, `--force` or `--upload-pack`. The
/// local fast-forward batch (`git fetch . …`) and new tracking branches are left
/// out on purpose: their permission prompt is the user's confirmation.
pub const RECAP_TOOLS: [&str; 16] = [
    "Read",
    "Grep",
    "Glob",
    "Edit(./.claude/notes/recap/**)",
    "Bash(git rev-parse:*)",
    "Bash(git for-each-ref:*)",
    "Bash(git rev-list:*)",
    "Bash(git log:*)",
    "Bash(git diff:*)",
    "Bash(git show:*)",
    "Bash(git range-diff:*)",
    "Bash(git status:*)",
    "Bash(git remote)",
    "Bash(git config --get-all remote.origin.fetch)",
    "Bash(git fetch --prune --no-tags --no-recurse-submodules origin)",
    "Bash(git merge --ff-only --no-overwrite-ignore @{u})",
];

/// One project MCP server (`.mcp.json` entry).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub name: String,
    /// `stdio`, `http` or `sse`.
    pub transport: String,
    /// stdio: the program to run, its arguments and environment.
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// http/sse: the endpoint and request headers.
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// Pre-approve for the team via `enabledMcpjsonServers` (applies once the
    /// workspace is trusted).
    pub pre_approve: bool,
    /// `.mcp.json` fields ocgen doesn't model (e.g. on a server added by hand),
    /// written back as they are. Kept under a key of their own in the state, not
    /// flattened: a value ocgen can't read into one of its fields (an argument
    /// that isn't a string) is kept here under that field's name.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl McpServer {
    /// The `.mcp.json` value for this server.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({ "type": self.transport });
        let o = v.as_object_mut().unwrap();
        if self.transport == "stdio" {
            o.insert("command".into(), self.command.clone().into());
            if !self.args.is_empty() {
                o.insert("args".into(), serde_json::json!(self.args));
            }
            if !self.env.is_empty() {
                o.insert("env".into(), serde_json::json!(self.env));
            }
        } else {
            o.insert("url".into(), self.url.clone().into());
            if !self.headers.is_empty() {
                o.insert("headers".into(), serde_json::json!(self.headers));
            }
        }
        for (k, x) in &self.extra {
            o.entry(k.clone()).or_insert_with(|| x.clone());
        }
        v
    }

    /// Where it runs, for listings: the command line or the URL.
    pub fn target(&self) -> String {
        if self.transport == "stdio" {
            std::iter::once(self.command.as_str())
                .chain(self.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            self.url.clone()
        }
    }

    /// Env/header entries that look like credentials but hold a literal value
    /// instead of a `${VAR}` reference.
    pub fn literal_secrets(&self) -> Vec<String> {
        const SECRETISH: [&str; 6] = ["TOKEN", "KEY", "SECRET", "PASSWORD", "AUTH", "CREDENTIAL"];
        self.env
            .iter()
            .chain(self.headers.iter())
            .filter(|(k, v)| {
                let k = k.to_uppercase();
                SECRETISH.iter().any(|s| k.contains(s)) && !v.trim().is_empty() && !v.contains("${")
            })
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// Parse `A=1, B=${B}` into a map (values may contain `=`). Empty input → empty map.
pub fn parse_kv_list(s: &str) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("'{part}' needs the form NAME=value"))?;
        let k = k.trim();
        if k.is_empty() {
            return Err(format!("'{part}' has an empty name"));
        }
        m.insert(k.to_string(), v.trim().to_string());
    }
    Ok(m)
}

/// Inverse of [`parse_kv_list`].
pub fn format_kv_list(m: &BTreeMap<String, String>) -> String {
    m.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Valid `effort` values for a subagent or skill.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Valid subagent `permissionMode` values.
pub const PERMISSION_MODES: [&str; 7] = [
    "default",
    "acceptEdits",
    "plan",
    "auto",
    "dontAsk",
    "bypassPermissions",
    "manual",
];
/// Valid subagent `memory` scopes.
pub const MEMORY_SCOPES: [&str; 3] = ["project", "local", "user"];

/// Split a comma-separated list into trimmed, non-empty names.
pub fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// Claude Code Agent Teams settings (experimental, opt-in). When enabled, the
/// generated settings.json sets `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` and a
/// `teammateMode`, and a `/team` command + CLAUDE.md guidance are emitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Team {
    pub enabled: bool,
    /// Teammate display mode: in-process (default) / auto / tmux / iterm2.
    pub mode: String,
    /// Emit commented TeammateIdle/TaskCreated/TaskCompleted hook stubs.
    pub hooks: bool,
    /// Emit the `/team-plan` command and gate task creation on an approved plan
    /// (`.claude/team/plan.md` must contain `Status: APPROVED`).
    pub plan_gate: bool,
    /// Minimum self-assessed confidence (0–100) a teammate must record before a
    /// task may be completed. `0` disables the gate. Defaults to 96 in the wizard.
    pub confidence_threshold: u8,
    /// Require a mitigation round for every risk in the plan's risk register
    /// before teammates may go idle.
    pub risk_rounds: bool,
    /// Gate high-impact external / substantial-side-effect actions (ssh, cloud
    /// mutations, git push/merge, deploys, publishes) behind a human-created
    /// approval marker, enforced deterministically by a PreToolUse hook. This is
    /// emitted independently of `hooks` so the safety line is never silently off.
    pub approval_gate: bool,
}

/// Which artifact trees to write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub project: bool,
    pub plugin: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            project: true,
            plugin: false,
        }
    }
}

/// Power-user defaults to fold into the generated settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Powerups {
    pub permissions: bool,
    pub hooks: bool,
    pub output_style: bool,
    pub statusline: bool,
}

impl Default for Powerups {
    fn default() -> Self {
        Self {
            permissions: true,
            hooks: true,
            output_style: true,
            statusline: true,
        }
    }
}

/// Which reusable workflow commands to emit into the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Workflow {
    pub intake: bool,
    pub refine: bool,
    /// Emit the `/improve-prompt` command (improves a prompt in-session using
    /// Anthropic's prompt-engineering technique). The matching skill preset is
    /// available via `ocgen add skill`.
    pub improve_prompt: bool,
    /// Emit the `/fanout` command + `.worktreeinclude` + CLAUDE.md protocol for
    /// fanning work out to worktree-isolated subagents.
    pub fanout: bool,
    /// Emit CLAUDE.md guidance to build self-checking/verification into the todo
    /// list for complex prompts (definition-of-done, per-task verify, self-review,
    /// confidence self-rating).
    pub verify_todos: bool,
    /// Route research to the generated, shell-free `explorer` subagent: deny
    /// Claude Code's built-in `Explore` agent (`Agent(Explore)`) and say so in the
    /// workflow rule. Explore ignores CLAUDE.md and reaches for shell one-liners
    /// (`cd …;`, `find -exec`, loops, sed scripts) that prompt every time under
    /// `blockReadsOutsideWorkingDirectories`. Only applies when the project has
    /// an `explorer` subagent.
    pub prefer_explorer: bool,
    /// Emit the `/deliver` end-to-end pipeline command + delivery/multi-session
    /// guidance.
    pub deliver: bool,
    /// Emit the `/inquire` codebase-understanding loop (sharpen each question,
    /// answer with file:line evidence, end with one hint (not a question), keep a git-ignored
    /// ledger under `.claude/notes/`) and route `/deliver` "understand" goals to it.
    pub inquire: bool,
    /// Emit the `/intent` workflow: improve the prompt, investigate, agree a plan,
    /// optionally write a numbered intent file, and draft the GitHub issue (which
    /// the user files — `gh issue create` is denied). See [`IntentSettings`].
    pub intent: bool,
    /// Emit the `/recap` daily recap: sync branches with the remote (fast-forward
    /// only, new remote branches tracked locally), analyze each branch changed
    /// since the last recap, and report per branch, plus new GitHub comments and
    /// reviews on the related issues and PRs (fetched by the `recap-github` hook).
    /// User-run only — it moves refs.
    pub recap: bool,
    /// Minimum confidence (0–100) a subagent that wrote files must state before it
    /// may stop, enforced by a `SubagentStop` hook. `0` disables. This pairs with
    /// worktree isolation to give isolated writes + enforced per-worker confidence.
    pub subagent_confidence: u8,
    /// How many times one gate may block the same agent on the same thing before
    /// the loop guard escalates (quality gates release the agent, marked
    /// UNRESOLVED; approval gates stay closed and halt it). `0` = unbounded.
    pub loop_guard_max: u8,
    /// A command that must pass (exit 0) before a worker that changed files may
    /// finish or a team task may complete — e.g. `cargo test`. An objective check
    /// next to the self-reported confidence. Empty = off.
    pub check_cmd: String,
}

impl Default for Workflow {
    fn default() -> Self {
        Self {
            intake: true,
            refine: true,
            improve_prompt: true,
            fanout: true,
            verify_todos: true,
            prefer_explorer: true,
            deliver: true,
            inquire: true,
            intent: true,
            recap: true,
            subagent_confidence: 96,
            loop_guard_max: 3,
            check_cmd: String::new(),
        }
    }
}

/// Model aliases a subagent's `model` field accepts (besides a full model ID).
pub const AGENT_MODEL_ALIASES: [&str; 5] = ["opus", "sonnet", "haiku", "fable", "inherit"];

/// Whether `m` is a valid subagent model: an alias, or a full Claude model ID such
/// as `claude-opus-5-5` (optionally with the `[1m]` long-context suffix).
pub fn is_valid_agent_model(m: &str) -> bool {
    let m = m.trim();
    if AGENT_MODEL_ALIASES.contains(&m) {
        return true;
    }
    let id = m.strip_suffix("[1m]").unwrap_or(m);
    id.strip_prefix("claude-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    })
}

/// Longest skill name Claude Code accepts.
pub const SKILL_NAME_MAX: usize = 64;
/// Longest description the portable Agent Skills format (claude.ai / API upload) accepts.
pub const SKILL_DESCRIPTION_MAX: usize = 1024;
/// Claude Code truncates `description` + `when_to_use` beyond this in the skill listing.
pub const SKILL_LISTING_MAX: usize = 1536;

/// Whether `name` follows the skill naming rule: lowercase letters, digits and
/// hyphens, starting with a letter or digit, at most [`SKILL_NAME_MAX`] chars.
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SKILL_NAME_MAX
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// "What makes a skill good", as checks: each entry is one problem that makes a
/// skill fail to trigger, overreach, or bloat context. Empty = no problems found.
pub fn skill_issues(s: &Skill) -> Vec<String> {
    let mut w = Vec::new();
    if !is_valid_skill_name(&s.name) {
        w.push(format!(
            "name must be lowercase letters, digits and hyphens (≤ {SKILL_NAME_MAX} chars)"
        ));
    }

    // The description is the trigger: Claude loads a skill from it alone.
    let desc = s.description.trim();
    let auto = !s.disable_model_invocation;
    if desc.is_empty() {
        w.push("description is empty — Claude can't know when to use it".into());
    } else if desc.chars().count() < 40 && auto {
        w.push("description is too short to trigger reliably — say what it does and when".into());
    }
    let listed = desc.chars().count() + s.when_to_use.trim().chars().count();
    if listed > SKILL_LISTING_MAX {
        w.push(format!(
            "description + when_to_use is over {SKILL_LISTING_MAX} characters — Claude Code truncates the rest"
        ));
    } else if desc.chars().count() > SKILL_DESCRIPTION_MAX {
        w.push(format!(
            "description is over {SKILL_DESCRIPTION_MAX} characters — too long to upload to claude.ai or the API"
        ));
    }
    if auto && !desc.is_empty() {
        let text = format!("{} {}", desc, s.when_to_use).to_lowercase();
        let says_when = ["use when", "when ", "use for", "invoke", "use it"]
            .iter()
            .any(|k| text.contains(k));
        if !says_when {
            w.push("description doesn't say when to use it — add \"Use when …\" in the words you'd type".into());
        }
    }

    // Keep SKILL.md short; detail loads on demand from supporting files.
    if s.body.lines().count() > 500 {
        w.push("body is over 500 lines — move reference detail to reference.md".into());
    }

    // Fewest tools that work.
    let tools: Vec<&str> = s
        .allowed_tools
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if tools.contains(&"Bash") {
        w.push(
            "allowed-tools pre-approves every shell command — scope it, e.g. Bash(git status:*)"
                .into(),
        );
    }
    // Scoped Bash(cmd:*) patterns are the recommended least-privilege form; only
    // file edits and an unscoped Bash are broad side effects.
    let side_effects = tools
        .iter()
        .any(|t| matches!(*t, "Write" | "Edit" | "NotebookEdit" | "Bash"));
    if auto && side_effects {
        w.push("Claude may run it on its own with edits or any shell command pre-approved — make it user-run only, or drop those tools".into());
    }

    if s.disable_model_invocation && s.hidden_from_menu {
        w.push("user-run only AND hidden from the / menu — nobody can invoke it".into());
    }
    if s.background && !s.context_fork {
        w.push("background only applies to a forked skill (context: fork)".into());
    }
    let effort = s.effort.trim();
    if !effort.is_empty() && !EFFORT_LEVELS.contains(&effort) {
        w.push(format!(
            "unknown effort '{effort}' (use {})",
            EFFORT_LEVELS.join("/")
        ));
    }
    w
}

/// Plugin/marketplace metadata for the distributable output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMeta {
    pub repo_owner: String,
    pub repo_name: String,
    pub version: String,
    pub display_name: String,
}

/// A Claude Code skill (`.claude/skills/<name>/SKILL.md`). May be authored
/// standalone or derived from an agent role (`role` names the archetype).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Comma-separated allow-list, e.g. `Read, Grep, Bash(git *)`.
    pub allowed_tools: String,
    pub body: String,
    /// The archetype this skill was derived from, if any.
    pub role: Option<String>,
    /// Extra trigger context appended to the description (`when_to_use`).
    pub when_to_use: String,
    /// Autocomplete hint for arguments (`argument-hint`).
    pub argument_hint: String,
    /// `disable-model-invocation: true` — a user-run-only task skill.
    pub disable_model_invocation: bool,
    /// Emit `user-invocable: false` (Claude-only background knowledge).
    pub hidden_from_menu: bool,
    /// Run in a forked subagent context (`context: fork`).
    pub context_fork: bool,
    /// Subagent type to fork into (`agent`, only with `context_fork`).
    pub agent: String,
    /// Model alias to pin for this skill's turn (empty = inherit).
    pub model: String,
    /// Globs (space-separated) that limit when the skill auto-activates.
    pub paths: String,
    /// Named positional arguments (space-separated) → `$name` in the body.
    pub arguments: String,
    /// Tools removed while the skill is active.
    pub disallowed_tools: String,
    /// Effort for the skill's turn (low/medium/high/xhigh/max; empty = inherit).
    pub effort: String,
    /// With `context: fork`, run the forked skill in the background.
    pub background: bool,
}
