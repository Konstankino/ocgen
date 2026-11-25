//! /deliver proves its work with the project's own checks: each success
//! criterion names the check that proves it, a baseline run before the first
//! task tells a pre-existing failure from a regression, the coordinator runs
//! the checks after every task (the implementer has no shell), and a last
//! Verify run, after every review loop's fixes, supplies the numbers the report
//! gives. The plan is approved with the precedent sweep already in it, a bug is
//! reproduced before it is planned for, the merge the approval gate lets pass is
//! named exactly, and the report has a fixed shape.

use std::collections::BTreeMap;

use ocgen::agent::Agent;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use ocgen::templates;

fn manifest() -> Manifest {
    toml::from_str(&templates::load_embedded("manifest.toml").unwrap()).unwrap()
}

const FULL: [(&str, &str); 6] = [
    ("coordinator", "coordinator"),
    ("explorer", "explorer"),
    ("implementer", "implementer"),
    ("reviewer", "reviewer"),
    ("scope-guard", "scope-guard"),
    ("adversary", "adversary"),
];

/// A Claude project with these agents: `(name, archetype)`.
fn project(team: &[(&str, &str)]) -> Project {
    let mut p = Project::from_manifest(&manifest(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "checks".into();
    p.providers.clear();
    p.agents = team
        .iter()
        .map(|(n, a)| Agent::from_archetype_claude(n, a, "English").unwrap())
        .collect();
    p
}

fn without(drop: &str) -> Vec<(&'static str, &'static str)> {
    FULL.iter().copied().filter(|(n, _)| *n != drop).collect()
}

/// The rendered /deliver skill.
fn deliver(p: &Project) -> String {
    let files: BTreeMap<String, String> = p
        .render_all()
        .unwrap()
        .into_iter()
        .map(|(rel, text)| (rel.to_string_lossy().replace('\\', "/"), text))
        .collect();
    files[".claude/skills/deliver/SKILL.md"].clone()
}

/// The text from `from` up to the next `to`.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text.find(from).unwrap_or_else(|| panic!("no {from:?}"));
    let end = text[start + from.len()..]
        .find(to)
        .map_or(text.len(), |i| start + from.len() + i);
    &text[start..end]
}

fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{text}"))
}

/// `text` on one line: a phrase matches wherever the template wraps it.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every phrase is in `text`, wherever the template wraps its lines.
fn has_all(text: &str, musts: &[&str]) {
    let flat = flat(text);
    for must in musts {
        assert!(flat.contains(must), "missing {must:?} in:\n{text}");
    }
}

// ------------------------------------------------------ the checks run --

/// With a scope guard nothing ran the tests on the finished change: the last
/// step does, after every review loop's fixes, and its run is the one reported.
#[test]
fn deliver_verifies_the_finished_change_last() {
    for team in [
        FULL.to_vec(),
        without("scope-guard"),
        without("adversary"),
        without("reviewer"),
    ] {
        let md = deliver(&project(&team));
        let verify = section(&md, "**Verify.**", "## 6. Synthesize");
        has_all(
            verify,
            &[
                "the project check",
                "every criterion's Check",
                "compare with the baseline",
                // Rounds of fixes, each followed by a run: the numbers come
                // after the last fix.
                "up to 2 fixes and 3 Verify runs",
                "**not done**",
                "the last Verify run",
            ],
        );
        // Last: after every other check of phase 5.
        for check in [
            "**Scope check.**",
            "**Final check.**",
            "**Review check.**",
            "**Conformance check.**",
            "**Adversary check.**",
        ] {
            if let Some(i) = md.find(check) {
                assert!(i < at(&md, "**Verify.**"), "{check} after Verify:\n{md}");
            }
        }
        assert!(at(&md, "**Verify.**") < at(&md, "## 6. Synthesize"));
    }
}

#[test]
fn deliver_verify_merges_a_worktree_implementers_last_fix_first() {
    let md = deliver(&project(&FULL));
    let verify = section(&md, "**Verify.**", "## 6. Synthesize");
    has_all(verify, &["`implementer`'s"]);
    // An implementer in the main checkout has nothing to merge.
    let mut p = project(&FULL);
    for a in &mut p.agents {
        if a.role == "implementer" {
            a.isolation.clear();
        }
    }
    let md = deliver(&p);
    let verify = section(&md, "**Verify.**", "## 6. Synthesize");
    assert!(!flat(verify).contains("merg"), "{verify}");
}

