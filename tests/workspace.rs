//! Every project ocgen sets up tells its agents to write only inside the project
//! and nowhere else unless the user allows it. A Claude project says it once, in
//! the workflow rule every session loads (its subagents and teammates load it
//! too); every OpenCode agent's text says it, in the instruction language.

use std::collections::BTreeMap;
use std::fs;

use ocgen::agent::{self, Agent};
use ocgen::claude::Workflow;
use ocgen::manifest::Manifest;
use ocgen::render::{ChangeKind, Project};
use ocgen::target::Target;
use ocgen::templates;
use tempfile::tempdir;

const RULE: &str = ".claude/rules/ocgen-workflow.md";
const EN: &str = "## Where you write";
const UK: &str = "## Куди ти пишеш";

/// The shipped manifest — never a developer's override.
fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

fn claude(lang: &str) -> Project {
    let mut p = Project::from_manifest(&manifest(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "ws".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline(lang).unwrap();
    p
}

fn opencode(lang: &str) -> Project {
    let mut p = Project::from_manifest(&manifest(), lang);
    p.project_name = "ws".into();
    p.agents = agent::default_pipeline(lang, "mac").unwrap();
    p
}

fn rendered(p: &Project) -> BTreeMap<String, String> {
    p.render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect()
}

/// `text` on one line, so a phrase wrapped across lines still matches.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn told(text: &str, mark: &str) -> usize {
    text.matches(mark).count()
}

/// The section that starts at `mark`, up to `until`.
fn section<'a>(text: &'a str, mark: &str, until: &str) -> &'a str {
    let start = text.find(mark).unwrap();
    let end = start + text[start..].find(until).unwrap();
    &text[start..end]
}

// ------------------------------------------------------------ Claude Code --

#[test]
fn the_claude_workflow_rule_says_it_once_in_both_instruction_languages() {
    for lang in ["English", "Ukrainian"] {
        let files = rendered(&claude(lang));
        // The workflow rule is English whatever the instruction language.
        let rule = &files[RULE];
        assert_eq!(told(rule, EN), 1, "{lang}:\n{rule}");
        assert_eq!(told(rule, UK), 0, "{lang}:\n{rule}");
        // Subagents and teammates load the rule, so nothing else repeats it.
        for (rel, text) in &files {
            if rel != RULE {
                assert_eq!(told(text, EN) + told(text, UK), 0, "{lang} {rel}");
            }
        }
    }
}

#[test]
fn a_claude_project_with_nothing_else_turned_on_still_gets_it() {
    let mut p = claude("English");
    p.agents.clear();
    p.claude.workflow = Workflow {
        intake: false,
        refine: false,
        improve_prompt: false,
        fanout: false,
        verify_todos: false,
        prefer_explorer: false,
        deliver: false,
        workstreams: false,
        inquire: false,
        intent: false,
        recap: false,
        subagent_confidence: 0,
        loop_guard_max: 0,
        check_cmd: String::new(),
    };
    let rule = &rendered(&p)[RULE];
    assert_eq!(told(rule, EN), 1, "{rule}");
}

#[test]
fn the_rule_keeps_every_write_inside_the_project() {
    let rule = &rendered(&claude("English"))[RULE];
    let block = flat(section(rule, EN, "\n## Shell commands"));
    for must in [
        "Write only inside this project: the folder the session started in and everything under it",
        "worktrees under `.claude/worktrees/` included",
        "Outside it, create, change, move or delete nothing unless the user has explicitly allowed \
         that path or action in this conversation",
        "your home folder, its dotfiles and tool config (`~/.config`, `~/.claude`, `~/.gitconfig`",
        "`/tmp` and every other temp folder (a scratchpad your tool offers included)",
        "other repositories and projects, and system folders",
        "installing anything globally (`npm install -g`, `pip install --user`, `cargo install`",
        "It holds for every tool: Write and Edit, and every command",
        "redirections (`>`, `>>`), `cp`, `mv`, `tee`, `ln`, `mkdir`, `touch`",
        "output options (`-o`, `--output`, `--out-dir`)",
        "`git clone` or `git worktree add` into another folder, `git config --global`",
        "Scratch files go in this project's `.claude/scratch/`, never in a temp folder",
        "a `.gitignore` holding the one line `*`",
        "Never point their output, an install or their config anywhere outside the project",
        "stop and ask the user first: say exactly what would be written, and where",
        "An approval covers only what the user named, for this task",
        "(a built-in one such as Explore, Plan or general-purpose), put this section in its prompt",
    ] {
        assert!(block.contains(must), "missing {must:?} in:\n{block}");
    }
}

