//! Turning a [`Project`] into concrete files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::claude::{ClaudeConfig, Skill};
use crate::manifest::{Manifest, Provider};
use crate::target::Target;
use crate::templates;

/// Everything needed to render a scaffold. Serializable so it can be saved to
/// the target's state file (e.g. `.opencode/.ocgen-state.json`) and reloaded by
/// `ocgen add agent`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)] // tolerate state files from older versions (see migrate_state)
pub struct Project {
    /// Which platform this project targets. Defaults to OpenCode for old state files.
    pub target: Target,
    pub project_name: String,
    pub language: String,
    pub providers: Vec<Provider>,
    /// Provider + model for the built-in compaction/title/summary agents.
    pub utility_provider: String,
    pub utility_model: String,
    /// Fallback model id (under `utility_provider`) when no agent is `primary`.
    pub default_primary_model: String,
    pub agents: Vec<Agent>,
    /// Claude Code–only settings (ignored for the OpenCode target).
    pub claude: ClaudeConfig,
    /// Claude Code skills to emit (Claude target only).
    pub skills: Vec<Skill>,
    /// Fingerprint of every file the last scaffold wrote (relative path → hash), so
    /// `doctor` can tell a hand edit from an ocgen change.
    pub generated: std::collections::BTreeMap<String, String>,
}

/// Outcome of [`Project::install_pre_push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrePush {
    /// ocgen's hook was written (or refreshed) at this path.
    Installed(PathBuf),
    /// Another pre-push hook (or a custom `core.hooksPath`) is in place; it was
    /// left alone. Chain `.claude/hooks/git-pre-push.sh` from it to get the gate.
    Foreign(PathBuf),
    /// Not a git repository, not a Claude project, or the approval gate is off.
    NotApplicable,
}

/// What `doctor` would do to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
    Unchanged,
}

/// One entry of [`Project::plan_changes`].
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Path relative to the project root (as written).
    pub rel: String,
    pub kind: ChangeKind,
    /// Current content on disk (for modified/removed files).
    pub old: Option<String>,
    /// Content ocgen would write (for added/modified files).
    pub new: Option<String>,
    /// For a modified file: `Some(true)` = changed by hand since ocgen wrote it,
    /// `Some(false)` = untouched (the change is ocgen's), `None` = unknown (the
    /// project predates fingerprints).
    pub hand_edited: Option<bool>,
}

