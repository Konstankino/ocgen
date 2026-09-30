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
}

/// Opt-in sandbox profile: OS-level containment of Bash (Seatbelt on macOS,
/// bubblewrap on Linux/WSL2), a starter network allowlist, and no unsandboxed
/// retries. Secret read-denies need no repeat here — permission rules feed the
/// sandbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxProfile {
    pub enabled: bool,
    /// Domains allowed on top of [`SANDBOX_DOMAINS`].
    pub extra_domains: Vec<String>,
    /// Let sandboxed commands use your push/deploy credentials. Off by default:
    /// without credentials an agent cannot push or deploy at all, however the
    /// command is phrased — you do those steps yourself.
    pub allow_credentials: bool,
}

/// Credential files withheld from sandboxed commands.
pub const CREDENTIAL_PATHS: [&str; 7] = [
    "~/.ssh",
    "~/.aws",
    "~/.config/gh",
    "~/.netrc",
    "~/.docker/config.json",
    "~/.kube",
    "~/.config/gcloud",
];

/// Credential environment variables withheld from sandboxed commands.
pub const CREDENTIAL_ENV: [&str; 9] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "NPM_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "DOCKER_PASSWORD",
    "KUBECONFIG",
];

/// Denied everywhere (with the power-user permissions): reading credentials, and
/// reading, editing or running anything that could self-approve execution.
pub const GUARD_DENY: [&str; 10] = [
    "Read(~/.ssh/**)",
    "Read(~/.aws/**)",
    "Read(~/.config/gh/**)",
    "Read(~/.netrc)",
    "Read(~/.docker/config.json)",
    "Read(~/.kube/**)",
    "Read(~/.config/gcloud/**)",
    "Read(~/.claude/ocgen/**)",
    "Edit(~/.claude/ocgen/**)",
    "Bash(ocgen approve*)",
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
/// second line behind the approval gate's pattern match.
pub const HIGH_IMPACT_ASK: [&str; 17] = [
    "Bash(git push:*)",
    "Bash(git commit:*)",
    "Bash(gh pr merge*)",
    "Bash(gh release*)",
    "Bash(terraform apply*)",
    "Bash(terraform destroy*)",
    "Bash(kubectl apply*)",
    "Bash(kubectl delete*)",
    "Bash(helm install*)",
    "Bash(helm upgrade*)",
    "Bash(helm uninstall*)",
    "Bash(npm publish*)",
    "Bash(cargo publish*)",
    "Bash(docker push*)",
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
            "ask": HIGH_IMPACT_ASK,
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
    /// Log settings/skills changes made during a session.
    pub config_audit: bool,
}

impl Default for HooksExtra {
    fn default() -> Self {
        Self {
            compact_context: true,
            notify: false,
            format_cmd: String::new(),
            config_audit: false,
        }
    }
}

/// The workflow skills ocgen generates (formerly `.claude/commands/`). A user
/// skill with one of these names would collide with the generated one.
pub const WORKFLOW_SKILLS: [&str; 9] = [
    "multi",
    "intake",
    "refine",
    "improve-prompt",
    "fanout",
    "deliver",
    "inquire",
    "team",
    "team-plan",
];

/// Workflow skills with side effects: only the user starts them (`/name`).
pub const USER_RUN_WORKFLOWS: [&str; 5] = ["multi", "fanout", "deliver", "team", "team-plan"];

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
    /// Emit the `/deliver` end-to-end pipeline command + delivery/multi-session
    /// guidance.
    pub deliver: bool,
    /// Emit the `/inquire` codebase-understanding loop (sharpen each question,
    /// answer with file:line evidence, end with one hint (not a question), keep a git-ignored
    /// ledger under `.claude/notes/`) and route `/deliver` "understand" goals to it.
    pub inquire: bool,
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
            deliver: true,
            inquire: true,
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
