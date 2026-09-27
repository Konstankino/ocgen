//! Integration tests exercise the library's rendering/scaffolding directly.
//! (The wizard is a thin dialoguer layer over this; its logic lives here.)

use std::fs;
use std::path::Path;

use ocgen::agent::{self, Agent};
use ocgen::archetype::Archetype;
use ocgen::manifest::{Manifest, Model, Provider};
use ocgen::render::Project;
use ocgen::templates;
use ocgen::target::Target;
use ocgen::claude::{Output, Powerups, Skill, Workflow};
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
    // 4 agents + orchestrator prompt + opencode.json + multi command = 7.
    assert_eq!(written.len(), 7);

    let json: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert_eq!(json["model"], "mac/qwen3-35b-a3b");
    assert_eq!(json["provider"]["mac"]["name"], "M4 Pro 24GB");
    // Utility agents are now provider-qualified.
    assert_eq!(json["agent"]["compaction"]["model"], "mac/qwen3.5-9b");

    let orch = read(dir.path(), ".opencode/agents/orchestrator.md");
    assert!(orch.contains("mode: primary"));
    assert!(orch.contains("model: mac/qwen3-35b-a3b"));
    assert!(orch.contains("prompt: \"{file:../prompts/orchestrator.txt}\""));
    for sub in ["structure", "editor", "grammar"] {
        assert!(orch.contains(&format!("\"{sub}\": allow")), "task perm for {sub}");
    }

    let structure = read(dir.path(), ".opencode/agents/structure.md");
    assert!(structure.contains("edit: deny"));
    let editor = read(dir.path(), ".opencode/agents/editor.md");
    assert!(editor.contains("steps: 30"));
    assert!(editor.contains("color: \"#4ec9b0\""));

    let multi = read(dir.path(), ".opencode/commands/multi.md");
    assert!(multi.contains("agent: orchestrator"));
    assert!(multi.contains("@structure"));

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
    let mut lead = Agent::from_archetype("lead", "orchestrator", "English", "cloud").unwrap();
    lead.model = "gpt-omni".into();
    project.agents = vec![
        lead,
        Agent::from_archetype("grammar", "corrector", "English", "mac").unwrap(),
    ];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
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
    project.agents = vec![Agent::from_archetype("grammar", "corrector", "English", "mac").unwrap()];
    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), ".opencode/agents/grammar.md");
    for absent in ["variant:", "top_p:", "disable:", "hidden:", "options:"] {
        assert!(!md.contains(absent), "unexpected `{absent}` in default agent");
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
            let mut lead = Agent::from_archetype("lead", "orchestrator", "English", "mac").unwrap();
            lead.description = "coordinator".into();
            lead
        },
        Agent::from_archetype("proofer", "corrector", "English", "mac").unwrap(),
        Agent::from_archetype("critic", "reviewer", "English", "mac").unwrap(),
    ];

    let dir = tempdir().unwrap();
    project.scaffold(dir.path(), false).unwrap();

    for name in ["lead", "proofer", "critic"] {
        assert!(dir.path().join(format!(".opencode/agents/{name}.md")).exists());
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
    let orch = read(dir.path(), ".opencode/agents/orchestrator.md");
    assert!(orch.contains("\"factcheck\": allow"));
    let prompt = read(dir.path(), ".opencode/prompts/orchestrator.txt");
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
        .find(|a| a.name == "editor")
        .unwrap()
        .temperature = "0.5".into();
    // Rename another agent.
    reloaded
        .agents
        .iter_mut()
        .find(|a| a.name == "grammar")
        .unwrap()
        .name = "proof".into();

    reloaded.scaffold(dir.path(), true).unwrap();
    Project::remove_agent_artifacts(dir.path(), "grammar").unwrap();

    // Field change took effect.
    let editor = read(dir.path(), ".opencode/agents/editor.md");
    assert!(editor.contains("temperature: 0.5"));

    // Rename: new file exists, old removed, references follow.
    assert!(dir.path().join(".opencode/agents/proof.md").exists());
    assert!(!dir.path().join(".opencode/agents/grammar.md").exists());
    let orch = read(dir.path(), ".opencode/agents/orchestrator.md");
    assert!(orch.contains("\"proof\": allow"));
    assert!(!orch.contains("\"grammar\": allow"));
    let prompt = read(dir.path(), ".opencode/prompts/orchestrator.txt");
    // The subagent is listed by its new name (description text may still mention "grammar").
    assert!(prompt.contains("- proof —") && !prompt.contains("- grammar —"));
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
    let ed = reloaded.agents.iter_mut().find(|a| a.name == "editor").unwrap();
    ed.provider = "cloud".into();
    ed.model = "gpt-omni".into();
    reloaded.scaffold(dir.path(), true).unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert!(json["provider"]["cloud"].is_object());
    assert!(read(dir.path(), ".opencode/agents/editor.md").contains("model: cloud/gpt-omni"));

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
    assert!(read(dir.path(), ".opencode/agents/editor.md").contains("model: vllm/gpt-omni"));
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
            {"name":"orchestrator","archetype":"orchestrator","model":"qwen3-35b-a3b","mode":"primary","description":"boss"},
            {"name":"grammar","archetype":"corrector","model":"qwen3.5-9b","mode":"subagent","description":"fix"}
          ]
        }"#,
    );

    let project = Project::load_state(dir.path()).unwrap();
    assert_eq!(project.providers.len(), 1);
    assert_eq!(project.providers[0].key, "mac");
    assert_eq!(project.utility_provider, "mac");

    let orch = project.agents.iter().find(|a| a.name == "orchestrator").unwrap();
    assert_eq!(orch.provider, "mac"); // backfilled
    assert_eq!(orch.role, "orchestrator"); // from legacy archetype
    assert!(!orch.permissions.is_empty()); // from archetype
    assert!(orch.prompt_file); // orchestrator archetype uses a prompt file

    // And it re-renders into a valid current-schema project.
    project.scaffold(dir.path(), true).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
    assert_eq!(json["model"], "mac/qwen3-35b-a3b");
    assert!(read(dir.path(), ".opencode/agents/orchestrator.md").contains("model: mac/qwen3-35b-a3b"));
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
        let prov = project.providers.iter().find(|p| p.key == ag.provider).unwrap();
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
    for n in ["orchestrator", "reviewer", "editor", "corrector"] {
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

    let orch = Archetype::load("orchestrator").unwrap();
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
    assert!(issues.iter().any(|s| s.contains("unknown provider 'ghost'")));
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
    let json: serde_json::Value =
        serde_json::from_str(&read(dir.path(), "opencode.json")).unwrap();
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
    assert!(seeds.prompt_for("English").contains("{% for sub in subagents %}"));
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
            ("orchestrator", "orchestrator"),
            ("structure", "reviewer"),
            ("editor", "editor"),
            ("grammar", "corrector"),
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
    assert_eq!(serde_json::to_string(&Target::ClaudeCode).unwrap(), "\"claude\"");
    assert_eq!(serde_json::to_string(&Target::OpenCode).unwrap(), "\"opencode\"");
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
    });
    proj.agents = agent::default_pipeline("English", "mac").unwrap();
    proj.agents[0].tools = "Read, Grep, Glob".into();

    let json = serde_json::to_string(&proj).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.target, Target::ClaudeCode);
    assert_eq!(back.claude.model, "sonnet");
    assert_eq!(back.claude.instructions, "Be excellent.");
    assert_eq!(back.claude.output.project, true); // default preserved
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
    assert!(written.iter().any(|w| w.ends_with(".claude/agents/reviewer.md")));
    assert!(!written.iter().any(|w| w.ends_with(".claude/agents/boss.md")));

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
    assert!(claude_md.contains("**reviewer**"));
    assert!(claude_md.contains("You coordinate the team."));

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "sonnet");
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

    // orchestrator is primary → coordinator (CLAUDE.md), not an agent file.
    assert!(!dir.path().join(".claude/agents/orchestrator.md").exists());
    let structure = read(dir.path(), ".claude/agents/structure.md");
    assert!(structure.contains("model: sonnet"));
    assert!(structure.contains("tools: Read, Grep, Glob"));
    let grammar = read(dir.path(), ".claude/agents/grammar.md");
    assert!(grammar.contains("model: haiku"));

    let claude_md = read(dir.path(), "CLAUDE.md");
    assert!(claude_md.contains("# writing"));
    assert!(claude_md.contains("**structure**"));
    // The coordinator's prompt template is fully rendered — no raw Jinja leaks.
    assert!(!claude_md.contains("{% for"), "unrendered Jinja in CLAUDE.md");
    assert!(!claude_md.contains("{{ sub.name }}"));

    let multi = read(dir.path(), ".claude/commands/multi.md");
    assert!(multi.contains("@structure") && multi.contains("@grammar"));

    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "sonnet");
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
    let settings: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".claude/settings.json")).unwrap();
    assert_eq!(settings["model"], "sonnet");
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
    // An agent with no alias falls back to sonnet.
    assert!(read(dir.path(), ".claude/agents/w.md").contains("model: sonnet"));
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
    assert!(claude_md.contains("coordinate"));
}