/// FNV-1a 64-bit fingerprint of file content (stable across Rust versions).
fn fingerprint(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// UTC `YYYYMMDD-HHMMSS` for backup folder names.
fn utc_stamp() -> String {
    crate::clock::compact_stamp()
}

/// Subagent view passed to prompt/command templates.
#[derive(Serialize)]
struct SubCtx {
    name: String,
    description: String,
}

/// Probe order for locating a project's state file (target-agnostic discovery).
fn state_file_in(dir: &Path) -> Option<PathBuf> {
    for sf in Target::state_files() {
        let p = dir.join(sf);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// YAML-safe colour: hex values need quoting so `#` isn't read as a comment.
fn color_yaml(color: &str) -> String {
    if color.starts_with('#') {
        format!("\"{color}\"")
    } else {
        color.to_string()
    }
}

impl Project {
    /// Seed provider/model fields from the manifest; agents are filled in by the caller.
    pub fn from_manifest(manifest: &Manifest, language: &str) -> Self {
        Project {
            project_name: String::new(),
            language: language.to_string(),
            providers: manifest.providers.clone(),
            utility_provider: manifest.defaults.utility_provider.clone(),
            utility_model: manifest.defaults.utility_model.clone(),
            default_primary_model: manifest.defaults.primary_model.clone(),
            agents: Vec::new(),
            ..Default::default()
        }
    }

    fn subagents(&self) -> Vec<SubCtx> {
        self.agents
            .iter()
            .filter(|a| a.mode == "subagent")
            .map(|a| SubCtx {
                name: a.name.clone(),
                description: a.description.clone(),
            })
            .collect()
    }

    fn primary(&self) -> Option<&Agent> {
        self.agents.iter().find(|a| a.mode == "primary")
    }

    /// Render every output file as (relative path, contents). Pure — no disk I/O.
    pub fn render_all(&self) -> Result<Vec<(PathBuf, String)>> {
        match self.target {
            Target::OpenCode => self.render_opencode(),
            Target::ClaudeCode => self.render_claude(),
        }
    }

    /// OpenCode output: `opencode.json` plus the `.opencode/` tree.
    fn render_opencode(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut env = Environment::new();
        env.set_keep_trailing_newline(true);
        let lang = &self.language;
        let subs = self.subagents();
        let mut out = Vec::new();

        // opencode.json — top-level model + utility agents are provider-qualified.
        let primary_ref = match self.primary() {
            Some(a) => format!("{}/{}", a.provider, a.model),
            None => format!("{}/{}", self.utility_provider, self.default_primary_model),
        };
        let utility_ref = format!("{}/{}", self.utility_provider, self.utility_model);
        let json = env
            .render_str(
                &templates::load("opencode.json.j2")?,
                context! {
                    primary_ref => primary_ref,
                    providers => self.providers,
                    utility_ref => utility_ref,
                },
            )
            .context("rendering opencode.json")?;
        serde_json::from_str::<serde_json::Value>(&json)
            .context("rendered opencode.json is not valid JSON (check your template)")?;
        out.push((PathBuf::from("opencode.json"), json));

        // Per-agent files (+ external prompt files).
        let agent_tmpl = templates::load("opencode/agents/_agent.md.j2")?;
        for agent in &self.agents {
            let mut permissions = agent.permissions.trim_end().to_string();
            if agent.mode == "primary" {
                permissions.push_str("\n  task:\n    \"*\": deny");
                for sub in &subs {
                    permissions.push_str(&format!("\n    \"{}\": allow", sub.name));
                }
            }

            let agent_val = context! {
                name => agent.name,
                description => agent.description,
                mode => agent.mode,
                provider => agent.provider,
                model => agent.model,
                variant => agent.variant,
                temperature => agent.temperature,
                top_p => agent.top_p,
                steps => agent.steps,
                color_yaml => color_yaml(&agent.color),
                disable => agent.disable,
                hidden => agent.hidden,
                options => agent.options.trim_end(),
                prompt_file => agent.prompt_file,
                permissions => permissions,
                body => agent.body,
            };
            let md = env
                .render_str(&agent_tmpl, context! { agent => agent_val })
                .with_context(|| format!("rendering agent '{}'", agent.name))?;
            out.push((
                PathBuf::from(format!(".opencode/agents/{}.md", agent.name)),
                md,
            ));

            if agent.prompt_file {
                if let Some(prompt_src) = &agent.prompt_body {
                    let txt = env
                        .render_str(
                            prompt_src,
                            context! { subagents => &subs, language => lang, parallel => false },
                        )
                        .with_context(|| format!("rendering prompt for '{}'", agent.name))?;
                    out.push((
                        PathBuf::from(format!(".opencode/prompts/{}.txt", agent.name)),
                        txt,
                    ));
                }
            }
        }

        // multi command — only meaningful with a coordinating primary agent.
        if let Some(primary) = self.primary() {
            let cmd = env
                .render_str(
                    &templates::load("opencode/commands/multi.md.j2")?,
                    context! {
                        primary => context! { name => primary.name },
                        subagents => &subs,
                        language => lang,
                    },
                )
                .context("rendering multi command")?;
            out.push((PathBuf::from(".opencode/commands/multi.md"), cmd));
        }

        Ok(out)
    }

    /// Claude Code output: `.claude/` agents + `/multi` command + settings.json,
    /// and `CLAUDE.md` (project instructions + roster + coordinator body).
    fn render_claude(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut env = Environment::new();
        env.set_keep_trailing_newline(true);
        let lang = &self.language;

        let subs: Vec<SubCtx> = self
            .agents
            .iter()
            .filter(|a| a.mode != "primary")
            .map(|a| SubCtx {
                name: a.name.clone(),
                description: a.description.clone(),
            })
            .collect();

        // Shared components as (path within a .claude/ or plugin tree, contents).
        let mut components: Vec<(String, String)> = Vec::new();

        let agent_tmpl = templates::load("claude/agent.md.j2")?;
        for agent in self.agents.iter().filter(|a| a.mode != "primary") {
            let model = if agent.model.trim().is_empty() {
                "opus"
            } else {
                agent.model.trim()
            };
            // An explicit tools allow-list must also name each MCP server the agent
            // uses, or its tools stay unavailable.
            let servers = crate::claude::split_list(&agent.mcp_servers);
            let mut tools = agent.tools.trim().to_string();
            if !tools.is_empty() {
                for s in &servers {
                    let t = format!("mcp__{s}");
                    if !crate::claude::split_list(&tools).contains(&t) {
                        tools = format!("{tools}, {t}");
                    }
                }
            }
            let agent_val = context! {
                name => agent.name,
                description => agent.description,
                tools => tools,
                model => model,
                color => agent.color,
                isolation => agent.isolation.trim(),
                max_turns => agent.steps,
                disallowed_tools => agent.disallowed_tools.trim(),
                permission_mode => agent.permission_mode.trim(),
                effort => agent.effort.trim(),
                memory => agent.memory.trim(),
                skills => crate::claude::split_list(&agent.preload_skills),
                mcp_servers => servers,
                background => agent.background,
                body => agent.body,
            };
            let md = env
                .render_str(&agent_tmpl, context! { agent => agent_val })
                .with_context(|| format!("rendering agent '{}'", agent.name))?;
            components.push((format!("agents/{}.md", agent.name), md));
        }

        if !subs.is_empty() {
            let cmd = env
                .render_str(
                    &templates::load("claude/commands/multi.md.j2")?,
                    context! { subagents => &subs, language => lang },
                )
                .context("rendering multi command")?;
            components.push(("commands/multi.md".to_string(), cmd));
        }
        if self.claude.workflow.intake {
            components.push((
                "commands/intake.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/intake.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering intake command")?,
            ));
        }
        if self.claude.workflow.refine {
            components.push((
                "commands/refine.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/refine.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering refine command")?,
            ));
        }
        if self.claude.workflow.improve_prompt {
            components.push((
                "commands/improve-prompt.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/improve-prompt.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering improve-prompt command")?,
            ));
        }
        if self.claude.workflow.fanout {
            components.push((
                "commands/fanout.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/fanout.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering fanout command")?,
            ));
        }
        if self.claude.workflow.deliver {
            components.push((
                "commands/deliver.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/deliver.md.j2")?,
                    context! { language => lang, inquire => self.claude.workflow.inquire },
                )
                .context("rendering deliver command")?,
            ));
        }
        if self.claude.workflow.inquire {
            components.push((
                "commands/inquire.md".to_string(),
                env.render_str(
                    &templates::load("claude/commands/inquire.md.j2")?,
                    context! { language => lang },
                )
                .context("rendering inquire command")?,
            ));
        }

        let skill_tmpl = templates::load("claude/skill/SKILL.md.j2")?;
        for skill in &self.skills {
            let md = env
                .render_str(
                    &skill_tmpl,
                    context! {
                        skill => skill,
                        paths => skill.paths.split_whitespace().collect::<Vec<_>>(),
                    },
                )
                .with_context(|| format!("rendering skill '{}'", skill.name))?;
            components.push((format!("skills/{}/SKILL.md", skill.name), md));
        }

        if self.claude.powerups.output_style {
            components.push((
                "output-styles/ocgen-concise.md".to_string(),
                env.render_str(
                    &templates::load("claude/output-styles/ocgen-concise.md.j2")?,
                    context! {},
                )
                .context("rendering output style")?,
            ));
        }

        // Agent Teams: a /team command plus optional quality-gate hook scripts.
        if self.claude.team.enabled {
            let team_cmd = env
                .render_str(
                    &templates::load("claude/commands/team.md.j2")?,
                    context! {
                        subagents => &subs,
                        language => lang,
                        plan_gate => self.claude.team.plan_gate,
                    },
                )
                .context("rendering team command")?;
            components.push(("commands/team.md".to_string(), team_cmd));
            if self.claude.team.plan_gate {
                let plan_cmd = env
                    .render_str(
                        &templates::load("claude/commands/team-plan.md.j2")?,
                        context! {
                            subagents => &subs,
                            language => lang,
                            confidence_threshold => self.claude.team.confidence_threshold,
                        },
                    )
                    .context("rendering team-plan command")?;
                components.push(("commands/team-plan.md".to_string(), plan_cmd));
            }
            if self.claude.team.hooks {
                for h in [
                    "team-teammate-idle.sh",
                    "team-task-created.sh",
                    "team-task-completed.sh",
                ] {
                    let body = templates::load(&format!("claude/hooks/{h}"))?;
                    components.push((format!("hooks/{h}"), body));
                }
            }
            // The execution-approval gate is emitted independently of `hooks` so
            // this safety line is never silently disabled.
            if self.claude.team.approval_gate {
                let body = templates::load("claude/hooks/team-approval-gate.sh")?;
                components.push(("hooks/team-approval-gate.sh".to_string(), body));
                // The git-side half of the gate (installed into .git/hooks by
                // `scaffold`, or chained by hand when another hook is present).
                let body = templates::load("claude/hooks/git-pre-push.sh")?;
                components.push(("hooks/git-pre-push.sh".to_string(), body));
            }
        }
        // Per-worker confidence gate (SubagentStop) — independent of Agent Teams.
        if self.worker_gate() {
            let body = templates::load("claude/hooks/subagent-confidence-gate.sh")?;
            components.push(("hooks/subagent-confidence-gate.sh".to_string(), body));
        }
        // Optional quality-of-life hook scripts.
        let x = &self.claude.hooks_extra;
        for (on, script) in [
            (x.notify, "notify.sh"),
            (!x.format_cmd.trim().is_empty(), "format.sh"),
            (x.config_audit, "config-audit.sh"),
        ] {
            if on {
                let body = templates::load(&format!("claude/hooks/{script}"))?;
                components.push((format!("hooks/{script}"), body));
            }
        }
        // Shared loop guard, sourced by every blocking hook so no gate can hold an
        // agent forever.
        if self.has_blocking_hooks() {
            let body = templates::load("claude/hooks/loop-guard.sh")?;
            components.push(("hooks/loop-guard.sh".to_string(), body));
        }

        // Workflow commands ship as skills (`.claude/commands` is legacy): same
        // templates, rendered to skills/<name>/SKILL.md with a `name` and — for the
        // side-effecting ones — `disable-model-invocation`.
        let mut components: Vec<(String, String)> = components
            .into_iter()
            .map(|(rel, c)| {
                match rel
                    .strip_prefix("commands/")
                    .and_then(|r| r.strip_suffix(".md"))
                {
                    Some(name) => (
                        format!("skills/{name}/SKILL.md"),
                        command_to_skill(
                            name,
                            &c,
                            crate::claude::USER_RUN_WORKFLOWS.contains(&name),
                        ),
                    ),
                    None => (rel, c),
                }
            })
            .collect();

        // Coordinator body may itself be a template (the orchestrator prompt loops
        // over subagents), so render it with that context for CLAUDE.md.
        let coordinator = match self.primary() {
            Some(p) => env
                .render_str(
                    &p.body,
                    context! { subagents => &subs, language => lang, parallel => true },
                )
                .context("rendering coordinator instructions")?,
            None => String::new(),
        };
        // Behavioral guidance lives in .claude/rules/ (loads every session at CLAUDE.md
        // priority) so ocgen never has to touch a user-owned CLAUDE.md.
        let has_coordinator = !coordinator.is_empty();
        let rules_ctx = context! {
            coordinator => coordinator,
            subagents => &subs,
            team => self.claude.team.enabled,
            team_mode => self.teammate_mode(),
            plan_gate => self.claude.team.plan_gate,
            confidence_threshold => self.claude.team.confidence_threshold,
            risk_rounds => self.claude.team.risk_rounds,
            approval_gate => self.claude.team.approval_gate,
            fanout => self.claude.workflow.fanout,
            verify_todos => self.claude.workflow.verify_todos,
            deliver => self.claude.workflow.deliver,
            inquire => self.claude.workflow.inquire,
            subagent_confidence => self.claude.workflow.subagent_confidence,
            loop_guard_max => self.claude.workflow.loop_guard_max,
            check_cmd => self.claude.workflow.check_cmd.trim(),
        };
        components.push((
            "rules/ocgen-workflow.md".to_string(),
            env.render_str(
                &templates::load("claude/rules/ocgen-workflow.md.j2")?,
                rules_ctx.clone(),
            )
            .context("rendering ocgen-workflow rule")?,
        ));
        if has_coordinator || !subs.is_empty() || self.claude.team.enabled {
            components.push((
                "rules/ocgen-team.md".to_string(),
                env.render_str(
                    &templates::load("claude/rules/ocgen-team.md.j2")?,
                    rules_ctx.clone(),
                )
                .context("rendering ocgen-team rule")?,
            ));
        }

        // CLAUDE.md is a slim, user-owned starter (written once — never overwritten;
        // see `scaffold`).
        let claude_md = env
            .render_str(
                &templates::load("claude/CLAUDE.md.j2")?,
                context! {
                    project_name => self.project_name,
                    instructions => self.claude.instructions,
                },
            )
            .context("rendering CLAUDE.md")?;
        let settings = self.claude_settings_json()?;

        let want_plugin = self.claude.output.plugin;
        // Always write the project tree unless the user asked for plugin-only.
        let want_project = self.claude.output.project || !want_plugin;

        let mut out = Vec::new();

        if want_project {
            for (rel, c) in &components {
                out.push((PathBuf::from(format!(".claude/{rel}")), c.clone()));
            }
            out.push((PathBuf::from(".claude/settings.json"), settings.clone()));
            if self.claude.powerups.statusline {
                // Project-only: a plugin can't set statusLine.
                out.push((
                    PathBuf::from(".claude/statusline.sh"),
                    templates::load("claude/statusline.sh")?,
                ));
            }
            out.push((PathBuf::from("CLAUDE.md"), claude_md));
            if let Some(mcp) = self.mcp_json()? {
                out.push((PathBuf::from(".mcp.json"), mcp));
            }
            if self.claude.workflow.fanout || self.claude.workflow.deliver {
                // Root-level file (not under .claude/): gitignored files copied into
                // each new worktree — `.env` plus, when `.claude/` is ignored, the
                // settings/hooks/rules that /fanout and multi-session delivery need.
                out.push((
                    PathBuf::from(".worktreeinclude"),
                    templates::load("claude/worktreeinclude")?,
                ));
            }
        }

        if want_plugin {
            let plugin_name = self.plugin_name();
            let base = format!("plugin/{plugin_name}");
            for (rel, c) in &components {
                out.push((PathBuf::from(format!("{base}/{rel}")), c.clone()));
            }
            if let Some(mcp) = self.mcp_json()? {
                out.push((PathBuf::from(format!("{base}/.mcp.json")), mcp));
            }
            let owner = self.claude.plugin.repo_owner.clone();
            let repo = self.claude.plugin.repo_name.clone();
            let version = if self.claude.plugin.version.trim().is_empty() {
                "0.1.0".to_string()
            } else {
                self.claude.plugin.version.clone()
            };
            let display = if self.claude.plugin.display_name.trim().is_empty() {
                self.project_name.clone()
            } else {
                self.claude.plugin.display_name.clone()
            };
            let description = format!("{display} — a Claude Code multi-agent team.");
            let mctx = context! {
                name => plugin_name,
                display_name => display,
                version => version,
                description => description,
                owner => owner,
                repo => repo,
                subagents => &subs,
                intake => self.claude.workflow.intake,
                refine => self.claude.workflow.refine,
            };
            let plugin_json = env
                .render_str(
                    &templates::load("claude/plugin/plugin.json.j2")?,
                    mctx.clone(),
                )
                .context("rendering plugin.json")?;
            serde_json::from_str::<Value>(&plugin_json)
                .context("rendered plugin.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/plugin.json")),
                plugin_json,
            ));
            let market = env
                .render_str(
                    &templates::load("claude/plugin/marketplace.json.j2")?,
                    mctx.clone(),
                )
                .context("rendering marketplace.json")?;
            serde_json::from_str::<Value>(&market)
                .context("rendered marketplace.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/marketplace.json")),
                market,
            ));
            out.push((
                PathBuf::from(format!("{base}/README.md")),
                env.render_str(
                    &templates::load("claude/plugin/README.md.j2")?,
                    mctx.clone(),
                )
                .context("rendering plugin README")?,
            ));
            out.push((
                PathBuf::from(format!("{base}/.github/workflows/release.yml")),
                env.render_str(&templates::load("claude/plugin/release.yml.j2")?, mctx)
                    .context("rendering release workflow")?,
            ));
            if let Some(hooks) = self.plugin_hooks_json()? {
                out.push((PathBuf::from(format!("{base}/hooks/hooks.json")), hooks));
            }
        }

        Ok(out)
    }
    /// The effective teammate display mode (defaults to in-process).
    /// Whether the per-worker gate (SubagentStop) is on: a confidence bar, a check
    /// command, or both.
    fn worker_gate(&self) -> bool {
        self.claude.workflow.subagent_confidence > 0
            || !self.claude.workflow.check_cmd.trim().is_empty()
    }

    /// Whether any hook that can block (exit 2) is emitted — those source the loop guard.
    fn has_blocking_hooks(&self) -> bool {
        self.worker_gate()
            || (self.claude.team.enabled
                && (self.claude.team.hooks || self.claude.team.approval_gate))
    }

    fn teammate_mode(&self) -> String {
        let m = self.claude.team.mode.trim();
        if m.is_empty() {
            "in-process".to_string()
        } else {
            m.to_string()
        }
    }

    /// Build `.claude/settings.json` as validated JSON from the power-up toggles.
    fn claude_settings_json(&self) -> Result<String> {
        let model = if self.claude.model.trim().is_empty() {
            "opus"
        } else {
            self.claude.model.trim()
        };
        let mut root = json!({
            "$schema": "https://json.schemastore.org/claude-code-settings.json",
            "model": model
        });
        let obj = root.as_object_mut().unwrap();
        if self.claude.powerups.permissions {
            let mut deny = vec!["Bash(rm -rf:*)"];
            deny.extend(crate::claude::SECRET_READ_DENY);
            deny.extend(crate::claude::GUARD_DENY);
            obj.insert(
                "permissions".into(),
                json!({
                    "allow": ["Read","Grep","Glob","Edit","Write","Bash(git status:*)","Bash(git diff:*)","Bash(git log:*)"],
                    // Asked even in auto mode: a second line behind the approval gate.
                    "ask": crate::claude::HIGH_IMPACT_ASK,
                    "deny": deny
                }),
            );
        }
        if self.claude.sandbox.enabled {
            let mut domains: Vec<String> = crate::claude::SANDBOX_DOMAINS
                .iter()
                .map(|d| d.to_string())
                .collect();
            for d in &self.claude.sandbox.extra_domains {
                if !domains.contains(d) {
                    domains.push(d.clone());
                }
            }
            let mut sandbox = json!({
                "enabled": true,
                // Strict: a sandboxed failure can't be retried unsandboxed.
                "allowUnsandboxedCommands": false,
                "network": { "allowedDomains": domains },
                // No sandboxed command may touch the approval store.
                "filesystem": {
                    "denyWrite": ["~/.claude/ocgen"],
                    "denyRead": ["~/.claude/ocgen"]
                }
            });
            if !self.claude.sandbox.allow_credentials {
                // Without credentials an agent can't push or deploy at all,
                // however the command is phrased.
                let files: Vec<Value> = crate::claude::CREDENTIAL_PATHS
                    .iter()
                    .map(|p| json!({ "path": p, "mode": "deny" }))
                    .collect();
                let vars: Vec<Value> = crate::claude::CREDENTIAL_ENV
                    .iter()
                    .map(|n| json!({ "name": n, "mode": "deny" }))
                    .collect();
                sandbox["credentials"] = json!({ "files": files, "envVars": vars });
            }
            obj.insert("sandbox".into(), sandbox);
        }
        if self.claude.powerups.output_style {
            // Named apart from the built-in `Concise` so it never shadows it.
            obj.insert("outputStyle".into(), json!("ocgen-concise"));
        }
        let approved: Vec<&str> = self
            .claude
            .mcp_servers
            .iter()
            .filter(|s| s.pre_approve)
            .map(|s| s.name.as_str())
            .collect();
        if !approved.is_empty() {
            // Takes effect once the user trusts the workspace.
            obj.insert("enabledMcpjsonServers".into(), json!(approved));
        }
        if self.claude.workflow.fanout {
            // Worktrees (fanout workers, --worktree sessions) branch from the current
            // HEAD, so they see unpushed work and merge back cleanly.
            obj.insert("worktree".into(), json!({ "baseRef": "head" }));
        }
        if self.claude.powerups.statusline {
            // CLAUDE_PROJECT_DIR when set, else the repo top (works in worktrees too).
            obj.insert(
                "statusLine".into(),
                json!({
                    "type": "command",
                    "command": "sh \"${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}/.claude/statusline.sh\""
                }),
            );
        }
        let sub_conf = self.claude.workflow.subagent_confidence;
        if self.claude.team.enabled || sub_conf > 0 || self.worker_gate() {
            let mut env = serde_json::Map::new();
            if self.claude.team.enabled {
                env.insert("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".into(), json!("1"));
            }
            env.extend(self.gate_env());
            obj.insert("env".into(), Value::Object(env));
            if self.claude.team.enabled {
                obj.insert("teammateMode".into(), json!(self.teammate_mode()));
            }
        }

        let hooks = self.hook_table("${CLAUDE_PROJECT_DIR}/.claude/hooks", "");
        if !hooks.is_empty() {
            obj.insert("hooks".into(), Value::Object(hooks));
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&root)?))
    }

    /// Environment the gate hooks read (thresholds, switches, loop budget). Project
    /// output puts it in `settings.json` `env`; a plugin can't set `env`, so its hook
    /// commands carry these as a prefix instead.
    fn gate_env(&self) -> serde_json::Map<String, Value> {
        let mut env = serde_json::Map::new();
        if self.claude.team.enabled {
            if self.claude.team.plan_gate {
                env.insert("TEAM_PLAN_GATE".into(), json!("1"));
            }
            if self.claude.team.confidence_threshold > 0 {
                env.insert(
                    "TEAM_CONFIDENCE_THRESHOLD".into(),
                    json!(self.claude.team.confidence_threshold.to_string()),
                );
            }
            if self.claude.team.risk_rounds {
                env.insert("TEAM_RISK_ROUNDS".into(), json!("1"));
                // Read-only roles (no Write/Edit tool) can't mitigate, so the
                // TeammateIdle gate exempts them instead of livelocking them.
                let readonly: Vec<&str> = self
                    .agents
                    .iter()
                    .filter(|a| a.mode != "primary")
                    .filter(|a| {
                        let t = a.tools.trim();
                        let d = &a.disallowed_tools;
                        (!t.is_empty() && !t.contains("Write") && !t.contains("Edit"))
                            || (d.contains("Write") && d.contains("Edit"))
                    })
                    .map(|a| a.name.as_str())
                    .collect();
                if !readonly.is_empty() {
                    env.insert("TEAM_READONLY_ROLES".into(), json!(readonly.join(" ")));
                }
            }
            if self.claude.team.approval_gate {
                env.insert("TEAM_APPROVAL_GATE".into(), json!("1"));
            }
        }
        let sub_conf = self.claude.workflow.subagent_confidence;
        if sub_conf > 0 {
            env.insert(
                "SUBAGENT_CONFIDENCE_THRESHOLD".into(),
                json!(sub_conf.to_string()),
            );
        }
        let check = self.claude.workflow.check_cmd.trim();
        if !check.is_empty() {
            env.insert("OCGEN_CHECK_CMD".into(), json!(check));
        }
        if self.has_blocking_hooks() {
            env.insert(
                "LOOP_GUARD_MAX_BLOCKS".into(),
                json!(self.claude.workflow.loop_guard_max.to_string()),
            );
        }
        env
    }

    /// The hooks table, shared by `settings.json` and the plugin's `hooks.json`.
    /// `dir` is where the hook scripts live; `prefix` is prepended to each script
    /// command (the plugin passes the gate env this way).
    fn hook_table(&self, dir: &str, prefix: &str) -> serde_json::Map<String, Value> {
        let mut hooks = serde_json::Map::new();
        if self.claude.powerups.hooks
            && (self.claude.workflow.intake || self.claude.workflow.improve_prompt)
        {
            let mut tip = String::from("Tip:");
            if self.claude.workflow.intake {
                tip.push_str(
                    " run /intake to gather requirements, then /refine to iterate before approval.",
                );
            }
            if self.claude.workflow.improve_prompt {
                tip.push_str(
                    " run /improve-prompt to sharpen a prompt or an agent's system prompt.",
                );
            }
            // Single-quote the echo argument, escaping any apostrophe in the tip
            // (e.g. "agent's") as '\'' so the command stays valid POSIX shell.
            let escaped = tip.trim().replace('\'', "'\\''");
            hooks.insert(
                "SessionStart".into(),
                json!([ { "hooks": [ {
                    "type": "command",
                    "command": format!("echo '{escaped}'")
                } ] } ]),
            );
        }
        if self.claude.team.enabled && self.claude.team.hooks {
            for (event, script) in [
                ("TeammateIdle", "team-teammate-idle.sh"),
                ("TaskCreated", "team-task-created.sh"),
                ("TaskCompleted", "team-task-completed.sh"),
            ] {
                hooks.insert(
                    event.to_string(),
                    json!([ { "hooks": [ {
                        "type": "command",
                        "command": hook_cmd(prefix, dir, script)
                    } ] } ]),
                );
            }
        }
        // Execution-approval gate: a PreToolUse hook scoped (via `matcher`) to the
        // tools that can perform or self-approve high-impact actions. Emitted
        // whenever the gate is on, independent of the `hooks` toggle.
        if self.claude.team.enabled && self.claude.team.approval_gate {
            hooks.insert(
                "PreToolUse".into(),
                json!([ {
                    "matcher": "Bash|Write|Edit|MultiEdit|NotebookEdit",
                    "hooks": [ {
                        "type": "command",
                        "command": hook_cmd(prefix, dir, "team-approval-gate.sh")
                    } ]
                } ]),
            );
        }
        // Per-worker confidence gate: a SubagentStop hook that blocks a subagent
        // which wrote files but isn't confident enough. Independent of teams —
        // it pairs with worktree isolation for isolated writes + enforced confidence.
        if self.worker_gate() {
            hooks.insert(
                "SubagentStop".into(),
                json!([ { "hooks": [ {
                    "type": "command",
                    "command": hook_cmd(prefix, dir, "subagent-confidence-gate.sh")
                } ] } ]),
            );
        }
        // Optional quality-of-life hooks. SessionStart may already hold the tip,
        // so groups are appended rather than inserted.
        let mut push = |event: &str, group: Value| {
            if let Some(arr) = hooks
                .entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
            {
                arr.push(group);
            }
        };
        let x = &self.claude.hooks_extra;
        if x.compact_context {
            push(
                "SessionStart",
                json!({ "matcher": "compact", "hooks": [ {
                    "type": "command",
                    "command": "echo 'Context was just compacted. Re-read the project rules in .claude/rules/ (workflow, team, loop discipline). If an /inquire study session is active, re-read its ledger resume point in .claude/notes/ before continuing.'"
                } ] }),
            );
        }
        if x.notify {
            for event in ["Notification", "StopFailure"] {
                push(
                    event,
                    json!({ "hooks": [ {
                        "type": "command",
                        "command": hook_cmd(prefix, dir, "notify.sh")
                    } ] }),
                );
            }
        }
        let fmt = x.format_cmd.trim();
        if !fmt.is_empty() {
            let fmt = fmt.replace('\'', "'\\''");
            push(
                "PostToolUse",
                json!({ "matcher": "Edit|Write", "hooks": [ {
                    "type": "command",
                    "command": hook_cmd(&format!("OCGEN_FORMAT_CMD='{fmt}' {prefix}"), dir, "format.sh")
                } ] }),
            );
        }
        if x.config_audit {
            push(
                "ConfigChange",
                json!({ "hooks": [ {
                    "type": "command",
                    "command": hook_cmd(prefix, dir, "config-audit.sh")
                } ] }),
            );
        }
        hooks
    }

    /// The project's `.mcp.json`; `None` when it has no MCP servers.
    fn mcp_json(&self) -> Result<Option<String>> {
        if self.claude.mcp_servers.is_empty() {
            return Ok(None);
        }
        let servers: serde_json::Map<String, Value> = self
            .claude
            .mcp_servers
            .iter()
            .map(|s| (s.name.clone(), s.to_json()))
            .collect();
        let v = json!({ "mcpServers": Value::Object(servers) });
        Ok(Some(format!("{}\n", serde_json::to_string_pretty(&v)?)))
    }

    /// Folder name for the generated plugin (repo name, else project name).
    fn plugin_name(&self) -> String {
        let raw = if !self.claude.plugin.repo_name.trim().is_empty() {
            self.claude.plugin.repo_name.trim()
        } else {
            self.project_name.trim()
        };
        raw.to_lowercase().replace(' ', "-")
    }

    /// The plugin's `hooks/hooks.json`: the same hooks as the project, with script
    /// paths under `${CLAUDE_PLUGIN_ROOT}` and the gate env passed as a command
    /// prefix (plugins can't set `env`). `None` when there are no hooks.
    fn plugin_hooks_json(&self) -> Result<Option<String>> {
        let prefix: String = self
            .gate_env()
            .iter()
            .map(|(k, v)| {
                let v = v.as_str().unwrap_or_default().replace('\'', "'\\''");
                format!("{k}='{v}' ")
            })
            .collect();
        let hooks = self.hook_table("${CLAUDE_PLUGIN_ROOT}/hooks", &prefix);
        if hooks.is_empty() {
            return Ok(None);
        }
        let v = json!({ "hooks": Value::Object(hooks) });
        Ok(Some(format!("{}\n", serde_json::to_string_pretty(&v)?)))
    }

    /// Write the rendered files (plus a state file) under `target`.
    /// Fails on any pre-existing output file unless `force` is set.
    pub fn scaffold(&self, target: &Path, force: bool) -> Result<Vec<PathBuf>> {
        let files = self.render_all()?;

        // CLAUDE.md is user-owned: ocgen creates it once and never overwrites it, so
        // it is exempt from both the conflict check and the (forced) rewrite below.
        let claude_md_path = std::path::Path::new("CLAUDE.md");
        if !force {
            for (rel, _) in &files {
                if rel.as_path() == claude_md_path {
                    continue;
                }
                let p = target.join(rel);
                if p.exists() {
                    bail!(
                        "{} already exists — re-run and choose to overwrite",
                        p.display()
                    );
                }
            }
        }

        let mut written = Vec::new();
        for (rel, contents) in &files {
            let path = target.join(rel);
            if rel.as_path() == claude_md_path && path.exists() {
                continue; // preserve the user's CLAUDE.md
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
        }
        if self.target == Target::ClaudeCode {
            self.remove_stale_generated(target)?;
            if let PrePush::Installed(p) = self.install_pre_push(target)? {
                written.push(p);
            }
        }

        // Persist project state so `add agent` can reload and re-render, with a
        // fingerprint of every file just written (lets `doctor` spot hand edits).
        let mut state = self.clone();
        state.generated = files
            .iter()
            .filter(|(rel, _)| rel.as_path() != claude_md_path)
            .map(|(rel, c)| (rel.to_string_lossy().replace('\\', "/"), fingerprint(c)))
            .collect();
        let state_path = target.join(self.target.state_file());
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&state_path, serde_json::to_string_pretty(&state)?)
            .with_context(|| format!("writing {}", state_path.display()))?;

        Ok(written)
    }

    /// Delete files an older ocgen generated under a name it no longer uses — only
    /// when their content is still ocgen's, so a user's own file is never touched.
    fn remove_stale_generated(&self, target: &Path) -> Result<()> {
        for p in self.stale_generated(target) {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            if let Some(dir) = p.parent() {
                // Drop a legacy dir (e.g. `.claude/commands`) once it's empty.
                if fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
                    let _ = fs::remove_dir(dir);
                }
            }
        }
        Ok(())
    }

    /// Install the git `pre-push` half of the approval gate into the repository's
    /// hooks — only when the gate is on, the target is a git repository, and no
    /// other pre-push hook exists. Never overwrites someone else's hook.
    pub fn install_pre_push(&self, target: &Path) -> Result<PrePush> {
        let gated = self.target == Target::ClaudeCode
            && self.claude.team.enabled
            && self.claude.team.approval_gate
            && (self.claude.output.project || !self.claude.output.plugin);
        if !gated {
            return Ok(PrePush::NotApplicable);
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(target)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) else {
            return Ok(PrePush::NotApplicable);
        };
        let hook = PathBuf::from(common).join("hooks/pre-push");
        // A custom hooks path (husky, lefthook…) means .git/hooks isn't used.
        if git(&["config", "core.hooksPath"]).is_some_and(|p| !p.is_empty()) {
            return Ok(PrePush::Foreign(hook));
        }
        if let Ok(existing) = fs::read_to_string(&hook) {
            if !existing.contains("ocgen:pre-push") {
                return Ok(PrePush::Foreign(hook));
            }
        }
        if let Some(dir) = hook.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&hook, templates::load("claude/hooks/git-pre-push.sh")?)
            .with_context(|| format!("writing {}", hook.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
        }
        Ok(PrePush::Installed(hook))
    }

    /// Files an older ocgen generated under names it no longer uses — only those
    /// whose content is still ocgen's, so a user's own file is never listed.
    fn stale_generated(&self, target: &Path) -> Vec<PathBuf> {
        if self.target != Target::ClaudeCode {
            return Vec::new();
        }
        let mut roots = vec![target.join(".claude")];
        if self.claude.output.plugin {
            roots.push(target.join(format!("plugin/{}", self.plugin_name())));
        }
        let mut stale = Vec::new();
        for root in roots {
            // Workflow commands moved to skills; old copies start with ocgen's
            // `description:` frontmatter.
            for name in crate::claude::WORKFLOW_SKILLS {
                let p = root.join(format!("commands/{name}.md"));
                if fs::read_to_string(&p).is_ok_and(|old| old.starts_with("---\ndescription:")) {
                    stale.push(p);
                }
            }
            // The old `Concise` style shadowed Claude Code's built-in style of that name.
            let p = root.join("output-styles/concise.md");
            if fs::read_to_string(&p).is_ok_and(|old| {
                old.contains("name: Concise")
                    && old.contains("Lead each answer with the result or recommendation")
            }) {
                stale.push(p);
            }
        }
        stale
    }

    /// What a forced re-scaffold (`doctor`) would do, file by file: add, modify,
    /// remove (stale legacy files) or leave unchanged. The user-owned `CLAUDE.md`
    /// is never part of the plan.
    pub fn plan_changes(&self, target: &Path) -> Result<Vec<FileChange>> {
        let mut plan = Vec::new();
        for (rel, new) in self.render_all()? {
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            let path = target.join(&rel);
            if rel_s == "CLAUDE.md" && path.exists() {
                continue;
            }
            let (kind, old, hand_edited) = match fs::read_to_string(&path) {
                Err(_) => (ChangeKind::Added, None, None),
                Ok(old) if old == new => (ChangeKind::Unchanged, None, None),
                Ok(old) => {
                    let edited = self.generated.get(&rel_s).map(|h| *h != fingerprint(&old));
                    (ChangeKind::Modified, Some(old), edited)
                }
            };
            let new = (kind != ChangeKind::Unchanged).then_some(new);
            plan.push(FileChange {
                path,
                rel: rel_s,
                kind,
                old,
                new,
                hand_edited,
            });
        }
        for path in self.stale_generated(target) {
            let rel = path
                .strip_prefix(target)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let old = fs::read_to_string(&path).ok();
            plan.push(FileChange {
                path,
                rel,
                kind: ChangeKind::Removed,
                old,
                new: None,
                hand_edited: None,
            });
        }
        Ok(plan)
    }

    /// Copy the current version of every file the plan would modify or remove into
    /// `<target>/.ocgen-backup/<UTC stamp>/` (the folder git-ignores itself). Returns
    /// the backup folder, or `None` when nothing would be overwritten.
    pub fn backup(target: &Path, plan: &[FileChange]) -> Result<Option<PathBuf>> {
        let at_risk: Vec<&FileChange> = plan
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Modified | ChangeKind::Removed))
            .collect();
        if at_risk.is_empty() {
            return Ok(None);
        }
        let root = target.join(".ocgen-backup");
        fs::create_dir_all(&root)?;
        let ignore = root.join(".gitignore");
        if !ignore.exists() {
            fs::write(&ignore, "*\n")?;
        }
        let mut dir = root.join(utc_stamp());
        let mut n = 1;
        while dir.exists() {
            n += 1;
            dir = root.join(format!("{}-{n}", utc_stamp()));
        }
        for c in at_risk {
            if let Some(old) = &c.old {
                let dest = dir.join(&c.rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dest, old).with_context(|| format!("backing up {}", c.rel))?;
            }
        }
        Self::prune_backups(target, 5)?;
        Ok(Some(dir))
    }

    /// Keep only the newest `keep` backup folders (names sort chronologically).
    pub fn prune_backups(target: &Path, keep: usize) -> Result<()> {
        let root = target.join(".ocgen-backup");
        let Ok(entries) = fs::read_dir(&root) else {
            return Ok(());
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        let excess = dirs.len().saturating_sub(keep);
        for d in dirs.into_iter().take(excess) {
            fs::remove_dir_all(&d).with_context(|| format!("pruning {}", d.display()))?;
        }
        Ok(())
    }

    /// Consistency problems worth surfacing: agents pointing at an unknown
    /// provider or a model the provider doesn't offer, a missing/duplicate
    /// coordinator, or an undefined utility provider. Used by `landscape`.
    /// Named colours Claude Code accepts.
    const CLAUDE_COLORS: [&'static str; 8] = [
        "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
    ];

    /// Consistency checks for a Claude Code project (aliases + coordinators).
    fn claude_issues(&self) -> Vec<String> {
        let mut w = Vec::new();
        for a in &self.agents {
            let m = if a.model.trim().is_empty() {
                "opus"
            } else {
                a.model.trim()
            };
            if !crate::claude::is_valid_agent_model(m) {
                w.push(format!(
                    "agent '{}' uses unknown model alias '{}'",
                    a.name, a.model
                ));
            }
            w.extend(Self::agent_field_issues(a));
            for s in crate::claude::split_list(&a.mcp_servers) {
                if !self.claude.mcp_servers.iter().any(|m| m.name == s) {
                    w.push(format!(
                        "agent '{}' uses MCP server '{s}', which isn't defined (add it with `ocgen add mcp`)",
                        a.name
                    ));
                }
            }
        }
        for m in &self.claude.mcp_servers {
            for k in m.literal_secrets() {
                w.push(format!(
                    "MCP server '{}': {k} holds a literal value — reference an env var instead, e.g. ${{{k}}}",
                    m.name
                ));
            }
        }
        let primaries = self.agents.iter().filter(|a| a.mode == "primary").count();
        if primaries > 1 {
            w.push(format!(
                "{primaries} coordinators — usually there is a single primary agent"
            ));
        }
        for s in &self.skills {
            if crate::claude::WORKFLOW_SKILLS.contains(&s.name.as_str()) {
                w.push(format!(
                    "skill '{}': collides with the generated /{} workflow skill — rename it",
                    s.name, s.name
                ));
            }
            for issue in crate::claude::skill_issues(s) {
                w.push(format!("skill '{}': {issue}", s.name));
            }
        }
        w
    }

    /// Invalid values in an agent's enum fields (effort, permission mode, memory),
    /// plus a call-out for `bypassPermissions`.
    fn agent_field_issues(a: &Agent) -> Vec<String> {
        use crate::claude::{EFFORT_LEVELS, MEMORY_SCOPES, PERMISSION_MODES};
        let mut w = Vec::new();
        let bad = |v: &str, ok: &[&str]| !v.is_empty() && !ok.contains(&v);
        if bad(a.effort.trim(), &EFFORT_LEVELS) {
            w.push(format!(
                "agent '{}': unknown effort '{}' (use {})",
                a.name,
                a.effort,
                EFFORT_LEVELS.join("/")
            ));
        }
        if bad(a.permission_mode.trim(), &PERMISSION_MODES) {
            w.push(format!(
                "agent '{}': unknown permission mode '{}' (use {})",
                a.name,
                a.permission_mode,
                PERMISSION_MODES.join("/")
            ));
        }
        if a.permission_mode.trim() == "bypassPermissions" {
            w.push(format!(
                "agent '{}': permissionMode bypassPermissions skips every permission check — prefer acceptEdits or a narrower tools list",
                a.name
            ));
        }
        if bad(a.memory.trim(), &MEMORY_SCOPES) {
            w.push(format!(
                "agent '{}': unknown memory scope '{}' (use {})",
                a.name,
                a.memory,
                MEMORY_SCOPES.join("/")
            ));
        }
        w
    }

    /// Repair a Claude Code project: fix invalid model aliases, colours, and empty roles.
    fn claude_doctor(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        for a in &mut self.agents {
            let m = a.model.trim().to_string();
            if !crate::claude::is_valid_agent_model(&m) {
                fixes.push(format!(
                    "agent '{}': model alias '{}' → opus",
                    a.name, a.model
                ));
                a.model = "opus".into();
            }
            let c = a.color.trim().to_string();
            if !c.is_empty() && !Self::CLAUDE_COLORS.contains(&c.as_str()) {
                fixes.push(format!("agent '{}': colour '{}' → blue", a.name, a.color));
                a.color = "blue".into();
            }
            for (label, field, ok) in [
                ("effort", &mut a.effort, &crate::claude::EFFORT_LEVELS[..]),
                (
                    "permission mode",
                    &mut a.permission_mode,
                    &crate::claude::PERMISSION_MODES[..],
                ),
                (
                    "memory scope",
                    &mut a.memory,
                    &crate::claude::MEMORY_SCOPES[..],
                ),
            ] {
                let v = field.trim().to_string();
                if !v.is_empty() && !ok.contains(&v.as_str()) {
                    fixes.push(format!("agent '{}': unknown {label} '{v}' → unset", a.name));
                    field.clear();
                }
            }
            if a.role.trim().is_empty() {
                a.role = "custom".into();
                fixes.push(format!("agent '{}': empty role → 'custom'", a.name));
            }
        }
        if self.claude.team.enabled {
            let m = self.claude.team.mode.trim().to_string();
            if m.is_empty() || !["in-process", "auto", "tmux", "iterm2"].contains(&m.as_str()) {
                fixes.push(format!(
                    "teammate mode '{}' → in-process",
                    self.claude.team.mode
                ));
                self.claude.team.mode = "in-process".into();
            }
            if self.claude.team.confidence_threshold > 100 {
                fixes.push(format!(
                    "team confidence threshold {} → 96",
                    self.claude.team.confidence_threshold
                ));
                self.claude.team.confidence_threshold = 96;
            }
        }
        if self.claude.workflow.subagent_confidence > 100 {
            fixes.push(format!(
                "subagent confidence {} → 96",
                self.claude.workflow.subagent_confidence
            ));
            self.claude.workflow.subagent_confidence = 96;
        }
        fixes
    }

    pub fn issues(&self) -> Vec<String> {
        if self.target == Target::ClaudeCode {
            return self.claude_issues();
        }
        let mut warnings = Vec::new();
        let keys: std::collections::HashSet<&str> =
            self.providers.iter().map(|p| p.key.as_str()).collect();

        for a in &self.agents {
            match self.providers.iter().find(|p| p.key == a.provider) {
                None => warnings.push(format!(
                    "agent '{}' uses unknown provider '{}'",
                    a.name, a.provider
                )),
                Some(prov) => {
                    if !prov.models.iter().any(|m| m.id == a.model) {
                        warnings.push(format!(
                            "agent '{}' uses model '{}' not offered by provider '{}'",
                            a.name, a.model, a.provider
                        ));
                    }
                }
            }
        }

        let primaries = self.agents.iter().filter(|a| a.mode == "primary").count();
        if primaries == 0 {
            warnings.push("no primary agent — there is no coordinator or /multi command".into());
        } else if primaries > 1 {
            warnings.push(format!(
                "{primaries} primary agents — usually there is a single coordinator"
            ));
        }

        if !keys.contains(self.utility_provider.as_str()) {
            warnings.push(format!(
                "utility provider '{}' is not defined",
                self.utility_provider
            ));
        }
        warnings
    }

    /// Delete a Claude subagent's generated file (used on rename).
    pub fn remove_claude_agent_file(target: &Path, name: &str) -> Result<()> {
        let p = target.join(format!(".claude/agents/{name}.md"));
        if p.exists() {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        Ok(())
    }

    /// Delete a Claude skill's generated directory (used on rename).
    /// Create a skill's supporting-file stubs — `reference.md` (detail loaded on
    /// demand) and `scripts/README.md` (deterministic helpers) — only where absent.
    /// ocgen never rewrites these, so they're the user's from then on. Returns the
    /// files it created.
    pub fn scaffold_skill_extras(
        target: &Path,
        name: &str,
        reference: bool,
        scripts: bool,
    ) -> Result<Vec<PathBuf>> {
        let dir = target.join(format!(".claude/skills/{name}"));
        let mut made = Vec::new();
        let mut stub = |rel: &str, body: &str| -> Result<()> {
            let p = dir.join(rel);
            if !p.exists() {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&p, body).with_context(|| format!("writing {}", p.display()))?;
                made.push(p);
            }
            Ok(())
        };
        if reference {
            stub(
                "reference.md",
                &format!(
                    "# {name} — reference\n\nDetail SKILL.md points to and Claude reads only when needed:\n\
                     checklists, schemas, examples, edge cases. Keep SKILL.md itself short.\n"
                ),
            )?;
        }
        if scripts {
            stub(
                "scripts/README.md",
                "# Scripts\n\nDeterministic helpers SKILL.md tells Claude to run (e.g. `scripts/check.sh`),\n\
                 so the same logic isn't re-derived every time. Scope them in allowed-tools as\n\
                 Bash(scripts/check.sh:*).\n",
            )?;
        }
        Ok(made)
    }

    /// After a rename (and a re-scaffold that wrote the new SKILL.md), carry the
    /// old skill dir's supporting files over, then remove the old dir.
    pub fn rename_claude_skill_dir(target: &Path, old: &str, new: &str) -> Result<()> {
        let from = target.join(format!(".claude/skills/{old}"));
        let to = target.join(format!(".claude/skills/{new}"));
        if !from.exists() || from == to {
            return Ok(());
        }
        fs::create_dir_all(&to)?;
        for entry in fs::read_dir(&from)? {
            let entry = entry?;
            let name = entry.file_name();
            let dest = to.join(&name);
            if name == "SKILL.md" || dest.exists() {
                continue;
            }
            fs::rename(entry.path(), &dest).with_context(|| {
                format!("moving {} to {}", entry.path().display(), dest.display())
            })?;
        }
        Self::remove_claude_skill_dir(target, old)
    }

    pub fn remove_claude_skill_dir(target: &Path, name: &str) -> Result<()> {
        let p = target.join(format!(".claude/skills/{name}"));
        if p.exists() {
            fs::remove_dir_all(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        Ok(())
    }

    /// Delete an agent's generated files (its agent md and any prompt file).
    /// Used to clean up stale files after an agent is renamed.
    pub fn remove_agent_artifacts(target: &Path, name: &str) -> Result<()> {
        for rel in [
            format!(".opencode/agents/{name}.md"),
            format!(".opencode/prompts/{name}.txt"),
        ] {
            let p = target.join(rel);
            if p.exists() {
                fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        Ok(())
    }

    /// Load a previously scaffolded project's state from `target`, migrating the
    /// JSON forward from older schema versions first so older projects still load.
    pub fn load_state(target: &Path) -> Result<Project> {
        let path = state_file_in(target).ok_or_else(|| {
            anyhow!(
                "no ocgen project found at {} (missing {})",
                target.display(),
                Target::OpenCode.state_file()
            )
        })?;
        let data =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut value: Value =
            serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
        migrate_state(&mut value)?;
        serde_json::from_value(value).with_context(|| format!("loading {}", path.display()))
    }

    /// Repair a loaded project in place: reassign agents pointing at unknown
    /// providers/models, fill empty required fields, and fix the utility target.
    /// Returns a human-readable list of what was changed.
    pub fn doctor(&mut self) -> Vec<String> {
        if self.target == Target::ClaudeCode {
            return self.claude_doctor();
        }
        let mut fixes = Vec::new();
        let providers = self.providers.clone();
        let keys: Vec<String> = providers.iter().map(|p| p.key.clone()).collect();
        let fallback = if keys.contains(&self.utility_provider) {
            self.utility_provider.clone()
        } else {
            keys.first().cloned().unwrap_or_default()
        };

        // Utility provider + model must exist.
        if !keys.contains(&self.utility_provider) {
            if let Some(k) = keys.first() {
                fixes.push(format!(
                    "utility provider '{}' undefined → '{}'",
                    self.utility_provider, k
                ));
                self.utility_provider = k.clone();
            }
        }
        if let Some(prov) = providers.iter().find(|p| p.key == self.utility_provider) {
            if !prov.models.iter().any(|m| m.id == self.utility_model) {
                if let Some(m) = prov.models.first() {
                    fixes.push(format!(
                        "utility model '{}' not in '{}' → '{}'",
                        self.utility_model, self.utility_provider, m.id
                    ));
                    self.utility_model = m.id.clone();
                }
            }
        }

        for a in &mut self.agents {
            if !matches!(a.mode.as_str(), "primary" | "subagent" | "all") {
                fixes.push(format!(
                    "agent '{}': invalid mode '{}' → subagent",
                    a.name, a.mode
                ));
                a.mode = "subagent".into();
            }
            if a.role.trim().is_empty() {
                a.role = "custom".into();
                fixes.push(format!("agent '{}': empty role → 'custom'", a.name));
            }
            if !keys.contains(&a.provider) {
                fixes.push(format!(
                    "agent '{}': unknown provider '{}' → '{}'",
                    a.name, a.provider, fallback
                ));
                a.provider = fallback.clone();
            }
            if let Some(prov) = providers.iter().find(|p| p.key == a.provider) {
                if !prov.models.iter().any(|m| m.id == a.model) {
                    if let Some(m) = prov.models.first() {
                        fixes.push(format!(
                            "agent '{}': model '{}' not in '{}' → '{}'",
                            a.name, a.model, a.provider, m.id
                        ));
                        a.model = m.id.clone();
                    }
                }
            }
            if a.temperature.trim().is_empty() {
                a.temperature = "0.2".into();
                fixes.push(format!("agent '{}': empty temperature → 0.2", a.name));
            }
            if a.color.trim().is_empty() {
                a.color = "accent".into();
                fixes.push(format!("agent '{}': empty color → accent", a.name));
            }
            if a.permissions.trim().is_empty() {
                a.permissions = "  edit: ask\n  bash:\n    \"*\": ask".into();
                fixes.push(format!(
                    "agent '{}': empty permissions → default block",
                    a.name
                ));
            }
            let empty_prompt = a
                .prompt_body
                .as_deref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true);
            if a.prompt_file && empty_prompt {
                a.prompt_file = false;
                a.prompt_body = None;
                fixes.push(format!(
                    "agent '{}': prompt file enabled but body empty → disabled",
                    a.name
                ));
            }
        }
        fixes
    }

    /// Find the project root by looking for the state file in `start` and each of
    /// its ancestor directories (like `git` locating `.git`), and load it.
    /// Returns the resolved root alongside the project.
    pub fn discover(start: &Path) -> Result<(PathBuf, Project)> {
        let root = find_root(start).ok_or_else(|| {
            anyhow!(
                "no ocgen project found in {} or any parent directory\n\
                 run this from inside a project, pass its path, or create one with `ocgen new`",
                start.display()
            )
        })?;
        let project = Self::load_state(&root)?;
        Ok((root, project))
    }
}

/// Bring an older state-file JSON up to the current schema before typed loading:
/// reconstruct `providers` from the old single-provider fields and backfill agent
/// fields (role/provider and archetype-derived values) that older versions omitted.
fn migrate_state(value: &mut Value) -> Result<()> {
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("English")
        .to_string();

    // Old layout stored one provider inline (provider_key/provider_name/npm/...).
    let has_providers = value
        .get("providers")
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !has_providers {
        if let Some(key) = value
            .get("provider_key")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let name = value
                .get("provider_name")
                .and_then(Value::as_str)
                .unwrap_or(&key)
                .to_string();
            let npm = value
                .get("npm")
                .and_then(Value::as_str)
                .unwrap_or("@ai-sdk/openai-compatible")
                .to_string();
            let base_url = value
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let models = value.get("models").cloned().unwrap_or_else(|| json!([]));
            if let Some(obj) = value.as_object_mut() {
                obj.entry("utility_provider")
                    .or_insert_with(|| json!(key.clone()));
                obj.insert(
                    "providers".into(),
                    json!([{ "key": key, "name": name, "npm": npm, "base_url": base_url, "models": models }]),
                );
            }
        }
    }

    // Determine a default provider key for agents that lack one.
    let first_key = value
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|p| p.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(obj) = value.as_object_mut() {
        obj.entry("utility_provider")
            .or_insert_with(|| json!(first_key));
    }
    let default_provider = value
        .get("utility_provider")
        .and_then(Value::as_str)
        .unwrap_or(&first_key)
        .to_string();

    if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            migrate_agent(agent, &language, &default_provider)?;
        }
    }
    Ok(())
}

