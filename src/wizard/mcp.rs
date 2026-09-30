//! `ocgen add mcp` / `ocgen edit mcp`: project MCP servers in `.mcp.json`, and
//! which subagents may use them.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use console::style;
use dialoguer::theme::ColorfulTheme;

use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::validate::{self, unique_ident};

use super::report_written;
use crate::prompt::{ask, ask_confirm, ask_multi, ask_select, ask_v};
use crate::ui;

/// `ocgen add mcp` — add a project MCP server (`.mcp.json`) and choose which
/// subagents may use it.
pub fn run_add_mcp(path_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let start = path_arg.unwrap_or_else(|| ".".to_string());
    let (target, mut project) = Project::discover(Path::new(&start))?;
    if project.target != Target::ClaudeCode {
        bail!("MCP servers are a Claude Code feature — open a Claude project (`ocgen new --target claude`)");
    }
    println!(
        "Adding an MCP server to {}.\n",
        style(&project.project_name).bold()
    );
    let taken: Vec<String> = project
        .claude
        .mcp_servers
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let server = configure_mcp(&theme, ocgen::claude::McpServer::default(), &taken)?;
    assign_mcp_to_agents(&theme, &mut project, &server.name)?;
    project.claude.mcp_servers.push(server);
    let written = project.scaffold(&target, true)?;
    report_written(&written);
    mcp_next_steps();
    Ok(())
}

/// `ocgen edit mcp` — edit or remove a project MCP server.
pub fn run_edit_mcp(path: String, name_arg: Option<String>) -> Result<()> {
    let theme = ColorfulTheme::default();
    let (target, mut project) = Project::discover(Path::new(&path))?;
    if project.target != Target::ClaudeCode {
        bail!("MCP servers are a Claude Code feature");
    }
    if project.claude.mcp_servers.is_empty() {
        bail!("this project has no MCP servers (add one with `ocgen add mcp`)");
    }
    let names: Vec<String> = project
        .claude
        .mcp_servers
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let idx = match name_arg {
        Some(n) => names
            .iter()
            .position(|x| *x == n)
            .ok_or_else(|| anyhow!("no MCP server named '{n}' (have: {})", names.join(", ")))?,
        None => {
            let labels: Vec<String> = project
                .claude
                .mcp_servers
                .iter()
                .map(|s| format!("{} — {} {}", s.name, s.transport, s.target()))
                .collect();
            ask_select(&theme, "Which MCP server?", "", &labels, 0)?
        }
    };
    let old = names[idx].clone();
    let actions = vec!["Edit it".to_string(), "Remove it".to_string()];
    if ask_select(&theme, "Action", "", &actions, 0)? == 1 {
        project.claude.mcp_servers.remove(idx);
        for a in &mut project.agents {
            let kept: Vec<String> = ocgen::claude::split_list(&a.mcp_servers)
                .into_iter()
                .filter(|s| *s != old)
                .collect();
            a.mcp_servers = kept.join(", ");
        }
        println!("Removed MCP server {old} (and from every agent that used it).");
    } else {
        let taken: Vec<String> = names.iter().filter(|n| **n != old).cloned().collect();
        let edited = configure_mcp(&theme, project.claude.mcp_servers[idx].clone(), &taken)?;
        let new = edited.name.clone();
        project.claude.mcp_servers[idx] = edited;
        if new != old {
            for a in &mut project.agents {
                let renamed: Vec<String> = ocgen::claude::split_list(&a.mcp_servers)
                    .into_iter()
                    .map(|s| if s == old { new.clone() } else { s })
                    .collect();
                a.mcp_servers = renamed.join(", ");
            }
        }
        assign_mcp_to_agents(&theme, &mut project, &new)?;
    }
    let written = project.scaffold(&target, true)?;
    report_written(&written);
    Ok(())
}

