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
    // `discover` returns a plain path (no Windows `\\?\` prefix).
    assert_eq!(
        root,
        ocgen::paths::plain(&dir.path().canonicalize().unwrap())
    );
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

    let multi = read(dir.path(), ".claude/skills/multi/SKILL.md");
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
    assert_eq!(settings["outputStyle"], "ocgen-concise");

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

    let intake = read(dir.path(), ".claude/skills/intake/SKILL.md");
    assert!(intake.contains("intake interview"));
    assert!(intake.contains("$ARGUMENTS"));
    let refine = read(dir.path(), ".claude/skills/refine/SKILL.md");
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

    let multi = read(dir.path(), ".claude/skills/multi/SKILL.md");
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
    p.claude.hooks_extra.compact_context = false; // the wizard clears it with the power-ups
    p.claude.hooks_extra.drop_noop_cd = false; // so does this one
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
        prefer_explorer: false,
        deliver: false,
        inquire: false,
        intent: false,
        loop_guard_max: 0,
        check_cmd: String::new(),
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

    assert!(!dir.path().join(".claude/skills/intake/SKILL.md").exists());
    assert!(!dir.path().join(".claude/skills/refine/SKILL.md").exists());
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

    assert!(!dir.path().join(".claude/skills/multi/SKILL.md").exists());
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
    assert!(base.join("skills/multi/SKILL.md").is_file());
    assert!(base.join("skills/intake/SKILL.md").is_file());
    assert!(base.join("skills/commit/SKILL.md").is_file());
    assert!(base.join("output-styles/ocgen-concise.md").is_file());

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

    let team_md = read(dir.path(), ".claude/skills/team/SKILL.md");
    assert!(team_md.contains("$ARGUMENTS"));
    assert!(team_md.contains("explorer"));
    assert!(!team_md.contains("{% for"), "unrendered Jinja in team.md");
    assert!(dir.path().join(".claude/skills/multi/SKILL.md").is_file());

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
    assert!(!dir.path().join(".claude/skills/team/SKILL.md").exists());
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
    p.claude.workflow.intent = false; // its https-only WebFetch hook isn't a team hook
    p.claude.workflow.inquire = false; // nor is its notes-view hook
    p.claude.hooks_extra.drop_noop_cd = false; // nor is the no-op cd hook

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
    assert!(dir.path().join(".claude/skills/team/SKILL.md").is_file());
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
    assert!(gate.contains("ocgen approve")); // only a human approves, outside the project
    assert!(gate.contains("git") && gate.contains("push")); // deterministic patterns

    // The collaborative planning command is emitted and fully rendered.
    let plan_cmd = read(dir.path(), ".claude/skills/team-plan/SKILL.md");
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
    p.claude.workflow.intent = false; // its https-only WebFetch hook isn't a team hook
    p.claude.hooks_extra.drop_noop_cd = false; // nor is the no-op cd hook
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
    assert!(!dir
        .path()
        .join(".claude/skills/team-plan/SKILL.md")
        .exists());
    // The base team command is still emitted.
    assert!(dir.path().join(".claude/skills/team/SKILL.md").is_file());
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
    // A git repository, like a real project, so the approval key comes from git.
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // A private HOME: approvals live outside the project, under ~/.claude/ocgen/.
    let home = tempdir().unwrap();

    // Run the generated hook with a PreToolUse payload; return its exit code.
    let run = |payload: &str| -> i32 {
        let mut child = Command::new("sh")
            .arg(&hook)
            .env("CLAUDE_PROJECT_DIR", dir.path())
            .env("HOME", home.path())
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
    let bash = |cmd: &str| {
        serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": cmd } }).to_string()
    };

    // Locked: local/read-only work is allowed…
    for cmd in [
        "git status",
        "npm test",
        "git log --grep='push'",
        "terraform plan",
        "cat scripts/deploy.sh",
    ] {
        assert_eq!(run(&bash(cmd)), 0, "should allow: {cmd}");
    }
    // …but every high-impact external action is blocked (exit 2) — including the
    // phrasings that used to slip past a plain `git push` match.
    for cmd in [
        "git push origin main",
        "git -C . push origin main",
        "git -c user.name=x push",
        "git \"push\" origin",
        "gh pr merge 42 --merge",
        "aws s3 rm s3://b/x --recursive",
        "aws ec2 terminate-instances --instance-ids i-1",
        "ssh prod uptime",
        "terraform apply -auto-approve",
        "terraform -chdir=infra apply",
        "sh ./deploy.sh",
        "make deploy",
        "curl -X POST https://api/x -d @p",
    ] {
        assert_eq!(run(&bash(cmd)), 2, "should block: {cmd}");
    }
    // An agent may not approve itself: not via the command, not by writing the store.
    assert_eq!(run(&bash("ocgen approve")), 2);
    assert_eq!(
        run(&bash("echo 9999999999 > ~/.claude/ocgen/approvals/x")),
        2
    );
    let store = home
        .path()
        .join(".claude/ocgen/approvals/x")
        .display()
        .to_string();
    assert_eq!(
        run(&serde_json::json!({ "tool_name": "Write", "tool_input": { "file_path": store, "content": "9999999999" } }).to_string()),
        2
    );
    // Merely mentioning the old marker is ordinary work now (no false positives)…
    assert_eq!(run(&bash("grep -r execution-approved docs/")), 0);
    assert_eq!(
        run(
            r#"{"tool_name":"Write","tool_input":{"file_path":"docs/runbook.md","content":"ask a human for execution-approved"}}"#
        ),
        0
    );
    // …and the old in-project marker no longer unlocks anything.
    fs::create_dir_all(dir.path().join(".claude/team")).unwrap();
    fs::write(dir.path().join(".claude/team/execution-approved"), "").unwrap();
    assert_eq!(run(&bash("git push origin main")), 2);

    // A human approves from outside the agent → high-impact actions proceed…
    let marker = ocgen::approval::grant(home.path(), dir.path(), 30).unwrap();
    assert_eq!(run(&bash("git push origin main")), 0);
    assert_eq!(run(&bash("aws s3 rm s3://b/x")), 0);
    assert_eq!(
        run(&bash("ocgen approve --minutes 600")),
        2,
        "still can't extend its own approval"
    );
    // …until the approval expires by itself.
    fs::write(&marker, "1\n").unwrap();
    assert_eq!(run(&bash("git push origin main")), 2);
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
fn teammate_idle_gate_is_ownership_scoped() {
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
    let plan = dir.path().join(".claude/team/plan.md");
    let set = |txt: &str| fs::write(&plan, txt).unwrap();

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

    // Two risks, each owned by a different role, both pending.
    set("- Risk: A — Owner: implementer — Mitigation: pending\n\
         - Risk: B — Owner: dbadmin — Mitigation: pending\n");
    // A teammate is held only for a risk IT owns.
    assert_eq!(run("implementer"), 2); // owns A (pending)
    assert_eq!(run("dbadmin"), 2); // owns B (pending)
                                   // Read-only roles are exempt regardless of ownership.
    assert_eq!(run("explorer"), 0);
    assert_eq!(run("reviewer"), 0);

    // Close only implementer's risk; dbadmin's stays pending.
    set("- Risk: A — Owner: implementer — Mitigation: done\n\
         - Risk: B — Owner: dbadmin — Mitigation: pending\n");
    // The key fix: implementer idles free even though another owner's risk is pending.
    assert_eq!(run("implementer"), 0);
    assert_eq!(run("dbadmin"), 2);

    // Risk rounds off → never blocks.
    let mut child = Command::new("sh")
        .arg(&hook)
        .env("TEAM_RISK_ROUNDS", "0")
        .env("CLAUDE_PROJECT_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"agent_type":"dbadmin"}"#);
    assert_eq!(child.wait().unwrap().code().unwrap(), 0);
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
        ..Default::default()
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

    let cmd = read(dir.path(), ".claude/skills/deliver/SKILL.md");
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
    assert!(!dir.path().join(".claude/skills/deliver/SKILL.md").exists());
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Delivery pipeline"));
}

#[test]
fn inquire_command_and_router() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "inq".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let cmd = read(dir.path(), ".claude/skills/inquire/SKILL.md");
    assert!(cmd.contains("Sharpen") && cmd.contains("Verified") && cmd.contains("Inferred"));
    assert!(cmd.contains(".claude/notes"));
    // Resumable across sessions: a list with no arguments, a stored resume point,
    // a refresher, and a staleness check against what changed in git.
    assert!(cmd.contains("No arguments") && cmd.contains("Resume point"));
    assert!(cmd.contains("Refresher") && cmd.contains("Mental model"));
    assert!(cmd.contains("git log") && cmd.contains("Stale"));
    // The nudge is a single hint, never a list of ready-made next questions.
    assert!(cmd.contains("## 3. Nudge — one hint") && cmd.contains("Hint:"));
    assert!(cmd.contains("Never list") && cmd.contains("form the question myself"));
    for gone in [
        "Offer 3 next questions",
        "Next questions:",
        "pending next questions",
    ] {
        assert!(!cmd.contains(gone), "still offers questions: {gone}");
    }
    // Ledger writes go to a quiet background subagent, not inline diffs.
    assert!(cmd.contains("background subagent") && cmd.contains("haiku"));
    assert!(cmd.contains("≤ 4 lines"));
    // A structured ledger the HTML view can draw: numbered entries tagged with
    // lens and evidence, a mermaid map, open questions and a glossary.
    for needle in [
        "### Q<n> · <Lens> · <Verified|Inferred NN%>",
        "```mermaid",
        "## Open questions",
        "## Glossary",
        " · Stale",
    ] {
        assert!(cmd.contains(needle), "ledger format lacks {needle}");
    }
    // The HTML view is ocgen's job: written with Write/Edit so the hook sees
    // it, never edited by the model, reopened on resume.
    assert!(cmd.contains(".claude/notes/<topic-slug>.html"));
    assert!(cmd.contains("Write or Edit tool"));
    assert!(cmd.contains("never write or edit the `.html`"));
    assert!(cmd.contains("ocgen notes open <topic-slug>"));
    assert!(cmd.contains("allowed-tools: Bash(ocgen notes open:*)"));
    // The page is a phased report: one tab per phase, built from visual blocks.
    for needle in [
        "## Phase <n> · <short name>",
        "Summary:",
        "Status:",
        "`stats` block",
        "`claims` block",
        "`Corrected`",
        "`steps` block",
        "`bars` block",
        "`compare` block",
        "`stack` block",
        "`cards` block",
        "[half]",
        "> **Hint (<Lens>):**",
    ] {
        assert!(cmd.contains(needle), "report format lacks {needle}");
    }
    assert!(
        !cmd.contains("{{") && !cmd.contains("{%"),
        "unrendered Jinja"
    );
    let deliver = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    assert!(deliver.contains("Route the goal") && deliver.contains("/inquire"));
    assert!(read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Codebase inquiry"));
}

#[test]
fn inquire_disabled_omits_command_and_router() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "noinq".into();
    p.claude.workflow.inquire = false;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/inquire/SKILL.md").exists());
    let deliver = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    assert!(!deliver.contains("/inquire") && !deliver.contains("Route the goal"));
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Codebase inquiry"));
}

#[test]
fn workflow_without_inquire_field_backfills_true() {
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert!(wf.inquire);
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
    let fanout = read(dir.path(), ".claude/skills/fanout/SKILL.md");
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
    p.claude.workflow.deliver = false; // /deliver's multi-session flow also uses worktrees
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/fanout/SKILL.md").exists());
    assert!(!dir.path().join(".worktreeinclude").exists());
    let settings = read(dir.path(), ".claude/settings.json");
    assert!(
        !settings.contains("baseRef"),
        "no fanout → default worktree base"
    );
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
    let cmd = read(dir.path(), ".claude/skills/improve-prompt/SKILL.md");
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
        .join(".claude/skills/improve-prompt/SKILL.md")
        .exists());
}

// ---------------------------------------------------------------- loop guard --

/// A Claude project with every blocking gate on (teams + governance + subagent gate).
fn governed_claude_project(dir: &Path) {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "lg".into();
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
    p.scaffold(dir, false).unwrap();
}

/// Run a generated hook with `envs` and a JSON `payload`; return (exit, stdout, stderr).
fn run_hook(hook: &Path, envs: &[(&str, &str)], payload: &str) -> (i32, String, String) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("sh");
    cmd.arg(hook)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    // The hook may exit before reading stdin; ignore a BrokenPipe on our write.
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A git worktree with an uncommitted change, so the subagent gate treats it as a writer.
fn dirty_worktree(root: &Path) -> std::path::PathBuf {
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&wt)
        .output()
        .unwrap();
    fs::write(wt.join("new.rs"), "// change\n").unwrap();
    wt
}

