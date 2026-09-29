//! Integration tests exercise the library's rendering/scaffolding directly.
//! (The wizard is a thin dialoguer layer over this; its logic lives here.)

use std::fs;
use std::path::Path;

use ocgen::agent::{self, Agent};
use ocgen::archetype::Archetype;
use ocgen::claude::{Output, Powerups, Skill, Team, Workflow};
use ocgen::manifest::{Manifest, Model, Provider};
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;
use tempfile::tempdir;

fn base_project(language: &str) -> Project {
    let manifest = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&manifest, language);
    p.project_name = "demo".into();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

#[test]
fn default_pipeline_scaffolds_and_matches_sample() {
    let mut project = base_project("Ukrainian");
    project.agents = agent::default_pipeline("Ukrainian", "mac").unwrap();

    let dir = tempdir().unwrap();
    let written = project.scaffold(dir.path(), false).unwrap();
    // 4 agents + coordinator prompt + opencode.json + multi command = 7.
    assert_eq!(written.len(), 7);

    let json: serde_json::Value = serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert_eq!(json["model"], "mac/qwen3-35b-a3b");
    assert_eq!(json["provider"]["mac"]["name"], "M4 Pro 24GB");
    // Utility agents are now provider-qualified.
    assert_eq!(json["agent"]["compaction"]["model"], "mac/qwen3.5-9b");

    let orch = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(orch.contains("mode: primary"));
    assert!(orch.contains("model: mac/qwen3-35b-a3b"));
    assert!(orch.contains("prompt: \"{file:../prompts/coordinator.txt}\""));
    for sub in ["explorer", "implementer", "reviewer"] {
        assert!(
            orch.contains(&format!("\"{sub}\": allow")),
            "task perm for {sub}"
        );
    }

    let explorer = read(dir.path(), ".opencode/agents/explorer.md");
    assert!(explorer.contains("edit: deny"));
    let implementer = read(dir.path(), ".opencode/agents/implementer.md");
    assert!(implementer.contains("steps: 30"));
    assert!(implementer.contains("color: \"#4ec9b0\""));

    let multi = read(dir.path(), ".opencode/commands/multi.md");
    assert!(multi.contains("agent: coordinator"));
    assert!(multi.contains("@explorer"));

    for (rel, _) in project.render_all().unwrap() {
        let content = read(dir.path(), rel.to_str().unwrap());
        assert!(content.ends_with('\n') && !content.ends_with("\n\n"));
    }
}

#[test]
fn multiple_providers_render_and_agents_pick_one() {
    let mut project = base_project("English");
    project.providers.push(Provider {
        key: "cloud".into(),
        name: "Cloud API".into(),
        npm: "@ai-sdk/openai".into(),
        base_url: "https://api.example.com/v1".into(),
        models: vec![Model {
            id: "gpt-omni".into(),
            name: "GPT Omni".into(),
        }],
    });

    // A coordinator on the cloud provider, a subagent on the local one.
    let mut lead = Agent::from_archetype("lead", "coordinator", "English", "cloud").unwrap();
    lead.model = "gpt-omni".into();
    project.agents = vec![
        lead,
        Agent::from_archetype("grammar", "verifier", "English", "mac").unwrap(),
    ];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    let json: serde_json::Value = serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    // Both providers present; top-level model is the primary's provider/model.
    assert!(json["provider"]["mac"].is_object());
    assert!(json["provider"]["cloud"].is_object());
    assert_eq!(json["model"], "cloud/gpt-omni");

    let lead_md = read(dir.path(), ".opencode/agents/lead.md");
    assert!(lead_md.contains("model: cloud/gpt-omni"));
    let grammar_md = read(dir.path(), ".opencode/agents/grammar.md");
    assert!(grammar_md.contains("model: mac/"));
}

#[test]
fn all_agent_fields_are_configurable() {
    let mut project = base_project("English");
    let mut a = Agent::blank("custom", "myrole", "mac");
    a.mode = "subagent".into();
    a.model = "gemma-26b".into();
    a.temperature = "0.9".into();
    a.steps = Some(7);
    a.color = "#123456".into();
    a.description = "a bespoke agent".into();
    a.permissions = "  edit: deny\n  webfetch: allow".into();
    a.body = "Bespoke instructions.".into();
    project.agents = vec![a];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".opencode/agents/custom.md");

    assert!(md.contains("temperature: 0.9"));
    assert!(md.contains("steps: 7"));
    assert!(md.contains("color: \"#123456\"")); // hex quoted
    assert!(md.contains("description: a bespoke agent"));
    assert!(md.contains("webfetch: allow"));
    assert!(md.contains("Bespoke instructions."));
}

#[test]
fn all_opencode_agent_fields_render() {
    let mut project = base_project("English");
    let mut a = Agent::blank("tuned", "custom", "mac");
    a.mode = "all".into();
    a.model = "qwen3-35b-a3b".into();
    a.variant = "thinking".into();
    a.temperature = "0.1".into();
    a.top_p = "0.9".into();
    a.steps = Some(12);
    a.color = "success".into();
    a.disable = true;
    a.hidden = true;
    a.options = "  reasoningEffort: high".into();
    a.permissions = "  edit: allow\n  websearch: allow".into();
    a.body = "Tuned agent.".into();
    project.agents = vec![a];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".opencode/agents/tuned.md");

    assert!(md.contains("mode: all"));
    assert!(md.contains("model: mac/qwen3-35b-a3b"));
    assert!(md.contains("variant: thinking"));
    assert!(md.contains("temperature: 0.1"));
    assert!(md.contains("top_p: 0.9"));
    assert!(md.contains("steps: 12"));
    assert!(md.contains("color: success"));
    assert!(md.contains("disable: true"));
    assert!(md.contains("hidden: true"));
    assert!(md.contains("options:\n  reasoningEffort: high"));
    assert!(md.contains("websearch: allow"));

    // State round-trips through save + reload with the new fields intact.
    let reloaded = Project::load_state(dir.path()).unwrap();
    let t = &reloaded.agents[0];
    assert_eq!(t.variant, "thinking");
    assert_eq!(t.top_p, "0.9");
    assert!(t.disable && t.hidden);
    assert_eq!(t.options.trim(), "reasoningEffort: high");
}

#[test]
fn omitted_optional_fields_are_absent() {
    // A plain agent must not emit variant/top_p/disable/hidden/options lines.
    let mut project = base_project("English");
    project.agents = vec![Agent::from_archetype("grammar", "verifier", "English", "mac").unwrap()];
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".opencode/agents/grammar.md");
    for absent in ["variant:", "top_p:", "disable:", "hidden:", "options:"] {
        assert!(
            !md.contains(absent),
            "unexpected `{absent}` in default agent"
        );
    }
}

#[test]
fn dynamic_role_without_archetype_scaffolds() {
    // A role name that is not one of the built-in archetypes, built from blank.
    let mut project = base_project("English");
    let mut a = Agent::blank("summarizer", "condenser", "mac");
    a.model = "qwen3.5-9b".into();
    a.body = "Summarize the input in three bullets.".into();
    project.agents = vec![a];

    let dir = tempdir().unwrap();
    // No panic / no archetype lookup at render time.
    project.scaffold(dir.path(), false).unwrap();
    assert!(dir.path().join(".opencode/agents/summarizer.md").exists());
    let md = read(dir.path(), ".opencode/agents/summarizer.md");
    assert!(md.contains("model: mac/qwen3.5-9b"));
    assert!(md.contains("Summarize the input in three bullets."));
}

#[test]
fn custom_named_roster_drives_cross_references() {
    let mut project = base_project("English");
    project.agents = vec![
        {
            let mut lead = Agent::from_archetype("lead", "coordinator", "English", "mac").unwrap();
            lead.description = "coordinator".into();
            lead
        },
        Agent::from_archetype("proofer", "verifier", "English", "mac").unwrap(),
        Agent::from_archetype("critic", "reviewer", "English", "mac").unwrap(),
    ];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    for name in ["lead", "proofer", "critic"] {
        assert!(dir
            .path()
            .join(format!(".opencode/agents/{name}.md"))
            .exists());
    }
    assert!(!dir.path().join(".opencode/agents/orchestrator.md").exists());

    let lead = read(dir.path(), ".opencode/agents/lead.md");
    assert!(lead.contains("\"proofer\": allow"));
    assert!(lead.contains("\"critic\": allow"));
    assert!(!lead.contains("\"lead\": allow"));

    let prompt = read(dir.path(), ".opencode/prompts/lead.txt");
    assert!(prompt.contains("proofer") && prompt.contains("critic"));

    let multi = read(dir.path(), ".opencode/commands/multi.md");
    assert!(multi.contains("agent: lead"));
    assert!(multi.contains("@proofer") && multi.contains("@critic"));
    assert!(multi.contains("Several models in parallel"));
}

