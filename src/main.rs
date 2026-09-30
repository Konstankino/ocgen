mod cli;
mod overview;
mod prompt;
mod reference;
mod ui;
mod wizard;

use anyhow::Result;
use clap::Parser;

use cli::{AddWhat, Cli, Command, EditWhat, OutputArg, ShowWhat, TeamCli, TemplatesAction};
use ocgen::templates;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::New {
        path: None,
        target: None,
        base_url: None,
        output: OutputArg::Project,
        repo: None,
        team: false,
        team_confidence: None,
        no_plan_gate: false,
        no_risk_rounds: false,
        no_approval_gate: false,
    }) {
        Command::New {
            path,
            target,
            base_url,
            output,
            repo,
            team,
            team_confidence,
            no_plan_gate,
            no_risk_rounds,
            no_approval_gate,
        } => {
            let team = TeamCli {
                enabled: team,
                confidence: team_confidence,
                plan_gate: !no_plan_gate,
                risk_rounds: !no_risk_rounds,
                approval_gate: !no_approval_gate,
            };
            wizard::run_new(path, target, base_url, output, repo, team)?
        }
        Command::Add { what } => match what {
            AddWhat::Agent { path } => wizard::run_add_agent(path)?,
            AddWhat::Provider { path } => wizard::run_add_provider(path)?,
            AddWhat::Skill { path } => wizard::run_add_skill(path)?,
            AddWhat::Mcp { path } => wizard::run_add_mcp(path)?,
        },
        Command::Edit { what } => match what {
            EditWhat::Agent { name, path } => wizard::run_edit_agent(path, name)?,
            EditWhat::Provider { key, path } => wizard::run_edit_provider(path, key)?,
            EditWhat::Skill { name, path } => wizard::run_edit_skill(path, name)?,
            EditWhat::Team { path } => wizard::run_edit_team(path)?,
            EditWhat::Mcp { name, path } => wizard::run_edit_mcp(path, name)?,
        },
        Command::Show {
            what: ShowWhat::Agent { name, path },
        } => overview::show_agent(path, name)?,
        Command::Landscape { path } => overview::run(path.unwrap_or_else(|| ".".to_string()))?,
        Command::Doctor { path, dry_run, yes } => {
            wizard::run_doctor(path.unwrap_or_else(|| ".".to_string()), dry_run, yes)?
        }
        Command::Hook { name, check } => {
            if check {
                println!("{}", ocgen::hooks::PROTOCOL);
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!("name a hook: {}", ocgen::hooks::NAMES.join(", "));
            };
            let mut payload = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload)?;
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let out = ocgen::hooks::run(&name, &payload, &env);
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            std::process::exit(out.code);
        }
        Command::Approve {
            path,
            minutes,
            revoke,
            status,
        } => overview::run_approve(
            path.unwrap_or_else(|| ".".to_string()),
            minutes,
            revoke,
            status,
        )?,
        Command::Verify {
            path,
            no_claude,
            run_check,
        } => {
            let ok = overview::run_verify(
                path.unwrap_or_else(|| ".".to_string()),
                !no_claude,
                run_check,
            )?;
            if !ok {
                std::process::exit(1);
            }
        }
        Command::ManagedSettings => {
            print!("{}", ocgen::claude::managed_settings_json());
            eprintln!(
                "# Deploy as managed-settings.json — macOS: /Library/Application Support/ClaudeCode/, \
                 Linux/WSL: /etc/claude-code/, Windows: C:\\Program Files\\ClaudeCode\\. Projects can't override it."
            );
        }
        Command::Fields => reference::run(),
        Command::Templates { action } => match action {
            TemplatesAction::Init => {
                let dir = templates::init_override()?;
                println!("Templates copied to {}", dir.display());
                println!("Edit them there — ocgen prefers these over the built-in defaults.");
            }
            TemplatesAction::Path => match templates::override_dir() {
                Some(d) => println!("{}", d.display()),
                None => println!("(could not determine home directory)"),
            },
            TemplatesAction::List => {
                for (path, overridden) in templates::list() {
                    let tag = if overridden { "override" } else { "embedded" };
                    println!("{path:<40} [{tag}]");
                }
            }
            TemplatesAction::Edit { path } => wizard::run_templates_edit(path)?,
        },
    }
    Ok(())
}