/// The implementer has no shell, so whoever runs /deliver runs the checks after
/// each task and sends a new failure back as that task's next attempt.
#[test]
fn deliver_runs_the_checks_after_each_task() {
    let md = deliver(&project(&FULL));
    let execute = section(&md, "## 5. Execute", "**Scope check.**");
    has_all(
        execute,
        &[
            "After each task and each rework, run the project check and that task's Checks yourself",
            "may have no shell",
            "as that task's next attempt",
        ],
    );
}

// ------------------------------------------------------ criteria name it --

#[test]
fn deliver_criteria_name_their_check() {
    for team in [FULL.to_vec(), without("scope-guard")] {
        let md = deliver(&project(&team));
        let plan = section(&md, "## 3. Plan", "## 4. Research");
        has_all(
            plan,
            &[
                "Give each success criterion a slug",
                "and a **Check**: the test or command that proves it",
                "what it gives on the base now",
                "test-first",
                "before the code that makes it pass",
            ],
        );
    }
    // A collapsed plan keeps them too.
    let md = deliver(&project(&FULL));
    let head = section(&md, "Deliver this end-to-end", "## ");
    has_all(head, &["criteria still name their Checks"]);
}

// ----------------------------------------------------------- baseline --

/// The base was noted only for the scope guard, though the review, conformance
/// and privacy checks name it too; and no check ran on it, so a failure that was
/// there already read as a regression.
#[test]
fn deliver_runs_a_baseline_before_the_first_task() {
    for team in [FULL.to_vec(), without("scope-guard")] {
        let md = deliver(&project(&team));
        let execute = section(&md, "## 5. Execute", "## 6. Synthesize");
        let baseline = section(execute, "**Baseline.**", "\n\n");
        has_all(
            baseline,
            &[
                "Before the first task, note the base",
                "git status --porcelain=v2 --branch --untracked-files=all",
                "run the project check and every criterion's Check",
                "**pre-existing failure**",
                "don't fix, skip, silence or delete it",
                "report it apart in phase 6",
                "already passes on the base",
            ],
        );
        assert!(
            at(execute, "**Baseline.**") < at(execute, "Work the approved plan"),
            "{execute}"
        );
    }
    // Only the scope guard is named for it, and only when there is one.
    let md = deliver(&project(&without("scope-guard")));
    assert!(!flat(&md).contains("scope check"), "{md}");
}

// -------------------------------------------------------------- premise --

#[test]
fn deliver_reproduces_a_bug_before_planning() {
    let md = deliver(&project(&FULL));
    let gather = section(&md, "## 2. Gather requirements", "## 3. Plan");
    let premise = section(gather, "**Check the premise.**", "\n\n");
    has_all(
        premise,
        &[
            "reproduce it before you plan",
            "quote the output",
            "If it doesn't reproduce",
            "ask me before planning",
            "root cause",
            "every caller",
        ],
    );
}

// ------------------------------------------------ the sweep before approval --

#[test]
fn deliver_sweeps_precedents_before_the_plan_is_approved() {
    let md = deliver(&project(&FULL));
    let plan = section(&md, "## 3. Plan", "## 4. Research");
    has_all(
        plan,
        &[
            "Before you present the plan",
            "run the precedent sweep",
            "`explorer`",
            "**Precedents**",
            "the callers that rely on",
            "keep the sweep short, but run it",
        ],
    );
    assert!(
        at(plan, "run the precedent sweep") < at(plan, "**invite push-back**"),
        "{plan}"
    );
    let research = section(&md, "## 4. Research", "## 5.");
    has_all(research, &["again on what changed", "re-plan"]);
    assert!(
        !flat(research).contains("One thread is always"),
        "{research}"
    );

    // No explorer: the built-in Explore agent does the sweep.
    let md = deliver(&project(&without("explorer")));
    let plan = section(&md, "## 3. Plan", "## 4. Research");
    has_all(plan, &["built-in `Explore`"]);
    assert!(!flat(plan).contains("`explorer`"), "{plan}");
}