#[test]
fn add_agent_reloads_state_and_updates_references() {
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    let mut reloaded = Project::load_state(dir.path()).unwrap();
    assert_eq!(reloaded.agents.len(), 4);
    assert_eq!(reloaded.providers.len(), 1);
    reloaded
        .agents
        .push(Agent::from_archetype("factcheck", "reviewer", "English", "mac").unwrap());
    reloaded.scaffold(dir.path(), true).unwrap();

    assert!(dir.path().join(".opencode/agents/factcheck.md").exists());
    let orch = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(orch.contains("\"factcheck\": allow"));
    let prompt = read(dir.path(), ".opencode/prompts/coordinator.txt");
    assert!(prompt.contains("factcheck"));
}

#[test]
fn edit_agent_field_and_rename_updates_and_cleans_up() {
    // Simulates `ocgen edit agent`: reload state, mutate fields, re-render, and
    // remove the stale files when an agent is renamed.
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    let mut reloaded = Project::load_state(dir.path()).unwrap();
    // Tweak a field on an in-use agent.
    reloaded
        .agents
        .iter_mut()
        .find(|a| a.name == "implementer")
        .unwrap()
        .temperature = "0.5".into();
    // Rename another agent.
    reloaded
        .agents
        .iter_mut()
        .find(|a| a.name == "reviewer")
        .unwrap()
        .name = "proof".into();

    reloaded.scaffold(dir.path(), true).unwrap();
    Project::remove_agent_artifacts(dir.path(), "reviewer").unwrap();

    // Field change took effect.
    let editor = read(dir.path(), ".opencode/agents/implementer.md");
    assert!(editor.contains("temperature: 0.5"));

    // Rename: new file exists, old removed, references follow.
    assert!(dir.path().join(".opencode/agents/proof.md").exists());
    assert!(!dir.path().join(".opencode/agents/reviewer.md").exists());
    let orch = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(orch.contains("\"proof\": allow"));
    assert!(!orch.contains("\"reviewer\": allow"));
    let prompt = read(dir.path(), ".opencode/prompts/coordinator.txt");
    // The subagent is listed by its new name (description text may still mention "reviewer").
    assert!(prompt.contains("- proof —") && !prompt.contains("- reviewer —"));
}

#[test]
fn add_provider_then_rename_key_propagates_to_agents() {
    // Simulates `ocgen add provider` + `ocgen edit provider` (key rename).
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    // Add a second provider and point one agent at it.
    let mut reloaded = Project::load_state(dir.path()).unwrap();
    reloaded.providers.push(Provider {
        key: "cloud".into(),
        name: "Cloud API".into(),
        npm: "@ai-sdk/openai".into(),
        base_url: "https://api.example.com/v1".into(),
        models: vec![Model {
            id: "gpt-omni".into(),
            name: "GPT Omni".into(),
        }],
    });
    let ed = reloaded
        .agents
        .iter_mut()
        .find(|a| a.name == "implementer")
        .unwrap();
    ed.provider = "cloud".into();
    ed.model = "gpt-omni".into();
    reloaded.scaffold(dir.path(), true).unwrap();

    let json: serde_json::Value = serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert!(json["provider"]["cloud"].is_object());
    assert!(read(dir.path(), ".opencode/agents/implementer.md").contains("model: cloud/gpt-omni"));

    // Rename provider key cloud -> vllm; propagate to the agent that used it.
    let mut reloaded2 = Project::load_state(dir.path()).unwrap();
    let old_key = "cloud";
    let new_key = "vllm";
    reloaded2
        .providers
        .iter_mut()
        .find(|p| p.key == old_key)
        .unwrap()
        .key = new_key.into();
    for a in &mut reloaded2.agents {
        if a.provider == old_key {
            a.provider = new_key.into();
        }
    }
    reloaded2.scaffold(dir.path(), true).unwrap();

    let json2: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert!(json2["provider"]["vllm"].is_object());
    assert!(json2["provider"]["cloud"].is_null());
    assert!(read(dir.path(), ".opencode/agents/implementer.md").contains("model: vllm/gpt-omni"));
}

#[test]
fn discover_finds_project_from_a_subdirectory() {
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    // Discovering from a nested dir inside the project resolves to the root.
    let nested = dir.path().join(".opencode").join("agents");
    let (root, loaded) = Project::discover(&nested).unwrap();
    assert_eq!(
        root.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
    assert_eq!(loaded.agents.len(), 4);

    // A directory with no project (and no project ancestor) errors clearly.
    let empty = tempdir().unwrap();
    let err = Project::discover(empty.path()).unwrap_err();
    assert!(err.to_string().contains("no ocgen project found"));
}

fn write_state(dir: &Path, json: &str) {
    fs::create_dir_all(dir.join(".opencode")).unwrap();
    fs::write(dir.join(".opencode/.ocgen-state.json"), json).unwrap();
}

#[test]
fn loads_legacy_single_provider_state() {
    // Old schema: inline provider fields + agents with `archetype` and no role/provider.
    let dir = tempdir().unwrap();
    write_state(
        dir.path(),
        r#"{
          "project_name": "old",
          "language": "English",
          "provider_key": "mac",
          "provider_name": "M4",
          "npm": "@ai-sdk/openai-compatible",
          "base_url": "http://mac.home:8080/v1",
          "models": [{"id":"qwen3-35b-a3b","name":"Q"},{"id":"qwen3.5-9b","name":"Q9"}],
          "utility_model": "qwen3.5-9b",
          "agents": [
            {"name":"coordinator","archetype":"coordinator","model":"qwen3-35b-a3b","mode":"primary","description":"boss"},
            {"name":"grammar","archetype":"verifier","model":"qwen3.5-9b","mode":"subagent","description":"fix"}
          ]
        }"#,
    );

    let project = Project::load_state(dir.path()).unwrap();
    assert_eq!(project.providers.len(), 1);
    assert_eq!(project.providers[0].key, "mac");
    assert_eq!(project.utility_provider, "mac");

    let orch = project
        .agents
        .iter()
        .find(|a| a.name == "coordinator")
        .unwrap();
    assert_eq!(orch.provider, "mac"); // backfilled
    assert_eq!(orch.role, "coordinator"); // from archetype
    assert!(!orch.permissions.is_empty()); // from archetype
    assert!(orch.prompt_file); // coordinator archetype uses a prompt file

    // And it re-renders into a valid current-schema project.
    project.scaffold(dir.path(), true).unwrap();
    let json: serde_json::Value = serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert_eq!(json["model"], "mac/qwen3-35b-a3b");
    assert!(
        read(dir.path(), ".opencode/agents/coordinator.md").contains("model: mac/qwen3-35b-a3b")
    );
}

#[test]
fn loads_state_with_agent_missing_only_role() {
    // The reported case: providers present, but an agent lacks the newer `role` field.
    let dir = tempdir().unwrap();
    write_state(
        dir.path(),
        r#"{
          "project_name": "p",
          "language": "English",
          "providers": [{"key":"mac","name":"M4","npm":"x","base_url":"u","models":[{"id":"m1","name":"M1"}]}],
          "utility_provider": "mac",
          "utility_model": "m1",
          "agents": [
            {"name":"a1","mode":"subagent","provider":"mac","model":"m1","temperature":"0.2","color":"accent","description":"d","permissions":"  edit: ask","body":"b","prompt_file":false}
          ]
        }"#,
    );

    let project = Project::load_state(dir.path()).unwrap();
    assert_eq!(project.agents.len(), 1);
    assert_eq!(project.agents[0].role, "custom"); // defaulted, no archetype present
    assert_eq!(project.agents[0].provider, "mac");
}

#[test]
fn doctor_repairs_broken_references() {
    let mut project = base_project("English");
    let mut a = agent::default_pipeline("English", "mac").unwrap();
    // Break one agent: unknown provider; break another: model not in provider.
    a[1].provider = "ghost".into();
    a[2].model = "nonexistent".into();
    project.agents = a;

    let fixes = project.doctor();
    assert!(fixes.iter().any(|f| f.contains("unknown provider 'ghost'")));
    assert!(fixes.iter().any(|f| f.contains("not in 'mac'")));
    // After doctor, everything references a real provider/model.
    for ag in &project.agents {
        let prov = project
            .providers
            .iter()
            .find(|p| p.key == ag.provider)
            .unwrap();
        assert!(prov.models.iter().any(|m| m.id == ag.model));
    }

    // A healthy project yields no fixes.
    let mut healthy = base_project("English");
    healthy.agents = agent::default_pipeline("English", "mac").unwrap();
    assert!(healthy.doctor().is_empty());
}

