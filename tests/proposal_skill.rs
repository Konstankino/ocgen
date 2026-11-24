//! The `/proposal` workflow skill: a short proposal page for another team, with
//! only the diagrams its readers need, published as a private Claude Artifact.
//! It works in small steps, one working file each (facts, the page's text, one
//! file per diagram), and puts the page together from them, so what was checked
//! is what gets published.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::claude::{Output, Skill};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::Value;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/proposal/SKILL.md";
const DIAGRAMS: &str = ".claude/skills/proposal/diagrams.md";
const PAGE: &str = ".claude/skills/proposal/page.html";
const STATE: &str = ".claude/.ocgen-state.json";

/// Exactly the tools /proposal may use without asking. The `Edit` rule is
/// scoped: a bare `Edit` or `Write` is flagged on a model-invocable skill.
const PROPOSAL_TOOLS: [&str; 7] = [
    "Bash(git rev-parse:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git status:*)",
    "Bash(ls:*)",
    "Bash(wc:*)",
    "Edit(./.claude/notes/proposals/**)",
];

/// The seven kinds of diagram the skill can draw.
const KINDS: [&str; 7] = [
    "System context",
    "Container / component",
    "Deployment",
    "Network",
    "Entity-relationship (ER)",
    "Sequence",
    "Flowchart / activity",
];

