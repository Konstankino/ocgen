//! The `/chapter` workflow skill: one plain-language chapter about a part of the
//! system, written from the docs, code, git history and GitHub, as text a
//! text-to-speech tool reads aloud. Its evidence goes in a separate sources file,
//! because such a tool reads everything it is given.

use std::fs;
use std::path::Path;

use ocgen::agent;
use ocgen::claude::{Output, Skill};
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::Value;
use tempfile::tempdir;

const SKILL: &str = ".claude/skills/chapter/SKILL.md";
const STATE: &str = ".claude/.ocgen-state.json";

/// Exactly the tools /chapter may use without asking. The `Edit` rules are
/// scoped: a bare `Edit` or `Write` is flagged on a model-invocable skill.
const CHAPTER_TOOLS: [&str; 10] = [
    "Bash(git rev-parse:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git blame:*)",
    "Bash(git status:*)",
    "Bash(git diff:*)",
    "Bash(git remote)",
    "Bash(wc:*)",
    "Edit(./.claude/notes/book/**)",
    "Edit(./.claude/notes/github/**)",
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

fn tools(md: &str) -> Vec<String> {
    frontmatter(md)
        .iter()
        .find_map(|l| l.strip_prefix("allowed-tools: ").map(String::from))
        .expect("allowed-tools")
        .split(", ")
        .map(String::from)
        .collect()
}

/// The text with its line breaks gone, so a phrase can be found however it wraps.
fn flat(md: &str) -> String {
    md.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

#[test]
fn chapter_is_a_model_invocable_skill_with_exact_tools() {
    let (dir, md) = scaffolded(&claude("ch"));
    let front = frontmatter(&md);
    assert_eq!(front.first().map(String::as_str), Some("name: chapter"));
    // description first in the template: legacy-command cleanup reads it.
    assert!(front[1].starts_with("description: "), "{front:?}");
    assert!(
        front
            .iter()
            .any(|l| l.starts_with("when_to_use: Use when ")),
        "{front:?}"
    );
    assert!(
        !front
            .iter()
            .any(|l| l.starts_with("disable-model-invocation")),
        "Claude may write a chapter when asked to: {front:?}"
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
    assert_eq!(tools(&md), CHAPTER_TOOLS.map(String::from).to_vec());
    assert_eq!(
        ocgen::claude::CHAPTER_TOOLS.to_vec(),
        CHAPTER_TOOLS.to_vec(),
        "the shipped list is the pinned one"
    );
    assert!(!md.contains("{{") && !md.contains("{%"), "unrendered Jinja");
    assert!(md.lines().count() < 500, "{} lines", md.lines().count());
    assert!(
        md.len() < 20_000,
        "{} bytes: compaction would cut it",
        md.len()
    );
    assert!(ocgen::claude::WORKFLOW_SKILLS.contains(&"chapter"));
    assert!(!ocgen::claude::USER_RUN_WORKFLOWS.contains(&"chapter"));

    // The skill rules ocgen checks in yours hold for it too.
    let field = |key: &str| {
        front
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")))
            .unwrap_or_default()
            .to_string()
    };
    let as_skill = Skill {
        name: field("name"),
        description: field("description"),
        when_to_use: field("when_to_use"),
        allowed_tools: field("allowed-tools"),
        body: md.clone(),
        ..Default::default()
    };
    let issues = ocgen::claude::skill_issues(&as_skill);
    assert!(issues.is_empty(), "{issues:?}");
    let issues = ocgen::claude::skill_file_issues(&dir.path().join(".claude/skills/chapter"), &md);
    assert!(issues.is_empty(), "{issues:?}");
}

/// `tests/guidance.rs` takes its expectation from `command_guidance`, so it can't
/// catch a missing arm: an unlisted name silently gets no rules at all.
#[test]
fn it_carries_the_evidence_rules_and_says_how_they_fit_read_aloud_text() {
    let (_dir, md) = scaffolded(&claude("ch"));
    assert!(md.contains("**evidence-required.**"), "{md}");
    assert!(md.contains("**test-the-evidence.**"), "{md}");
    assert!(md.contains("in the sources file"), "{md}");
    assert!(md.contains("Behaviour must rest on code"), "{md}");
}

/// What the first real chapter had to guess at: each of these changed what it
/// wrote or how it checked it, so each stays settled in the text.
#[test]
fn it_settles_what_a_chapter_would_otherwise_guess() {
    let (_dir, md) = scaffolded(&claude("ch"));
    let md = flat(&md);
    for must in [
        // Evidence: a command's output counts, and general knowledge has a label.
        "the command and what it printed",
        "`General knowledge`",
        // Code says what the system is told to do, not how a cloud service acts.
        "how a cloud service will act on it",
        // One row per fact the chapter relies on, not one per sentence.
        "not per sentence",
        // Running tests is allowed, but writes stay out of the project.
        "scratch copy outside the project",
        // `git log` can't see uncommitted work or a staged rename.
        "git status --short -- <its Files read>",
        "Uncommitted:",
        // A shell-free reviewer can't open everything: the leftovers are yours.
        "any you couldn't open or re-run",
        // The check ends clean, and the pattern's engine and digits are covered.
        "after the last fixes",
        "`grep -E` reads this pattern differently",
        "`\\d`",
        // A wall of definitions doesn't work by ear.
        "ten or so",
        // A vendor's reference shows what it says, not what happens here.
        "A vendor's reference",
        // The copy lasts through the check, and a big rewrite gets looked at again.
        "once the check is done",
        "changed more than wording",
        // The change check hands these to git, so they're plain paths.
        "plain repository paths",
        "add this chapter to its why",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
}

#[test]
fn it_keeps_the_book_in_its_own_ignored_folder() {
    let (_dir, md) = scaffolded(&claude("ch"));
    for must in [
        ".claude/notes/book/.gitignore",
        ".claude/notes/book/000-contents.txt",
        ".claude/notes/book/glossary.txt",
        "`<NNN>-<slug>.txt`",
        "`sources/<NNN>-<slug>.txt`",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    // Search tools skip a git-ignored folder, so a Grep over it finds nothing and
    // passes; a Glob misses the book.
    assert!(md.contains("exact path"), "{md}");
    // A linked worktree can't see the book: its numbers would collide.
    assert!(md.contains("--git-common-dir"), "{md}");
    assert!(md.contains("main checkout"), "{md}");
    // An existing chapter is never overwritten silently.
    assert!(md.contains("Never overwrite"), "{md}");
}

#[test]
fn it_reads_github_the_way_the_request_file_allows() {
    let (_dir, md) = scaffolded(&claude("ch"));
    for must in [
        ".claude/notes/github/request.json",
        "one request at a time",
        "commits/<sha>/pulls",
        "`truncated`",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
    assert!(
        !md.contains("gh pr ") && !md.contains("gh issue "),
        "the sandbox hides the login from gh: {md}"
    );
}

/// The chapter's sections come in the order a newcomer needs them, and end with
/// where to dig deeper and what to remember.
#[test]
fn it_fixes_the_shape_of_a_chapter() {
    let (_dir, md) = scaffolded(&claude("ch"));
    let mut at = 0;
    for section in [
        "In this chapter.",
        "Why this part exists.",
        "The words you need.",
        "The big picture.",
        "One example, start to finish.",
        "How it works, in detail.",
        "Why it was built this way.",
        "When things go wrong.",
        "What is still unclear.",
        "Where to dig deeper.",
        "What to remember.",
    ] {
        let found = md[at..]
            .find(section)
            .unwrap_or_else(|| panic!("{section:?} missing or out of order"));
        at += found + section.len();
    }
}

/// What makes a chapter work when read aloud, and for this reader.
#[test]
fn it_teaches_the_read_aloud_and_plain_language_rules() {
    let (_dir, md) = scaffolded(&claude("ch"));
    for must in [
        "read aloud",
        "No Markdown",
        "in parentheses",
        "3,000 to 6,000 words",
        "I know AWS",
        "never instructions",
    ] {
        assert!(md.contains(must), "missing {must:?}");
    }
}

/// The book follows the answer language; the skill's own text stays English.
#[test]
fn the_book_follows_the_answer_language_and_the_skill_stays_english() {
    let mut p = claude("ch");
    p.response_language = "Ukrainian".into();
    let (_dir, md) = scaffolded(&p);
    assert!(md.contains("in Ukrainian"), "{md}");
    assert!(!has_cyrillic(&md), "commands stay English");
    // Titles and free text in the one language; names keep their spelling.
    let text = flat(&md);
    assert!(text.contains("the same titles as earlier chapters"), "{md}");
    assert!(text.contains("writes the rest in Ukrainian too"), "{md}");
    assert!(text.contains("keep their usual spelling"), "{md}");
    assert!(text.contains("`Chapters` and `Suggested chapters`"), "{md}");

    let (_dir, md) = scaffolded(&claude("ch"));
    assert!(!md.contains("Ukrainian"), "{md}");
    assert!(!has_cyrillic(&md));
}

#[test]
fn turning_it_off_removes_the_skill_and_its_mentions() {
    let mut p = claude("ch");
    p.claude.workflow.chapter = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join(SKILL).exists());
    assert!(!p.chapter_enabled());
}

#[test]
fn an_old_state_gains_the_skill_and_an_explicit_choice_is_kept() {
    let dir = tempdir().unwrap();
    let mut p = claude("ch");
    p.claude.workflow.chapter = false;
    p.scaffold(dir.path(), false).unwrap();
    let path = dir.path().join(STATE);
    let mut v: Value = serde_json::from_str(&read(dir.path(), STATE)).unwrap();
    let workflow = v["claude"]["workflow"]
        .as_object_mut()
        .expect("the workflow block");
    assert_eq!(workflow.remove("chapter"), Some(Value::Bool(false)));
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let loaded = Project::load_state(dir.path()).unwrap();
    assert!(
        loaded.claude.workflow.chapter,
        "an old state gains /chapter"
    );
    assert!(loaded.chapter_enabled());
    // An explicit choice is kept.
    p.scaffold(dir.path(), true).unwrap();
    let kept = Project::load_state(dir.path()).unwrap();
    assert!(!kept.claude.workflow.chapter);
}

/// A user's own `chapter` skill keeps its name.
#[test]
fn a_user_skill_named_chapter_disables_the_generated_one() {
    let mut p = claude("ch");
    p.skills.push(Skill {
        name: "chapter".into(),
        description: "My own chapter writer — use when I ask for a chapter of my kind".into(),
        body: "Write my kind of chapter.".into(),
        ..Default::default()
    });
    assert!(p.claude.workflow.chapter && !p.chapter_enabled());
    assert!(
        p.issues()
            .iter()
            .any(|i| i.contains("skill 'chapter'") && i.contains("collides")),
        "{:?}",
        p.issues()
    );
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    let md = read(dir.path(), SKILL);
    assert!(md.contains("Write my kind of chapter."), "{md}");
    assert!(!md.contains(".claude/notes/book"), "{md}");
}

/// A `/chapter` command written by hand is shadowed by the generated skill; one
/// an older ocgen wrote is `doctor`'s to remove, and with /chapter off nothing
/// shadows.
#[test]
fn a_hand_made_chapter_command_is_reported() {
    let dir = tempdir().unwrap();
    let p = claude("ch");
    p.scaffold(dir.path(), false).unwrap();
    assert!(p.skill_file_issues(dir.path()).is_empty());
    let command = dir.path().join(".claude/commands/chapter.md");
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    let mine = "---\ndescription: My chapter\n---\nWrite a chapter for $ARGUMENTS\n";
    fs::write(&command, mine).unwrap();
    let issues = p.skill_file_issues(dir.path());
    assert!(
        issues
            .iter()
            .any(|i| i.contains("hand-made /chapter command") && i.contains("shadowed")),
        "{issues:?}"
    );

    let mut off = claude("ch");
    off.claude.workflow.chapter = false;
    assert!(off.skill_file_issues(dir.path()).is_empty());
}

#[test]
fn the_plugin_carries_the_skill() {
    let mut p = claude("ch");
    p.claude.output = Output {
        project: true,
        plugin: true,
    };
    p.claude.plugin.repo_owner = "me".into();
    p.claude.plugin.repo_name = "ch".into();
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(dir
        .path()
        .join("plugin/ch/skills/chapter/SKILL.md")
        .is_file());
    let readme = read(dir.path(), "plugin/ch/README.md");
    assert!(readme.contains("`/chapter`"), "{readme}");

    p.claude.workflow.chapter = false;
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    assert!(!dir.path().join("plugin/ch/skills/chapter").exists());
    assert!(!read(dir.path(), "plugin/ch/README.md").contains("/chapter"));
}

/// A chapter is plain text in a subfolder: the notes view must not take it for a
/// ledger. Nothing here is new behaviour; the skill relies on it.
#[test]
fn the_notes_view_leaves_the_book_alone() {
    use ocgen::notes::{ledger_target, Target as NoteTarget};
    for path in [
        ".claude/notes/book/001-orders.txt",
        ".claude/notes/book/000-contents.txt",
        ".claude/notes/book/sources/001-orders.txt",
    ] {
        assert_eq!(ledger_target(path), NoteTarget::NotLedger, "{path}");
    }
}