#[test]
fn scaffold_refuses_to_overwrite_without_force() {
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    let err = project.scaffold(dir.path(), false).unwrap_err();
    assert!(err.to_string().contains("already exists"));
    assert!(project.scaffold(dir.path(), true).is_ok());
}

#[test]
fn templates_embedded_access() {
    assert!(templates::load("manifest.toml").is_ok());
    assert!(templates::load("archetypes/reviewer.toml").is_ok());
    assert!(templates::load("does/not/exist").is_err());

    let names = templates::archetype_names();
    for n in [
        "coordinator",
        "explorer",
        "implementer",
        "reviewer",
        "verifier",
    ] {
        assert!(names.contains(&n.to_string()), "missing archetype {n}");
    }

    let list = templates::list();
    assert!(list.iter().any(|(p, _)| p == "manifest.toml"));
    // The `overridden` flag must match what is actually on disk — robust whether or
    // not the developer running the tests has an override dir of their own.
    let dir = templates::override_dir();
    for (p, overridden) in &list {
        let on_disk = dir.as_ref().map(|d| d.join(p).is_file()).unwrap_or(false);
        assert_eq!(*overridden, on_disk, "override flag wrong for {p}");
    }
    assert!(templates::override_dir().is_some());
}

#[test]
fn manifest_and_archetype_helpers() {
    let m = Manifest::load().unwrap();
    assert!(!m.providers.is_empty());
    let prov = &m.providers[0];
    assert_eq!(prov.model_index("qwen3.5-9b"), 0);
    assert_eq!(prov.model_index("does-not-exist"), 0); // fallback to 0

    let orch = Archetype::load("coordinator").unwrap();
    assert_eq!(orch.mode, "primary");
    assert!(orch.prompt_file);
    assert!(orch.prompt_for("English").is_some());
    // Unknown language falls back to English rather than empty.
    assert!(!orch.description_for("Klingon").is_empty());

    let reviewer = Archetype::load("reviewer").unwrap();
    assert!(reviewer.prompt_for("English").is_none()); // no external prompt
    assert!(Archetype::load("nonexistent-archetype").is_err());
}

#[test]
fn project_issues_detects_problems() {
    let mut p = base_project("English");
    p.agents = agent::default_pipeline("English", "mac").unwrap();
    assert!(p.issues().is_empty()); // healthy

    p.agents[1].provider = "ghost".into();
    p.agents[2].mode = "primary".into(); // now two primaries
    p.utility_provider = "nope".into();
    let issues = p.issues();
    assert!(issues
        .iter()
        .any(|s| s.contains("unknown provider 'ghost'")));
    assert!(issues.iter().any(|s| s.contains("primary agents")));
    assert!(issues.iter().any(|s| s.contains("utility provider 'nope'")));

    p.agents.clear();
    assert!(p.issues().iter().any(|s| s.contains("no primary")));
}

#[test]
fn render_without_primary_skips_multi_and_prompt() {
    let mut p = base_project("English");
    let mut a = Agent::blank("solo", "custom", "mac");
    a.model = "qwen3.5-9b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    assert!(dir.path().join(".opencode/agents/solo.md").exists());
    assert!(!dir.path().join(".opencode/commands/multi.md").exists());
    assert!(!dir.path().join(".opencode/prompts/solo.txt").exists());

    // Top-level model falls back to utility_provider/default_primary_model.
    let json: serde_json::Value = serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert!(json["model"].as_str().unwrap().starts_with("mac/"));
}

#[test]
fn ukrainian_language_variant_renders() {
    let mut p = base_project("Ukrainian");
    p.agents = agent::default_pipeline("Ukrainian", "mac").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let multi = read(dir.path(), ".opencode/commands/multi.md");
    assert!(multi.contains("Запусти")); // Ukrainian command body
}

#[test]
fn doctor_fills_empty_fields_and_fixes_prompt_file() {
    let mut p = base_project("English");
    let mut a = Agent::blank("x", "custom", "mac");
    a.mode = "weird".into(); // invalid
    a.model = "qwen3.5-9b".into();
    a.temperature = String::new();
    a.color = String::new();
    a.permissions = String::new();
    a.prompt_file = true; // but no body → should be disabled
    a.prompt_body = None;
    p.agents = vec![a];

    let fixes = p.doctor();
    assert!(fixes.iter().any(|f| f.contains("invalid mode")));
    assert!(fixes.iter().any(|f| f.contains("empty temperature")));
    assert!(fixes.iter().any(|f| f.contains("empty color")));
    assert!(fixes.iter().any(|f| f.contains("empty permissions")));
    assert!(fixes.iter().any(|f| f.contains("prompt file")));
    assert_eq!(p.agents[0].mode, "subagent");
    assert!(!p.agents[0].prompt_file);

    let mut p2 = base_project("English");
    p2.agents = agent::default_pipeline("English", "mac").unwrap();
    p2.utility_model = "bogus".into();
    assert!(p2.doctor().iter().any(|f| f.contains("utility model")));
}

#[test]
fn discover_errors_on_nonexistent_path() {
    let err = Project::discover(Path::new("/no/such/ocgen/path/xyz")).unwrap_err();
    assert!(err.to_string().contains("no ocgen project"));
}

// ---- externalized seeds & data-driven pipeline ----

#[test]
fn seeds_load_and_pick_by_language_with_english_fallback() {
    let seeds = ocgen::seeds::Seeds::load().expect("seeds.toml loads");

    // Body seed: English is the known string, Ukrainian differs, unknown falls back.
    assert_eq!(
        seeds.body_for("English"),
        "You are an agent. Describe your task here."
    );
    assert!(!seeds.body_for("Ukrainian").is_empty());
    assert_ne!(seeds.body_for("Ukrainian"), seeds.body_for("English"));
    assert_eq!(seeds.body_for("Klingon"), seeds.body_for("English"));

    // Prompt seed: non-empty per language, unknown falls back, keeps the Jinja loop.
    assert!(seeds
        .prompt_for("English")
        .contains("{% for sub in subagents %}"));
    assert!(!seeds.prompt_for("Ukrainian").is_empty());
    assert_ne!(seeds.prompt_for("Ukrainian"), seeds.prompt_for("English"));
    assert_eq!(seeds.prompt_for("Klingon"), seeds.prompt_for("English"));
}

#[test]
fn manifest_mac_provider_matches_server_models() {
    // Assert the SHIPPED template, not whatever override the dev happens to have
    // (Manifest::load() would prefer ~/.config/ocgen/templates/manifest.toml).
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/templates/manifest.toml"
    ))
    .unwrap();
    let m: Manifest = toml::from_str(&src).unwrap();
    let mac = m
        .providers
        .iter()
        .find(|p| p.key == "mac")
        .expect("mac provider");
    let ids: Vec<&str> = mac.models.iter().map(|m| m.id.as_str()).collect();
    // The exact model ids the llama-swap server exposes (qwen3.5-9b stays first so
    // `model_index("qwen3.5-9b") == 0` holds).
    assert_eq!(
        ids,
        vec![
            "qwen3.5-9b",
            "gemma-26b",
            "qwen3-35b-a3b",
            "qwen3-coder-30b",
            "devstral-24b",
            "qwen38-27b",
        ]
    );
}

#[test]
fn provider_with_base_url_overrides_only_the_url() {
    let m = Manifest::load().unwrap();
    let orig = m.providers[0].clone();
    let overridden = orig.clone().with_base_url("http://192.168.88.89:8080/v1");

    assert_eq!(overridden.base_url, "http://192.168.88.89:8080/v1");
    // Everything else is untouched.
    assert_eq!(overridden.key, orig.key);
    assert_eq!(overridden.name, orig.name);
    assert_eq!(overridden.npm, orig.npm);
    assert_eq!(overridden.models.len(), orig.models.len());
}

#[test]
fn manifest_declares_the_default_pipeline() {
    let m = Manifest::load().unwrap();
    let pairs: Vec<(&str, &str)> = m
        .pipeline
        .iter()
        .map(|p| (p.name.as_str(), p.archetype.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("coordinator", "coordinator"),
            ("explorer", "explorer"),
            ("implementer", "implementer"),
            ("reviewer", "reviewer"),
        ]
    );
}

