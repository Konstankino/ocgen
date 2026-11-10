//! The review check: once the change is done, three independent `reviewer` runs
//! check it at once — `conformance`, `correctness`, `tests` — a fresh run confirms
//! each finding, the confirmed ones go back in one list, and a new wave checks the
//! fixes. Only a team with a reviewer gets the loop, so the rest render as before.

use std::fs;
use std::path::Path;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::tempdir;

/// The default team with every check.
const TEAM: [&str; 7] = [
    "coordinator",
    "explorer",
    "implementer",
    "scope-guard",
    "reviewer",
    "privacy-guard",
    "adversary",
];

/// A line only the coordinator's review loop writes.
const LOOP_MARK: &str = "Review check (";

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn claude_with(lang: &str, roles: &[&str]) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.target = Target::ClaudeCode;
    p.project_name = "review".into();
    p.providers.clear();
    p.agents = roles
        .iter()
        .map(|n| Agent::from_archetype_claude(n, n, lang).unwrap())
        .collect();
    p
}

fn opencode_with(lang: &str, roles: &[&str]) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = "review".into();
    p.agents = roles
        .iter()
        .map(|n| Agent::from_archetype(n, n, lang, "mac").unwrap())
        .collect();
    p
}

fn claude(lang: &str) -> Project {
    claude_with(lang, &TEAM)
}

fn opencode(lang: &str) -> Project {
    opencode_with(lang, &TEAM)
}

fn without_reviewer() -> Vec<&'static str> {
    TEAM.iter().copied().filter(|r| *r != "reviewer").collect()
}

fn scaffold(p: &Project) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    dir
}

fn team_rule(p: &Project) -> String {
    read(scaffold(p).path(), ".claude/rules/ocgen-team.md")
}

fn coordinator_prompt(p: &Project) -> String {
    read(scaffold(p).path(), ".opencode/prompts/coordinator.txt")
}