fn claude(name: &str) -> Project {
    let m = Manifest::load().expect("manifest loads");
    let mut p = Project::from_manifest(&m, "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn scaffolded(p: &Project) -> (tempfile::TempDir, String) {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    (dir, md)
}

/// The skill's frontmatter lines (between the `---` fences).
fn frontmatter(md: &str) -> Vec<String> {
    md.strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn field(front: &[String], key: &str) -> String {
    front
        .iter()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_default()
        .to_string()
}

/// The text with its line breaks gone, so a phrase can be found however it wraps.
fn flat(md: &str) -> String {
    md.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

/// Each phrase appears, in this order.
fn in_order(text: &str, phrases: &[&str]) {
    let mut at = 0;
    for p in phrases {
        let found = text[at..]
            .find(p)
            .unwrap_or_else(|| panic!("{p:?} missing or out of order"));
        at += found + p.len();
    }
}

#[test]
fn proposal_is_a_model_invocable_skill_with_exact_tools() {
    let (dir, md) = scaffolded(&claude("pr"));
    let front = frontmatter(&md);
    assert_eq!(front.first().map(String::as_str), Some("name: proposal"));
    // description first in the template: legacy-command cleanup reads it.
    assert!(front[1].starts_with("description: "), "{front:?}");
    assert!(
        field(&front, "when_to_use").starts_with("Use when "),
        "{front:?}"
    );
    assert!(
        !front
            .iter()
            .any(|l| l.starts_with("disable-model-invocation")),
        "Claude may write a proposal when asked to: {front:?}"
    );
    assert!(front.iter().any(|l| l == "effort: high"), "{front:?}");
    assert!(front.iter().any(|l| l.starts_with("model: ")), "{front:?}");
    let mut keys: Vec<&str> = front
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, _)| k)
        .collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate frontmatter keys: {front:?}");
    let tools: Vec<String> = field(&front, "allowed-tools")
        .split(", ")
        .map(String::from)
        .collect();
    assert_eq!(tools, PROPOSAL_TOOLS.map(String::from).to_vec());
    assert_eq!(
        ocgen::claude::PROPOSAL_TOOLS.to_vec(),
        PROPOSAL_TOOLS.to_vec(),
        "the shipped list is the pinned one"
    );
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    assert!(md.lines().count() < 500, "{} lines", md.lines().count());
    assert!(
        md.len() < 20_000,
        "{} bytes: compaction would cut it",
        md.len()
    );
    assert!(ocgen::claude::WORKFLOW_SKILLS.contains(&"proposal"));
    assert!(!ocgen::claude::USER_RUN_WORKFLOWS.contains(&"proposal"));

    // The skill rules ocgen checks in yours hold for it too, its supporting files
    // included: a long diagrams.md needs a contents list, and SKILL.md links it.
    let as_skill = Skill {
        name: field(&front, "name"),
        description: field(&front, "description"),
        when_to_use: field(&front, "when_to_use"),
        allowed_tools: field(&front, "allowed-tools"),
        body: md.clone(),
        ..Default::default()
    };
    let issues = ocgen::claude::skill_issues(&as_skill);
    assert!(issues.is_empty(), "{issues:?}");
    let issues = ocgen::claude::skill_file_issues(&dir.path().join(".claude/skills/proposal"), &md);
    assert!(issues.is_empty(), "{issues:?}");
}

/// The recipes and the page template sit beside SKILL.md, read only when needed.
#[test]
fn it_ships_the_diagram_recipes_and_a_self_contained_page() {
    let (dir, md) = scaffolded(&claude("pr"));
    assert!(md.contains("${CLAUDE_SKILL_DIR}/diagrams.md"), "{md}");
    assert!(md.contains("${CLAUDE_SKILL_DIR}/page.html"), "{md}");

    let page = read(dir.path(), PAGE);
    // An Artifact wraps the page in its own document: the title comes first,
    // and the page brings no skeleton of its own.
    assert!(page.starts_with("<title>"), "{page}");
    // (`<head>` exactly: the page's own `<header>` starts the same way.)
    for never in ["<!doctype", "<html", "<head>", "<body"] {
        assert!(
            !page.to_lowercase().contains(never),
            "{never:?} in page.html"
        );
    }
    // Self-contained: no scripts and nothing fetched.
    for never in ["<script", "<link", "@import", "http"] {
        assert!(!page.contains(never), "{never:?} in page.html");
    }
    // What an Artifact page needs: colour tokens, both dark-mode guards, an
    // explicit background, a side gutter that clears a phone's system bar (with
    // the tag that promises it), and print from the local file.
    for must in [
        "<meta name=\"claude:viewport-fit-sides\" content=\"cover\">",
        "padding-left: max(16px, env(safe-area-inset-left, 0px))",
        "padding-right: max(16px, env(safe-area-inset-right, 0px))",
        "padding-block",
        ":root {",
        "@media (prefers-color-scheme: dark)",
        ":root:not([data-theme=\"light\"])",
        ":root[data-theme=\"dark\"]",
        "background: var(--bg)",
        "overflow-x: auto",
        "@media print",
        "break-inside: avoid",
    ] {
        assert!(page.contains(must), "missing {must:?} in page.html");
    }
    // Every class a recipe draws with is styled by the page.
    let recipes = read(dir.path(), DIAGRAMS);
    for class in [
        ".box", ".new", ".ext", ".group", ".line", ".reply", ".dev", ".head", ".label", ".title",
        ".sub", ".step", ".check", ".note", ".life",
    ] {
        assert!(
            page.contains(&format!("svg {class}")),
            "page.html lacks {class}"
        );
        let name = class.trim_start_matches('.');
        assert!(
            recipes.contains(&format!("`{name}`")),
            "diagrams.md never names {name}"
        );
    }
    // In the page, the template's own words are all English.
    assert!(!has_cyrillic(&page) && !has_cyrillic(&recipes));
}

/// All seven kinds are there, each with when to draw it, and only what the
/// readers need goes on the page.
#[test]
fn it_knows_seven_diagrams_and_draws_only_the_needed_ones() {
    let (dir, md) = scaffolded(&claude("pr"));
    let recipes = read(dir.path(), DIAGRAMS);
    for kind in KINDS {
        assert!(
            md.contains(&format!("| {kind} |")),
            "SKILL.md table lacks {kind}"
        );
    }
    for recipe in [
        "## Boxes: system context, container / component, deployment, network",
        "## Sequence",
        "## Entity-relationship (ER)",
        "## Flowchart / activity",
        "## Checklist",
    ] {
        assert!(recipes.contains(recipe), "diagrams.md lacks {recipe:?}");
    }
    // A contents list first: Claude may read only the top of a long file.
    assert!(recipes.contains("## Contents"), "{recipes}");
    let text = flat(&md);
    for must in [
        "Pick only the diagrams the readers need",
        "Usually 2 or 3",
        "A diagram that holds for every option goes once, under Today → Goal",
        "only where the options differ, with the same kind for each",
        "on a flow that is new from end to end, nothing is `new`",
        "data flow and its transformations",
        "the `new` style",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
    // Two inline SVGs on one page must not share an arrowhead id.
    assert!(flat(&recipes).contains("must not share an id"), "{recipes}");
}

/// The working files: one per step, so an interrupted session picks up where it
/// stopped, and the page is only put together from them.
#[test]
fn it_works_in_small_steps_with_one_file_each() {
    let (_dir, md) = scaffolded(&claude("pr"));
    for must in [
        ".claude/notes/proposals/.gitignore",
        ".claude/notes/proposals/<slug>/",
        "`facts.md`",
        "`outline.md`",
        "`diagrams/<kind>-<name>.svg`",
        "`<slug>.html`",
        "`wc -w`",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    let text = flat(&md);
    for must in [
        "one diagram at a time",
        "Add no content of your own",
        "exact path",
        "picks up where it stopped",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
    in_order(
        &md,
        &[
            "## 0. Start",
            "## 1. Ask only what's missing",
            "## 2. Facts",
            "## 3. Choose the diagrams",
            "## 4. Text",
            "## 5. Diagrams",
            "## 6. Cold read",
            "## 7. Page",
            "## 8. Publish a private Artifact",
            "## 9. Hand off",
        ],
    );
}

/// Less is more: the sections in the order a reader needs them, with limits a
/// check can count, and a cold reader to catch what is hard to follow.
#[test]
fn it_keeps_the_page_short_and_plain() {
    let (_dir, md) = scaffolded(&claude("pr"));
    in_order(
        &md,
        &[
            "**Header:**",
            "**In short:**",
            "**Today → Goal:**",
            "**Options:**",
            "**Compare:**",
            "**What we ask <team> to do:**",
            "**Questions for <team>:**",
            "**Risks and open points:**",
            "**Sources:**",
        ],
    );
    let text = flat(&md);
    for must in [
        "at most 3 questions in all",
        "Never ask what the request, the code or the docs answer",
        "at most 550, which leaves about 500 words of text",
        "`wc` counts the Markdown signs",
        "One fact per sentence, at most 20 words",
        "at most 12 words a cell",
        "at most 7 numbered lines",
        "No bare step numbers or codes",
        // Options are named for what they do (the rule on naming what you refer
        // back to): never "Option B".
        "never a letter or a number",
        "never shrink the type",
        "fresh, read-only subagent",
        "read twice",
        "at most 2 rounds",
        "if a fact the readers need to act is gone, put it back",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
}

/// The end result is a new private Artifact with a proper name, updated in place
/// later, and never shared by Claude.
#[test]
fn it_publishes_a_private_artifact_with_a_proper_name() {
    let (_dir, md) = scaffolded(&claude("pr"));
    let text = flat(&md);
    for must in [
        "`action: \"quickstart\"` and `intent: \"other\"`",
        "2 to 4 words naming the change",
        "never \"Proposal: …\"",
        "`icon: \"diagram\"`",
        "one-sentence `description`",
        "`Artifact: <url>`",
        "`action: \"read\"`",
        "that same `url`",
        "Never share, pin or delete it",
        "no keys, tokens, passwords, account IDs or IP addresses",
        "confidential or sensitive",
        "**No `Artifact` tool**",
        "ToolSearch",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
}

/// The shared evidence rules, and how they fit a page with no citations.
#[test]
fn it_carries_the_evidence_rules_and_marks_what_is_unconfirmed() {
    let (_dir, md) = scaffolded(&claude("pr"));
    assert!(md.contains("**evidence-required.**"), "{md}");
    assert!(md.contains("**test-the-evidence.**"), "{md}");
    let text = flat(&md);
    for must in [
        "`(to confirm)`",
        "becomes a question for the readers",
        // What a document says, unchecked, is its own tag; what only we can
        // confirm is ours to chase, not a question for the readers.
        "Documented (a document says it and nothing checked it)",
        "if only we can, it goes under Risks and open points",
        "only what they can answer",
        "marked `example`",
        "Change nothing in the project",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
}

/// The page is in English whatever the answer language; the skill's text is too.
#[test]
fn the_page_is_english_and_the_skill_does_not_change_with_the_answers() {
    let (_dir, english) = scaffolded(&claude("pr"));
    let mut p = claude("pr");
    p.response_language = "Ukrainian".into();
    let (_dir, ukrainian) = scaffolded(&p);
    assert_eq!(english, ukrainian);
    assert!(!has_cyrillic(&english), "commands stay English");
    assert!(
        flat(&english).contains("in English unless I ask for another language"),
        "{english}"
    );
}

#[test]
fn turning_it_off_removes_the_skill_and_its_files() {
    let mut p = claude("pr");
    p.claude.workflow.proposal = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(".claude/skills/proposal").exists());
    assert!(!p.proposal_enabled());
}

/// `doctor` after turning it off removes all three files it wrote before.
#[test]
fn rewriting_with_it_off_removes_the_files_written_before() {
    let dir = tempdir().unwrap();
    let mut p = claude("pr");
    p.scaffold(dir.path(), false).unwrap();
    for rel in [SKILL, DIAGRAMS, PAGE] {
        assert!(dir.path().join(rel).is_file(), "{rel}");
    }
    p.claude.workflow.proposal = false;
    p.scaffold(dir.path(), true).unwrap();
    for rel in [SKILL, DIAGRAMS, PAGE] {
        assert!(!dir.path().join(rel).exists(), "{rel} left behind");
    }
}

#[test]
fn an_old_state_gains_the_skill_and_an_explicit_choice_is_kept() {
    let dir = tempdir().unwrap();
    let mut p = claude("pr");
    p.claude.workflow.proposal = false;
    p.scaffold(dir.path(), false).unwrap();
    let path = dir.path().join(STATE);
    let mut v: Value = serde_json::from_str(&read(dir.path(), STATE)).unwrap();
    let workflow = v["claude"]["workflow"]
        .as_object_mut()
        .expect("the workflow block");
    assert_eq!(workflow.remove("proposal"), Some(Value::Bool(false)));
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let loaded = Project::load_state(dir.path()).unwrap();
    assert!(
        loaded.claude.workflow.proposal,
        "an old state gains /proposal"
    );
    assert!(loaded.proposal_enabled());
    // An explicit choice is kept.
    p.scaffold(dir.path(), true).unwrap();
    let kept = Project::load_state(dir.path()).unwrap();
    assert!(!kept.claude.workflow.proposal);
}

/// A user's own `proposal` skill keeps its name, and its folder gets none of the
/// generated skill's files.
#[test]
fn a_user_skill_named_proposal_disables_the_generated_one() {
    let mut p = claude("pr");
    p.skills.push(Skill {
        name: "proposal".into(),
        description: "My own proposal writer — use when I ask for a proposal of my kind".into(),
        body: "Write my kind of proposal.".into(),
        ..Default::default()
    });
    assert!(p.claude.workflow.proposal && !p.proposal_enabled());
    assert!(
        p.issues()
            .iter()
            .any(|i| i.contains("skill 'proposal'") && i.contains("collides")),
        "{:?}",
        p.issues()
    );
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    assert!(md.contains("Write my kind of proposal."), "{md}");
    assert!(!dir.path().join(DIAGRAMS).exists());
    assert!(!dir.path().join(PAGE).exists());
}

/// A `/proposal` command written by hand is shadowed by the generated skill; with
/// /proposal off nothing shadows.
#[test]
fn a_hand_made_proposal_command_is_reported() {
    let dir = tempdir().unwrap();
    let p = claude("pr");
    p.scaffold(dir.path(), false).unwrap();
    assert!(p.skill_file_issues(dir.path()).is_empty());
    let command = dir.path().join(".claude/commands/proposal.md");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    let mine = "---\ndescription: My proposal\n---\nWrite a proposal for $ARGUMENTS\n";
    fs::write(&command, mine).unwrap();
    let issues = p.skill_file_issues(dir.path());
    assert!(
        issues
            .iter()
            .any(|i| i.contains("hand-made /proposal command") && i.contains("shadowed")),
        "{issues:?}"
    );

    let mut off = claude("pr");
    off.claude.workflow.proposal = false;
    assert!(off.skill_file_issues(dir.path()).is_empty());
}

#[test]
fn the_plugin_carries_the_skill_and_its_files() {
    let mut p = claude("pr");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "pr".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    for file in ["SKILL.md", "diagrams.md", "page.html"] {
        assert!(
            dir.path()
                .join(format!("plugin/pr/skills/proposal/{file}"))
                .is_file(),
            "{file}"
        );
    }
    let readme = read(dir.path(), "plugin/pr/README.md");
    assert!(readme.contains("`/proposal`"), "{readme}");

    p.claude.workflow.proposal = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join("plugin/pr/skills/proposal").exists());
    assert!(!read(dir.path(), "plugin/pr/README.md").contains("/proposal"));
}

/// /refine is for talking a plan through in chat; a page for others is
/// /proposal's. Each says so only when the other is there.
#[test]
fn refine_and_proposal_point_at_each_other() {
    let (dir, md) = scaffolded(&claude("pr"));
    let refine = read(dir.path(), ".claude/skills/refine/SKILL.md");
    assert!(
        field(&frontmatter(&refine), "when_to_use").contains("(/proposal)"),
        "{refine}"
    );
    assert!(
        field(&frontmatter(&md), "when_to_use").contains("(/refine)"),
        "{md}"
    );

    let mut p = claude("pr");
    p.claude.workflow.proposal = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let refine = read(dir.path(), ".claude/skills/refine/SKILL.md");
    assert!(!refine.contains("/proposal"), "{refine}");

    let mut p = claude("pr");
    p.claude.workflow.refine = false;
    let (_dir, md) = scaffolded(&p);
    assert!(!md.contains("/refine"), "{md}");
}

/// A proposal's files sit one folder down: the notes view must not take them for
/// ledgers. Nothing here is new behaviour; the skill relies on it.
#[test]
fn the_notes_view_leaves_proposals_alone() {
    use ocgen::notes::{ledger_target, Target as NoteTarget};
    for path in [
        ".claude/notes/proposals/vendor-connect/facts.md",
        ".claude/notes/proposals/vendor-connect/outline.md",
        ".claude/notes/proposals/vendor-connect/diagrams/sequence-api-does-the-work.svg",
        ".claude/notes/proposals/vendor-connect/vendor-connect.html",
    ] {
        assert_eq!(ledger_target(path), NoteTarget::NotLedger, "{path}");
    }
}

/// What the probe runs needed when the happy path wasn't there: no git, no
/// subagent, no browser, a spec too exact for prose. And nothing reaches the
/// Artifact service before a confidentiality check.
#[test]
fn it_has_a_way_on_when_a_tool_or_fact_is_missing() {
    let (_dir, md) = scaffolded(&claude("pr"));
    let text = flat(&md);
    for must in [
        "Outside a git repository, carry on",
        "If no subagent can be started, read those files cold yourself",
        "Read only these files.",
        "if you can't, say so in the hand-off",
        "`spec.md`",
        "at most 12 lines",
        "`context`, `container`, `deployment`, `network`, `er`, `sequence`, `flowchart`",
        "a date (or none yet)",
        "We chose …, when the choice is ours",
    ] {
        assert!(text.contains(must), "missing {must:?}");
    }
    let ask = text
        .find("If I called the material confidential or sensitive, ask before you go on")
        .expect("the confidentiality check");
    let quickstart = text.find("`action: \"quickstart\"`").expect("quickstart");
    assert!(
        ask < quickstart,
        "confidential material is checked before any Artifact call"
    );
}

/// The recipes' own examples keep the rules they teach: every shape 16px inside
/// the viewBox, the size equal to it, and every arrow pointing at a marker the
/// same SVG defines, named after its file.
#[test]
fn the_recipe_examples_keep_their_own_rules() {
    let (dir, _md) = scaffolded(&claude("pr"));
    let recipes = read(dir.path(), DIAGRAMS);
    let num = |tag: &str, attr: &str| -> f64 {
        let key = format!(" {attr}=\"");
        let at = tag.find(&key).unwrap_or_else(|| panic!("{attr} in {tag}")) + key.len();
        tag[at..].split('"').next().unwrap().parse().unwrap()
    };
    let blocks: Vec<&str> = recipes
        .split("```svg\n")
        .skip(1)
        .map(|b| b.split("```").next().unwrap())
        .filter(|b| b.starts_with("<svg"))
        .collect();
    assert_eq!(blocks.len(), 2, "the boxes and sequence examples");
    for svg in blocks {
        let head = svg.lines().next().unwrap();
        let vb: Vec<f64> = head
            .split("viewBox=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .split(' ')
            .map(|n| n.parse().unwrap())
            .collect();
        let (w, h) = (vb[2], vb[3]);
        assert_eq!(num(head, "width"), w, "{head}");
        assert_eq!(num(head, "height"), h, "{head}");
        for rect in svg.lines().filter(|l| l.trim_start().starts_with("<rect")) {
            let (x, y) = (num(rect, "x"), num(rect, "y"));
            let (rw, rh) = (num(rect, "width"), num(rect, "height"));
            assert!(x >= 16.0 && y >= 16.0, "too near the top or left: {rect}");
            assert!(x + rw <= w - 16.0, "too near the right: {rect}");
            assert!(y + rh <= h - 16.0, "too near the bottom: {rect}");
        }
        let id = svg
            .split("<marker id=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("an arrowhead");
        assert!(id.starts_with("arrow-"), "{id}");
        for (i, _) in svg.match_indices("marker-end=\"url(#") {
            let used = svg[i + 17..].split(')').next().unwrap();
            assert_eq!(
                used, id,
                "an arrow points at a marker this SVG doesn't define"
            );
        }
    }
}