#[test]
fn default_pipeline_is_driven_by_the_manifest() {
    let m = Manifest::load().unwrap();
    let agents = agent::default_pipeline("English", "mac").unwrap();

    // One agent per manifest pipeline entry, in order, with matching names.
    let got_names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    let want_names: Vec<&str> = m.pipeline.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(got_names, want_names);

    // Fields come from the referenced archetype (proves the coupling).
    for (a, entry) in agents.iter().zip(&m.pipeline) {
        let arch = Archetype::load(&entry.archetype).unwrap();
        assert_eq!(a.mode, arch.mode);
        assert_eq!(a.model, arch.default_model);
        assert_eq!(a.provider, "mac");
    }
}

// ---- Claude Code target: state plumbing (phase 1) ----

#[test]
fn target_default_is_opencode_and_serde_roundtrips() {
    assert_eq!(Target::default(), Target::OpenCode);
    assert_eq!(
        serde_json::to_string(&Target::ClaudeCode).unwrap(),
        "\"claude\""
    );
    assert_eq!(
        serde_json::to_string(&Target::OpenCode).unwrap(),
        "\"opencode\""
    );
    let t: Target = serde_json::from_str("\"claude\"").unwrap();
    assert_eq!(t, Target::ClaudeCode);
}

#[test]
fn target_state_file_paths() {
    assert_eq!(Target::OpenCode.state_file(), ".opencode/.ocgen-state.json");
    assert_eq!(Target::ClaudeCode.state_file(), ".claude/.ocgen-state.json");
}

#[test]
fn claude_project_state_roundtrips_and_is_back_compat() {
    // Old-style state (no target/claude/skills/tools) defaults to OpenCode.
    let old = r#"{"project_name":"x","language":"English","providers":[],"agents":[]}"#;
    let p: Project = serde_json::from_str(old).unwrap();
    assert_eq!(p.target, Target::OpenCode);
    assert!(p.skills.is_empty());

    // A Claude project round-trips with its config, agent tools, and skills.
    let mut proj = base_project("English");
    proj.target = Target::ClaudeCode;
    proj.claude.model = "sonnet".into();
    proj.claude.instructions = "Be excellent.".into();
    proj.claude.plugin.repo_owner = "me".into();
    proj.claude.plugin.repo_name = "team".into();
    proj.skills.push(Skill {
        name: "commit".into(),
        description: "make a commit".into(),
        allowed_tools: "Bash(git *)".into(),
        body: "Do it".into(),
        role: None,
        ..Default::default()
    });
    proj.agents = agent::default_pipeline("English", "mac").unwrap();
    proj.agents[0].tools = "Read, Grep, Glob".into();

    let json = serde_json::to_string(&proj).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.target, Target::ClaudeCode);
    assert_eq!(back.claude.model, "sonnet");
    assert_eq!(back.claude.instructions, "Be excellent.");
    assert!(back.claude.output.project); // default preserved
    assert_eq!(back.skills.len(), 1);
    assert_eq!(back.skills[0].name, "commit");
    assert_eq!(back.agents[0].tools, "Read, Grep, Glob");
}

#[test]
fn discover_finds_a_claude_project_by_its_state_file() {
    let dir = tempdir().unwrap();
    let mut proj = base_project("English");
    proj.target = Target::ClaudeCode;
    proj.project_name = "demo".into();
    fs::create_dir_all(dir.path().join(".claude")).unwrap();
    fs::write(
        dir.path().join(".claude/.ocgen-state.json"),
        serde_json::to_string_pretty(&proj).unwrap(),
    )
    .unwrap();

    let (root, loaded) = Project::discover(dir.path()).unwrap();
    assert_eq!(root, dir.path().canonicalize().unwrap());
    assert_eq!(loaded.target, Target::ClaudeCode);
    assert_eq!(loaded.project_name, "demo");
}

// ---- Claude Code target: project renderer (phase 2) ----

#[test]
fn claude_project_renders_agents_command_settings_and_claude_md() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "team".into();
    p.claude.instructions = "House rules: be terse.".into();

    let mut boss = Agent::blank("boss", "orchestrator", "");
    boss.mode = "primary".into();
    boss.body = "You coordinate the team.".into();

    let mut rev = Agent::blank("reviewer", "reviewer", "");
    rev.mode = "subagent".into();
    rev.model = "sonnet".into();
    rev.tools = "Read, Grep, Glob".into();
    rev.color = "cyan".into();
    rev.description = "Reviews code".into();
    rev.body = "You review.".into();

    p.agents = vec![boss, rev];

    let dir = tempdir().unwrap();
    let written = p.scaffold(dir.path(), false).unwrap();
    assert!(written
        .iter()
        .any(|w| w.ends_with(".claude/agents/reviewer.md")));
    assert!(!written
        .iter()
        .any(|w| w.ends_with(".claude/agents/boss.md")));

    let rev_md = read(dir.path(), ".claude/agents/reviewer.md");
    assert!(rev_md.contains("name: reviewer"));
    assert!(rev_md.contains("model: sonnet"));
    assert!(rev_md.contains("tools: Read, Grep, Glob"));
    assert!(rev_md.contains("color: cyan"));
    assert!(rev_md.contains("You review."));

    let multi = read(dir.path(), ".claude/commands/multi.md");
    assert!(multi.contains("@reviewer"));
    assert!(multi.contains("$ARGUMENTS"));

    let claude_md = read(dir.path(), "CLAUDE.md");
    assert!(claude_md.contains("# team"));
    assert!(claude_md.contains("House rules: be terse."));
    // Roster + coordination live in the ocgen team rule now, not CLAUDE.md.
    let team_rule = read(dir.path(), ".claude/rules/ocgen-team.md");
    assert!(team_rule.contains("**reviewer**"));
    assert!(team_rule.contains("You coordinate the team."));

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "opus");
    assert!(settings["permissions"]["allow"].is_array());
    assert_eq!(settings["outputStyle"], "Concise");

    let reloaded = Project::load_state(dir.path()).unwrap();
    assert_eq!(reloaded.target, Target::ClaudeCode);
}

// ---- Claude Code target: workflow, skills, hooks (phase 3) ----

#[test]
fn claude_renders_workflow_commands_skills_and_hooks() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.skills.push(Skill {
        name: "commit".into(),
        description: "Craft a conventional commit".into(),
        allowed_tools: "Bash(git add:*), Bash(git commit:*)".into(),
        body: "Stage and commit with a clear message.".into(),
        role: None,
        ..Default::default()
    });
    let mut a = Agent::blank("worker", "custom", "");
    a.mode = "subagent".into();
    a.model = "haiku".into();
    a.description = "does work".into();
    a.body = "work".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let intake = read(dir.path(), ".claude/commands/intake.md");
    assert!(intake.contains("intake interview"));
    assert!(intake.contains("$ARGUMENTS"));
    let refine = read(dir.path(), ".claude/commands/refine.md");
    assert!(refine.contains("push back"));

    let skill = read(dir.path(), ".claude/skills/commit/SKILL.md");
    assert!(skill.contains("name: commit"));
    assert!(skill.contains("allowed-tools: Bash(git add:*), Bash(git commit:*)"));
    assert!(skill.contains("Stage and commit"));

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
}

#[test]
fn claude_default_pipeline_scaffolds_a_full_project() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "writing".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    // coordinator is primary → coordinator (CLAUDE.md), not an agent file.
    assert!(!dir.path().join(".claude/agents/coordinator.md").exists());
    let structure = read(dir.path(), ".claude/agents/explorer.md");
    assert!(structure.contains("model: opus"));
    // The explorer can fetch web pages and search — needed for parallel research.
    assert!(structure.contains("tools: Read, Grep, Glob"));
    assert!(structure.contains("WebFetch") && structure.contains("WebSearch"));
    let grammar = read(dir.path(), ".claude/agents/reviewer.md");
    assert!(grammar.contains("model: opus"));

    let claude_md = read(dir.path(), "CLAUDE.md");
    assert!(claude_md.contains("# writing"));
    assert!(read(dir.path(), ".claude/rules/ocgen-team.md").contains("**explorer**"));
    // The coordinator's prompt template is fully rendered — no raw Jinja leaks.
    assert!(
        !claude_md.contains("{% for"),
        "unrendered Jinja in CLAUDE.md"
    );
    assert!(!claude_md.contains("{{ sub.name }}"));

    let multi = read(dir.path(), ".claude/commands/multi.md");
    assert!(multi.contains("@explorer") && multi.contains("@reviewer"));

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "opus");
}

