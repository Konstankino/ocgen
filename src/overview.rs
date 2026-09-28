//! `ocgen landscape` (alias `horizon`) — a read-only overview of a project:
//! its providers, every agent's key config, how they delegate, and consistency
//! checks, so you can review the whole setup at a glance and see what to update.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

use ocgen::agent::Agent;
use ocgen::render::Project;
use ocgen::target::Target;

use crate::ui;

pub fn run(path: String) -> Result<()> {
    let (root, project) = Project::discover(Path::new(&path))?;
    if project.target == Target::ClaudeCode {
        return run_claude(&root, &project);
    }

    // Header.
    println!();
    println!(
        "  {} {}  {}",
        style("◆").cyan().bold(),
        style(&project.project_name).bold(),
        ui::muted(&format!("({})", project.language))
    );
    ui::kv("path", &ui::muted(&root.display().to_string()));

    // Providers.
    ui::section(&format!("Providers ({})", project.providers.len()));
    let prov_rows: Vec<Vec<String>> = project
        .providers
        .iter()
        .map(|p| {
            let is_util = p.key == project.utility_provider;
            let key = if is_util {
                format!("{} {}", style(&p.key).bold(), style("★").yellow())
            } else {
                style(&p.key).bold().to_string()
            };
            vec![
                key,
                p.name.clone(),
                ui::muted(&p.base_url),
                p.models
                    .iter()
                    .map(|m| m.id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    ui::table(&["KEY", "NAME", "BASE URL", "MODELS"], &prov_rows);
    println!(
        "  {} {}",
        style("★").yellow(),
        ui::muted(&format!(
            "utility provider — compaction/title/summary run on {}/{}",
            project.utility_provider, project.utility_model
        ))
    );

    // Agents.
    ui::section(&format!("Agents ({})", project.agents.len()));
    let agent_rows: Vec<Vec<String>> = project
        .agents
        .iter()
        .map(|a| {
            let mode = match a.mode.as_str() {
                "primary" => style("primary").magenta().bold().to_string(),
                "all" => style("all").cyan().to_string(),
                _ => ui::muted("subagent"),
            };
            let prompt = if a.prompt_file {
                style("file").cyan().to_string()
            } else {
                ui::muted("-")
            };
            let color_swatch = color_cell(&a.color);
            vec![
                style(&a.name).bold().to_string(),
                mode,
                format!("{}/{}", style(&a.provider).cyan(), a.model),
                a.temperature.clone(),
                a.steps
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| ui::muted("-")),
                color_swatch,
                prompt,
                ui::muted(&ui::truncate(&a.description, 38)),
            ]
        })
        .collect();
    ui::table(
        &[
            "NAME",
            "MODE",
            "MODEL",
            "TEMP",
            "STEPS",
            "COLOR",
            "PROMPT",
            "DESCRIPTION",
        ],
        &agent_rows,
    );

    // Topology: how primaries delegate to subagents.
    let subs: Vec<&str> = project
        .agents
        .iter()
        .filter(|a| a.mode == "subagent")
        .map(|a| a.name.as_str())
        .collect();
    let primaries: Vec<&Agent> = project
        .agents
        .iter()
        .filter(|a| a.mode == "primary")
        .collect();
    if !primaries.is_empty() {
        ui::section("Topology");
        for p in &primaries {
            println!(
                "  {} {}",
                style(&p.name).magenta().bold(),
                ui::muted("(primary)")
            );
            if subs.is_empty() {
                println!("  {}", ui::muted("╰─ (no subagents to delegate to)"));
            } else {
                let list = subs
                    .iter()
                    .map(|s| style(*s).cyan().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  {} delegates to: {}", ui::muted("╰─"), list);
            }
        }
        println!(
            "  {} {} → {}",
            ui::muted("command:"),
            style("/multi").cyan(),
            style(&primaries[0].name).magenta()
        );
    }

    // Consistency checks — the "where to update" hints.
    let warnings = project.issues();
    if warnings.is_empty() {
        ui::section("Checks");
        ui::success("no problems found");
    } else {
        ui::section(&format!("Checks ({})", warnings.len()));
        for w in &warnings {
            ui::warning(w);
        }
    }

    ui::tip(
        "update with `ocgen edit agent <name>`, `ocgen edit provider <key>`, or `ocgen doctor`",
    );
    Ok(())
}

/// Show the colour value with a filled swatch of how it looks (hex or theme name).
fn color_cell(color: &str) -> String {
    match ui::color_swatch(color) {
        Some(swatch) => format!("{swatch} {color}"),
        None => color.to_string(),
    }
}

/// `ocgen show agent [NAME]` — print one agent's full configuration.
pub fn show_agent(path: String, name: Option<String>) -> Result<()> {
    let (root, project) = Project::discover(Path::new(&path))?;
    if project.agents.is_empty() {
        bail!("this project has no agents");
    }

    let idx = match name {
        Some(n) => project
            .agents
            .iter()
            .position(|a| a.name == n)
            .ok_or_else(|| {
                let have = project
                    .agents
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!("no agent named '{n}' (have: {have})")
            })?,
        None => {
            let labels: Vec<String> = project
                .agents
                .iter()
                .map(|a| format!("{} — {}, {}/{}", a.name, a.mode, a.provider, a.model))
                .collect();
            Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Which agent?")
                .items(&labels)
                .default(0)
                .interact()?
        }
    };

    print_agent(&project, &root, idx);
    Ok(())
}

/// Render one agent's full config to the terminal.
fn print_agent(project: &Project, root: &Path, idx: usize) {
    if project.target == Target::ClaudeCode {
        return print_agent_claude(project, root, idx);
    }
    let a = &project.agents[idx];
    let dash = |s: &str| {
        if s.trim().is_empty() {
            ui::muted("—")
        } else {
            s.to_string()
        }
    };

    // Header.
    println!();
    println!(
        "  {} {}  {}",
        style("◆").cyan().bold(),
        style(&a.name).bold(),
        ui::muted(&format!("(in {})", project.project_name))
    );
    let file = root.join(format!(".opencode/agents/{}.md", a.name));
    ui::kv("file", &ui::muted(&file.display().to_string()));

    // Core configuration.
    ui::section("Configuration");
    ui::kv("role", &dash(&a.role));
    let mode = match a.mode.as_str() {
        "primary" => style("primary").magenta().bold().to_string(),
        "all" => style("all").cyan().to_string(),
        _ => a.mode.clone(),
    };
    ui::kv("mode", &mode);
    ui::kv(
        "model",
        &format!("{}/{}", style(&a.provider).cyan(), a.model),
    );
    ui::kv("variant", &dash(&a.variant));
    ui::kv("temperature", &a.temperature);
    ui::kv("top_p", &dash(&a.top_p));
    ui::kv(
        "steps",
        &a.steps
            .map(|s| s.to_string())
            .unwrap_or_else(|| ui::muted("—")),
    );
    ui::kv("color", &color_cell(&a.color));
    if a.disable {
        ui::kv("disable", &style("true").yellow().to_string());
    }
    if a.hidden {
        ui::kv("hidden", &style("true").yellow().to_string());
    }
    ui::kv("description", &dash(&a.description));

    // Permissions (as they appear in the generated file, incl. the task block).
    ui::section("Permissions");
    print_block(&effective_permissions(project, a));

    // Provider-specific options.
    if !a.options.trim().is_empty() {
        ui::section("Model options");
        print_block(a.options.trim_end());
    }

    // System prompt.
    if a.prompt_file {
        ui::section("System prompt (external file)");
        let pf = root.join(format!(".opencode/prompts/{}.txt", a.name));
        ui::kv("file", &ui::muted(&pf.display().to_string()));
        if let Some(body) = &a.prompt_body {
            print_block(body.trim_end());
        }
    } else {
        ui::section("System prompt");
        print_block(if a.body.trim().is_empty() {
            "(none)"
        } else {
            a.body.trim_end()
        });
    }

    // How it fits with the rest of the team.
    ui::section("Relationships");
    let subs: Vec<&str> = project
        .agents
        .iter()
        .filter(|x| x.mode == "subagent")
        .map(|x| x.name.as_str())
        .collect();
    let is_primary = a.mode == "primary" || a.mode == "all";
    let is_sub = a.mode == "subagent" || a.mode == "all";
    if is_primary {
        let list = if subs.is_empty() {
            ui::muted("(none)")
        } else {
            subs.iter()
                .map(|s| style(*s).cyan().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        ui::kv("delegates to", &list);
        ui::kv(
            "command",
            &format!("{} → {}", style("/multi").cyan(), a.name),
        );
    }
    if is_sub {
        match project.agents.iter().find(|x| x.mode == "primary") {
            Some(p) => ui::kv("called by", &style(&p.name).magenta().to_string()),
            None => ui::kv("called by", &ui::muted("(no primary agent defined)")),
        }
    }
    println!(
        "\n  {} {}",
        style("✦").cyan(),
        ui::muted(&format!("edit with `ocgen edit agent {}`", a.name))
    );
}

/// Compute the permission block as written to the agent file (primary agents get
/// a generated `task:` block listing the subagents they may call).
fn effective_permissions(project: &Project, agent: &Agent) -> String {
    let mut perms = agent.permissions.trim_end().to_string();
    if agent.mode == "primary" {
        perms.push_str("\n  task:\n    \"*\": deny");
        for sub in project.agents.iter().filter(|x| x.mode == "subagent") {
            perms.push_str(&format!("\n    \"{}\": allow", sub.name));
        }
    }
    perms
}

/// Print an indented, dimmed multi-line block.
fn print_block(text: &str) {
    for line in text.lines() {
        println!("  {}", ui::muted(line));
    }
}

/// `ocgen landscape` for a Claude Code project.
fn run_claude(root: &Path, project: &Project) -> Result<()> {
    println!();
    println!(
        "  {} {}  {}",
        style("◆").cyan().bold(),
        style(&project.project_name).bold(),
        ui::muted(&format!("(Claude Code · {})", project.language))
    );
    ui::kv("path", &ui::muted(&root.display().to_string()));

    ui::section(&format!("Agents ({})", project.agents.len()));
    let rows: Vec<Vec<String>> = project
        .agents
        .iter()
        .map(|a| {
            let mode = if a.mode == "primary" {
                style("coordinator").magenta().bold().to_string()
            } else {
                ui::muted("subagent")
            };
            let model = if a.mode == "primary" {
                ui::muted("—")
            } else if a.model.trim().is_empty() {
                "opus".to_string()
            } else {
                a.model.trim().to_string()
            };
            let tools = if a.tools.trim().is_empty() {
                ui::muted("(all)")
            } else {
                ui::truncate(a.tools.trim(), 30)
            };
            vec![
                style(&a.name).bold().to_string(),
                mode,
                model,
                tools,
                color_cell(&a.color),
                ui::muted(&ui::truncate(&a.description, 34)),
            ]
        })
        .collect();
    ui::table(
        &["NAME", "MODE", "MODEL", "TOOLS", "COLOR", "DESCRIPTION"],
        &rows,
    );

    if !project.skills.is_empty() {
        ui::section(&format!("Skills ({})", project.skills.len()));
        let srows: Vec<Vec<String>> = project
            .skills
            .iter()
            .map(|s| {
                vec![
                    style(&s.name).bold().to_string(),
                    ui::muted(&ui::truncate(&s.description, 44)),
                ]
            })
            .collect();
        ui::table(&["NAME", "DESCRIPTION"], &srows);
    }

    ui::section("Setup");
    let mut wf = Vec::new();
    if project.claude.workflow.intake {
        wf.push("/intake");
    }
    if project.claude.workflow.refine {
        wf.push("/refine");
    }
    ui::kv(
        "workflow",
        &if wf.is_empty() {
            ui::muted("(none)")
        } else {
            wf.join(", ")
        },
    );
    let mut out = Vec::new();
    if project.claude.output.project {
        out.push("project (.claude/)");
    }
    if project.claude.output.plugin {
        out.push("plugin");
    }
    ui::kv("output", &out.join(" + "));
    ui::kv(
        "default model",
        &if project.claude.model.trim().is_empty() {
            "opus".to_string()
        } else {
            project.claude.model.clone()
        },
    );
    if project.claude.team.enabled {
        let mode = if project.claude.team.mode.trim().is_empty() {
            "in-process"
        } else {
            project.claude.team.mode.trim()
        };
        let mut tags = vec![mode.to_string()];
        if project.claude.team.plan_gate {
            tags.push("plan-gate".to_string());
        }
        if project.claude.team.confidence_threshold > 0 {
            tags.push(format!("≥{}%", project.claude.team.confidence_threshold));
        }
        if project.claude.team.risk_rounds {
            tags.push("risk rounds".to_string());
        }
        if project.claude.team.approval_gate {
            tags.push("approval-gate".to_string());
        }
        ui::kv("agent teams", &format!("enabled ({})", tags.join(", ")));
    }

    let subs: Vec<&str> = project
        .agents
        .iter()
        .filter(|a| a.mode != "primary")
        .map(|a| a.name.as_str())
        .collect();
    if let Some(p) = project.agents.iter().find(|a| a.mode == "primary") {
        ui::section("Topology");
        println!(
            "  {} {}",
            style(&p.name).magenta().bold(),
            ui::muted("(coordinator → CLAUDE.md)")
        );
        if subs.is_empty() {
            println!("  {}", ui::muted("╰─ (no subagents)"));
        } else {
            let list = subs
                .iter()
                .map(|s| style(*s).cyan().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("  {} delegates to: {}", ui::muted("╰─"), list);
        }
        println!("  {} {}", ui::muted("command:"), style("/multi").cyan());
    }

    let warnings = project.issues();
    if warnings.is_empty() {
        ui::section("Checks");
        ui::success("no problems found");
    } else {
        ui::section(&format!("Checks ({})", warnings.len()));
        for w in &warnings {
            ui::warning(w);
        }
    }
    ui::tip("update with `ocgen edit agent <name>`, `ocgen add skill`, or `ocgen doctor`");
    Ok(())
}

/// `ocgen show agent` for a Claude Code project.
fn print_agent_claude(project: &Project, root: &Path, idx: usize) {
    let a = &project.agents[idx];
    let dash = |s: &str| {
        if s.trim().is_empty() {
            ui::muted("—")
        } else {
            s.to_string()
        }
    };
    println!();
    println!(
        "  {} {}  {}",
        style("◆").cyan().bold(),
        style(&a.name).bold(),
        ui::muted(&format!("(in {})", project.project_name))
    );
    let file = if a.mode == "primary" {
        root.join("CLAUDE.md")
    } else {
        root.join(format!(".claude/agents/{}.md", a.name))
    };
    ui::kv("file", &ui::muted(&file.display().to_string()));

    ui::section("Configuration");
    ui::kv("role", &dash(&a.role));
    ui::kv(
        "mode",
        &if a.mode == "primary" {
            style("coordinator").magenta().bold().to_string()
        } else {
            "subagent".to_string()
        },
    );
    if a.mode == "primary" {
        ui::kv(
            "model",
            &ui::muted("— (coordinator runs as the main session)"),
        );
    } else {
        let m = if a.model.trim().is_empty() {
            "opus"
        } else {
            a.model.trim()
        };
        ui::kv("model", m);
    }
    ui::kv(
        "tools",
        &if a.tools.trim().is_empty() {
            ui::muted("(inherit all)")
        } else {
            a.tools.trim().to_string()
        },
    );
    ui::kv("color", &color_cell(&a.color));
    ui::kv("description", &dash(&a.description));

    ui::section(if a.mode == "primary" {
        "Coordinator instructions (CLAUDE.md)"
    } else {
        "System prompt"
    });
    print_block(if a.body.trim().is_empty() {
        "(none)"
    } else {
        a.body.trim_end()
    });

    ui::section("Relationships");
    let subs: Vec<&str> = project
        .agents
        .iter()
        .filter(|x| x.mode != "primary")
        .map(|x| x.name.as_str())
        .collect();
    if a.mode == "primary" {
        let list = if subs.is_empty() {
            ui::muted("(none)")
        } else {
            subs.iter()
                .map(|s| style(*s).cyan().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        ui::kv("delegates to", &list);
        ui::kv(
            "command",
            &format!("{} → coordinator", style("/multi").cyan()),
        );
    } else {
        match project.agents.iter().find(|x| x.mode == "primary") {
            Some(p) => ui::kv("coordinated by", &style(&p.name).magenta().to_string()),
            None => ui::kv("coordinated by", &ui::muted("(no coordinator)")),
        }
    }
    println!(
        "\n  {} {}",
        style("✦").cyan(),
        ui::muted(&format!("edit with `ocgen edit agent {}`", a.name))
    );
}