// ---- Claude Code target: plugin + GitHub release output (phase 4) ----

#[test]
fn claude_plugin_output_emits_manifest_marketplace_and_workflow() {
    let mut p = base_project("English");
    p.target = Target::ClaudeCode;
    p.project_name = "Writing Kit".into();
    p.claude.output = Output { project: true, plugin: true };
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
    assert!(base.join("agents/structure.md").is_file());
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
    assert!(dir.path().join(".claude/agents/structure.md").is_file());
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
    assert!(issues.iter().any(|i| i.contains("unknown model alias 'gpt-4'")));
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
    assert!(fixes.iter().any(|f| f.contains("model alias") && f.contains("sonnet")));
    assert!(fixes.iter().any(|f| f.contains("colour") && f.contains("blue")));
    assert!(fixes.iter().any(|f| f.contains("empty role")));
    assert_eq!(p.agents[0].model, "sonnet");
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
    });
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();

    assert!(dir.path().join(".claude/agents/structure.md").is_file());
    Project::remove_claude_agent_file(dir.path(), "structure").unwrap();
    assert!(!dir.path().join(".claude/agents/structure.md").exists());

    assert!(dir.path().join(".claude/skills/commit").is_dir());
    Project::remove_claude_skill_dir(dir.path(), "commit").unwrap();
    assert!(!dir.path().join(".claude/skills/commit").exists());

    // Removing something that isn't there is a no-op.
    Project::remove_claude_agent_file(dir.path(), "nope").unwrap();
    Project::remove_claude_skill_dir(dir.path(), "nope").unwrap();
}
