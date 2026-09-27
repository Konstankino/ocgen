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
                        .render_str(prompt_src, context! { subagents => &subs, language => lang })
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
                "sonnet"
            } else {
                agent.model.trim()
            };
            let agent_val = context! {
                name => agent.name,
                description => agent.description,
                tools => agent.tools.trim(),
                model => model,
                color => agent.color,
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

        let skill_tmpl = templates::load("claude/skill/SKILL.md.j2")?;
        for skill in &self.skills {
            let sval = context! {
                name => skill.name,
                description => skill.description,
                allowed_tools => skill.allowed_tools.trim(),
                body => skill.body,
            };
            let md = env
                .render_str(&skill_tmpl, context! { skill => sval })
                .with_context(|| format!("rendering skill '{}'", skill.name))?;
            components.push((format!("skills/{}/SKILL.md", skill.name), md));
        }

        if self.claude.powerups.output_style {
            components.push((
                "output-styles/concise.md".to_string(),
                env.render_str(&templates::load("claude/output-styles/concise.md.j2")?, context! {})
                    .context("rendering output style")?,
            ));
        }

        // Coordinator body may itself be a template (the orchestrator prompt loops
        // over subagents), so render it with that context for CLAUDE.md.
        let coordinator = match self.primary() {
            Some(p) => env
                .render_str(&p.body, context! { subagents => &subs, language => lang })
                .context("rendering coordinator instructions")?,
            None => String::new(),
        };
        let claude_md = env
            .render_str(
                &templates::load("claude/CLAUDE.md.j2")?,
                context! {
                    project_name => self.project_name,
                    instructions => self.claude.instructions,
                    coordinator => coordinator,
                    subagents => &subs,
                    language => lang,
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
            out.push((PathBuf::from("CLAUDE.md"), claude_md));
        }

        if want_plugin {
            let plugin_name = self.plugin_name();
            let base = format!("plugin/{plugin_name}");
            for (rel, c) in &components {
                out.push((PathBuf::from(format!("{base}/{rel}")), c.clone()));
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
                .render_str(&templates::load("claude/plugin/plugin.json.j2")?, mctx.clone())
                .context("rendering plugin.json")?;
            serde_json::from_str::<Value>(&plugin_json)
                .context("rendered plugin.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/plugin.json")),
                plugin_json,
            ));
            let market = env
                .render_str(&templates::load("claude/plugin/marketplace.json.j2")?, mctx.clone())
                .context("rendering marketplace.json")?;
            serde_json::from_str::<Value>(&market)
                .context("rendered marketplace.json is not valid JSON (check your template)")?;
            out.push((
                PathBuf::from(format!("{base}/.claude-plugin/marketplace.json")),
                market,
            ));
            out.push((
                PathBuf::from(format!("{base}/README.md")),
                env.render_str(&templates::load("claude/plugin/README.md.j2")?, mctx.clone())
                    .context("rendering plugin README")?,
            ));
            out.push((
                PathBuf::from(format!("{base}/.github/workflows/release.yml")),
                env.render_str(&templates::load("claude/plugin/release.yml.j2")?, mctx)
                    .context("rendering release workflow")?,
            ));
            if self.claude.powerups.hooks && self.claude.workflow.intake {
                out.push((
                    PathBuf::from(format!("{base}/hooks/hooks.json")),
                    self.plugin_hooks_json()?,
                ));
            }
        }

        Ok(out)
    }
    /// Build `.claude/settings.json` as validated JSON from the power-up toggles.
    fn claude_settings_json(&self) -> Result<String> {
        let model = if self.claude.model.trim().is_empty() {
            "sonnet"
        } else {
            self.claude.model.trim()
        };
        let mut root = json!({ "model": model });
        let obj = root.as_object_mut().unwrap();
        if self.claude.powerups.permissions {
            obj.insert(
                "permissions".into(),
                json!({
                    "allow": ["Read","Grep","Glob","Edit","Write","Bash(git status:*)","Bash(git diff:*)","Bash(git log:*)"],
                    "ask": ["Bash(git push:*)","Bash(git commit:*)"],
                    "deny": ["Bash(rm -rf:*)"]
                }),
            );
        }
        if self.claude.powerups.output_style {
            obj.insert("outputStyle".into(), json!("Concise"));
        }
        if self.claude.powerups.statusline {
            obj.insert(
                "statusLine".into(),
                json!({ "type": "command", "command": "echo \"[$(basename \"$PWD\")]\"" }),
            );
        }
        if self.claude.powerups.hooks && self.claude.workflow.intake {
            obj.insert(
                "hooks".into(),
                json!({
                    "SessionStart": [ { "hooks": [ {
                        "type": "command",
                        "command": "echo 'Tip: run /intake to gather requirements, then /refine to iterate before approval.'"
                    } ] } ]
                }),
            );
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&root)?))
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

    /// The plugin's `hooks/hooks.json` (SessionStart nudge to /intake).
    fn plugin_hooks_json(&self) -> Result<String> {
        let v = json!({
            "hooks": {
                "SessionStart": [ { "hooks": [ {
                    "type": "command",
                    "command": "echo 'Tip: run /intake to gather requirements, then /refine to iterate before approval.'"
                } ] } ]
            }
        });
        Ok(format!("{}\n", serde_json::to_string_pretty(&v)?))
    }

    /// Write the rendered files (plus a state file) under `target`.
    /// Fails on any pre-existing output file unless `force` is set.
    pub fn scaffold(&self, target: &Path, force: bool) -> Result<Vec<PathBuf>> {
        let files = self.render_all()?;

        if !force {
            for (rel, _) in &files {
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
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
        }

        // Persist project state so `add agent` can reload and re-render.
        let state_path = target.join(self.target.state_file());
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&state_path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", state_path.display()))?;

        Ok(written)
    }

    /// Consistency problems worth surfacing: agents pointing at an unknown
    /// provider or a model the provider doesn't offer, a missing/duplicate
    /// coordinator, or an undefined utility provider. Used by `landscape`.
    /// Model aliases Claude Code accepts for an agent.
    const CLAUDE_ALIASES: [&'static str; 5] = ["opus", "sonnet", "haiku", "fable", "inherit"];
    /// Named colours Claude Code accepts.
    const CLAUDE_COLORS: [&'static str; 8] =
        ["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"];

    /// Consistency checks for a Claude Code project (aliases + coordinators).
    fn claude_issues(&self) -> Vec<String> {
        let mut w = Vec::new();
        for a in &self.agents {
            let m = if a.model.trim().is_empty() {
                "sonnet"
            } else {
                a.model.trim()
            };
            if !Self::CLAUDE_ALIASES.contains(&m) {
                w.push(format!(
                    "agent '{}' uses unknown model alias '{}'",
                    a.name, a.model
                ));
            }
        }
        let primaries = self.agents.iter().filter(|a| a.mode == "primary").count();
        if primaries > 1 {
            w.push(format!(
                "{primaries} coordinators — usually there is a single primary agent"
            ));
        }
        w
    }

    /// Repair a Claude Code project: fix invalid model aliases, colours, and empty roles.
    fn claude_doctor(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        for a in &mut self.agents {
            let m = a.model.trim().to_string();
            if m.is_empty() || !Self::CLAUDE_ALIASES.contains(&m.as_str()) {
                fixes.push(format!("agent '{}': model alias '{}' → sonnet", a.name, a.model));
                a.model = "sonnet".into();
            }
            let c = a.color.trim().to_string();
            if !c.is_empty() && !Self::CLAUDE_COLORS.contains(&c.as_str()) {
                fixes.push(format!("agent '{}': colour '{}' → blue", a.name, a.color));
                a.color = "blue".into();
            }
            if a.role.trim().is_empty() {
                a.role = "custom".into();
                fixes.push(format!("agent '{}': empty role → 'custom'", a.name));
            }
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
        let data = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
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
                fixes.push(format!("agent '{}': invalid mode '{}' → subagent", a.name, a.mode));
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
                fixes.push(format!("agent '{}': empty permissions → default block", a.name));
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
        if let Some(key) = value.get("provider_key").and_then(Value::as_str).map(str::to_string) {
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
    let archetype = agent.get("archetype").and_then(Value::as_str).map(str::to_string);
    let name = agent.get("name").and_then(Value::as_str).unwrap_or("agent").to_string();
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
    let start = start.canonicalize().ok()?;
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        if state_file_in(dir).is_some() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}