/// The review loop alone: from its heading to the next loop's, or the end.
fn review_block(text: &str, heading: &str) -> String {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("no {heading:?} in:\n{text}"));
    let rest = &text[start..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn coordination(p: &Project) -> String {
    match p.target {
        Target::ClaudeCode => team_rule(p),
        Target::OpenCode => coordinator_prompt(p),
    }
}

// ----------------------------------------------------------- the wave --

#[test]
fn claude_coordinator_runs_the_review_wave() {
    let rule = team_rule(&claude("English"));
    let block = review_block(&rule, "Review check (reviewer)");
    for must in [
        "three fresh reviewer runs, one per angle",
        "as subagents in one message",
        "none waits for another",
        // The angles.
        "`conformance`",
        "`correctness`",
        "`tests`",
        "Followed, Broken or Not applicable",
        "duplicates an existing function, type or helper",
        "missed and edge cases",
        "missing negative tests",
        "tests that pass whatever the code does",
        // Every run reads the same pack from one file — the diff in it, written
        // once, not pasted into every prompt — and nothing of the others'.
        "Before each wave, write the pack to `.claude/scratch/review-pack.md`",
        "a scratch file, not a change to the work tree",
        "`.claude/scratch/.gitignore` holding `*`",
        "the diff itself, untracked new files included",
        "can't run git",
        "give every run its absolute path, the same file for each",
        "3,000 lines",
        "each listing every changed file",
        "the runs for all groups make up one wave, at most 8 at a time",
        "Never pass a run another run's report",
        // The ban leaves room for exactly what the confirmation and the new wave need.
        "confirmation run checks and what a new wave is given below",
        "Delete the pack files once the review check is done",
        // Each run is told its angle and the report format.
        "Tell each run its angle and this report format",
        "`Verdict: PASS`",
        "`Verdict: REWORK`",
        "(REWORK for any Critical or High finding, Broken precedent or duplicate)",
        "`Not checked:`",
        "that angle is UNRESOLVED",
        "its findings are still confirmed",
        "a finding with no severity counts as Medium",
        // The work tree holds still while the wave runs.
        "Change nothing in the work tree until the last report is in",
        "give each a slug",
        "keeps its slug and isn't confirmed again",
        // Each finding is confirmed on its own.
        "a fresh reviewer run",
        "counts as Medium",
        "at most 8 at a time",
        "never the finder's reasoning or confidence",
        "try to disprove",
        "Confirmed (by reading",
        "Rejected",
        "Unsettled",
        "An Unsettled Critical or High finding is UNRESOLVED",
        // One list back — or none, and the check is done — then a new wave.
        "send implementer one list, verbatim",
        "the confirmed Broken precedents",
        "the confirmed duplicates",
        "If that list is empty, whatever the verdicts, the review check is done: go on.",
        "a new wave on the whole change from the same base",
        "treat its findings the same way",
        "as slug, claim, `file:line` and fix",
        "first confirm or refute each fix",
        "At most 2 rework rounds",
        "3 waves",
        "never as followed",
        // The implementer works in a worktree: its branch is merged first.
        "implementer works in its own git worktree",
        "merge that branch into the working branch before every wave",
        // …alone in its command: the approval gate lets exactly that merge
        // through, and ocgen commits what the worker left before it.
        "`git merge --no-ff --no-edit <its worktree-agent-* branch>` and nothing else in that command",
        "holds that merge to commit what was left uncommitted, run it again",
    ] {
        assert!(block.contains(must), "missing {must:?} in:\n{block}");
    }
    assert!(!rule.contains("{{") && !rule.contains("{%"), "{rule}");
}

#[test]
fn the_review_check_runs_after_scope_and_before_privacy_and_the_adversary() {
    for p in [claude("English"), opencode("English")] {
        let text = coordination(&p);
        let scope = text.find("Scope check (scope-guard)").expect("scope");
        let review = text.find("Review check (reviewer)").expect("review");
        let privacy = text.find("Privacy check (privacy-guard)").expect("privacy");
        let adversary = text.find("Adversary check (adversary)").expect("adversary");
        assert!(
            scope < review && review < privacy && privacy < adversary,
            "{text}"
        );
        assert!(
            text.contains("after scope-guard and before privacy-guard, run a wave"),
            "{text}"
        );
        // A blank line between the loops, none doubled.
        for joint in [
            ".\n\nReview check",
            ".\n\nPrivacy check",
            ".\n\nAdversary check",
        ] {
            assert!(text.contains(joint), "missing {joint:?} in:\n{text}");
        }
        let loops = &text[scope..adversary];
        assert!(!loops.contains("\n\n\n"), "{loops}");
    }
}

#[test]
fn without_the_privacy_guard_it_runs_before_the_adversary() {
    let roles: Vec<&str> = TEAM
        .iter()
        .copied()
        .filter(|r| *r != "privacy-guard")
        .collect();
    let rule = team_rule(&claude_with("English", &roles));
    assert!(
        rule.contains("after scope-guard and before adversary, run a wave"),
        "{rule}"
    );
}

#[test]
fn without_the_other_checks_the_review_loop_names_none() {
    let rule = team_rule(&claude_with(
        "English",
        &["coordinator", "explorer", "implementer", "reviewer"],
    ));
    let block = review_block(&rule, "Review check (reviewer)");
    for never in ["scope-guard", "privacy", "adversary", "Scope check"] {
        assert!(!block.contains(never), "{never:?} in:\n{block}");
    }
    assert!(
        block.contains("When every change is done (with Agent Teams, every task), run a wave"),
        "{block}"
    );
    // The whole block, not a slice cut short by a stray blank line.
    assert!(block.contains("At most 2 rework rounds"), "{block}");
}

#[test]
fn opencode_runs_the_wave_one_call_at_a_time() {
    let prompt = coordinator_prompt(&opencode("English"));
    let block = review_block(&prompt, "Review check (reviewer)");
    for must in [
        "three fresh reviewer runs, one per angle, one after another",
        "`conformance`",
        "`Verdict: REWORK`",
        "At most 2 rework rounds",
        "pasted into every prompt costs a copy per run",
    ] {
        assert!(block.contains(must), "missing {must:?} in:\n{block}");
    }
    // Its reviewer may run `git diff`: no "no shell" reason there.
    assert!(!block.contains("can't run git"), "{block}");
    // With no loop after it, it ends the prompt with its final-report line.
    let prompt = coordinator_prompt(&opencode_with(
        "English",
        &["coordinator", "explorer", "implementer", "reviewer"],
    ));
    assert!(
        prompt.ends_with("every rejected or unsettled one with its reason.\n"),
        "{prompt}"
    );
    // One server, one agent at a time; no Agent Teams, no worktrees, no rules
    // folder (the scratch folder is every target's).
    for never in [
        "in one message",
        "at most 8",
        "Agent Teams",
        "worktree",
        ".claude/rules",
    ] {
        assert!(!block.contains(never), "{never:?} in:\n{block}");
    }
    assert!(prompt.ends_with('\n') && !prompt.ends_with("\n\n"));
}

// ------------------------------------------------------------ Ukrainian --

#[test]
fn the_review_loop_speaks_ukrainian() {
    let rule = team_rule(&claude("Ukrainian"));
    let block = review_block(&rule, "Перевірка ревʼю (reviewer)");
    for must in [
        "`Verdict: REWORK`",
        "`conformance`",
        "`correctness`",
        "`tests`",
        "в одному повідомленні",
        "не більше 8 одночасно",
        "Не більше 2 кіл доопрацювання",
        "злий його гілку",
        "`git merge --no-ff --no-edit <його гілка worktree-agent-*>`, і нічого більше в цій команді",
        "запусти його ще раз",
        "`.claude/scratch/review-pack.md`",
        "Confirmed (за читанням коду",
        "Якщо список порожній",
        "як підагенти",
        "абсолютний шлях",
        "3 хвиль",
        "повторно не підтверджується",
        "підтверджувальний запуск, і того, що нова хвиля отримує нижче",
        "видали файли пакета",
    ] {
        assert!(block.contains(must), "missing {must:?} in:\n{block}");
    }
    assert!(!block.contains("by reading"), "{block}");
    let prompt = coordinator_prompt(&opencode("Ukrainian"));
    let block = review_block(&prompt, "Перевірка ревʼю (reviewer)");
    assert!(block.contains("по черзі"), "{block}");
    assert!(!block.contains("в одному повідомленні"), "{block}");
}

#[test]
fn the_ukrainian_review_loop_has_no_russian_letters() {
    for text in [
        team_rule(&claude("Ukrainian")),
        coordinator_prompt(&opencode("Ukrainian")),
    ] {
        let block = review_block(&text, "Перевірка ревʼю (reviewer)");
        for c in ['ы', 'э', 'ъ', 'ё', 'Ы', 'Э', 'Ъ', 'Ё'] {
            assert!(!block.contains(c), "{c:?} in:\n{block}");
        }
    }
}

// ------------------------------------------------------------- names --

#[test]
fn a_renamed_reviewer_and_implementer_are_named_in_the_loop() {
    let mut p = claude("English");
    for a in &mut p.agents {
        match a.role.as_str() {
            "reviewer" => a.name = "critic".into(),
            "implementer" => a.name = "builder".into(),
            _ => {}
        }
    }
    let rule = team_rule(&p);
    let block = review_block(&rule, "Review check (critic)");
    assert!(block.contains("three fresh critic runs"), "{block}");
    assert!(block.contains("a fresh critic run"), "{block}");
    assert!(block.contains("send builder one list"), "{block}");
    // No name is written into the template itself.
    assert!(
        !block.contains("reviewer") && !block.contains("implementer"),
        "{block}"
    );
}

#[test]
fn a_team_without_an_implementer_sends_rework_to_whoever_changed_the_code() {
    let roles: Vec<&str> = TEAM
        .iter()
        .copied()
        .filter(|r| *r != "implementer")
        .collect();
    let rule = team_rule(&claude_with("English", &roles));
    let block = review_block(&rule, "Review check (reviewer)");
    assert!(
        block.contains("send the subagent that made the change one list"),
        "{block}"
    );
    assert!(!block.contains("worktree"), "{block}");

    // The same in Ukrainian.
    let rule = team_rule(&claude_with("Ukrainian", &roles));
    let block = review_block(&rule, "Перевірка ревʼю (reviewer)");
    assert!(
        block.contains("передай агентові, який вносив зміни, один список"),
        "{block}"
    );
}

#[test]
fn an_implementer_in_the_shared_checkout_has_no_branch_to_merge() {
    let mut p = claude("English");
    for a in &mut p.agents {
        if a.role == "implementer" {
            a.isolation = String::new();
        }
    }
    let rule = team_rule(&p);
    let block = review_block(&rule, "Review check (reviewer)");
    assert!(!block.contains("worktree"), "{block}");
    assert!(block.contains("send implementer one list"), "{block}");
    let md = deliver(&p);
    let check = &md[md.find("**Review check.**").unwrap()..md.find("**Privacy check.**").unwrap()];
    assert!(!check.contains("Merge `implementer`"), "{check}");
}

// ------------------------------------------------- existing projects --

#[test]
fn a_disabled_reviewer_turns_the_loop_off() {
    let mut p = opencode("English");
    p.agents
        .iter_mut()
        .find(|a| a.role == "reviewer")
        .unwrap()
        .disable = true;
    assert!(!coordinator_prompt(&p).contains(LOOP_MARK));

    // On Claude too: no loop in the rules, and /deliver checks conformance itself.
    let mut p = claude("English");
    p.agents
        .iter_mut()
        .find(|a| a.role == "reviewer")
        .unwrap()
        .disable = true;
    let dir = scaffold(&p);
    assert!(!read(dir.path(), ".claude/rules/ocgen-team.md").contains(LOOP_MARK));
    let md = read(dir.path(), ".claude/skills/deliver/SKILL.md");
    assert!(md.contains("**Conformance check.**") && !md.contains("**Review check.**"));
    let ws = read(dir.path(), ".claude/skills/workstreams/SKILL.md");
    assert!(!ws.contains("Review check"), "{ws}");
}

#[test]
fn a_team_without_a_reviewer_renders_as_before() {
    for lang in ["English", "Ukrainian"] {
        let rule = team_rule(&claude_with(lang, &without_reviewer()));
        assert!(
            !rule.contains(LOOP_MARK) && !rule.contains("Перевірка ревʼю"),
            "{rule}"
        );
        assert!(!rule.contains("`correctness`"), "{rule}");
        let prompt = coordinator_prompt(&opencode_with(lang, &without_reviewer()));
        assert!(
            !prompt.contains(LOOP_MARK) && !prompt.contains("Перевірка ревʼю"),
            "{prompt}"
        );
    }
    // A coordinator with nothing to check still ends where it did.
    let prompt = coordinator_prompt(&opencode_with(
        "English",
        &["coordinator", "explorer", "implementer", "verifier"],
    ));
    assert!(prompt.ends_with("Respond in English.\n"), "{prompt}");
}

#[test]
fn adding_a_reviewer_to_an_existing_project_adds_the_loop() {
    for (p, file) in [
        (
            claude_with("English", &without_reviewer()),
            ".claude/rules/ocgen-team.md",
        ),
        (
            opencode_with("English", &without_reviewer()),
            ".opencode/prompts/coordinator.txt",
        ),
    ] {
        let dir = scaffold(&p);
        assert!(!read(dir.path(), file).contains(LOOP_MARK));

        // What `ocgen add agent` does: reload the state, append, re-render.
        let mut loaded = Project::load_state(dir.path()).unwrap();
        let reviewer = match loaded.target {
            Target::ClaudeCode => {
                Agent::from_archetype_claude("reviewer", "reviewer", "English").unwrap()
            }
            Target::OpenCode => {
                Agent::from_archetype("reviewer", "reviewer", "English", "mac").unwrap()
            }
        };
        loaded.agents.push(reviewer);
        loaded.scaffold(dir.path(), true).unwrap();
        let text = read(dir.path(), file);
        assert!(text.contains(LOOP_MARK), "{file}");
        // Appended last in the roster, the loop still runs before privacy and the adversary.
        let review = text.find("Review check").unwrap();
        assert!(text.find("Scope check").unwrap() < review, "{text}");
        assert!(review < text.find("Privacy check").unwrap(), "{text}");
        assert!(review < text.find("Adversary check").unwrap(), "{text}");
    }
}

// ----------------------------------------------------------- /deliver --

fn deliver(p: &Project) -> String {
    read(scaffold(p).path(), ".claude/skills/deliver/SKILL.md")
}

#[test]
fn deliver_runs_the_review_check_between_scope_and_privacy() {
    let md = deliver(&claude("English"));
    assert!(!md.contains("**Conformance check.**"), "{md}");
    let at = |s: &str| md.find(s).unwrap_or_else(|| panic!("no {s:?} in:\n{md}"));
    assert!(at("**Scope check.**") < at("**Review check.**"));
    assert!(at("**Review check.**") < at("**Privacy check.**"));
    assert!(at("**Privacy check.**") < at("**Adversary check.**"));
    assert!(
        md.contains("**Privacy check.** After the review check"),
        "{md}"
    );
    let start = at("**Review check.**");
    let check = &md[start..at("**Privacy check.**")];
    for must in [
        "three independent `reviewer` runs",
        "`conformance`, `correctness`, `tests`",
        "fresh `reviewer` runs confirm the findings",
        "the review loop in `.claude/rules/ocgen-team.md`",
        "Merge `implementer`'s worktree branch into the working branch before each wave",
        "Write the pack the loop describes to `.claude/scratch/review-pack.md`",
        "give every run its absolute path",
        "the precedent list from phase 4",
        "the changed files and the diff",
        "Followed, Broken or Not applicable",
        "A Broken precedent with a reason I approved is not a finding",
        "the confirmed findings that are Critical or High, Broken precedents or duplicates",
        "in one list, verbatim",
        "when nothing is left to send, the check is done",
        "At most 2 rework rounds",
        "**UNRESOLVED**",
        "never as followed",
    ] {
        assert!(check.contains(must), "missing {must:?} in:\n{check}");
    }
    let synth = &md[at("## 6. Synthesize")..];
    for must in [
        "From the review check",
        "each angle's verdict",
        "every UNRESOLVED finding",
        "rejected or unsettled",
        "the confirmed Medium and Low findings under open risks",
    ] {
        assert!(synth.contains(must), "missing {must:?} in:\n{synth}");
    }
    assert!(!synth.contains("From the conformance check"), "{synth}");
}

#[test]
fn deliver_without_a_scope_guard_reviews_after_the_final_check() {
    let roles: Vec<&str> = TEAM
        .iter()
        .copied()
        .filter(|r| *r != "scope-guard")
        .collect();
    let md = deliver(&claude_with("English", &roles));
    let at = |s: &str| md.find(s).unwrap_or_else(|| panic!("no {s:?} in:\n{md}"));
    assert!(at("**Final check.**") < at("**Review check.**"));
    assert!(
        md.contains("**Review check.** After the final check"),
        "{md}"
    );
}

#[test]
fn deliver_without_a_reviewer_keeps_the_conformance_check() {
    let md = deliver(&claude_with("English", &without_reviewer()));
    assert!(!md.contains("**Review check.**"), "{md}");
    assert!(
        md.contains("**Privacy check.** After the conformance check"),
        "{md}"
    );
    let synth = &md[md.find("## 6. Synthesize").unwrap()..];
    assert!(synth.contains("From the conformance check"), "{synth}");
    assert!(!synth.contains("From the review check"), "{synth}");
}

#[test]
fn deliver_without_an_implementer_sends_rework_to_whoever_changed_the_code() {
    let roles: Vec<&str> = TEAM
        .iter()
        .copied()
        .filter(|r| *r != "implementer")
        .collect();
    let md = deliver(&claude_with("English", &roles));
    let check = &md[md.find("**Review check.**").unwrap()..md.find("**Privacy check.**").unwrap()];
    assert!(
        check.contains("Send the subagent that made the change the confirmed findings"),
        "{check}"
    );
    assert!(!check.contains("``") && !check.contains("Merge"), "{check}");
}

// -------------------------------------------------------- the rules --

#[test]
fn the_pipeline_summary_names_the_review_check() {
    let rules = |p: &Project| read(scaffold(p).path(), ".claude/rules/ocgen-workflow.md");
    let with = rules(&claude("English"));
    assert!(with.contains("**Review check**"), "{with}");
    assert!(
        with.contains("then runs the scope, review and adversary\nloops on the whole."),
        "{with}"
    );
    let without = rules(&claude_with("English", &without_reviewer()));
    assert!(!without.contains("**Review check**"), "{without}");
    assert!(without.contains("**Conformance check**"), "{without}");
    assert!(
        without.contains(
            "The implementer works from the precedent\n   list, and a **Conformance check** compares the finished change against it."
        ),
        "{without}"
    );
    // Word for word as before, line break included, so `ocgen doctor` has
    // nothing to rewrite in a project without a reviewer.
    assert!(
        without.contains("then runs the scope and adversary\nloops on the whole."),
        "{without}"
    );
}