#[test]
fn loop_guard_releases_stuck_subagent_writer_and_escalates() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/subagent-confidence-gate.sh");
    let wt = dirty_worktree(dir.path());
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    // No stated confidence → no progress signal; only the budget can trip.
    let ev = format!(
        r#"{{"session_id":"s1","agent_id":"a1","cwd":"{}","last_assistant_message":"still stuck"}}"#,
        wt.display()
    );
    assert_eq!(run_hook(&hook, &env, &ev).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev).0, 2);
    let (code, out, _) = run_hook(&hook, &env, &ev);
    assert_eq!(code, 0, "released on the 3rd block");
    assert!(
        out.contains("systemMessage") && out.contains("UNRESOLVED"),
        "{out}"
    );
    let log = read(dir.path(), ".claude/loop-guard/escalations.md");
    assert!(
        log.contains("subagent-confidence") && log.contains("a1"),
        "{log}"
    );
    // The state dir ignores itself, so nothing leaks into the repo.
    assert_eq!(read(dir.path(), ".claude/loop-guard/.gitignore"), "*\n");
    // Escalated once, not per event.
    run_hook(&hook, &env, &ev);
    let log = read(dir.path(), ".claude/loop-guard/escalations.md");
    assert_eq!(log.lines().count(), 1, "{log}");
}

#[test]
fn loop_guard_trips_early_when_confidence_stalls() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/subagent-confidence-gate.sh");
    let wt = dirty_worktree(dir.path());
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let ev = |agent: &str, score: u8| {
        format!(
            r#"{{"session_id":"s1","agent_id":"{agent}","cwd":"{}","last_assistant_message":"Confidence: {score}%"}}"#,
            wt.display()
        )
    };
    // Stalled: 60 then 60 → released at block 2.
    assert_eq!(run_hook(&hook, &env, &ev("stall", 60)).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("stall", 60)).0, 0);
    // Rising: 60 then 80 → still making progress, still blocked.
    assert_eq!(run_hook(&hook, &env, &ev("rise", 60)).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("rise", 80)).0, 2);
}

#[test]
fn loop_guard_resets_after_the_gate_passes() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-task-completed.sh");
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_CONFIDENCE_THRESHOLD", "96"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let ev = |tid: &str, msg: &str| {
        format!(r#"{{"session_id":"s1","task_id":"{tid}","summary":"{msg}"}}"#)
    };
    assert_eq!(run_hook(&hook, &env, &ev("t1", "no rating")).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("t1", "no rating")).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("t1", "Confidence: 97%")).0, 0);
    // Fresh budget after the pass: two more blocks before a release.
    assert_eq!(run_hook(&hook, &env, &ev("t1", "no rating")).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("t1", "no rating")).0, 2);
}

#[test]
fn loop_guard_task_budgets_are_per_task() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-task-completed.sh");
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_CONFIDENCE_THRESHOLD", "96"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let ev =
        |tid: &str| format!(r#"{{"session_id":"s1","task_id":"{tid}","summary":"no rating"}}"#);
    assert_eq!(run_hook(&hook, &env, &ev("a")).0, 2);
    assert_eq!(run_hook(&hook, &env, &ev("a")).0, 2);
    // Another task's blocks don't consume task a's budget.
    assert_eq!(run_hook(&hook, &env, &ev("b")).0, 2);
    let (code, out, _) = run_hook(&hook, &env, &ev("a"));
    assert_eq!(code, 0, "task a released on its own 3rd block");
    assert!(out.contains("UNRESOLVED"));
    assert_eq!(run_hook(&hook, &env, &ev("b")).0, 2);
}

#[test]
fn loop_guard_releases_idle_teammate_and_names_owned_risk() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-teammate-idle.sh");
    fs::create_dir_all(dir.path().join(".claude/team")).unwrap();
    fs::write(
        dir.path().join(".claude/team/plan.md"),
        "- Risk: data loss — Owner: implementer — Mitigation: pending\n",
    )
    .unwrap();
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_RISK_ROUNDS", "1"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let ev = r#"{"session_id":"s1","agent_type":"implementer"}"#;
    assert_eq!(run_hook(&hook, &env, ev).0, 2);
    assert_eq!(run_hook(&hook, &env, ev).0, 2);
    let (code, out, _) = run_hook(&hook, &env, ev);
    assert_eq!(code, 0);
    assert!(out.contains("UNRESOLVED"));
    let log = read(dir.path(), ".claude/loop-guard/escalations.md");
    assert!(log.contains("data loss"), "{log}");
}

#[test]
fn loop_guard_never_releases_the_plan_gate() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-task-created.sh");
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_PLAN_GATE", "1"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let ev = r#"{"session_id":"s1","agent_type":"implementer"}"#;
    for _ in 0..2 {
        assert_eq!(run_hook(&hook, &env, ev).0, 2);
    }
    for _ in 0..3 {
        let (code, _, err) = run_hook(&hook, &env, ev);
        assert_eq!(code, 2, "an unapproved plan is never released");
        assert!(err.contains("STOP retrying"), "{err}");
    }
    let log = read(dir.path(), ".claude/loop-guard/escalations.md");
    assert_eq!(log.lines().count(), 1, "{log}");
}

#[test]
fn loop_guard_halts_repeated_high_impact_attempts_without_allowing() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-approval-gate.sh");
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_APPROVAL_GATE", "1"),
        ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ];
    let push =
        r#"{"session_id":"s1","tool_name":"Bash","tool_input":{"command":"git push origin main"}}"#;
    assert_eq!(run_hook(&hook, &env, push).0, 2);
    assert_eq!(run_hook(&hook, &env, push).0, 2);
    // Budget spent: halt the agent — but the command is still denied, never allowed.
    let (code, out, _) = run_hook(&hook, &env, push);
    assert_eq!(code, 0);
    assert!(out.contains(r#""permissionDecision": "deny""#), "{out}");
    assert!(out.contains(r#""continue": false"#), "{out}");
    // Self-approval is still blocked outright.
    let forge =
        r#"{"session_id":"s2","tool_name":"Bash","tool_input":{"command":"ocgen approve"}}"#;
    assert_eq!(run_hook(&hook, &env, forge).0, 2);
}

#[test]
fn loop_guard_zero_budget_is_unbounded() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    let hook = dir.path().join(".claude/hooks/team-task-completed.sh");
    let proj = dir.path().to_str().unwrap();
    let env = [
        ("CLAUDE_PROJECT_DIR", proj),
        ("TEAM_CONFIDENCE_THRESHOLD", "96"),
        ("LOOP_GUARD_MAX_BLOCKS", "0"),
    ];
    let ev = r#"{"session_id":"s1","task_id":"t","summary":"no rating"}"#;
    for _ in 0..6 {
        assert_eq!(run_hook(&hook, &env, ev).0, 2);
    }
    assert!(!dir.path().join(".claude/loop-guard").exists());
}

#[test]
fn loop_guard_renders_library_budget_turn_caps_and_discipline() {
    let dir = tempdir().unwrap();
    governed_claude_project(dir.path());
    assert!(dir.path().join(".claude/hooks/loop-guard.sh").exists());
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["env"]["LOOP_GUARD_MAX_BLOCKS"], "3");
    assert!(read(dir.path(), ".claude/agents/implementer.md").contains("maxTurns: 60"));
    assert!(read(dir.path(), ".claude/agents/explorer.md").contains("maxTurns: 40"));
    assert!(read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("Loop discipline"));
    assert!(read(dir.path(), ".claude/skills/deliver/SKILL.md").contains("at most 2 re-plans"));

    // No blocking gate at all → no library and no budget env.
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "nolg".into();
    p.claude.workflow.subagent_confidence = 0;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir2 = tempdir().unwrap();
    p.scaffold(dir2.path(), false).unwrap();
    assert!(!dir2.path().join(".claude/hooks/loop-guard.sh").exists());
    assert!(!read(dir2.path(), ".claude/settings.json").contains("LOOP_GUARD_MAX_BLOCKS"));
}

#[test]
fn workflow_without_loop_guard_field_backfills_three() {
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert_eq!(wf.loop_guard_max, 3);
}

// ------------------------------------------------------------ worktrees -----

#[test]
fn worktreeinclude_carries_claude_config_but_not_live_or_runtime_dirs() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "wti".into();
    p.claude.workflow.fanout = false; // /deliver alone still needs worktree support
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let wti = read(dir.path(), ".worktreeinclude");
    for want in [
        ".env",
        ".claude/settings.json",
        ".claude/rules/*.md",
        ".claude/hooks/*.sh",
        ".claude/output-styles/*.md",
    ] {
        assert!(
            wti.lines().any(|l| l.trim() == want),
            "missing {want}:\n{wti}"
        );
    }
    // Agents/commands/skills are read through live from the main checkout; runtime
    // state must never be copied into a worktree.
    for never in [
        ".claude/agents",
        ".claude/commands",
        ".claude/skills",
        "notes",
        "loop-guard",
        "worktrees",
    ] {
        assert!(
            !wti.lines()
                .any(|l| !l.starts_with('#') && l.contains(never)),
            "must not copy {never}"
        );
    }
}

#[test]
fn fanout_worktrees_branch_from_current_head() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "head".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["worktree"]["baseRef"], "head");
}

#[test]
fn detects_git_ignored_claude_config() {
    use ocgen::gitcheck::claude_config_ignored;
    use std::process::Command;

    let dir = tempdir().unwrap();
    // Not a repository → unknown.
    assert_eq!(claude_config_ignored(dir.path()), None);

    Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(claude_config_ignored(dir.path()), Some(false));

    // Only runtime dirs ignored → config is still committable.
    fs::write(
        dir.path().join(".gitignore"),
        ".claude/worktrees/\n.claude/notes/\n",
    )
    .unwrap();
    assert_eq!(claude_config_ignored(dir.path()), Some(false));

    // The whole .claude/ ignored → worktree sessions would lose settings and hooks.
    fs::write(dir.path().join(".gitignore"), ".claude/*\n").unwrap();
    assert_eq!(claude_config_ignored(dir.path()), Some(true));
}

#[test]
fn worktree_guidance_in_rules_and_inquire() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "wtg".into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let wf = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(
        wf.contains("commit `.claude/`") && wf.contains("baseRef"),
        "{wf}"
    );
    let inq = read(dir.path(), ".claude/skills/inquire/SKILL.md");
    assert!(inq.contains(".claude/worktrees/") && inq.contains("main checkout"));
}

// --------------------------------------------------------------- skills -----

/// A skill that follows every "what makes a skill good" rule.
fn good_skill() -> Skill {
    Skill {
        name: "tf-plan-review".into(),
        description: "Review a Terraform plan for S3/KMS misconfigurations. Use when asked to check a plan, a bucket policy, or encryption settings.".into(),
        allowed_tools: "Read, Grep, Bash(terraform show:*)".into(),
        body: "1. Run `terraform show -json`.\n2. Check it against reference.md.\n3. Report findings.\n".into(),
        ..Default::default()
    }
}

fn has(issues: &[String], needle: &str) -> bool {
    issues.iter().any(|i| i.contains(needle))
}

#[test]
fn good_skill_has_no_issues() {
    use ocgen::claude::skill_issues;
    assert!(
        skill_issues(&good_skill()).is_empty(),
        "{:?}",
        skill_issues(&good_skill())
    );
}

#[test]
fn skill_issues_flag_each_rule() {
    use ocgen::claude::skill_issues;
    let with = |f: &dyn Fn(&mut Skill)| {
        let mut s = good_skill();
        f(&mut s);
        skill_issues(&s)
    };
    // Name: lowercase letters, digits, hyphens, ≤ 64.
    assert!(has(&with(&|s| s.name = "Tf_Plan".into()), "name"));
    assert!(has(&with(&|s| s.name = "a".repeat(65)), "name"));
    // The description is the trigger.
    assert!(has(&with(&|s| s.description.clear()), "description"));
    assert!(has(
        &with(&|s| s.description = "TF helper".into()),
        "too short"
    ));
    assert!(has(&with(&|s| s.description = "x".repeat(1025)), "1024"));
    let no_when = "Reviews Terraform plans for S3 and KMS misconfigurations in detail.";
    assert!(has(&with(&|s| s.description = no_when.into()), "when"));
    // ...but a when_to_use line satisfies it, and user-run skills need no trigger.
    assert!(!has(
        &with(&|s| {
            s.description = no_when.into();
            s.when_to_use = "Use when asked to check a plan.".into();
        }),
        "when"
    ));
    assert!(!has(
        &with(&|s| {
            s.description = no_when.into();
            s.disable_model_invocation = true;
        }),
        "when"
    ));
    // Keep SKILL.md short.
    assert!(has(
        &with(&|s| s.body = "line\n".repeat(501)),
        "reference.md"
    ));
    // Fewest tools.
    assert!(has(
        &with(&|s| s.allowed_tools = "Read, Bash".into()),
        "Bash("
    ));
    assert!(has(
        &with(&|s| s.allowed_tools = "Read, Write".into()),
        "user-run"
    ));
    assert!(!has(
        &with(&|s| {
            s.allowed_tools = "Read, Write".into();
            s.disable_model_invocation = true;
        }),
        "user-run"
    ));
    // Contradiction: nobody can invoke it.
    assert!(has(
        &with(&|s| {
            s.disable_model_invocation = true;
            s.hidden_from_menu = true;
        }),
        "nobody"
    ));
}