#[test]
fn claude_powerups_and_workflow_can_be_disabled() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "bare".into();
    p.claude.powerups = Powerups {
        permissions: false,
        hooks: false,
        output_style: false,
        statusline: false,
    };
    p.claude.workflow = Workflow {
        intake: false,
        refine: false,
        improve_prompt: false,
        fanout: false,
        verify_todos: false,
        deliver: false,
        subagent_confidence: 0,
    };
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "haiku".into();
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    assert!(!dir.path().join(".claude/commands/intake.md").exists());
    assert!(!dir.path().join(".claude/commands/refine.md").exists());
    // verify_todos off → no Verification-first todos guidance in the workflow rule.
    assert!(
        !read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Verification-first todos")
    );
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "opus");
    assert!(settings.get("permissions").is_none());
    assert!(settings.get("hooks").is_none());
    assert!(settings.get("outputStyle").is_none());
    assert!(settings.get("statusLine").is_none());
}

#[test]
fn claude_project_without_coordinator_still_renders() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "flat".into();
    let mut a = Agent::blank("solo", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    a.description = "only".into();
    a.body = "do".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let claude_md = read(dir.path(), "CLAUDE.md");
    assert!(claude_md.contains("# flat"));
    assert!(!claude_md.contains("## Coordination"));
    assert!(dir.path().join(".claude/agents/solo.md").exists());
}

#[test]
fn claude_settings_honors_explicit_model_and_defaults_empty_alias() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "m".into();
    p.claude.model = "opus".into(); // explicit global default
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into(); // model intentionally left empty
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "opus");
    // An agent with no alias falls back to the default (opus).
    assert!(read(dir.path(), ".claude/agents/w.md").contains("model: opus"));
}

#[test]
fn claude_project_with_only_coordinator_skips_multi() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "solo".into();
    let mut boss = Agent::blank("boss", "orchestrator", "");
    boss.mode = "primary".into();
    boss.body = "coordinate".into();
    p.agents = vec![boss];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    assert!(!dir.path().join(".claude/commands/multi.md").exists());
    assert!(!dir.path().join(".claude/agents/boss.md").exists());
    let claude_md = read(dir.path(), "CLAUDE.md");
    assert!(claude_md.contains("# solo"));
    assert!(read(dir.path(), ".claude/rules/ocgen-team.md").contains("coordinate"));
}

// ---- Claude Code target: plugin + GitHub release output (phase 4) ----

#[test]
fn claude_plugin_output_emits_manifest_marketplace_and_workflow() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "Writing Kit".into();
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "acme".into();
    p.claude.plugin.repo_name = "writing-kit".into();
    p.claude.plugin.version = "1.2.0".into();
    p.claude.plugin.display_name = "Writing Kit".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(Skill {
        name: "commit".into(),
        description: "c".into(),
        allowed_tools: "Bash(git commit:*)".into(),
        body: "b".into(),
        role: None,
        ..Default::default()
    });

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let base = dir.path().join("plugin/writing-kit");

    let pj: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(base.join(".claude-plugin/plugin.json")).unwrap())
            .unwrap();
    assert_eq!(pj["name"], "writing-kit");
    assert_eq!(pj["version"], "1.2.0");
    assert_eq!(pj["author"]["name"], "acme");

    let mk: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(base.join(".claude-plugin/marketplace.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mk["plugins"][0]["name"], "writing-kit");
    assert_eq!(mk["plugins"][0]["source"], "./");

    // Components live at the plugin root (no .claude/ prefix).
    assert!(base.join("agents/explorer.md").is_file());
    assert!(base.join("commands/multi.md").is_file());
    assert!(base.join("commands/intake.md").is_file());
    assert!(base.join("skills/commit/SKILL.md").is_file());
    assert!(base.join("output-styles/concise.md").is_file());

    let readme = fs::read_to_string(base.join("README.md")).unwrap();
    assert!(readme.contains("claude plugin marketplace add acme/writing-kit"));
    let wf = fs::read_to_string(base.join(".github/workflows/release.yml")).unwrap();
    assert!(wf.contains("softprops/action-gh-release"));

    let hj: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(base.join("hooks/hooks.json")).unwrap()).unwrap();
    assert!(hj["hooks"]["SessionStart"].is_array());

    // `both` also wrote the project tree.
    assert!(dir.path().join(".claude/agents/explorer.md").is_file());
    assert!(dir.path().join("CLAUDE.md").is_file());
}

#[test]
fn claude_plugin_only_skips_project_tree() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "kit".into();
    p.claude.output = Output {
        project: false,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "kit".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    // Only the state file lives under .claude/ in plugin-only mode; no components.
    assert!(!dir.path().join(".claude/agents").exists());
    assert!(!dir.path().join(".claude/commands").exists());
    assert!(!dir.path().join("CLAUDE.md").exists());
    assert!(dir
        .path()
        .join("plugin/kit/.claude-plugin/plugin.json")
        .is_file());
}

#[test]
fn plugin_name_falls_back_to_project_name_slug() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "My Kit".into();
    p.claude.output = Output {
        project: false,
        plugin: true,
    };
    // repo_name intentionally empty → plugin name derives from the project name.
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "haiku".into();
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(dir
        .path()
        .join("plugin/my-kit/.claude-plugin/plugin.json")
        .is_file());
}

// ---- Claude Code target: parity checks/doctor (phase 6) ----

#[test]
fn claude_issues_flags_bad_alias_and_multiple_coordinators() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    let mut a = Agent::blank("a", "custom", "");
    a.mode = "primary".into();
    a.model = "sonnet".into();
    let mut b = Agent::blank("b", "custom", "");
    b.mode = "primary".into(); // second coordinator
    b.model = "gpt-4".into(); // invalid alias
    p.agents = vec![a, b];

    let issues = p.issues();
    assert!(issues
        .iter()
        .any(|i| i.contains("unknown model alias 'gpt-4'")));
    assert!(issues.iter().any(|i| i.contains("coordinators")));
    // No OpenCode provider/utility noise.
    assert!(!issues.iter().any(|i| i.contains("utility provider")));
}

#[test]
fn claude_doctor_repairs_alias_colour_and_role() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    let mut a = Agent::blank("a", "", ""); // empty role
    a.mode = "subagent".into();
    a.model = "gpt-4".into(); // bad alias
    a.color = "#ff0000".into(); // not a Claude named colour
    p.agents = vec![a];

    let fixes = p.doctor();
    assert!(fixes
        .iter()
        .any(|f| f.contains("model alias") && f.contains("opus")));
    assert!(fixes
        .iter()
        .any(|f| f.contains("colour") && f.contains("blue")));
    assert!(fixes.iter().any(|f| f.contains("empty role")));
    assert_eq!(p.agents[0].model, "opus");
    assert_eq!(p.agents[0].color, "blue");
    assert_eq!(p.agents[0].role, "custom");

    // A healthy Claude project reports nothing to fix.
    let mut ok = base_project("English");
    ok.target = Target::ClaudeCode;
    ok.agents = agent::claude_default_pipeline("English").unwrap();
    assert!(ok.doctor().is_empty());
    assert!(ok.issues().is_empty());
}

#[test]
fn claude_rename_cleanup_helpers_remove_files() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "c".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(Skill {
        name: "commit".into(),
        description: "d".into(),
        allowed_tools: "".into(),
        body: "b".into(),
        role: None,
        ..Default::default()
    });
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    assert!(dir.path().join(".claude/agents/explorer.md").is_file());
    Project::remove_claude_agent_file(dir.path(), "explorer").unwrap();
    assert!(!dir.path().join(".claude/agents/explorer.md").exists());

    assert!(dir.path().join(".claude/skills/commit").is_dir());
    Project::remove_claude_skill_dir(dir.path(), "commit").unwrap();
    assert!(!dir.path().join(".claude/skills/commit").exists());

    // Removing something that isn't there is a no-op.
    Project::remove_claude_agent_file(dir.path(), "nope").unwrap();
    Project::remove_claude_skill_dir(dir.path(), "nope").unwrap();
}

// ---- Claude Code target: Agent Teams (phase 7) ----

