//! Manual inspection helper: scaffold a sample project without the wizard.
//! Usage: cargo run --example demo -- <target-dir> [default|custom|claude] [language]

use ocgen::agent::{self, Agent};
use ocgen::manifest::{Manifest, Model, Provider};
use ocgen::render::Project;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let target = args.next().expect("target dir required");
    let roster = args.next().unwrap_or_else(|| "default".into());
    let language = args.next().unwrap_or_else(|| "Ukrainian".into());

    let manifest = Manifest::load()?;

    // Claude Code target: render a full .claude/ project non-interactively.
    if roster == "claude-intent" {
        let mut cp = ocgen::preset::Preset::load("intent")?.build(&language)?;
        cp.project_name = "demo-intent".into();
        let written = cp.scaffold(std::path::Path::new(&target), true)?;
        println!("Wrote {} Claude files to {target}", written.len());
        for p in written {
            println!("  {}", p.display());
        }
        return Ok(());
    }
    if roster == "claude" || roster == "claude-plugin" || roster == "claude-team" {
        let mut cp = Project::from_manifest(&manifest, &language);
        cp.target = ocgen::target::Target::ClaudeCode;
        cp.project_name = "demo-claude".into();
        cp.providers.clear();
        if roster == "claude-plugin" {
            cp.claude.output = ocgen::claude::Output {
                project: true,
                plugin: true,
            };
            cp.claude.plugin.repo_owner = "demo-owner".into();
            cp.claude.plugin.repo_name = "demo-claude".into();
            cp.claude.plugin.version = "0.1.0".into();
            cp.claude.plugin.display_name = "Demo Claude".into();
        }
        if roster == "claude-team" {
            cp.claude.team = ocgen::claude::Team {
                enabled: true,
                mode: "in-process".into(),
                hooks: true,
            };
        }
        cp.claude.instructions =
            "Conventions:\n- Write tests first.\n- Keep changes small and focused.\n".into();
        cp.agents = agent::claude_default_pipeline(&language)?;
        // A preset-derived skill, to exercise the richer skill authoring.
        if let Some(cmd) = ocgen::claude::skill_presets()?
            .into_iter()
            .find(|p| p.name == "command")
        {
            cp.skills.push(cmd.to_skill("commit", &language));
        }
        let written = cp.scaffold(std::path::Path::new(&target), true)?;
        println!("Wrote {} Claude files to {target}", written.len());
        for p in written {
            println!("  {}", p.display());
        }
        return Ok(());
    }

    let mut project = Project::from_manifest(&manifest, &language);
    project.project_name = "demo-project".into();

    if roster == "custom" {
        // Two providers; agents split across them, including a dynamic custom role.
        project.providers = vec![
            manifest.providers[0].clone(),
            Provider {
                key: "cloud".into(),
                name: "Cloud API".into(),
                npm: "@ai-sdk/openai".into(),
                base_url: "https://api.example.com/v1".into(),
                models: vec![Model {
                    id: "gpt-omni".into(),
                    name: "GPT Omni".into(),
                }],
            },
        ];

        let mut lead = Agent::from_archetype("lead", "orchestrator", &language, "mac")?;
        lead.provider = "cloud".into();
        lead.model = "gpt-omni".into();

        let mut translator = Agent::blank("translator", "translator", "cloud");
        translator.model = "gpt-omni".into();
        translator.description = "Translates the text to another language".into();
        translator.temperature = "0.4".into();
        translator.body = "You are a translator. Preserve tone and meaning.".into();
        translator.permissions = "  edit: allow\n  bash:\n    \"*\": deny".into();

        project.agents = vec![
            lead,
            Agent::from_archetype("proofer", "corrector", &language, "mac")?,
            translator,
        ];
    } else {
        project.agents = agent::default_pipeline(&language, "mac")?;
    }

    let written = project.scaffold(std::path::Path::new(&target), true)?;
    println!("Wrote {} files to {target}", written.len());
    for p in written {
        println!("  {}", p.display());
    }
    Ok(())
}