#[test]
fn a_project_made_before_gets_it_from_doctor() {
    let p = claude("English");
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let rule = fs::read_to_string(dir.path().join(RULE)).unwrap();
    let old = rule.replace(section(&rule, EN, "## Shell commands"), "");
    fs::write(dir.path().join(RULE), &old).unwrap();
    let loaded = Project::load_state(dir.path()).unwrap();
    let plan = loaded.plan_changes(dir.path()).unwrap();
    let modified: Vec<&str> = plan
        .iter()
        .filter(|c| c.kind != ChangeKind::Unchanged)
        .map(|c| c.rel.as_str())
        .collect();
    assert_eq!(modified, [RULE]);
}

// --------------------------------------------------------------- OpenCode --

#[test]
fn every_opencode_agent_is_told_in_the_instruction_language() {
    for (lang, mark, other) in [("English", EN, UK), ("Ukrainian", UK, EN)] {
        let p = opencode(lang);
        let files = rendered(&p);
        assert!(p.agents.iter().any(|a| a.mode == "primary"));
        for a in &p.agents {
            let mut rels = vec![format!(".opencode/agents/{}.md", a.name)];
            if a.prompt_file && a.prompt_body.is_some() {
                rels.push(format!(".opencode/prompts/{}.txt", a.name));
            }
            for rel in rels {
                let text = &files[&rel];
                assert_eq!(told(text, mark), 1, "{lang} {rel}:\n{text}");
                assert_eq!(told(text, other), 0, "{lang} {rel}:\n{text}");
                // Before the mindset, which closes the agent's own text.
                assert!(
                    text.find(mark).unwrap() < text.find("<mindset>").unwrap(),
                    "{lang} {rel}:\n{text}"
                );
            }
        }
    }
}

#[test]
fn a_custom_opencode_agent_is_told_too() {
    let mut p = opencode("English");
    let mut own = Agent::blank("docs-writer", "docs", "mac");
    own.body = "You write the docs.".into();
    p.agents.push(own);
    let md = &rendered(&p)[".opencode/agents/docs-writer.md"];
    assert!(
        md.find("You write the docs.").unwrap() < md.find(EN).unwrap(),
        "{md}"
    );
    assert_eq!(told(md, EN), 1, "{md}");
}

#[test]
fn the_ukrainian_text_says_the_same() {
    let p = opencode("Ukrainian");
    let a = p.agents.iter().find(|a| a.mode != "primary").unwrap();
    let md = &rendered(&p)[&format!(".opencode/agents/{}.md", a.name)];
    let block = flat(section(md, UK, "<mindset>"));
    for must in [
        "Пиши лише всередині цього проєкту",
        "разом із робочими деревами в `.claude/worktrees/`",
        "якщо користувач явно не дозволив цей шлях чи цю дію в цій розмові",
        "у `/tmp` чи будь-яку іншу тимчасову теку",
        "нічого не встановлюй глобально",
        "Це стосується кожного інструмента: Write і Edit, а також кожної команди",
        "`git config --global`",
        "Тимчасові файли клади в `.claude/scratch/` цього проєкту",
        "зупинись і спершу запитай користувача",
        "Дозвіл стосується лише того, що користувач назвав",
        "додай цей розділ до тексту його завдання",
    ] {
        assert!(block.contains(must), "missing {must:?} in:\n{block}");
    }
    // Ukrainian, never Russian.
    for letter in ['ы', 'э', 'ъ', 'ё'] {
        assert!(!block.contains(letter), "{letter} in:\n{block}");
    }
}