#[test]
fn claude_team_enabled_wires_settings_command_hooks_and_guidance() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "team-kit".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        ..Default::default()
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(s["env"]["CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"], "1");
    assert_eq!(s["teammateMode"], "in-process");
    assert!(s["hooks"]["TeammateIdle"].is_array());
    assert!(s["hooks"]["TaskCreated"].is_array());
    assert!(s["hooks"]["TaskCompleted"].is_array());

    let team_md = read(dir.path(), ".claude/commands/team.md");
    assert!(team_md.contains("$ARGUMENTS"));
    assert!(team_md.contains("explorer"));
    assert!(!team_md.contains("{% for"), "unrendered Jinja in team.md");
    assert!(dir.path().join(".claude/commands/multi.md").is_file());

    for h in [
        "team-teammate-idle.sh",
        "team-task-created.sh",
        "team-task-completed.sh",
    ] {
        assert!(
            dir.path().join(format!(".claude/hooks/{h}")).is_file(),
            "{h}"
        );
    }

    assert!(read(dir.path(), ".claude/rules/ocgen-team.md").contains("Agent Teams"));
}

#[test]
fn claude_team_disabled_by_default_adds_nothing() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "plain".into();
    p.claude.workflow.subagent_confidence = 0; // isolate team behavior from the SubagentStop gate
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert!(s.get("teammateMode").is_none());
    assert!(s.get("env").is_none());
    assert!(!dir.path().join(".claude/commands/team.md").exists());
    assert!(!dir
        .path()
        .join(".claude/hooks/team-task-created.sh")
        .exists());
    // The Agent Teams guidance *section* is absent (prose elsewhere may mention the term).
    assert!(!read(dir.path(), "CLAUDE.md").contains("## Agent Teams"));
}

#[test]
fn claude_doctor_repairs_bad_teammate_mode() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.claude.team = Team {
        enabled: true,
        mode: "split".into(),
        hooks: false,
        ..Default::default()
    };
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    p.agents = vec![a];

    let fixes = p.doctor();
    assert!(fixes
        .iter()
        .any(|f| f.contains("teammate mode") && f.contains("in-process")));
    assert_eq!(p.claude.team.mode, "in-process");
}

#[test]
fn claude_team_enabled_without_hook_stubs() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "th".into();
    p.claude.workflow.subagent_confidence = 0; // isolate team-hook behavior
    p.claude.team = Team {
        enabled: true,
        mode: "auto".into(),
        hooks: false,
        ..Default::default()
    };
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(s["env"]["CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"], "1");
    assert_eq!(s["teammateMode"], "auto");
    // Hooks disabled → no team hook events, and no hook scripts on disk…
    assert!(s["hooks"].get("TaskCreated").is_none());
    assert!(!dir.path().join(".claude/hooks").exists());
    // …but the /team command is still emitted.
    assert!(dir.path().join(".claude/commands/team.md").is_file());
}

#[test]
fn team_governance_wires_settings_command_and_hooks() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "gov".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    // settings.json carries the governance env vars alongside the experiment flag.
    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(s["env"]["CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"], "1");
    assert_eq!(s["env"]["TEAM_CONFIDENCE_THRESHOLD"], "96");
    assert_eq!(s["env"]["TEAM_PLAN_GATE"], "1");
    assert_eq!(s["env"]["TEAM_RISK_ROUNDS"], "1");
    assert_eq!(s["env"]["TEAM_APPROVAL_GATE"], "1");

    // The execution-approval gate is a PreToolUse hook with a tool-name matcher.
    let pre = &s["hooks"]["PreToolUse"][0];
    assert!(pre["matcher"].as_str().unwrap().contains("Bash"));
    assert!(pre["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("team-approval-gate.sh"));
    let gate = read(dir.path(), ".claude/hooks/team-approval-gate.sh");
    assert!(gate.contains("TEAM_APPROVAL_GATE"));
    assert!(gate.contains("execution-approved")); // the human-created marker
    assert!(gate.contains("git") && gate.contains("push")); // deterministic patterns

    // The collaborative planning command is emitted and fully rendered.
    let plan_cmd = read(dir.path(), ".claude/commands/team-plan.md");
    assert!(plan_cmd.contains("Status: APPROVED"));
    assert!(
        !plan_cmd.contains("{{") && !plan_cmd.contains("{%"),
        "unrendered Jinja"
    );

    // The gate logic lives in the hook scripts.
    let created = read(dir.path(), ".claude/hooks/team-task-created.sh");
    assert!(created.contains("TEAM_PLAN_GATE"));
    assert!(created.contains("Status: APPROVED"));
    let completed = read(dir.path(), ".claude/hooks/team-task-completed.sh");
    assert!(completed.contains("TEAM_CONFIDENCE_THRESHOLD"));
    let idle = read(dir.path(), ".claude/hooks/team-teammate-idle.sh");
    assert!(idle.contains("TEAM_RISK_ROUNDS"));

    // Guidance documents the governed workflow and the threshold (in the team rule).
    let team_rule = read(dir.path(), ".claude/rules/ocgen-team.md");
    assert!(team_rule.contains("Governed workflow"));
    assert!(team_rule.contains("96%"));
}

#[test]
fn team_confidence_zero_disables_gate() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "noconf".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: false,
        confidence_threshold: 0,
        risk_rounds: false,
        approval_gate: false,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    // Governance env vars are omitted when their features are off…
    assert!(s["env"].get("TEAM_CONFIDENCE_THRESHOLD").is_none());
    assert!(s["env"].get("TEAM_PLAN_GATE").is_none());
    assert!(s["env"].get("TEAM_RISK_ROUNDS").is_none());
    assert!(s["env"].get("TEAM_APPROVAL_GATE").is_none());
    assert!(s["hooks"].get("PreToolUse").is_none());
    assert!(!dir
        .path()
        .join(".claude/hooks/team-approval-gate.sh")
        .exists());
    // …and the plan command is not emitted when the plan gate is off.
    assert!(!dir.path().join(".claude/commands/team-plan.md").exists());
    // The base team command is still emitted.
    assert!(dir.path().join(".claude/commands/team.md").is_file());
}

#[test]
fn approval_gate_emits_even_without_hook_stubs() {
    // The safety line must never be silently off: approval_gate emits its
    // PreToolUse hook independently of the `hooks` toggle.
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "gate".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: false,
        plan_gate: false,
        confidence_threshold: 0,
        risk_rounds: false,
        approval_gate: true,
    };
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    p.agents = vec![a];
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(s["env"]["TEAM_APPROVAL_GATE"], "1");
    assert!(s["hooks"]["PreToolUse"].is_array());
    // The other team quality-gate hooks stay off (hooks: false).
    assert!(s["hooks"].get("TaskCreated").is_none());
    assert!(dir
        .path()
        .join(".claude/hooks/team-approval-gate.sh")
        .is_file());
    assert!(!dir
        .path()
        .join(".claude/hooks/team-task-created.sh")
        .exists());
}

#[test]
fn approval_gate_hook_blocks_high_impact_until_human_unlock() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "exec".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let hook = dir.path().join(".claude/hooks/team-approval-gate.sh");

    // Run the generated hook with a PreToolUse payload; return its exit code.
    let run = |payload: &str| -> i32 {
        let mut child = Command::new("sh")
            .arg(&hook)
            .env("CLAUDE_PROJECT_DIR", dir.path())
            .env("TEAM_APPROVAL_GATE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The hook may exit before reading stdin (e.g. threshold 0), closing the
        // pipe; a BrokenPipe on our write is expected, so ignore the write result.
        let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
        child.wait().unwrap().code().unwrap()
    };
    let bash = |cmd: &str| format!(r#"{{"tool_name":"Bash","tool_input":{{"command":"{cmd}"}}}}"#);

    // Locked (no marker): local/read-only work is allowed…
    assert_eq!(run(&bash("git status")), 0);
    assert_eq!(run(&bash("npm test")), 0);
    // …but every high-impact external action is blocked (exit 2).
    for cmd in [
        "git push origin main",
        "gh pr merge 42 --merge",
        "aws s3 rm s3://b/x --recursive",
        "aws ec2 terminate-instances --instance-ids i-1",
        "ssh prod uptime",
        "terraform apply -auto-approve",
        "curl -X POST https://api/x -d @p",
    ] {
        assert_eq!(run(&bash(cmd)), 2, "should block: {cmd}");
    }
    // An agent may not self-approve by creating the marker (Bash or file write).
    assert_eq!(run(&bash("touch .claude/team/execution-approved")), 2);
    assert_eq!(
        run(
            r#"{"tool_name":"Write","tool_input":{"file_path":".claude/team/execution-approved","content":"x"}}"#
        ),
        2
    );

    // Human unlocks from outside the agent → high-impact actions proceed.
    fs::create_dir_all(dir.path().join(".claude/team")).unwrap();
    fs::write(dir.path().join(".claude/team/execution-approved"), "").unwrap();
    assert_eq!(run(&bash("git push origin main")), 0);
    assert_eq!(run(&bash("aws s3 rm s3://b/x")), 0);
}

#[test]
fn team_readonly_roles_env_lists_read_only_agents() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "ro".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    let ro = s["env"]["TEAM_READONLY_ROLES"].as_str().unwrap();
    // explorer/reviewer are read-only; implementer writes.
    assert!(ro.split_whitespace().any(|r| r == "explorer"));
    assert!(ro.split_whitespace().any(|r| r == "reviewer"));
    assert!(!ro.split_whitespace().any(|r| r == "implementer"));
}

