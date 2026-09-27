mod cli;
mod overview;
mod reference;
mod ui;
mod wizard;

use anyhow::Result;
use clap::Parser;

use cli::{AddWhat, Cli, Command, EditWhat, OutputArg, ShowWhat, TargetArg, TemplatesAction};
use ocgen::templates;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::New {
        path: None,
        target: TargetArg::Opencode,
        base_url: None,
        output: OutputArg::Project,
        repo: None,
    }) {
        Command::New {
            path,
            target,
            base_url,
            output,
            repo,
        } => wizard::run_new(path, target, base_url, output, repo)?,
        Command::Add { what } => match what {
            AddWhat::Agent { path } => wizard::run_add_agent(path)?,
            AddWhat::Provider { path } => wizard::run_add_provider(path)?,
            AddWhat::Skill { path } => wizard::run_add_skill(path)?,
        },
        Command::Edit { what } => match what {
            EditWhat::Agent { name, path } => wizard::run_edit_agent(path, name)?,
            EditWhat::Provider { key, path } => wizard::run_edit_provider(path, key)?,
            EditWhat::Skill { name, path } => wizard::run_edit_skill(path, name)?,
        },
        Command::Show {
            what: ShowWhat::Agent { name, path },
        } => overview::show_agent(path, name)?,
        Command::Landscape { path } => overview::run(path.unwrap_or_else(|| ".".to_string()))?,
        Command::Doctor { path } => wizard::run_doctor(path.unwrap_or_else(|| ".".to_string()))?,
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