/// Backfill a single agent's newer fields, using its legacy `archetype` as the
/// source of truth for permissions/body/etc. where those fields are absent.
fn migrate_agent(agent: &mut Value, language: &str, default_provider: &str) -> Result<()> {
    let archetype = agent
        .get("archetype")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = agent
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let provider = agent
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(default_provider)
        .to_string();

    // Seed from the archetype only if some archetype-derived field is missing.
    let derived = [
        "mode",
        "model",
        "temperature",
        "steps",
        "color",
        "permissions",
        "body",
        "prompt_file",
        "prompt_body",
        "description",
    ];
    let needs_seed = derived.iter().any(|k| agent.get(k).is_none());
    let seed_val = if needs_seed {
        let seed = match &archetype {
            Some(arch) => Agent::from_archetype(&name, arch, language, &provider)
                .unwrap_or_else(|_| Agent::blank(&name, arch, &provider)),
            None => Agent::blank(&name, "custom", &provider),
        };
        Some(serde_json::to_value(seed)?)
    } else {
        None
    };

    let obj = agent
        .as_object_mut()
        .ok_or_else(|| anyhow!("agent entry in state file is not a JSON object"))?;

    obj.entry("role")
        .or_insert_with(|| json!(archetype.clone().unwrap_or_else(|| "custom".into())));
    obj.entry("provider")
        .or_insert_with(|| json!(default_provider));

    if let Some(seed) = &seed_val {
        for key in derived {
            if !obj.contains_key(key) {
                if let Some(v) = seed.get(key) {
                    obj.insert(key.into(), v.clone());
                }
            }
        }
    }
    Ok(())
}