#[test]
fn landscape_checks_include_skill_issues() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(Skill {
        name: "helper".into(),
        description: "TF helper".into(),
        body: "do it".into(),
        ..Default::default()
    });
    let issues = p.issues();
    assert!(
        issues.iter().any(|i| i.starts_with("skill 'helper'")),
        "{issues:?}"
    );
}

#[test]
fn every_preset_says_when_to_choose_it_and_seeds_a_good_skill() {
    use ocgen::claude::{skill_issues, skill_presets};
    for preset in skill_presets().unwrap() {
        assert!(
            !preset.choose_when.trim().is_empty(),
            "{} lacks choose_when",
            preset.name
        );
        let seeded = preset.to_skill("my-skill", "English");
        let issues = skill_issues(&seeded);
        assert!(
            issues.is_empty(),
            "preset {} seeds issues: {issues:?}",
            preset.name
        );
    }
}

#[test]
fn skill_extras_are_created_once_and_survive_regeneration() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(good_skill());
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let made = Project::scaffold_skill_extras(dir.path(), "tf-plan-review", true, true).unwrap();
    assert_eq!(made.len(), 2);
    let reference = dir
        .path()
        .join(".claude/skills/tf-plan-review/reference.md");
    let scripts = dir
        .path()
        .join(".claude/skills/tf-plan-review/scripts/README.md");
    assert!(reference.exists() && scripts.exists());

    // Never overwrites the user's content.
    fs::write(&reference, "my checklist\n").unwrap();
    let again = Project::scaffold_skill_extras(dir.path(), "tf-plan-review", true, true).unwrap();
    assert!(again.is_empty());
    assert_eq!(fs::read_to_string(&reference).unwrap(), "my checklist\n");

    // doctor-style forced regeneration keeps them.
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(fs::read_to_string(&reference).unwrap(), "my checklist\n");
    assert!(scripts.exists());
}

#[test]
fn renaming_a_skill_carries_its_supporting_files() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.skills.push(good_skill());
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let old = dir.path().join(".claude/skills/tf-plan-review");
    fs::write(old.join("reference.md"), "mine\n").unwrap();
    fs::create_dir_all(old.join("scripts")).unwrap();
    fs::write(old.join("scripts/check.sh"), "echo ok\n").unwrap();

    // The wizard renames in state, re-scaffolds, then moves the old dir's extras.
    p.skills[0].name = "plan-review".into();
    p.scaffold(dir.path(), true).unwrap();
    Project::rename_claude_skill_dir(dir.path(), "tf-plan-review", "plan-review").unwrap();

    let new = dir.path().join(".claude/skills/plan-review");
    assert_eq!(
        fs::read_to_string(new.join("reference.md")).unwrap(),
        "mine\n"
    );
    assert!(new.join("scripts/check.sh").exists());
    assert!(read(dir.path(), ".claude/skills/plan-review/SKILL.md").contains("name: plan-review"));
    assert!(!old.exists(), "old skill dir removed");
}

// ------------------------------------------------------ tier 1 defects -----

fn claude_default(name: &str) -> Project {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

#[test]
fn output_style_keeps_coding_instructions_and_does_not_shadow_builtin() {
    let dir = tempdir().unwrap();
    // A project generated by an older ocgen, plus a user-edited style elsewhere.
    fs::create_dir_all(dir.path().join(".claude/output-styles")).unwrap();
    fs::write(
        dir.path().join(".claude/output-styles/concise.md"),
        "---\nname: Concise\ndescription: Lead with the result; minimal preamble.\n---\n\
         Respond concisely. Lead each answer with the result or recommendation. Skip\n",
    )
    .unwrap();
    claude_default("os").scaffold(dir.path(), true).unwrap();

    let style = read(dir.path(), ".claude/output-styles/ocgen-concise.md");
    assert!(style.contains("name: ocgen-concise"));
    assert!(style.contains("keep-coding-instructions: true"));
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["outputStyle"], "ocgen-concise");
    assert!(
        !dir.path().join(".claude/output-styles/concise.md").exists(),
        "stale generated style removed so it stops shadowing the built-in Concise"
    );

    // A concise.md the user wrote themselves is never deleted.
    fs::write(
        dir.path().join(".claude/output-styles/concise.md"),
        "---\nname: Mine\n---\nmy style\n",
    )
    .unwrap();
    claude_default("os").scaffold(dir.path(), true).unwrap();
    assert!(dir.path().join(".claude/output-styles/concise.md").exists());
}

#[test]
fn full_model_ids_are_valid_and_survive_doctor() {
    use ocgen::validate::claude_model;
    for ok in [
        "opus",
        "sonnet",
        "haiku",
        "fable",
        "inherit",
        "claude-opus-5-5",
        "claude-sonnet-5-5[1m]",
        "claude-haiku-4-5-20251001",
    ] {
        assert!(claude_model(ok).is_ok(), "{ok}");
    }
    for bad in ["gemma", "gpt-4", "", "Claude-Opus", "claude-"] {
        assert!(claude_model(bad).is_err(), "{bad}");
    }
    let mut p = claude_default("ids");
    p.agents[1].model = "claude-opus-5-5".into();
    assert!(
        !p.issues().iter().any(|i| i.contains("model")),
        "{:?}",
        p.issues()
    );
    p.doctor();
    assert_eq!(p.agents[1].model, "claude-opus-5-5");
}

#[test]
fn settings_have_schema_and_deny_reading_secrets() {
    let dir = tempdir().unwrap();
    claude_default("sec").scaffold(dir.path(), false).unwrap();
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(
        settings["$schema"],
        "https://json.schemastore.org/claude-code-settings.json"
    );
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for want in [
        "Read(./.env)",
        "Read(./**/*.pem)",
        "Read(./**/*.key)",
        "Read(./**/*.tfstate)",
    ] {
        assert!(deny.contains(&want), "missing {want}: {deny:?}");
    }
    // Example files stay readable.
    assert!(!deny
        .iter()
        .any(|d| d.contains("tfvars") || d.contains(".env.*")));
}

#[test]
fn plugin_ships_the_same_gates_as_the_project() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("gates");
    p.claude.output = Output {
        project: false,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "gates".into();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    p.scaffold(dir.path(), false).unwrap();
    let base = dir.path().join("plugin/gates");
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(base.join("hooks/hooks.json")).unwrap()).unwrap();
    for ev in [
        "SubagentStop",
        "PreToolUse",
        "TaskCompleted",
        "TaskCreated",
        "TeammateIdle",
    ] {
        let cmd = hooks["hooks"][ev][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default();
        assert!(cmd.contains("${CLAUDE_PLUGIN_ROOT}/hooks/"), "{ev}: {cmd}");
        assert!(
            cmd.contains("LOOP_GUARD_MAX_BLOCKS='3'"),
            "{ev} gets its budget: {cmd}"
        );
    }
    let stop = hooks["hooks"]["SubagentStop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(stop.contains("SUBAGENT_CONFIDENCE_THRESHOLD='96'"));
    let gate = hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(gate.contains("TEAM_APPROVAL_GATE='1'"));
    for script in [
        "loop-guard.sh",
        "subagent-confidence-gate.sh",
        "team-approval-gate.sh",
    ] {
        assert!(base.join("hooks").join(script).is_file(), "{script}");
    }
}

#[test]
fn skill_description_length_limits() {
    use ocgen::claude::skill_issues;
    let mut s = good_skill();
    s.description = format!("Use when checking plans. {}", "x".repeat(1100));
    let issues = skill_issues(&s);
    assert!(has(&issues, "1024") && !has(&issues, "1536"), "{issues:?}");
    s.when_to_use = "y".repeat(500);
    assert!(has(&skill_issues(&s), "1536"));
}

// --------------------------------------------------- subagent fields + MCP -----

#[test]
fn agent_frontmatter_renders_every_new_field() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("fm");
    let i = p
        .agents
        .iter()
        .position(|a| a.name == "implementer")
        .unwrap();
    {
        let a = &mut p.agents[i];
        a.disallowed_tools = "WebFetch".into();
        a.permission_mode = "acceptEdits".into();
        a.effort = "high".into();
        a.memory = "project".into();
        a.preload_skills = "tf-plan-review, commit".into();
        a.background = true;
    }
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/agents/implementer.md");
    for line in [
        "disallowedTools: WebFetch",
        "permissionMode: acceptEdits",
        "effort: high",
        "memory: project",
        "skills:\n  - tf-plan-review\n  - commit",
        "background: true",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
}

#[test]
fn read_only_roles_are_enforced_and_reviewer_thinks_harder() {
    let dir = tempdir().unwrap();
    claude_default("ro").scaffold(dir.path(), false).unwrap();
    for role in ["explorer", "reviewer"] {
        let md = read(dir.path(), &format!(".claude/agents/{role}.md"));
        assert!(
            md.contains("disallowedTools: Edit, Write, NotebookEdit"),
            "{role}"
        );
    }
    assert!(read(dir.path(), ".claude/agents/reviewer.md").contains("effort: high"));
    let imp = read(dir.path(), ".claude/agents/implementer.md");
    assert!(!imp.contains("disallowedTools") && !imp.contains("effort:"));
    // Opt-in fields stay off by default.
    assert!(!imp.contains("memory:") && !imp.contains("permissionMode:"));
}

#[test]
fn old_agent_state_backfills_no_new_fields() {
    let a: Agent =
        serde_json::from_str(r#"{"name":"x","mode":"subagent","model":"opus"}"#).unwrap();
    assert!(a.disallowed_tools.is_empty() && a.effort.is_empty() && a.mcp_servers.is_empty());
    assert!(!a.background);
}

#[test]
fn disallowed_write_makes_a_role_read_only_for_risk_rounds() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("rr");
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: false,
        confidence_threshold: 0,
        risk_rounds: true,
        approval_gate: false,
    };
    // A role with every tool (empty list) but Edit/Write disallowed is read-only.
    let mut auditor = Agent::blank("auditor", "custom", "");
    auditor.mode = "subagent".into();
    auditor.model = "opus".into();
    auditor.disallowed_tools = "Edit, Write".into();
    p.agents.push(auditor);
    p.scaffold(dir.path(), false).unwrap();
    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert!(s["env"]["TEAM_READONLY_ROLES"]
        .as_str()
        .unwrap()
        .contains("auditor"));
}

#[test]
fn invalid_agent_enums_are_flagged_and_doctor_clears_them() {
    let mut p = claude_default("en");
    let i = 1;
    p.agents[i].effort = "extreme".into();
    p.agents[i].permission_mode = "yolo".into();
    p.agents[i].memory = "cloud".into();
    let issues = p.issues();
    for want in ["effort", "permission mode", "memory"] {
        assert!(
            issues.iter().any(|x| x.contains(want)),
            "{want}: {issues:?}"
        );
    }
    p.doctor();
    assert!(
        p.agents[i].effort.is_empty()
            && p.agents[i].permission_mode.is_empty()
            && p.agents[i].memory.is_empty()
    );
    // bypassPermissions is valid but called out.
    p.agents[i].permission_mode = "bypassPermissions".into();
    assert!(p.issues().iter().any(|x| x.contains("bypassPermissions")));
}

fn stdio_server() -> ocgen::claude::McpServer {
    ocgen::claude::McpServer {
        name: "tf".into(),
        transport: "stdio".into(),
        command: "npx".into(),
        args: vec!["-y".into(), "terraform-mcp".into()],
        env: [("TF_TOKEN".to_string(), "${TF_TOKEN}".to_string())]
            .into_iter()
            .collect(),
        pre_approve: true,
        ..Default::default()
    }
}