fn configure_mcp(
    theme: &ColorfulTheme,
    mut s: ocgen::claude::McpServer,
    taken: &[String],
) -> Result<ocgen::claude::McpServer> {
    // Decide "new" before the name is filled in below.
    let is_new = s.name.is_empty();
    s.name = ask_v(
        theme,
        "  Server name",
        "Becomes mcp__<name> in tool names; letters, digits, '-' or '_'.",
        if s.name.is_empty() {
            None
        } else {
            Some(s.name.as_str())
        },
        unique_ident(taken.to_vec(), "MCP server name"),
    )?;
    let transports = vec![
        "stdio — a local program Claude starts".to_string(),
        "http — a remote server (streamable HTTP)".to_string(),
        "sse — a remote server (legacy SSE)".to_string(),
    ];
    let cur = ["stdio", "http", "sse"]
        .iter()
        .position(|t| *t == s.transport)
        .unwrap_or(0);
    s.transport = ["stdio", "http", "sse"][ask_select(theme, "  Transport", "", &transports, cur)?]
        .to_string();
    let kv_hint = "NAME=value, comma-separated. Never paste a secret — write ${NAME} and set it in your shell.";
    if s.transport == "stdio" {
        s.command = ask(
            theme,
            "  Command",
            "e.g. npx, uvx, docker, or a path.",
            Some(&s.command),
            false,
        )?;
        let args = ask(
            theme,
            "  Arguments (space-separated; optional)",
            "e.g. -y @modelcontextprotocol/server-filesystem ./docs",
            Some(&s.args.join(" ")),
            true,
        )?;
        s.args = args.split_whitespace().map(String::from).collect();
        s.env = ask_kv(theme, "  Environment (optional)", kv_hint, &s.env)?;
        s.url.clear();
        s.headers.clear();
    } else {
        s.url = ask_v(
            theme,
            "  URL",
            "The server endpoint, e.g. https://mcp.example.com/mcp",
            if s.url.is_empty() {
                None
            } else {
                Some(s.url.as_str())
            },
            validate::url,
        )?;
        s.headers = ask_kv(
            theme,
            "  Headers (optional)",
            "e.g. Authorization=Bearer ${API_TOKEN}. Never paste a secret — use ${NAME}.",
            &s.headers,
        )?;
        s.command.clear();
        s.args.clear();
        s.env.clear();
    }
    s.pre_approve = ask_confirm(
        theme,
        "  Pre-approve it for the team?",
        "Writes enabledMcpjsonServers so teammates aren't prompted (after they trust the folder).",
        if is_new { true } else { s.pre_approve },
    )?;
    let leaks = s.literal_secrets();
    if !leaks.is_empty() {
        ui::warning(&format!(
            "{} look like secrets with literal values — use ${{VAR}} references so they never land in git",
            leaks.join(", ")
        ));
    }
    Ok(s)
}

/// Prompt for a `NAME=value, …` list, re-asking until it parses.
fn ask_kv(
    theme: &ColorfulTheme,
    prompt: &str,
    help: &str,
    current: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, String>> {
    let raw = ask_v(
        theme,
        prompt,
        help,
        Some(&ocgen::claude::format_kv_list(current)),
        |v: &str| ocgen::claude::parse_kv_list(v).map(|_| ()),
    )?;
    Ok(ocgen::claude::parse_kv_list(&raw).unwrap_or_default())
}

/// Multi-select which subagents may use `server`, updating their `mcp_servers`.
fn assign_mcp_to_agents(theme: &ColorfulTheme, project: &mut Project, server: &str) -> Result<()> {
    let subs: Vec<usize> = (0..project.agents.len())
        .filter(|&i| project.agents[i].mode != "primary")
        .collect();
    if subs.is_empty() {
        return Ok(());
    }
    let labels: Vec<String> = subs
        .iter()
        .map(|&i| project.agents[i].name.clone())
        .collect();
    let defaults: Vec<bool> = subs
        .iter()
        .map(|&i| {
            ocgen::claude::split_list(&project.agents[i].mcp_servers)
                .iter()
                .any(|s| s == server)
        })
        .collect();
    let chosen = ask_multi(
        theme,
        "  Subagents that may use it",
        "The main session always sees project MCP servers; pick which subagents get them too.",
        &labels.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
        &defaults,
    )?;
    for (pos, &i) in subs.iter().enumerate() {
        let mut list = ocgen::claude::split_list(&project.agents[i].mcp_servers);
        list.retain(|s| s != server);
        if chosen.contains(&pos) {
            list.push(server.to_string());
        }
        project.agents[i].mcp_servers = list.join(", ");
    }
    Ok(())
}

fn mcp_next_steps() {
    ui::tip("set any ${VAR} the server needs in your shell, then start Claude Code — /mcp shows its status");
    ui::tip("a subagent with a tools allow-list gets mcp__<server> added automatically");
}