#[test]
fn teammate_idle_gate_exempts_read_only_roles() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "idle".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let hook = dir.path().join(".claude/hooks/team-teammate-idle.sh");
    fs::create_dir_all(dir.path().join(".claude/team")).unwrap();
    fs::write(
        dir.path().join(".claude/team/plan.md"),
        "- Risk: data loss\nMitigation: pending\n",
    )
    .unwrap();

    let run = |agent_type: &str| -> i32 {
        let mut child = Command::new("sh")
            .arg(&hook)
            .env("TEAM_RISK_ROUNDS", "1")
            .env("TEAM_READONLY_ROLES", "explorer reviewer")
            .env("CLAUDE_PROJECT_DIR", dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let payload =
            format!(r#"{{"hook_event_name":"TeammateIdle","agent_type":"{agent_type}"}}"#);
        let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
        child.wait().unwrap().code().unwrap()
    };

    // Read-only roles are exempt even while a risk is pending (no livelock).
    assert_eq!(run("explorer"), 0);
    assert_eq!(run("reviewer"), 0);
    // A writer is still held while a risk is pending.
    assert_eq!(run("implementer"), 2);
    // Once the risk is resolved, the writer may idle.
    fs::write(
        dir.path().join(".claude/team/plan.md"),
        "- Risk: data loss\nMitigation: done\n",
    )
    .unwrap();
    assert_eq!(run("implementer"), 0);
}

#[test]
fn claude_doctor_repairs_bad_confidence_threshold() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 250,
        risk_rounds: true,
        approval_gate: true,
    };
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    p.agents = vec![a];

    let fixes = p.doctor();
    assert!(fixes
        .iter()
        .any(|f| f.contains("confidence") && f.contains("96")));
    assert_eq!(p.claude.team.confidence_threshold, 96);
}

#[test]
fn doctor_clamps_bad_subagent_confidence() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.claude.workflow.subagent_confidence = 250;
    let fixes = p.doctor();
    assert!(fixes
        .iter()
        .any(|f| f.contains("subagent confidence") && f.contains("96")));
    assert_eq!(p.claude.workflow.subagent_confidence, 96);
}

// ---- Claude Code target: richer skill authoring (phase 8) ----

#[test]
fn claude_skill_renders_full_frontmatter() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "sk".into();
    p.skills.push(Skill {
        name: "deploy".into(),
        description: "Deploy to prod".into(),
        allowed_tools: "Bash(npm run build), Bash(npm run deploy)".into(),
        body: "1. build\n2. deploy".into(),
        role: None,
        when_to_use: "When the user says ship it".into(),
        argument_hint: "[env]".into(),
        disable_model_invocation: true,
        hidden_from_menu: false,
        context_fork: true,
        agent: "Explore".into(),
        model: "sonnet".into(),
    });
    let mut a = Agent::blank("w", "custom", "");
    a.mode = "subagent".into();
    a.model = "sonnet".into();
    a.description = "d".into();
    a.body = "b".into();
    p.agents = vec![a];

    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let sk = read(dir.path(), ".claude/skills/deploy/SKILL.md");
    assert!(sk.contains("name: deploy"));
    assert!(sk.contains("when_to_use: When the user says ship it"));
    assert!(sk.contains("argument-hint: [env]"));
    assert!(sk.contains("allowed-tools: Bash(npm run build), Bash(npm run deploy)"));
    assert!(sk.contains("disable-model-invocation: true"));
    assert!(sk.contains("model: sonnet"));
    assert!(sk.contains("context: fork"));
    assert!(sk.contains("agent: Explore"));
    assert!(
        !sk.contains("user-invocable"),
        "hidden_from_menu false → omitted"
    );
    assert!(sk.contains("1. build"));
}

#[test]
fn unknown_tools_flags_only_unrecognized() {
    use ocgen::claude::unknown_tools;
    assert!(unknown_tools("Read, Grep, Bash(git add:*), mcp__gh__pr").is_empty());
    assert_eq!(
        unknown_tools("Read, Frobnicate, Grpe"),
        vec!["Frobnicate".to_string(), "Grpe".to_string()]
    );
    assert!(unknown_tools("").is_empty());
}

#[test]
fn skill_presets_load_and_seed_skills() {
    let presets = ocgen::claude::skill_presets().unwrap();
    let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"command"));
    assert!(names.contains(&"knowledge"));
    assert!(names.contains(&"forked-research"));

    let command = presets.iter().find(|p| p.name == "command").unwrap();
    let sk = command.to_skill("commit", "English");
    assert_eq!(sk.name, "commit");
    assert!(sk.disable_model_invocation);
    assert!(sk.allowed_tools.contains("Bash(git commit:*)"));
    assert!(sk.body.contains("1."));

    let forked = presets
        .iter()
        .find(|p| p.name == "forked-research")
        .unwrap()
        .to_skill("research", "English");
    assert!(forked.context_fork);
    assert_eq!(forked.agent, "Explore");
}

#[test]
fn claude_md_instructs_parallel_exploration() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "px".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let md = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(md.contains("Parallel exploration"));
    assert!(md.contains("in parallel"));
    assert!(md.contains("web page") && md.contains("PDF"));
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    // The coordination guidance (team rule) must not mandate serial delegation.
    let team_rule = read(dir.path(), ".claude/rules/ocgen-team.md");
    assert!(
        !team_rule.contains("do not run in parallel"),
        "Claude coordination should not carry the local-server serial note"
    );
}

#[test]
fn opencode_coordinator_prompt_stays_serial() {
    // The local-single-server serial note is preserved for OpenCode (prompt file).
    let mut p = base_project("English");
    p.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let prompt = read(dir.path(), ".opencode/prompts/coordinator.txt");
    assert!(prompt.contains("do not run in parallel"));
}