#[test]
fn mcp_json_and_pre_approval() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("mcp");
    p.claude.mcp_servers.push(stdio_server());
    p.claude.mcp_servers.push(ocgen::claude::McpServer {
        name: "docs".into(),
        transport: "http".into(),
        url: "https://mcp.example.com/mcp".into(),
        headers: [(
            "Authorization".to_string(),
            "Bearer ${DOCS_TOKEN}".to_string(),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    p.scaffold(dir.path(), false).unwrap();
    let mcp: serde_json::Value = serde_json::from_str(&read(dir.path(), ".mcp.json")).unwrap();
    assert_eq!(mcp["mcpServers"]["tf"]["type"], "stdio");
    assert_eq!(mcp["mcpServers"]["tf"]["command"], "npx");
    assert_eq!(mcp["mcpServers"]["tf"]["args"][1], "terraform-mcp");
    assert_eq!(mcp["mcpServers"]["tf"]["env"]["TF_TOKEN"], "${TF_TOKEN}");
    assert_eq!(mcp["mcpServers"]["docs"]["type"], "http");
    assert_eq!(
        mcp["mcpServers"]["docs"]["url"],
        "https://mcp.example.com/mcp"
    );
    assert!(mcp["mcpServers"]["docs"].get("command").is_none());
    let s: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    let approved = s["enabledMcpjsonServers"].as_array().unwrap();
    assert_eq!(approved.len(), 1, "only pre-approved servers");
    assert_eq!(approved[0], "tf");
    assert!(p.issues().is_empty(), "{:?}", p.issues());

    // No servers → no .mcp.json and no approval key.
    let dir2 = tempdir().unwrap();
    claude_default("nomcp")
        .scaffold(dir2.path(), false)
        .unwrap();
    assert!(!dir2.path().join(".mcp.json").exists());
    assert!(!read(dir2.path(), ".claude/settings.json").contains("enabledMcpjsonServers"));
}

#[test]
fn agent_mcp_servers_render_and_extend_a_tools_allowlist() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("amcp");
    p.claude.mcp_servers.push(stdio_server());
    let i = p.agents.iter().position(|a| a.name == "explorer").unwrap();
    p.agents[i].mcp_servers = "tf".into();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/agents/explorer.md");
    assert!(md.contains("mcpServers:\n  - tf"), "{md}");
    assert!(
        md.contains("tools: Read, Grep, Glob, WebFetch, WebSearch, mcp__tf"),
        "{md}"
    );
}

#[test]
fn mcp_checks_catch_unknown_servers_and_literal_secrets() {
    let mut p = claude_default("chk");
    p.agents[1].mcp_servers = "ghost".into();
    let mut leaky = stdio_server();
    leaky.env.insert("API_KEY".into(), "sk-live-123".into());
    p.claude.mcp_servers.push(leaky);
    let issues = p.issues();
    assert!(issues.iter().any(|i| i.contains("ghost")), "{issues:?}");
    assert!(
        issues
            .iter()
            .any(|i| i.contains("API_KEY") && i.contains("${")),
        "{issues:?}"
    );
}

#[test]
fn kv_lists_parse_and_round_trip() {
    use ocgen::claude::{format_kv_list, parse_kv_list};
    let m = parse_kv_list("A=1, B=${B} ,C=x=y").unwrap();
    assert_eq!(m["A"], "1");
    assert_eq!(m["B"], "${B}");
    assert_eq!(m["C"], "x=y");
    assert_eq!(parse_kv_list(&format_kv_list(&m)).unwrap(), m);
    assert!(parse_kv_list("").unwrap().is_empty());
    assert!(parse_kv_list("novalue").is_err());
}

// ------------------------------------------------ hooks library + skills -----

fn hook_groups(settings: &serde_json::Value, event: &str) -> Vec<serde_json::Value> {
    settings["hooks"][event]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn settings_of(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&read(dir, ".claude/settings.json")).unwrap()
}

#[test]
fn compaction_reinjects_context_by_default() {
    let dir = tempdir().unwrap();
    claude_default("cc").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let compact = hook_groups(&s, "SessionStart")
        .into_iter()
        .find(|g| g["matcher"] == "compact")
        .expect("a SessionStart group matching compact");
    let cmd = compact["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains(".claude/notes") && cmd.contains("rules"),
        "{cmd}"
    );
}

#[test]
fn optional_hooks_are_wired_with_their_scripts() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("hx");
    p.claude.hooks_extra.notify = true;
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    p.claude.hooks_extra.config_audit = true;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    for ev in ["Notification", "StopFailure"] {
        let cmd = hook_groups(&s, ev)[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cmd.contains("notify.sh"), "{ev}: {cmd}");
    }
    let fmt = &hook_groups(&s, "PostToolUse")[0];
    assert_eq!(fmt["matcher"], "Edit|Write");
    assert!(fmt["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("OCGEN_FORMAT_CMD='cargo fmt'"));
    assert!(hook_groups(&s, "ConfigChange")[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("config-audit.sh"));
    for script in ["notify.sh", "format.sh", "config-audit.sh"] {
        let path = dir.path().join(".claude/hooks").join(script);
        let ok = std::process::Command::new("sh")
            .arg("-n")
            .arg(&path)
            .status()
            .unwrap();
        assert!(ok.success(), "{script} parses");
    }

    // Off by default: none of them (PostToolUse only has the /inquire notes view).
    let dir2 = tempdir().unwrap();
    claude_default("hoff").scaffold(dir2.path(), false).unwrap();
    let s2 = settings_of(dir2.path());
    for ev in ["Notification", "ConfigChange"] {
        assert!(hook_groups(&s2, ev).is_empty(), "{ev} off by default");
    }
    assert!(!hook_groups(&s2, "PostToolUse")
        .iter()
        .any(|g| g.to_string().contains("format.sh")));
    assert!(!dir2.path().join(".claude/hooks/notify.sh").exists());
}

#[test]
fn format_hook_never_blocks_and_audit_log_ignores_itself() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("fb");
    p.claude.hooks_extra.format_cmd = "false".into();
    p.claude.hooks_extra.config_audit = true;
    p.scaffold(dir.path(), false).unwrap();
    let proj = dir.path().to_str().unwrap();
    let (code, _, err) = run_hook(
        &dir.path().join(".claude/hooks/format.sh"),
        &[("CLAUDE_PROJECT_DIR", proj), ("OCGEN_FORMAT_CMD", "false")],
        r#"{"tool_name":"Edit"}"#,
    );
    assert_eq!(code, 0, "a failing formatter never blocks");
    assert!(err.contains("failed"));
    let (code, _, _) = run_hook(
        &dir.path().join(".claude/hooks/config-audit.sh"),
        &[("CLAUDE_PROJECT_DIR", proj)],
        r#"{"hook_event_name":"ConfigChange","source":"project_settings"}"#,
    );
    assert_eq!(code, 0);
    assert!(read(dir.path(), ".claude/audit/config-changes.log").contains("project_settings"));
    assert_eq!(read(dir.path(), ".claude/audit/.gitignore"), "*\n");
}

#[test]
fn plugin_gets_the_optional_hooks_too() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("ph");
    p.claude.output = Output {
        project: false,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "ph".into();
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    p.scaffold(dir.path(), false).unwrap();
    let base = dir.path().join("plugin/ph");
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(base.join("hooks/hooks.json")).unwrap()).unwrap();
    let cmd = hooks["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        cmd.contains("${CLAUDE_PLUGIN_ROOT}/hooks/format.sh"),
        "{cmd}"
    );
    assert!(base.join("hooks/format.sh").is_file());
}

#[test]
fn workflow_commands_are_generated_as_skills() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("wf");
    p.claude.team.enabled = true;
    p.claude.team.plan_gate = true;
    p.scaffold(dir.path(), false).unwrap();
    for (name, user_only) in [
        ("deliver", true),
        ("fanout", true),
        ("multi", true),
        ("team", true),
        ("team-plan", true),
        ("inquire", false),
        ("intent", true),
        ("intake", false),
        ("refine", false),
        ("improve-prompt", false),
    ] {
        let md = read(dir.path(), &format!(".claude/skills/{name}/SKILL.md"));
        assert!(
            md.starts_with(&format!("---\nname: {name}\n")),
            "{name}:\n{md}"
        );
        assert_eq!(
            md.contains("disable-model-invocation: true"),
            user_only,
            "{name}"
        );
        assert!(md.contains("description:"), "{name}");
    }
    assert!(
        !dir.path().join(".claude/commands").exists(),
        "no legacy commands dir"
    );
}

#[test]
fn stale_generated_commands_are_removed_but_user_commands_kept() {
    let dir = tempdir().unwrap();
    let cmds = dir.path().join(".claude/commands");
    fs::create_dir_all(&cmds).unwrap();
    fs::write(cmds.join("deliver.md"), "---\ndescription: Take a complex task end-to-end\nargument-hint: \"[the goal]\"\n---\nold\n").unwrap();
    fs::write(
        cmds.join("my-own.md"),
        "---\ndescription: mine\n---\nkeep me\n",
    )
    .unwrap();
    fs::write(cmds.join("inquire.md"), "hand-written, no frontmatter\n").unwrap();
    claude_default("st").scaffold(dir.path(), true).unwrap();
    assert!(
        !cmds.join("deliver.md").exists(),
        "old generated command removed"
    );
    assert!(cmds.join("my-own.md").exists(), "user command kept");
    assert!(
        cmds.join("inquire.md").exists(),
        "a hand-written file with a workflow name is kept"
    );
}

#[test]
fn user_skills_cannot_take_a_workflow_name() {
    assert!(ocgen::validate::skill_name("deliver").is_err());
    assert!(ocgen::validate::skill_name("team-plan").is_err());
    let mut p = claude_default("rsv");
    let mut s = good_skill();
    s.name = "inquire".into();
    p.skills.push(s);
    assert!(
        p.issues()
            .iter()
            .any(|i| i.contains("inquire") && i.contains("generated")),
        "{:?}",
        p.issues()
    );
}

#[test]
fn full_skill_frontmatter_renders_and_is_checked() {
    use ocgen::claude::skill_issues;
    let dir = tempdir().unwrap();
    let mut p = claude_default("sf");
    let mut s = good_skill();
    s.paths = "**/*.tf **/*.tfvars".into();
    s.arguments = "plan_file".into();
    s.disallowed_tools = "Write".into();
    s.effort = "high".into();
    s.context_fork = true;
    s.agent = "Explore".into();
    s.background = true;
    p.skills.push(s.clone());
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/tf-plan-review/SKILL.md");
    for line in [
        "paths:\n  - \"**/*.tf\"\n  - \"**/*.tfvars\"",
        "arguments: plan_file",
        "disallowed-tools: Write",
        "effort: high",
        "background: true",
    ] {
        assert!(md.contains(line), "missing {line:?} in:\n{md}");
    }
    assert!(skill_issues(&s).is_empty(), "{:?}", skill_issues(&s));
    // background only makes sense for a forked skill; effort must be a real level.
    let mut bad = s.clone();
    bad.context_fork = false;
    bad.effort = "turbo".into();
    let issues = skill_issues(&bad);
    assert!(
        has(&issues, "background") && has(&issues, "effort"),
        "{issues:?}"
    );
}

// ------------------------------------------------------------- statusline -----

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn statusline_payload(dir: &Path) -> String {
    // Forward slashes keep the JSON valid on Windows (`C:\Users` is a bad escape).
    let d = ocgen::paths::for_shell(dir);
    format!(
        r#"{{"model":{{"id":"claude-opus-5-5","display_name":"Opus 5.5"}},"workspace":{{"current_dir":"{d}","project_dir":"{d}"}},"cost":{{"total_cost_usd":0.4213}},"context_window":{{"current_usage":{{"input_tokens":5}},"used_percentage":42,"remaining_percentage":58}},"rate_limits":{{"five_hour":{{"used_percentage":90}}}},"worktree":{{"name":"feature-a","path":"x","branch":"worktree-feature-a"}}}}"#
    )
}

#[test]
fn statusline_script_is_wired_for_the_project_only() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("sl");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "sl".into();
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let cmd = s["statusLine"]["command"].as_str().unwrap();
    assert!(cmd.contains(".claude/statusline.sh"), "{cmd}");
    assert!(dir.path().join(".claude/statusline.sh").is_file());
    // Plugins can't set statusLine, so the plugin doesn't carry the script.
    assert!(!dir.path().join("plugin/sl/statusline.sh").exists());
}

#[test]
fn statusline_shows_model_place_context_cost_worktree_and_escalations() {
    let dir = tempdir().unwrap();
    claude_default("slr").scaffold(dir.path(), false).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "trunk"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::create_dir_all(dir.path().join(".claude/loop-guard")).unwrap();
    fs::write(
        dir.path().join(".claude/loop-guard/escalations.md"),
        "- t [subagent-confidence] a1: x\n- t [task-confidence] t2: y\n",
    )
    .unwrap();
    let script = dir.path().join(".claude/statusline.sh");
    for no_jq in ["", "1"] {
        let (code, out, _) = run_hook(
            &script,
            &[("OCGEN_STATUSLINE_NO_JQ", no_jq)],
            &statusline_payload(dir.path()),
        );
        assert_eq!(code, 0);
        let line = strip_ansi(&out);
        let dir_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        for want in [
            "Opus 5.5",
            dir_name.as_str(),
            "(trunk)",
            "58% left",
            "$0.42",
            "wt:feature-a",
            "2 unresolved",
        ] {
            assert!(
                line.contains(want),
                "no_jq={no_jq:?}: missing {want:?} in {line:?}"
            );
        }
    }
}