/// Walk up from `start` looking for a directory that holds the state file.
fn find_root(start: &Path) -> Option<PathBuf> {
    // Plain, not verbatim: this path is handed to git, bash and hook environments.
    let start = crate::paths::plain(&start.canonicalize().ok()?);
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        if state_file_in(dir).is_some() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Turn a rendered command file (`---` frontmatter + body) into a skill: add its
/// `name` and, for side-effecting workflows, `disable-model-invocation: true`.
fn command_to_skill(name: &str, command_md: &str, user_run: bool) -> String {
    let rest = command_md.strip_prefix("---\n").unwrap_or(command_md);
    let (front, body) = match rest.split_once("\n---\n") {
        Some((f, b)) => (f, b),
        None => ("", rest),
    };
    let mut out = format!("---\nname: {name}\n");
    if !front.is_empty() {
        out.push_str(front);
        out.push('\n');
    }
    if user_run {
        out.push_str("disable-model-invocation: true\n");
    }
    out.push_str("---\n");
    out.push_str(body);
    out
}

/// A hook command: run `ocgen hook <name>` when the installed ocgen speaks exactly
/// this project's hook protocol, otherwise the bundled script — so a project works
/// without the binary, and a stale binary can never apply outdated gate logic.
/// `prefix` carries env assignments.
fn hook_cmd(prefix: &str, dir: &str, script: &str) -> String {
    let name = script.trim_end_matches(".sh");
    let protocol = crate::hooks::PROTOCOL;
    format!(
        "if [ \"$(ocgen hook --check 2>/dev/null)\" = \"{protocol}\" ]; then {prefix}ocgen hook {name}; else {prefix}sh \"{dir}/{script}\"; fi"
    )
}