// ------------------------------------------------------ the task's brief --

#[test]
fn deliver_gives_each_task_one_brief() {
    let md = deliver(&project(&FULL));
    let execute = section(&md, "## 5. Execute", "**Scope check.**");
    let brief = section(execute, "Work the approved plan", "\n\n");
    has_all(
        brief,
        &[
            "`implementer`",
            "the criteria it meets, each with its Check",
            "the files it may change",
            "precedents that bear on it",
            "the pre-existing failures",
            "change only what the task needs",
            "write each criterion's test before the code",
            "never change or delete an existing test or its assertions",
            "`Confidence: NN%`",
        ],
    );
    // Without an implementer, whoever makes the change gets it.
    let md = deliver(&project(&without("implementer")));
    has_all(&md, &["Give whoever makes the change one brief per task"]);
}

// ---------------------------------------------------------- the merges --

fn gated(team: &[(&str, &str)]) -> Project {
    let mut p = project(team);
    p.claude.team.enabled = true;
    p.claude.team.approval_gate = true;
    p
}

#[test]
fn deliver_names_the_merge_the_gate_lets_pass() {
    let md = deliver(&gated(&FULL));
    let execute = section(&md, "## 5. Execute", "**Scope check.**");
    let merge = section(execute, "**Merging `implementer`'s work.**", "\n\n");
    has_all(
        merge,
        &[
            "after each task and each rework",
            "or the next task starts without it",
            "`git merge --no-ff --no-edit worktree-agent-<id>`",
            "The approval gate lets exactly that merge pass",
            "`ocgen approve`",
            "a yes in chat doesn't open it",
            "`git merge --abort`",
            "never get around it",
        ],
    );
    // A local merge isn't an external action: only other merges wait.
    assert!(!flat(&md).contains("`git push`/merge"), "{md}");
    // Not "any merge into the working branch": the gate passes only that one.
    has_all(&md, &["any merge but the one of its branch above"]);

    // No gate: the same merge, nothing about approving it.
    let md = deliver(&project(&FULL));
    let merge = section(&md, "**Merging `implementer`'s work.**", "\n\n");
    has_all(merge, &["worktree-agent-<id>"]);
    assert!(!flat(merge).contains("approve"), "{merge}");

    // An implementer in the main checkout has no branch to merge.
    let mut p = gated(&FULL);
    for a in &mut p.agents {
        if a.role == "implementer" {
            a.isolation.clear();
        }
    }
    assert!(!deliver(&p).contains("**Merging"));
}

// ---------------------------------------------------- the project check --

#[test]
fn deliver_names_the_project_check() {
    let mut p = project(&FULL);
    p.claude.workflow.check_cmd = "cargo test --locked".into();
    let md = deliver(&p);
    let gather = section(&md, "## 2. Gather requirements", "## 3. Plan");
    let check = section(gather, "**Find the project check.**", "\n\n");
    has_all(
        check,
        &[
            "The project check is `cargo test --locked`",
            "what CI runs",
            "add any check it runs that this one leaves out",
        ],
    );
    // A red base fails every worker's stop gate: say so before the first task.
    let baseline = section(&md, "**Baseline.**", "\n\n");
    has_all(
        baseline,
        &["Except: if `cargo test --locked` fails on the base"],
    );

    // Without one, CI's own commands are the check.
    let md = deliver(&project(&FULL));
    let check = section(&md, "**Find the project check.**", "\n\n");
    has_all(
        check,
        &["what CI runs", "`.github/workflows/`", "the project check"],
    );
    assert!(
        !flat(&md).contains("fails on the base, every worker"),
        "{md}"
    );
}

// ------------------------------------------------------------- report --

const SHAPE: [&str; 9] = [
    "- **Done**",
    "- **Not done**",
    "- **Unresolved**",
    "- **Commands run**",
    // Where the scope, review and adversary checks' own lines go.
    "- **Check verdicts**",
    "- **Pre-existing failures**",
    "- **Decisions**",
    "- **Open risks**",
    "- **Next steps**",
];