#[test]
fn statusline_survives_a_sparse_payload() {
    let dir = tempdir().unwrap();
    claude_default("sls").scaffold(dir.path(), false).unwrap();
    let script = dir.path().join(".claude/statusline.sh");
    for no_jq in ["", "1"] {
        let (code, out, _) = run_hook(&script, &[("OCGEN_STATUSLINE_NO_JQ", no_jq)], "{}");
        assert_eq!(code, 0, "no_jq={no_jq:?}");
        let line = strip_ansi(&out);
        assert!(
            !line.contains("unresolved") && !line.contains("null"),
            "{line:?}"
        );
    }
}

// ------------------------------------------------------------ safe doctor -----

#[test]
fn plan_classifies_added_modified_unchanged_and_stale() {
    use ocgen::render::ChangeKind;
    let dir = tempdir().unwrap();
    claude_default("plan").scaffold(dir.path(), false).unwrap();
    // `doctor` works on the saved state, which carries the fingerprints.
    let mut p = Project::load_state(dir.path()).unwrap();
    assert!(
        p.plan_changes(dir.path())
            .unwrap()
            .iter()
            .all(|c| c.kind == ChangeKind::Unchanged),
        "fresh scaffold → nothing to change"
    );

    // A hand edit, an ocgen-side change, a deleted file, and a stale legacy command.
    fs::write(dir.path().join(".claude/settings.json"), "{}\n").unwrap();
    p.agents[1].description = "Explores differently now".into();
    fs::remove_file(dir.path().join(".claude/statusline.sh")).unwrap();
    fs::create_dir_all(dir.path().join(".claude/commands")).unwrap();
    fs::write(
        dir.path().join(".claude/commands/deliver.md"),
        "---\ndescription: old\n---\nx\n",
    )
    .unwrap();

    let plan = p.plan_changes(dir.path()).unwrap();
    let find = |rel: &str| {
        plan.iter()
            .find(|c| c.path.ends_with(rel))
            .unwrap_or_else(|| panic!("{rel}"))
    };
    let settings = find(".claude/settings.json");
    assert_eq!(settings.kind, ChangeKind::Modified);
    assert_eq!(
        settings.hand_edited,
        Some(true),
        "the user changed what ocgen wrote"
    );
    let explorer = find(".claude/agents/explorer.md");
    assert_eq!(explorer.kind, ChangeKind::Modified);
    assert_eq!(
        explorer.hand_edited,
        Some(false),
        "ocgen's own change, not a hand edit"
    );
    assert_eq!(find(".claude/statusline.sh").kind, ChangeKind::Added);
    assert_eq!(
        find(".claude/commands/deliver.md").kind,
        ChangeKind::Removed
    );
    // CLAUDE.md is the user's and never part of the plan.
    assert!(!plan.iter().any(|c| c.path.ends_with("CLAUDE.md")));
}

#[test]
fn backup_keeps_old_versions_and_prunes_to_five() {
    let dir = tempdir().unwrap();
    let p = claude_default("bk");
    p.scaffold(dir.path(), false).unwrap();
    fs::write(
        dir.path().join(".claude/settings.json"),
        "{\"mine\":true}\n",
    )
    .unwrap();
    let plan = p.plan_changes(dir.path()).unwrap();
    let bk = Project::backup(dir.path(), &plan)
        .unwrap()
        .expect("a backup was needed");
    assert_eq!(
        fs::read_to_string(bk.join(".claude/settings.json")).unwrap(),
        "{\"mine\":true}\n"
    );
    assert_eq!(read(dir.path(), ".ocgen-backup/.gitignore"), "*\n");
    // Nothing to back up → no new backup.
    p.scaffold(dir.path(), true).unwrap();
    assert!(
        Project::backup(dir.path(), &p.plan_changes(dir.path()).unwrap())
            .unwrap()
            .is_none()
    );
    // Only the newest five are kept.
    for i in 0..7 {
        fs::create_dir_all(dir.path().join(format!(".ocgen-backup/20000101-00000{i}"))).unwrap();
    }
    Project::prune_backups(dir.path(), 5).unwrap();
    let kept = fs::read_dir(dir.path().join(".ocgen-backup"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().is_dir())
        .count();
    assert_eq!(kept, 5);
    assert!(bk.exists(), "the real (newest) backup survives pruning");
}

#[test]
fn line_diff_reports_changes_with_context() {
    use ocgen::diff::{line_diff, DiffLine};
    let old = "a\nb\nc\nd\n";
    let new = "a\nB\nc\nd\ne\n";
    let d = line_diff(old, new);
    assert!(d.contains(&DiffLine::Removed("b".into())));
    assert!(d.contains(&DiffLine::Added("B".into())));
    assert!(d.contains(&DiffLine::Added("e".into())));
    assert!(d.contains(&DiffLine::Same("a".into())));
    assert!(line_diff("x\n", "x\n")
        .iter()
        .all(|l| matches!(l, DiffLine::Same(_))));
}

// ------------------------------------------------------ defence in depth -----

#[test]
fn high_impact_commands_always_ask_first() {
    let dir = tempdir().unwrap();
    claude_default("ask").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let ask: Vec<&str> = s["permissions"]["ask"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for want in [
        "Bash(git push:*)",
        "Bash(terraform apply*)",
        "Bash(kubectl delete*)",
        "Bash(npm publish*)",
        "Bash(ssh *)",
    ] {
        assert!(ask.contains(&want), "missing {want}: {ask:?}");
    }
}

#[test]
fn sandbox_profile_is_opt_in() {
    let dir = tempdir().unwrap();
    claude_default("sbx0").scaffold(dir.path(), false).unwrap();
    assert!(
        settings_of(dir.path()).get("sandbox").is_none(),
        "off by default"
    );

    let dir = tempdir().unwrap();
    let mut p = claude_default("sbx1");
    p.claude.sandbox.enabled = true;
    p.claude.sandbox.extra_domains = vec!["artifacts.example.com".into()];
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert_eq!(s["sandbox"]["enabled"], true);
    assert_eq!(
        s["sandbox"]["allowUnsandboxedCommands"], false,
        "strict: no unsandboxed retries"
    );
    let domains: Vec<&str> = s["sandbox"]["network"]["allowedDomains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(domains.contains(&"github.com") && domains.contains(&"registry.npmjs.org"));
    assert!(domains.contains(&"artifacts.example.com"));
}

#[test]
fn managed_policy_enforces_the_safety_line() {
    let v: serde_json::Value =
        serde_json::from_str(&ocgen::claude::managed_settings_json()).unwrap();
    assert_eq!(v["permissions"]["disableBypassPermissionsMode"], "disable");
    assert!(v["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d == "Read(./**/*.tfstate)"));
    assert!(v["permissions"]["ask"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d == "Bash(terraform apply*)"));
}

// ---------------------------------------------------------------- verify -----

fn verify_no_claude(dir: &Path) -> Vec<ocgen::verify::Check> {
    let p = Project::load_state(dir).unwrap();
    ocgen::verify::verify(
        &p,
        dir,
        &ocgen::verify::Options {
            run_claude: false,
            run_check: false,
        },
    )
}

fn status_of(checks: &[ocgen::verify::Check], name: &str) -> ocgen::verify::Status {
    checks
        .iter()
        .find(|c| c.name.contains(name))
        .unwrap_or_else(|| panic!("no check named {name}: {checks:?}"))
        .status
}

#[test]
fn verify_passes_a_fresh_project() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    let mut p = claude_default("vf");
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p.scaffold(dir.path(), false).unwrap();
    let checks = verify_no_claude(dir.path());
    assert!(
        !checks.iter().any(|c| c.status == Status::Fail),
        "{checks:#?}"
    );
    for name in [
        "up to date",
        "settings.json",
        "hook",
        "statusline",
        "approval gate",
    ] {
        assert_eq!(
            status_of(&checks, name),
            Status::Pass,
            "{name}: {checks:#?}"
        );
    }
    // Verification leaves no loop-guard state behind.
    assert!(!dir.path().join(".claude/loop-guard").exists());
}

#[test]
fn verify_catches_drift_bad_settings_broken_hooks_and_statusline() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    let mut p = claude_default("vb");
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p.scaffold(dir.path(), false).unwrap();

    // Drift + an unknown key.
    let mut s = settings_of(dir.path());
    s["teamateMode"] = serde_json::json!("auto");
    fs::write(
        dir.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&s).unwrap(),
    )
    .unwrap();
    // A gate that no longer blocks, and a statusline that crashes.
    fs::write(
        dir.path().join(".claude/hooks/team-approval-gate.sh"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    fs::write(
        dir.path().join(".claude/statusline.sh"),
        "#!/bin/sh\nexit 3\n",
    )
    .unwrap();
    fs::remove_file(dir.path().join(".claude/hooks/loop-guard.sh")).unwrap();

    let checks = verify_no_claude(dir.path());
    assert_eq!(status_of(&checks, "up to date"), Status::Warn);
    assert_eq!(
        status_of(&checks, "settings.json"),
        Status::Warn,
        "unknown key teamateMode"
    );
    assert_eq!(status_of(&checks, "statusline"), Status::Fail);
    assert_eq!(
        status_of(&checks, "hook scripts"),
        Status::Fail,
        "loop-guard.sh missing"
    );
    // With no compatible ocgen on PATH the command falls back to the (broken)
    // script, so the gate must be reported as not blocking.
    let gate = checks
        .iter()
        .find(|c| c.name.contains("approval gate"))
        .unwrap();
    assert!(
        gate.status == Status::Fail || gate.detail.contains("ocgen hook"),
        "{gate:?}"
    );
}

// ----------------------------------------------------- credentials + store -----

#[test]
fn credentials_and_the_approval_store_are_off_limits() {
    let dir = tempdir().unwrap();
    claude_default("cr").scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let deny: Vec<&str> = s["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for want in [
        "Read(~/.ssh/**)",
        "Read(~/.aws/**)",
        "Read(~/.config/gh/**)",
        "Read(~/.claude/ocgen/**)",
        "Edit(~/.claude/ocgen/**)",
        "Bash(ocgen approve*)",
    ] {
        assert!(deny.contains(&want), "missing {want}: {deny:?}");
    }
}

#[test]
fn sandbox_withholds_push_and_deploy_credentials_by_default() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("sc");
    p.claude.sandbox.enabled = true;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let files: Vec<&str> = s["sandbox"]["credentials"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        files.contains(&"~/.ssh") && files.contains(&"~/.config/gh") && files.contains(&"~/.aws")
    );
    assert!(s["sandbox"]["credentials"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["mode"] == "deny"));
    let vars: Vec<&str> = s["sandbox"]["credentials"]["envVars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(vars.contains(&"GH_TOKEN") && vars.contains(&"AWS_SECRET_ACCESS_KEY"));
    let no_write: Vec<&str> = s["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        no_write.contains(&"~/.claude/ocgen"),
        "the approval store can't be written from the sandbox"
    );

    // Opting in to credentials keeps the sandbox but drops the credential block.
    let dir = tempdir().unwrap();
    let mut p = claude_default("sc2");
    p.claude.sandbox.enabled = true;
    p.claude.sandbox.allow_credentials = true;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(s["sandbox"].get("credentials").is_none());
    assert!(
        s["sandbox"]["filesystem"]["denyWrite"].is_array(),
        "the store stays protected"
    );
}

// ------------------------------------------------------------ check command -----

#[test]
fn check_command_is_wired_into_settings_and_rules() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("chk");
    p.claude.workflow.check_cmd = "cargo test".into();
    p.claude.workflow.subagent_confidence = 0; // the check alone keeps the gate on
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert_eq!(s["env"]["OCGEN_CHECK_CMD"], "cargo test");
    assert!(
        !hook_groups(&s, "SubagentStop").is_empty(),
        "gate emitted for the check alone"
    );
    assert!(dir
        .path()
        .join(".claude/hooks/subagent-confidence-gate.sh")
        .exists());
    assert!(read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("cargo test"));

    // Off by default.
    let dir = tempdir().unwrap();
    claude_default("nochk").scaffold(dir.path(), false).unwrap();
    assert!(settings_of(dir.path())["env"]
        .get("OCGEN_CHECK_CMD")
        .is_none());
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert!(wf.check_cmd.is_empty());
}

// ------------------------------------------------------- keeping existing files -----

#[test]
fn scaffold_keeping_skips_kept_files_and_writes_the_rest() {
    let mut project = base_project("English");
    project.agents = agent::default_pipeline("English", "mac").unwrap();
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("opencode.json"), "{\"mine\": true}\n").unwrap();
    fs::create_dir_all(dir.path().join(".opencode/agents")).unwrap();
    fs::write(dir.path().join(".opencode/agents/reviewer.md"), "old\n").unwrap();

    let keep = std::collections::BTreeSet::from(["opencode.json".to_string()]);
    let written = project.scaffold_keeping(dir.path(), &keep).unwrap();

    // The kept file is untouched and not reported; everything else is (re)written.
    assert_eq!(read(dir.path(), "opencode.json"), "{\"mine\": true}\n");
    assert!(!written.contains(&dir.path().join("opencode.json")));
    assert!(read(dir.path(), ".opencode/agents/reviewer.md").contains("mode: subagent"));
    assert!(dir.path().join(".opencode/agents/coordinator.md").exists());

    // The state still fingerprints what ocgen would have written, so `doctor`
    // later reports the kept file as differing (hand-edited).
    let (_, state) = Project::discover(dir.path()).unwrap();
    assert!(state.generated.contains_key("opencode.json"));
    let plan = state.plan_changes(dir.path()).unwrap();
    let kept = plan.iter().find(|c| c.rel == "opencode.json").unwrap();
    assert_eq!(kept.hand_edited, Some(true));
}

// ------------------------------------------------------------------ hook shell -----

/// Every command hook in a hooks table, flattened.
fn all_hooks(table: &serde_json::Value) -> Vec<serde_json::Value> {
    table["hooks"]
        .as_object()
        .into_iter()
        .flat_map(|events| events.values())
        .flat_map(|groups| groups.as_array().cloned().unwrap_or_default())
        .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
        .collect()
}

#[test]
fn every_generated_hook_pins_bash() {
    // The commands are POSIX sh. Without a pinned shell, Claude Code on Windows
    // falls back to PowerShell when Git Bash is missing: the command fails to
    // parse and the gate fails open.
    let dir = tempdir().unwrap();
    let mut p = claude_default("sh");
    p.claude.team.enabled = true;
    p.claude.hooks_extra.notify = true;
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    p.claude.hooks_extra.config_audit = true;
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.scaffold(dir.path(), false).unwrap();

    let plugin: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "plugin/sh/hooks/hooks.json")).unwrap();
    for table in [settings_of(dir.path()), plugin] {
        let hooks = all_hooks(&table);
        assert!(hooks.len() >= 5, "hooks present: {}", hooks.len());
        for h in hooks {
            assert_eq!(h["shell"], "bash", "{h}");
        }
    }
}

