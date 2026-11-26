//! The agents that learn something worth keeping from run to run — the
//! explorer, the implementer and the verifier — get Claude Code's agent memory
//! (`memory: local`), and the lead keeps it for them. Claude Code gives a
//! memory agent Write and Edit, but `disallowedTools` takes them away again
//! from a read-only one, and it resolves the memory folder from the agent's
//! own working folder, so an isolated worker's copy goes with its worktree.
//! So each memory agent reads its memory but never writes it: it reports what
//! to keep under **Remember**, and the lead saves that in the main checkout,
//! whence `.worktreeinclude` copies it into each new worktree. The reviewer,
//! adversary and scope-guard keep none: their worth is a fresh look, and the
//! reviewer's runs are meant to be blind to each other.

use std::collections::BTreeMap;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

const TEAM: [(&str, &str); 7] = [
    ("coordinator", "coordinator"),
    ("explorer", "explorer"),
    ("implementer", "implementer"),
    ("verifier", "verifier"),
    ("reviewer", "reviewer"),
    ("scope-guard", "scope-guard"),
    ("adversary", "adversary"),
];

const KEEPERS: [&str; 3] = ["explorer", "implementer", "verifier"];

/// A Claude project with these agents, `(name, archetype)`, in `lang`.
fn project(team: &[(&str, &str)], lang: &str) -> Project {
    let mut p = Project::from_manifest(&manifest(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "memory".into();
    p.providers.clear();
    p.agents = team
        .iter()
        .map(|(n, a)| Agent::from_archetype_claude(n, a, lang).unwrap())
        .collect();
    p
}

fn files(p: &Project) -> BTreeMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

/// `text` with every run of whitespace made one space, so a phrase matches
/// across the template's line breaks.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The agent file's frontmatter lines.
fn frontmatter(md: &str) -> Vec<&str> {
    md.trim_start_matches("---\n")
        .split("\n---\n")
        .next()
        .unwrap()
        .lines()
        .collect()
}

#[test]
fn the_explorer_implementer_and_verifier_keep_local_memory() {
    let f = files(&project(&TEAM, "English"));
    for name in KEEPERS {
        let md = &f[&format!(".claude/agents/{name}.md")];
        assert!(
            frontmatter(md).contains(&"memory: local"),
            "{name} keeps no local memory:\n{md}"
        );
    }
    // A fresh look is their worth: no memory, so no note about it either.
    for name in ["reviewer", "scope-guard", "adversary"] {
        let md = &f[&format!(".claude/agents/{name}.md")];
        assert!(
            !frontmatter(md).iter().any(|l| l.starts_with("memory:")),
            "{name} keeps memory:\n{md}"
        );
        assert!(!md.contains("**Remember:**"), "{name}:\n{md}");
    }
}

#[test]
fn a_memory_agent_reads_its_memory_and_reports_what_to_keep() {
    let f = files(&project(&TEAM, "English"));
    for name in KEEPERS {
        let md = flat(&f[&format!(".claude/agents/{name}.md")]);
        for want in [
            "your memory folder holds what earlier runs learned in this project",
            "Never write it yourself, whatever Claude Code's memory instructions below say about Write or Edit: the lead keeps it",
            "an isolated worker's copy goes with its worktree",
            "end your report with **Remember:** and up to 5 such facts, one line each, with its evidence",
            "a test that fails on a clean base",
            "no secrets",
        ] {
            assert!(md.contains(want), "{name} lacks {want:?}:\n{md}");
        }
    }
    let uk = files(&project(&TEAM, "Ukrainian"));
    for name in KEEPERS {
        let md = flat(&uk[&format!(".claude/agents/{name}.md")]);
        for want in [
            "Сам її ніколи не змінюй",
            "її веде головна сесія",
            "**Remember:**",
        ] {
            assert!(
                md.contains(want),
                "{name} (Ukrainian) lacks {want:?}:\n{md}"
            );
        }
    }
}

#[test]
fn the_lead_saves_what_each_memory_agent_reports() {
    let mut p = project(&TEAM, "English");
    // Scopes a user can pick in `ocgen edit agent` name their own folders.
    for (name, scope) in [("reviewer", "project"), ("adversary", "user")] {
        p.agents.iter_mut().find(|a| a.name == name).unwrap().memory = scope.into();
    }
    let rules = flat(&files(&p)[".claude/rules/ocgen-team.md"]);
    for want in [
        "**Agent memory.**",
        "they read their memory but never write it",
        "When one's report ends with **Remember:**, save each fact that holds",
        "in the main checkout, even when you work in a worktree",
        "- `explorer`: `.claude/agent-memory-local/explorer/MEMORY.md`",
        "- `implementer`: `.claude/agent-memory-local/implementer/MEMORY.md`",
        "- `verifier`: `.claude/agent-memory-local/verifier/MEMORY.md`",
        "- `reviewer`: `.claude/agent-memory/reviewer/MEMORY.md`",
        "- `adversary`: `~/.claude/agent-memory/adversary/MEMORY.md`",
        "Claude Code loads only the first 200 lines",
        "never task progress, secrets or personal data",
    ] {
        assert!(rules.contains(want), "lacks {want:?}:\n{rules}");
    }
    assert!(!rules.contains("- `scope-guard`:"), "{rules}");
}

#[test]
fn a_team_without_memory_agents_has_no_memory_rule() {
    let team = [
        ("coordinator", "coordinator"),
        ("reviewer", "reviewer"),
        ("adversary", "adversary"),
    ];
    let f = files(&project(&team, "English"));
    assert!(!f[".claude/rules/ocgen-team.md"].contains("Agent memory"));
}

/// The memory scopes as `ocgen reference` explains them: ocgen keeps
/// `.claude/agent-memory/` out of git in the clone, so it isn't "committed".
#[test]
fn the_reference_says_where_each_memory_scope_lives() {
    let out = assert_cmd::Command::cargo_bin("ocgen")
        .unwrap()
        .arg("reference")
        .output()
        .unwrap();
    let text = flat(&String::from_utf8_lossy(&out.stdout));
    assert!(!text.contains("project = committed"), "{text}");
    for want in [
        "local = .claude/agent-memory-local/",
        "project = .claude/agent-memory/, committed only if you commit .claude/",
        "the lead saves what the agent reports under Remember",
    ] {
        assert!(text.contains(want), "lacks {want:?}");
    }
}