#[test]
fn session_start_tip_is_valid_shell() {
    use std::process::Command;
    // The default pipeline enables the improve-prompt tip, whose text contains an
    // apostrophe ("agent's"); the generated `echo '...'` command must stay valid shell.
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "tip".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    let cmd = s["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .expect("SessionStart command");
    let out = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
    assert!(
        out.status.success(),
        "SessionStart command is not valid shell: {cmd}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn subagent_confidence_gate_wires_independently_of_teams() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "sc".into();
    // No team — the SubagentStop gate must still wire.
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(s["env"]["SUBAGENT_CONFIDENCE_THRESHOLD"], "96");
    assert!(s["env"]
        .get("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")
        .is_none()); // team off
    let cmd = s["hooks"]["SubagentStop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(cmd.contains("subagent-confidence-gate.sh"));
    assert!(dir
        .path()
        .join(".claude/hooks/subagent-confidence-gate.sh")
        .is_file());

    // Off → nothing.
    let mut p2 = base_project("English");
    p2.target = Target::ClaudeCode;
    p2.project_name = "nosc".into();
    p2.claude.workflow.subagent_confidence = 0;
    p2.agents = agent::claude_default_pipeline("English").unwrap();
    let dir2 = tempdir().unwrap();
    p2.scaffold(dir2.path(), false).unwrap();
    let s2: serde_json::Value =
        serde_json::from_str(&read(dir2.path(), ".claude/settings.json")).unwrap();
    assert!(s2.get("env").is_none());
    assert!(s2["hooks"].get("SubagentStop").is_none());
    assert!(!dir2
        .path()
        .join(".claude/hooks/subagent-confidence-gate.sh")
        .exists());
}

#[test]
fn subagent_confidence_gate_blocks_low_confidence_writers() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "scx".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let hook = dir.path().join(".claude/hooks/subagent-confidence-gate.sh");

    // A real git worktree (the subagent's cwd). Start clean.
    let wt = dir.path().join("wt");
    fs::create_dir_all(&wt).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&wt)
            .output()
            .unwrap();
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);

    let run = |payload: &str| -> i32 {
        let mut child = Command::new("sh")
            .arg(&hook)
            .env("SUBAGENT_CONFIDENCE_THRESHOLD", "96")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The hook may exit before reading stdin (e.g. threshold 0), closing the
        // pipe; a BrokenPipe on our write is expected, so ignore the write result.
        let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
        child.wait().unwrap().code().unwrap()
    };
    let ev = |msg: &str| {
        format!(
            r#"{{"hook_event_name":"SubagentStop","cwd":"{}","last_assistant_message":"{}"}}"#,
            wt.display(),
            msg
        )
    };

    // Clean worktree (read-only worker) → always passes.
    assert_eq!(run(&ev("explored things, no confidence stated")), 0);

    // The worker writes a file → now it must be confident.
    fs::write(wt.join("new.rs"), "// change\n").unwrap();
    assert_eq!(run(&ev("done. Confidence: 97%")), 0);
    assert_eq!(run(&ev("done. Confidence: 50%")), 2);
    assert_eq!(run(&ev("done, no rating")), 2);

    // Threshold 0 disables the gate even for a low-confidence writer.
    let mut child = Command::new("sh")
        .arg(&hook)
        .env("SUBAGENT_CONFIDENCE_THRESHOLD", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Threshold 0 makes the hook exit before reading stdin; ignore the BrokenPipe.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(ev("Confidence: 10%").as_bytes());
    assert_eq!(child.wait().unwrap().code().unwrap(), 0);
}

#[test]
fn claude_md_is_created_once_and_never_overwritten() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "own".into();
    p.claude.instructions = "## Purpose\nBuild widgets.\n".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();

    // A pre-existing user CLAUDE.md is preserved — even under force (doctor).
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("CLAUDE.md"), "MY OWN NOTES\n").unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(read(dir.path(), "CLAUDE.md"), "MY OWN NOTES\n");
    // …while the guidance still lands in the rules.
    assert!(dir.path().join(".claude/rules/ocgen-workflow.md").is_file());

    // A fresh dir gets a slim starter with the purpose, and no guidance inline.
    let dir2 = tempdir().unwrap();
    p.scaffold(dir2.path(), false).unwrap();
    let starter = read(dir2.path(), "CLAUDE.md");
    assert!(starter.contains("# own") && starter.contains("Build widgets."));
    assert!(!starter.contains("Parallel exploration"));
}

#[test]
fn deliver_pipeline_command_and_guidance() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "dl".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let cmd = read(dir.path(), ".claude/commands/deliver.md");
    assert!(cmd.contains("Sharpen") && cmd.contains("requirements") && cmd.contains("Confidence"));
    assert!(
        !cmd.contains("{{") && !cmd.contains("{%"),
        "unrendered Jinja"
    );
    let wf = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(wf.contains("Delivery pipeline"));
    assert!(wf.contains("Multi-session delivery") && wf.contains("--worktree"));
}

#[test]
fn deliver_disabled_omits_command_and_guidance() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "nodl".into();
    p.claude.workflow.deliver = false;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/commands/deliver.md").exists());
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Delivery pipeline"));
}

#[test]
fn confidence_gate_is_parallel_safe_per_completion() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "cg".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let hook = dir.path().join(".claude/hooks/team-task-completed.sh");

    let run = |payload: &str, thr: &str| -> i32 {
        let mut child = Command::new("sh")
            .arg(&hook)
            .env("CLAUDE_PROJECT_DIR", dir.path())
            .env("TEAM_CONFIDENCE_THRESHOLD", thr)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The hook may exit before reading stdin (e.g. threshold 0), closing the
        // pipe; a BrokenPipe on our write is expected, so ignore the write result.
        let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
        child.wait().unwrap().code().unwrap()
    };

    // Confidence stated in THIS completion — read from its own stdin (race-free).
    assert_eq!(
        run(
            r#"{"task_id":"t1","summary":"done. Confidence: 97%"}"#,
            "96"
        ),
        0
    );
    assert_eq!(
        run(
            r#"{"task_id":"t2","summary":"done. Confidence: 50%"}"#,
            "96"
        ),
        2
    );
    assert_eq!(
        run(r#"{"task_id":"t3","summary":"done, no rating"}"#, "96"),
        2
    );
    // Fallback: per-task marker keyed by this event's task id.
    fs::create_dir_all(dir.path().join(".claude/team/confidence")).unwrap();
    fs::write(dir.path().join(".claude/team/confidence/t5.txt"), "97").unwrap();
    assert_eq!(run(r#"{"task_id":"t5","summary":"done"}"#, "96"), 0);
    // Threshold 0 disables the gate.
    assert_eq!(
        run(r#"{"task_id":"t6","summary":"Confidence: 10%"}"#, "0"),
        0
    );
}

#[test]
fn claude_md_instructs_verification_first_todos() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "vt".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let md = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(md.contains("Verification-first todos"));
    assert!(md.contains("definition of done"));
    assert!(md.contains("self-review"));
    assert!(md.contains("confidence"));
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
}

#[test]
fn fanout_scaffolds_worktree_isolation_command_and_include() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "wt".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    // The writer runs isolated; read-only roles do not.
    let implementer = read(dir.path(), ".claude/agents/implementer.md");
    assert!(implementer.contains("isolation: worktree"));
    assert!(!read(dir.path(), ".claude/agents/explorer.md").contains("isolation:"));
    assert!(!read(dir.path(), ".claude/agents/reviewer.md").contains("isolation:"));

    // The /fanout command and .worktreeinclude are emitted.
    let fanout = read(dir.path(), ".claude/commands/fanout.md");
    assert!(fanout.contains("worktree") && fanout.contains("merge") && fanout.contains("clean up"));
    assert!(
        !fanout.contains("{{") && !fanout.contains("{%"),
        "unrendered Jinja"
    );
    let wti = read(dir.path(), ".worktreeinclude"); // project root, not under .claude/
    assert!(wti.contains(".env"));

    // The workflow rule documents the protocol.
    assert!(read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Parallel worktrees"));
}

#[test]
fn fanout_disabled_omits_command_and_include() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "nowt".into();
    p.claude.workflow.fanout = false;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/commands/fanout.md").exists());
    assert!(!dir.path().join(".worktreeinclude").exists());
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Parallel worktrees"));
    // Isolation is archetype-driven, so it stays on the implementer regardless.
    assert!(read(dir.path(), ".claude/agents/implementer.md").contains("isolation: worktree"));
}

#[test]
fn rendered_artifacts_use_lf_line_endings() {
    // Guard against CRLF-corrupted templates (a Windows checkout without a
    // .gitattributes turns the embedded .j2 templates and generated .sh hooks into
    // CRLF, which breaks `contains("...\n...")` asserts and Unix shell hooks).
    // Every rendered artifact must be LF-only, on every platform.
    let mut oc = base_project("English");
    oc.agents = agent::default_pipeline("English", "mac").unwrap();
    for (rel, content) in oc.render_all().unwrap() {
        assert!(!content.contains('\r'), "CR in {}", rel.display());
    }

    let mut cc = base_project("English");
    cc.target = Target::ClaudeCode;
    cc.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    cc.agents = agent::claude_default_pipeline("English").unwrap();
    for (rel, content) in cc.render_all().unwrap() {
        assert!(!content.contains('\r'), "CR in {}", rel.display());
    }
}

#[test]
fn improve_prompt_command_and_skill_preset() {
    // Command is emitted when enabled (default), with the technique markers and no raw Jinja.
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "ip".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let cmd = read(dir.path(), ".claude/commands/improve-prompt.md");
    assert!(cmd.contains("<instructions>"));
    assert!(cmd.contains("frontmatter"));
    assert!(cmd.contains("example"));
    assert!(
        !cmd.contains("{{") && !cmd.contains("{%"),
        "unrendered Jinja"
    );

    // The matching skill preset is available with edit-capable tools.
    let presets = ocgen::claude::skill_presets().unwrap();
    let ip = presets
        .iter()
        .find(|p| p.name == "improve-prompt")
        .expect("improve-prompt preset");
    assert!(ip.allowed_tools.contains("Edit") && ip.allowed_tools.contains("Read"));
    assert!(ip.body.contains_key("English"));
}

#[test]
fn improve_prompt_disabled_omits_command() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "noip".into();
    p.claude.workflow.improve_prompt = false;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir
        .path()
        .join(".claude/commands/improve-prompt.md")
        .exists());
}