#[test]
fn verify_flags_hooks_that_cannot_run_in_bash() {
    use ocgen::verify::{hook_shell, Status};
    let bash = Path::new("C:/Program Files/Git/bin/bash.exe");
    let pinned = serde_json::json!({ "hooks": { "PreToolUse": [ { "hooks": [
        { "type": "command", "command": "if [ 1 ]; then :; fi", "shell": "bash" }
    ] } ] } });
    let unpinned = serde_json::json!({ "hooks": { "PreToolUse": [ { "hooks": [
        { "type": "command", "command": "if [ 1 ]; then :; fi" }
    ] } ] } });

    // Pinned and (off Windows, or with Git Bash) runnable.
    assert_eq!(hook_shell(&pinned, false, None).status, Status::Pass);
    assert_eq!(hook_shell(&pinned, true, Some(bash)).status, Status::Pass);

    // Windows without Git Bash: the hooks cannot run, so the gates fail open.
    let c = hook_shell(&pinned, true, None);
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("Git for Windows"), "{}", c.detail);
    assert_eq!(hook_shell(&unpinned, true, None).status, Status::Fail);

    // A project generated before the shell was pinned.
    let c = hook_shell(&unpinned, false, None);
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains("ocgen doctor"), "{}", c.detail);

    // Nothing to check.
    let none = serde_json::json!({});
    assert_eq!(hook_shell(&none, true, None).status, Status::Skip);
}

// ------------------------------------------------------------- extra permissions -----

#[test]
fn permission_rules_validate_and_stay_in_one_list() {
    use ocgen::claude::{PermissionRules, RuleList};
    let mut r = PermissionRules::default();
    assert!(r.add(RuleList::Allow, "Bash(gh run view:*)").unwrap());
    assert!(r.add(RuleList::Deny, "Read(./secrets/**)").unwrap());
    assert!(r.add(RuleList::Allow, "mcp__github__get_issue").unwrap());
    // Adding it again is a no-op.
    assert!(!r.add(RuleList::Allow, "Bash(gh run view:*)").unwrap());
    assert_eq!(r.allow, ["Bash(gh run view:*)", "mcp__github__get_issue"]);

    // A rule lives in one list only.
    let err = r.add(RuleList::Ask, "Read(./secrets/**)").unwrap_err();
    assert!(err.to_string().contains("deny"), "{err}");

    // Malformed rules are rejected.
    for bad in ["", "Bash(gh", "bash run", "Bash()", "Read(a)\nEdit(b)"] {
        assert!(r.add(RuleList::Allow, bad).is_err(), "accepted {bad:?}");
    }

    assert!(r.remove("Read(./secrets/**)"));
    assert!(!r.remove("Read(./secrets/**)"));
    assert!(r.deny.is_empty());
}

#[test]
fn extra_permissions_are_merged_into_settings_and_survive_regeneration() {
    use ocgen::claude::RuleList;
    let dir = tempdir().unwrap();
    let mut p = claude_default("perm");
    let extra = &mut p.claude.permissions;
    extra.add(RuleList::Allow, "Bash(gh run view:*)").unwrap();
    extra.add(RuleList::Allow, "Read").unwrap(); // already generated: not repeated
    extra.add(RuleList::Ask, "Bash(docker push:*)").unwrap();
    extra.add(RuleList::Deny, "Read(./secrets/**)").unwrap();
    p.scaffold(dir.path(), false).unwrap();

    let perms = settings_of(dir.path())["permissions"].clone();
    let list = |k: &str| -> Vec<String> {
        perms[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let allow = list("allow");
    assert_eq!(
        allow.first().map(String::as_str),
        Some("Read"),
        "generated first"
    );
    assert_eq!(allow.iter().filter(|r| *r == "Read").count(), 1);
    assert_eq!(
        allow.last().map(String::as_str),
        Some("Bash(gh run view:*)")
    );
    assert!(list("ask").contains(&"Bash(docker push:*)".to_string()));
    assert!(list("ask").contains(&"Bash(git push:*)".to_string()));
    let deny = list("deny");
    assert!(deny.contains(&"Read(./secrets/**)".to_string()));
    assert!(
        deny.contains(&"Bash(ocgen approve*)".to_string()),
        "guards kept"
    );

    // The rules are part of the saved state, so a later re-render keeps them.
    let (_, state) = Project::discover(dir.path()).unwrap();
    assert_eq!(state.claude.permissions.allow.len(), 2);
    state.scaffold(dir.path(), true).unwrap();
    assert!(read(dir.path(), ".claude/settings.json").contains("Bash(gh run view:*)"));
}

#[test]
fn extra_permissions_are_written_even_without_the_permission_defaults() {
    use ocgen::claude::RuleList;
    let dir = tempdir().unwrap();
    let mut p = claude_default("bare");
    p.claude.powerups.permissions = false;
    p.claude.workflow.intent = false; // its own deny rule stays even without defaults
    p.claude.workflow.prefer_explorer = false; // so does Agent(Explore)
    p.scaffold(dir.path(), false).unwrap();
    assert!(settings_of(dir.path()).get("permissions").is_none());

    p.claude
        .permissions
        .add(RuleList::Allow, "Bash(gh run view:*)")
        .unwrap();
    p.scaffold(dir.path(), true).unwrap();
    let perms = settings_of(dir.path())["permissions"].clone();
    assert_eq!(perms["allow"], serde_json::json!(["Bash(gh run view:*)"]));
    assert!(perms.get("deny").is_none());
}

#[test]
fn allow_rules_shadowed_by_generated_deny_or_ask_are_reported() {
    use ocgen::claude::RuleList;
    let p = claude_default("shadow");
    // Deny and ask win over allow in Claude Code, so these allows do nothing.
    assert_eq!(
        p.shadowing_rule(RuleList::Allow, "Bash(git push:*)"),
        Some(RuleList::Ask)
    );
    assert_eq!(
        p.shadowing_rule(RuleList::Allow, "Bash(ocgen approve*)"),
        Some(RuleList::Deny)
    );
    assert_eq!(
        p.shadowing_rule(RuleList::Ask, "Bash(ocgen approve*)"),
        Some(RuleList::Deny)
    );
    assert_eq!(
        p.shadowing_rule(RuleList::Allow, "Bash(gh run view:*)"),
        None
    );
    assert_eq!(p.shadowing_rule(RuleList::Deny, "Bash(git push:*)"), None);
}

// ------------------------------------------------------------------- /intent -----

#[test]
fn intent_skill_renders_the_project_settings() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("int");
    p.claude.intent.prefix = "RFC".into();
    p.claude.intent.digits = 3;
    p.claude.intent.dir = "docs/rfc".into();
    p.claude.intent.max_words = 180;
    p.claude.intent.branch = "trunk".into();
    p.scaffold(dir.path(), false).unwrap();

    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    assert!(md.contains("disable-model-invocation: true"), "user-run");
    // The workflow: improve the prompt, investigate, plan to approval, intent
    // file, issue draft, then link the issue I filed.
    for step in [
        "Improve the prompt",
        "Investigate",
        "approve",
        "Intent file",
        "issue",
        "Verified",
        "Inferred",
    ] {
        assert!(md.contains(step), "missing {step}");
    }
    // Numbering: the configured prefix, width and directory, checked against the
    // remote branch so a number is never reused.
    assert!(
        md.contains("docs/rfc/RFC-") && md.contains("3 digits"),
        "{md}"
    );
    assert!(md.contains("RFC-001"));
    assert!(md.contains("git fetch") && md.contains("git ls-tree") && md.contains("trunk"));
    // The issue: ≤ N words, from the project's template, never created by Claude.
    assert!(md.contains("180 words"));
    assert!(md.contains(".claude/intent/issue-template.md"));
    assert!(md.contains(".claude/intent/intent-template.md"));
    assert!(md.contains("gh issue create") && md.contains("Never create"));

    assert!(read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("/intent"));
}

#[test]
fn intent_defaults_use_adr_numbering_and_the_remote_default_branch() {
    let dir = tempdir().unwrap();
    claude_default("intd").scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(md.contains("docs/adr/ADR-0001") && md.contains("4 digits"));
    assert!(md.contains("250 words"));
    assert!(
        md.contains("git ls-remote --symref origin HEAD"),
        "no branch configured → the remote's default branch"
    );
}

#[test]
fn intent_templates_are_written_once_and_user_owned() {
    let dir = tempdir().unwrap();
    let p = claude_default("intt");
    p.scaffold(dir.path(), false).unwrap();
    let issue = ".claude/intent/issue-template.md";
    let intent = ".claude/intent/intent-template.md";
    assert!(read(dir.path(), issue).contains("Acceptance criteria"));
    assert!(read(dir.path(), intent).contains("Status:"));

    // A user edit survives regeneration and isn't a change doctor wants to undo.
    fs::write(dir.path().join(issue), "## My own structure\n").unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert_eq!(read(dir.path(), issue), "## My own structure\n");
    let plan = p.plan_changes(dir.path()).unwrap();
    assert!(
        plan.iter().all(|c| c.rel != issue),
        "user-owned files aren't in doctor's plan"
    );
    // Nor is it fingerprinted as generated.
    let (_, state) = Project::discover(dir.path()).unwrap();
    assert!(!state.generated.contains_key(issue));

    // A deleted template comes back with the default.
    fs::remove_file(dir.path().join(intent)).unwrap();
    p.scaffold(dir.path(), true).unwrap();
    assert!(read(dir.path(), intent).contains("Status:"));
}

#[test]
fn intent_denies_gh_issue_create_and_can_be_disabled() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("intg");
    p.scaffold(dir.path(), false).unwrap();
    let deny = |d: &Path| settings_of(d)["permissions"]["deny"].clone();
    assert!(deny(dir.path())
        .as_array()
        .unwrap()
        .contains(&"Bash(gh issue create*)".into()));

    let dir = tempdir().unwrap();
    p.claude.workflow.intent = false;
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/intent").exists());
    assert!(!dir.path().join(".claude/intent").exists());
    assert!(!deny(dir.path())
        .as_array()
        .unwrap()
        .contains(&"Bash(gh issue create*)".into()));
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("/intent"));
}