#[test]
fn deliver_reports_in_a_fixed_shape() {
    for team in [FULL.to_vec(), without("adversary"), without("scope-guard")] {
        let md = deliver(&project(&team));
        let synth = section(&md, "## 6. Synthesize", "\nFor several");
        let mut last = 0;
        for heading in SHAPE {
            let i = at(synth, heading);
            assert!(i > last, "{heading} out of order:\n{synth}");
            last = i;
        }
        has_all(synth, &["`none`", "the last Verify run"]);
    }
    // The adversary's verdict and rounds are reported, as the team rule asks.
    let md = deliver(&project(&FULL));
    let synth = section(&md, "## 6. Synthesize", "\nFor several");
    has_all(
        synth,
        &[
            "From the adversary check, report its verdict, the rounds used",
            // One format, not two: the team rules have their own.
            "This shape replaces any other final answer format for this run",
            "both positions",
            "If Verify didn't run",
            "where the command prints them",
        ],
    );
}

// ------------------------------------------- what a review of it found --

/// ocgen may hold the first merge to commit what a worker left uncommitted: the
/// team rule says run it again, so that hold is no reason to stop.
#[test]
fn deliver_reruns_a_held_merge_and_aborts_only_a_conflict() {
    let md = deliver(&gated(&FULL));
    let merge = section(&md, "**Merging `implementer`'s work.**", "\n\n");
    has_all(
        merge,
        &[
            "If ocgen holds the merge to commit what the worker left uncommitted, run the same command once more",
            "If git reports a conflict, run `git merge --abort`",
            "If the loop guard released that worker, read its branch's diff",
        ],
    );
}

/// An intent or a brief settles what it says, never the Checks, the project
/// check, the sweep or a bug's reproduction; a collapsed plan keeps them too.
#[test]
fn deliver_keeps_its_own_steps_on_every_route() {
    let mut p = project(&FULL);
    p.claude.workflow.intent = true;
    let md = deliver(&p);
    let sharpen = section(&md, "## 1. Sharpen", "## 2. Gather");
    has_all(
        sharpen,
        &[
            "An intent or a brief never settles these",
            "present the plan for approval before phase 4",
        ],
    );
    let head = section(&md, "Deliver this end-to-end", "## ");
    has_all(
        head,
        &[
            "a reported bug is still reproduced first",
            "the project check is still named",
        ],
    );
}

#[test]
fn deliver_checks_are_safe_and_mean_something() {
    let md = deliver(&project(&FULL));
    // A formatter in its writing form would rewrite my modified files.
    let check = section(&md, "**Find the project check.**", "\n\n");
    has_all(
        check,
        &["The project check never changes a file", "`--check`"],
    );
    // Reproducing a bug never takes a gated or destructive step unasked.
    let premise = section(&md, "**Check the premise.**", "\n\n");
    has_all(premise, &["destructive", "ask me first"]);
    // A criterion that keeps behavior passes on the base; one that spans
    // tasks is checked once, after its last task.
    let plan = section(&md, "## 3. Plan", "## 4. Research");
    has_all(
        plan,
        &["`holds on base`", "belongs to the last task it needs"],
    );
    let baseline = section(&md, "**Baseline.**", "\n\n");
    has_all(
        baseline,
        &[
            "A Check passes only when it ran at least one test or assertion for its criterion",
            "among the files already modified",
            "cut a working branch",
        ],
    );
    // The implementer is told it can't run them, and its confidence says so.
    let execute = section(&md, "## 5. Execute", "**Scope check.**");
    has_all(execute, &["I run the Checks after you stop"]);
}

/// The slug the report names each criterion by is given with or without a scope
/// guard; only the Scope line is the guard's.
#[test]
fn deliver_names_criteria_by_slug_without_a_guard_too() {
    let md = deliver(&project(&without("scope-guard")));
    let plan = section(&md, "## 3. Plan", "## 4. Research");
    has_all(plan, &["Give each success criterion a slug"]);
    assert!(!flat(plan).contains("**Scope**"), "{plan}");
}