#[test]
fn workflow_without_intent_field_backfills_true() {
    let wf: Workflow = serde_json::from_str(r#"{"deliver": true}"#).unwrap();
    assert!(wf.intent);
    let s: ocgen::claude::IntentSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(
        (s.prefix.as_str(), s.digits, s.dir.as_str(), s.max_words),
        ("ADR", 4, "docs/adr", 250)
    );
}

#[test]
fn intent_settings_are_validated() {
    use ocgen::validate::{git_branch, intent_digits, intent_dir, intent_max_words, intent_prefix};
    assert!(intent_prefix("ADR").is_ok() && intent_prefix("RFC2").is_ok());
    for bad in ["", "1ADR", "AD-R", "A B", "TOOLONGPREFIXX"] {
        assert!(intent_prefix(bad).is_err(), "{bad}");
    }
    assert!(intent_digits("4").is_ok() && intent_digits("1").is_ok());
    assert!(intent_digits("0").is_err() && intent_digits("7").is_err());
    assert!(intent_dir("docs/adr").is_ok() && intent_dir("adr").is_ok());
    for bad in ["", "/abs", "../up", "docs/../x", "C:\\adr", "a b"] {
        assert!(intent_dir(bad).is_err(), "{bad}");
    }
    assert!(intent_max_words("250").is_ok());
    assert!(intent_max_words("49").is_err() && intent_max_words("1001").is_err());
    assert!(
        git_branch("").is_ok() && git_branch("main").is_ok() && git_branch("release/1.x").is_ok()
    );
    for bad in ["a b", "x..y", "-x", "x~1", "x:y"] {
        assert!(git_branch(bad).is_err(), "{bad}");
    }
}

#[test]
fn user_skills_cannot_take_the_intent_name() {
    assert!(ocgen::validate::skill_name("intent").is_err());
}

// ------------------------------------------------- hand-added permission rules -----

/// Add rules to a list in the project's settings.json, as a user would by hand.
fn hand_add(dir: &Path, list: &str, rules: &[&str]) {
    let path = dir.join(".claude/settings.json");
    let mut s: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let arr = s["permissions"][list].as_array_mut().unwrap();
    for r in rules {
        arr.push((*r).into());
    }
    fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).unwrap();
}

#[test]
fn hand_added_permissions_are_found_but_not_generated_or_yours() {
    use ocgen::claude::RuleList;
    let dir = tempdir().unwrap();
    let mut p = claude_default("hand");
    p.claude
        .permissions
        .add(RuleList::Allow, "WebSearch")
        .unwrap();
    p.scaffold(dir.path(), false).unwrap();
    // Nothing added by hand yet: ocgen's and yours aren't reported.
    let found = p.hand_added_permissions(dir.path());
    assert!(
        found.rules.is_empty() && found.invalid.is_empty(),
        "{found:?}"
    );

    hand_add(
        dir.path(),
        "allow",
        &["Bash(gh run view:*)", "Bash(head:*)", "Read", "WebSearch"],
    );
    hand_add(dir.path(), "deny", &["Read(./secrets/**)", "not a rule"]);
    let found = p.hand_added_permissions(dir.path());
    assert_eq!(found.rules.allow, ["Bash(gh run view:*)", "Bash(head:*)"]);
    assert_eq!(found.rules.deny, ["Read(./secrets/**)"]);
    assert!(found.rules.ask.is_empty());
    assert_eq!(found.invalid, ["not a rule"]);

    // No settings.json, or not JSON: nothing to adopt.
    fs::write(dir.path().join(".claude/settings.json"), "{ nope").unwrap();
    assert!(p.hand_added_permissions(dir.path()).rules.is_empty());
    fs::remove_file(dir.path().join(".claude/settings.json")).unwrap();
    assert!(p.hand_added_permissions(dir.path()).rules.is_empty());
}

// ------------------------------------------------------- /intent: deep analysis -----

/// The skill's frontmatter lines (between the `---` fences).
fn frontmatter(md: &str) -> Vec<String> {
    md.strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f.lines().map(String::from).collect())
        .unwrap_or_default()
}

const INTENT_READS: [&str; 19] = [
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

#[test]
fn intent_skill_pre_approves_read_only_tools_and_runs_at_xhigh() {
    let dir = tempdir().unwrap();
    claude_default("ia").scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    let front = frontmatter(&md);

    // No key twice.
    let mut keys: Vec<&str> = front
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, _)| k)
        .collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate frontmatter keys: {front:?}");

    assert!(front.iter().any(|l| l == "effort: xhigh"), "{front:?}");
    let tools = front
        .iter()
        .find_map(|l| l.strip_prefix("allowed-tools: "))
        .expect("allowed-tools");
    let tools: Vec<&str> = tools.split(", ").collect();
    for t in INTENT_READS {
        assert!(tools.contains(&t), "missing {t}: {tools:?}");
    }
    assert!(tools.contains(&"WebFetch(domain:docs.rs)"));
    assert!(tools.contains(&"WebFetch(domain:docs.github.com)"));
    // Read-only: nothing that writes, creates issues or searches the open web.
    for t in &tools {
        assert!(
            !t.starts_with("Write")
                && !t.starts_with("Edit")
                && !t.contains("gh issue create")
                && *t != "WebSearch"
                && *t != "WebFetch"
                && *t != "Bash",
            "not read-only: {t}"
        );
    }
    // And the deny rule still stops issue creation.
    assert!(settings_of(dir.path())["permissions"]["deny"]
        .as_array()
        .unwrap()
        .contains(&"Bash(gh issue create*)".into()));
}

#[test]
fn intent_skill_demands_an_exhaustive_evidence_based_analysis() {
    let dir = tempdir().unwrap();
    claude_default("ib").scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");

    // Passes: map, parallel deep dive, gap closing, challenge.
    for pass in ["Pass 1", "Pass 2", "Pass 3", "Pass 4"] {
        assert!(md.contains(pass), "missing {pass}");
    }
    assert!(md.contains("search terms"));
    assert!(md.contains("`Explore` subagents") && !md.contains("explorer subagents"));
    assert!(md.contains("every call site"));
    assert!(md.contains("git log -S") && md.contains("git blame"));
    assert!(md.contains("gh search issues") && md.contains("closed"));
    assert!(md.contains("finds nothing new"));
    // Gap closing fans out too: one Explore subagent per open gap, in one message.
    let pass3 = md
        .split("**Pass 3")
        .nth(1)
        .and_then(|r| r.split("**Pass 4").next())
        .expect("a Pass 3 section");
    assert!(
        pass3.contains("in parallel")
            && pass3.contains("one per gap")
            && pass3.contains("in one message"),
        "{pass3}"
    );
    assert!(md.contains("contradict"));

    // Coverage checklist: every item.
    for item in [
        "Entry points",
        "All call sites",
        "Data and state",
        "Error and edge paths",
        "Concurrency",
        "Platforms",
        "Security and permissions",
        "Performance",
        "Tests",
        "Config",
        "Docs",
        "History",
        "Related issues and PRs",
        "Existing intents",
    ] {
        assert!(md.contains(item), "checklist missing {item}");
    }
    assert!(md.contains("N/A"));

    // The analysis report, numbered findings, and a plan that traces back to them.
    for section in [
        "Current behaviour",
        "Root cause",
        "Impact",
        "Constraints",
        "Prior decisions",
        "Risks",
        "Open questions",
        "What would make this wrong",
    ] {
        assert!(md.contains(section), "report missing {section}");
    }
    assert!(md.contains("F1") && md.contains("Assumption"));
    assert!(md.contains("Verified") && md.contains("Inferred"));

    // Web docs only from trusted domains; gh failures don't stop the analysis.
    assert!(md.contains("trusted") && md.contains("docs.rs"));
    assert!(md.contains("not authenticated"));
}

#[test]
fn intent_trusted_domains_are_configurable_and_validated() {
    use ocgen::validate::domain;
    let dir = tempdir().unwrap();
    let mut p = claude_default("ic");
    p.claude.intent.trusted_domains = vec!["docs.example.org".into()];
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(md.contains("WebFetch(domain:docs.example.org)"));
    assert!(!md.contains("WebFetch(domain:docs.rs)"));

    assert!(domain("docs.rs").is_ok() && domain("www.rfc-editor.org").is_ok());
    // A leading `*.` trusts every subdomain of a registrable domain…
    assert!(domain("*.amazon.com").is_ok() && domain("*.docs.example.org").is_ok());
    for bad in [
        "",
        "https://docs.rs",
        "docs.rs/std",
        "docs.rs:443",
        "a b",
        "localhost",
        // …but only as the whole first label, and never a whole TLD.
        "*",
        "*.com",
        "*amazon.com",
        "docs.*.com",
        "*.*.amazon.com",
        "amazon.*",
    ] {
        assert!(domain(bad).is_err(), "{bad}");
    }
    p.claude.intent.trusted_domains = vec!["https://x.org".into()];
    assert!(p.claude.intent.validate().is_err());

    // Old state without the field gets the defaults.
    let s: ocgen::claude::IntentSettings = serde_json::from_str("{}").unwrap();
    assert!(s.trusted_domains.contains(&"docs.github.com".to_string()));
}

#[test]
fn intent_template_has_an_evidence_section() {
    let dir = tempdir().unwrap();
    claude_default("id").scaffold(dir.path(), false).unwrap();
    assert!(read(dir.path(), ".claude/intent/intent-template.md").contains("## Evidence"));
}

#[test]
fn trusted_domains_are_https_only() {
    use ocgen::validate::trusted_domain;
    // An https:// URL is accepted and stored as its host; http:// never is.
    assert_eq!(trusted_domain("https://docs.rs").unwrap(), "docs.rs");
    assert_eq!(trusted_domain("HTTPS://Docs.RS/").unwrap(), "docs.rs");
    assert_eq!(trusted_domain(" *.amazon.com ").unwrap(), "*.amazon.com");
    assert_eq!(
        trusted_domain("https://*.amazon.com").unwrap(),
        "*.amazon.com"
    );
    let err = trusted_domain("http://docs.rs").unwrap_err();
    assert!(err.contains("https"), "{err}");
    for bad in [
        "HTTP://docs.rs",
        "ftp://docs.rs",
        "//docs.rs",
        "https://docs.rs/std",
        "https://",
    ] {
        assert!(trusted_domain(bad).is_err(), "{bad}");
    }
    // A wildcard over a shared-hosting suffix would trust every stranger's
    // subdomain; one exact host there is fine.
    for bad in ["*.github.io", "*.S3.amazonaws.com", "https://*.vercel.app/"] {
        let err = trusted_domain(bad).unwrap_err();
        assert!(err.contains("anyone"), "{bad}: {err}");
    }
    assert_eq!(
        trusted_domain("serde.readthedocs.io").unwrap(),
        "serde.readthedocs.io"
    );
    assert_eq!(trusted_domain("*.docs.rs").unwrap(), "*.docs.rs");
    assert!(ocgen::validate::shared_hosting_wildcard("*.github.io"));
    assert!(!ocgen::validate::shared_hosting_wildcard(
        "rust-lang.github.io"
    ));
    assert!(!ocgen::validate::shared_hosting_wildcard("*.notgithub.io"));

    // The skill fetches over HTTPS only.
    let dir = tempdir().unwrap();
    claude_default("https").scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    assert!(
        md.contains("only `https://` URLs") && md.contains("never `http://`"),
        "{md}"
    );
}

#[test]
fn the_noop_cd_hook_is_wired_after_the_gate_and_verified() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    let mut p = claude_default("cd");
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    let pre = hook_groups(&s, "PreToolUse");
    assert!(
        pre[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("team-approval-gate"),
        "the approval gate stays first: {pre:#?}"
    );
    let group = pre
        .iter()
        .find(|g| {
            g["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("drop-noop-cd")
        })
        .expect("a PreToolUse group for drop-noop-cd");
    assert_eq!(group["matcher"], "Bash");
    assert!(dir.path().join(".claude/hooks/drop-noop-cd.sh").exists());
    let checks = verify_no_claude(dir.path());
    let c = checks.iter().find(|c| c.name == "no-op cd").unwrap();
    assert_eq!(c.status, Status::Pass, "{checks:#?}");

    // A hook that strips every cd fails verification.
    fs::write(
        dir.path().join(".claude/hooks/drop-noop-cd.sh"),
        "#!/bin/sh\njq -c '{hookSpecificOutput:{hookEventName:\"PreToolUse\",updatedInput:(.tool_input|.command=\"echo ok\")}}'\n",
    )
    .unwrap();
    let mut s = settings_of(dir.path());
    for g in s["hooks"]["PreToolUse"].as_array_mut().unwrap() {
        if g["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("drop-noop-cd")
        {
            g["hooks"][0]["command"] =
                "sh \"$CLAUDE_PROJECT_DIR/.claude/hooks/drop-noop-cd.sh\"".into();
        }
    }
    fs::write(
        dir.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&s).unwrap(),
    )
    .unwrap();
    if std::process::Command::new("jq")
        .arg("--version")
        .output()
        .is_ok()
    {
        let checks = verify_no_claude(dir.path());
        let c = checks.iter().find(|c| c.name == "no-op cd").unwrap();
        assert_eq!(c.status, Status::Fail, "{checks:#?}");
    }

    // Off: no group, no script.
    let dir = tempdir().unwrap();
    let mut p = claude_default("cd");
    p.claude.hooks_extra.drop_noop_cd = false;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(!hook_groups(&s, "PreToolUse")
        .iter()
        .any(|g| g.to_string().contains("drop-noop-cd")));
    assert!(!dir.path().join(".claude/hooks/drop-noop-cd.sh").exists());
}

#[test]
fn research_goes_to_explorer_and_the_builtin_explore_is_denied() {
    let denies_explore = |dir: &Path| {
        settings_of(dir)["permissions"]["deny"]
            .as_array()
            .is_some_and(|d| d.iter().any(|r| r == "Agent(Explore)"))
    };
    // On by default: the shell-free explorer exists, so the built-in Explore is
    // denied and the rule says where research goes.
    let dir = tempdir().unwrap();
    claude_default("ex").scaffold(dir.path(), false).unwrap();
    assert!(denies_explore(dir.path()), "{}", settings_of(dir.path()));
    let rule = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(
        rule.contains("Delegate codebase research to the `explorer` subagent"),
        "{rule}"
    );
    // Off: Explore is allowed again and the rule doesn't mention it.
    let dir = tempdir().unwrap();
    let mut p = claude_default("ex");
    p.claude.workflow.prefer_explorer = false;
    p.scaffold(dir.path(), false).unwrap();
    assert!(!denies_explore(dir.path()));
    assert!(!read(dir.path(), ".claude/rules/ocgen-workflow.md").contains("`explorer` subagent"));
    // No explorer agent: nothing to route to, so Explore stays.
    let dir = tempdir().unwrap();
    let mut p = claude_default("ex");
    p.agents.retain(|a| a.name != "explorer");
    p.scaffold(dir.path(), false).unwrap();
    assert!(!denies_explore(dir.path()));
    assert!(!p.prefers_explorer());
}

#[test]
fn the_workflow_rule_steers_away_from_shell_that_always_prompts() {
    let dir = tempdir().unwrap();
    claude_default("sh").scaffold(dir.path(), false).unwrap();
    let rule = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    assert!(rule.contains("## Shell commands"), "{rule}");
    for want in [
        "**Read**",
        "**Glob**",
        "no `cd`",
        "`find -exec`",
        "PowerShell script",
        "put this paragraph in its prompt",
    ] {
        assert!(rule.contains(want), "missing {want}: {rule}");
    }
}

#[test]
fn intent_blocks_plain_http_webfetch_with_a_pretooluse_hook() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("hf");
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true; // its PreToolUse group must stay too
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.scaffold(dir.path(), false).unwrap();

    let s = settings_of(dir.path());
    let groups = hook_groups(&s, "PreToolUse");
    let fetch = groups
        .iter()
        .find(|g| g["matcher"] == "WebFetch")
        .expect("a PreToolUse group for WebFetch");
    let cmd = fetch["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.contains("https-only-fetch"), "{cmd}");
    assert_eq!(fetch["hooks"][0]["shell"], "bash");
    assert!(
        groups.iter().any(|g| g["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default()
            .contains("team-approval-gate")),
        "approval gate kept"
    );
    assert!(dir
        .path()
        .join(".claude/hooks/https-only-fetch.sh")
        .exists());
    // The plugin carries it as well.
    let plugin: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "plugin/hf/hooks/hooks.json")).unwrap();
    assert!(plugin["hooks"]["PreToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["matcher"] == "WebFetch"));
    assert!(dir
        .path()
        .join("plugin/hf/hooks/https-only-fetch.sh")
        .exists());

    // Off with /intent.
    let dir = tempdir().unwrap();
    let mut p = claude_default("hf2");
    p.claude.workflow.intent = false;
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert!(!hook_groups(&s, "PreToolUse")
        .iter()
        .any(|g| g["matcher"] == "WebFetch"));
    assert!(!dir
        .path()
        .join(".claude/hooks/https-only-fetch.sh")
        .exists());
}

#[test]
fn verify_checks_that_plain_http_fetches_are_blocked() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    claude_default("vh").scaffold(dir.path(), false).unwrap();
    let checks = verify_no_claude(dir.path());
    assert_eq!(
        status_of(&checks, "WebFetch guard"),
        Status::Pass,
        "{checks:#?}"
    );

    // A hook that lets http:// through fails verification.
    fs::write(
        dir.path().join(".claude/hooks/https-only-fetch.sh"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let mut s = settings_of(dir.path());
    for g in s["hooks"]["PreToolUse"].as_array_mut().unwrap() {
        if g["matcher"] == "WebFetch" {
            g["hooks"][0]["command"] =
                "sh \"$CLAUDE_PROJECT_DIR/.claude/hooks/https-only-fetch.sh\"".into();
        }
    }
    fs::write(
        dir.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&s).unwrap(),
    )
    .unwrap();
    let checks = verify_no_claude(dir.path());
    assert_eq!(
        status_of(&checks, "WebFetch guard"),
        Status::Fail,
        "{checks:#?}"
    );

    // So does the old guard: it dropped backslashes and took the host after the
    // last `@`, so `https://evil\@docs.github.com/` passed (it goes to evil).
    fs::write(
        dir.path().join(".claude/hooks/https-only-fetch.sh"),
        "#!/bin/sh\nurl=$(cat | sed -n 's/.*\"url\":\"\\([^\"]*\\)\".*/\\1/p' | tr -d '\\\\')\n\
         case \"$url\" in https://*) ;; *) exit 2 ;; esac\n\
         h=${url#https://}; h=${h%%/*}; h=${h##*@}; h=${h%%:*}\n\
         for d in $OCGEN_WEBFETCH_DOMAINS; do [ \"$h\" = \"$d\" ] && exit 0; done\nexit 2\n",
    )
    .unwrap();
    let checks = verify_no_claude(dir.path());
    let c = checks.iter().find(|c| c.name == "WebFetch guard").unwrap();
    assert_eq!(c.status, Status::Fail, "{checks:#?}");
    assert!(c.detail.contains("ocgen-verify.invalid\\@"), "{c:#?}");
}

#[test]
fn verify_fails_a_trusted_shared_hosting_wildcard() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    claude_default("vw").scaffold(dir.path(), false).unwrap();
    let mut s = settings_of(dir.path());
    s["env"]["OCGEN_WEBFETCH_DOMAINS"] = "docs.rs *.github.io".into();
    fs::write(
        dir.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&s).unwrap(),
    )
    .unwrap();
    let checks = verify_no_claude(dir.path());
    let c = checks.iter().find(|c| c.name == "WebFetch guard").unwrap();
    assert_eq!(c.status, Status::Fail, "{checks:#?}");
    assert!(
        c.detail.contains("*.github.io") && c.detail.contains("--untrust-domain"),
        "{c:#?}"
    );
}

#[test]
fn the_trusted_list_reaches_the_webfetch_guard_and_an_empty_one_trusts_nothing() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("wg");
    p.claude.intent.trusted_domains = vec!["docs.rs".into(), "*.amazon.com".into()];
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.scaffold(dir.path(), false).unwrap();
    assert_eq!(
        settings_of(dir.path())["env"]["OCGEN_WEBFETCH_DOMAINS"],
        "docs.rs *.amazon.com"
    );
    // Plugins can't set env: the list rides on the hook command.
    let plugin = read(dir.path(), "plugin/wg/hooks/hooks.json");
    assert!(
        plugin.contains("OCGEN_WEBFETCH_DOMAINS='docs.rs *.amazon.com'"),
        "{plugin}"
    );

    // An empty list: the variable is still set (to nothing), no WebFetch is
    // pre-approved, and the skill says plainly that no site may be fetched.
    let dir = tempdir().unwrap();
    p.claude.intent.trusted_domains.clear();
    p.scaffold(dir.path(), false).unwrap();
    let s = settings_of(dir.path());
    assert_eq!(s["env"]["OCGEN_WEBFETCH_DOMAINS"], "");
    for list in ["allow", "ask"] {
        assert!(
            !s["permissions"][list]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().starts_with("WebFetch")),
            "{list} pre-approves WebFetch"
        );
    }
    let md = read(dir.path(), ".claude/skills/intent/SKILL.md");
    let tools = frontmatter(&md)
        .into_iter()
        .find_map(|l| l.strip_prefix("allowed-tools: ").map(String::from))
        .unwrap();
    assert!(!tools.contains("WebFetch"), "nothing pre-approved: {tools}");
    assert!(!md.contains("trusted domains ()"));
    assert!(md.contains("No documentation sites are trusted"), "{md}");
}

// ------------------------------------------------------ /inquire HTML view --

#[test]
fn inquire_registers_the_notes_hook() {
    let dir = tempdir().unwrap();
    let mut p = claude_default("nv");
    p.claude.hooks_extra.format_cmd = "cargo fmt".into();
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "nv".into();
    p.scaffold(dir.path(), false).unwrap();

    let s = settings_of(dir.path());
    let groups = hook_groups(&s, "PostToolUse");
    assert!(
        groups[0].to_string().contains("format.sh"),
        "the formatter stays first"
    );
    let notes = groups
        .iter()
        .find(|g| g.to_string().contains("inquire-notes"))
        .expect("a PostToolUse group for the notes view");
    assert_eq!(notes["matcher"], "Write|Edit|MultiEdit");
    let cmd = notes["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains("ocgen hook inquire-notes") && cmd.contains("ocgen-hooks 8"),
        "{cmd}"
    );
    assert_eq!(notes["hooks"][0]["shell"], "bash");
    let script = dir.path().join(".claude/hooks/inquire-notes.sh");
    assert!(std::process::Command::new("sh")
        .arg("-n")
        .arg(&script)
        .status()
        .unwrap()
        .success());
    let plugin: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "plugin/nv/hooks/hooks.json")).unwrap();
    assert!(plugin["hooks"]["PostToolUse"]
        .to_string()
        .contains("${CLAUDE_PLUGIN_ROOT}/hooks/inquire-notes.sh"));
    assert!(dir
        .path()
        .join("plugin/nv/hooks/inquire-notes.sh")
        .is_file());

    // Off with /inquire.
    let dir = tempdir().unwrap();
    let mut p = claude_default("nv2");
    p.claude.workflow.inquire = false;
    p.scaffold(dir.path(), false).unwrap();
    assert!(!settings_of(dir.path())
        .to_string()
        .contains("inquire-notes"));
    assert!(!dir.path().join(".claude/hooks/inquire-notes.sh").exists());
}

#[test]
fn verify_reports_the_notes_view() {
    use ocgen::verify::Status;
    let dir = tempdir().unwrap();
    claude_default("vn").scaffold(dir.path(), false).unwrap();
    // Whatever ocgen is on PATH, a working project never fails this check.
    let checks = verify_no_claude(dir.path());
    assert_ne!(
        status_of(&checks, "notes view"),
        Status::Fail,
        "{checks:#?}"
    );

    // With the current binary, the view renders.
    let set_cmd = |cmd: String| {
        let mut s = settings_of(dir.path());
        for g in s["hooks"]["PostToolUse"].as_array_mut().unwrap() {
            if g.to_string().contains("inquire-notes") {
                g["hooks"][0]["command"] = cmd.clone().into();
            }
        }
        fs::write(
            dir.path().join(".claude/settings.json"),
            serde_json::to_string_pretty(&s).unwrap(),
        )
        .unwrap();
    };
    let bin = ocgen::paths::for_shell(Path::new(env!("CARGO_BIN_EXE_ocgen")));
    set_cmd(format!("\"{bin}\" hook inquire-notes # inquire-notes"));
    let checks = verify_no_claude(dir.path());
    assert_eq!(
        status_of(&checks, "notes view"),
        Status::Pass,
        "{checks:#?}"
    );

    // A hook that renders nothing is reported.
    set_cmd("cat >/dev/null # inquire-notes".into());
    let checks = verify_no_claude(dir.path());
    assert_ne!(
        status_of(&checks, "notes view"),
        Status::Pass,
        "{checks:#?}"
    );

    // Skipped without /inquire.
    let dir = tempdir().unwrap();
    let mut p = claude_default("vn2");
    p.claude.workflow.inquire = false;
    p.scaffold(dir.path(), false).unwrap();
    let checks = verify_no_claude(dir.path());
    assert_eq!(status_of(&checks, "notes view"), Status::Skip);
}
