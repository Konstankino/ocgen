//! Parity: the Rust hooks (`ocgen hook <name>`) must behave exactly like the
//! bundled `sh` scripts they replace. Each scenario runs step by step through both
//! implementations — each in its own fresh project, so loop-guard state evolves in
//! parallel — and every step's exit code, stdout kind and first stderr line must match.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use ocgen::agent;
use ocgen::claude::Team;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use tempfile::TempDir;

fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "par".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: true,
        confidence_threshold: 96,
        risk_rounds: true,
        approval_gate: true,
    };
    // Emit every optional hook script too.
    p.claude.hooks_extra.format_cmd = "true".into();
    p.claude.hooks_extra.config_audit = true;
    p.claude.hooks_extra.notify = true;
    p.scaffold(dir.path(), false).unwrap();
    // A git repository, like a real project: the approval key then comes from git
    // in both implementations (a bare directory would depend on how each resolves
    // Windows short names such as RUNNER~1).
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

fn sh(
    dir: &Path,
    hook: &str,
    env: &HashMap<String, String>,
    payload: &str,
) -> (i32, String, String) {
    let mut cmd = Command::new("sh");
    cmd.arg(dir.join(format!(".claude/hooks/{hook}.sh")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Start from the real environment (a shell needs more than PATH on Windows),
    // minus anything that would steer a gate.
    for k in [
        "TEAM_APPROVAL_GATE",
        "TEAM_PLAN_GATE",
        "TEAM_RISK_ROUNDS",
        "TEAM_READONLY_ROLES",
        "TEAM_CONFIDENCE_THRESHOLD",
        "SUBAGENT_CONFIDENCE_THRESHOLD",
        "LOOP_GUARD_MAX_BLOCKS",
        "OCGEN_CHECK_CMD",
        "OCGEN_CHECK_TIMEOUT",
        "OCGEN_FORMAT_CMD",
        "OCGEN_NO_JQ",
        "OCGEN_NOTES_OPEN",
        "OCGEN_NOTES_BROWSER",
        "OCGEN_RECAP_GH",
        "OCGEN_RECAP_GH_TIMEOUT",
    ] {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

struct Step {
    hook: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// `{dir}` is replaced with the project directory.
    payload: &'static str,
    /// Filesystem changes applied (to both projects) before this step.
    setup: fn(&Path),
}

fn none(_: &Path) {}

fn check(name: &str, steps: &[Step]) {
    let (a, b) = (project(), project());
    for (i, s) in steps.iter().enumerate() {
        let mut outs = Vec::new();
        for (dir, rust) in [(a.path(), false), (b.path(), true)] {
            (s.setup)(dir);
            let mut env: HashMap<String, String> = s
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            // Forward slashes: valid inside JSON and understood by bash and git on
            // Windows too (a raw `C:\Users` would be a broken JSON escape).
            let d = ocgen::paths::for_shell(dir);
            env.insert("CLAUDE_PROJECT_DIR".into(), d.clone());
            // A private HOME per project: approvals live under ~/.claude/ocgen/.
            env.insert("HOME".into(), format!("{d}/.home"));
            let payload = s.payload.replace("{dir}", &d);
            let o = if rust {
                let o = ocgen::hooks::run(s.hook, &payload, &env);
                (o.code, o.stdout, o.stderr)
            } else {
                sh(dir, s.hook, &env, &payload)
            };
            outs.push(o);
        }
        let (sh_o, rs_o) = (&outs[0], &outs[1]);
        let ctx = format!("{name} step {i} ({}): sh={sh_o:?}\nrust={rs_o:?}", s.hook);
        assert_eq!(sh_o.0, rs_o.0, "exit code differs — {ctx}");
        let kind = |o: &str| {
            let v: Option<serde_json::Value> = serde_json::from_str(o.trim()).ok();
            v.map(|v| {
                let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
                keys.sort();
                keys
            })
        };
        assert_eq!(kind(&sh_o.1), kind(&rs_o.1), "stdout differs — {ctx}");
        assert_eq!(
            sh_o.2.lines().next(),
            rs_o.2.lines().next(),
            "first stderr line differs — {ctx}"
        );
    }
    // Both implementations wrote the same loop-guard escalations.
    let log = |d: &Path| {
        fs::read_to_string(d.join(".claude/loop-guard/escalations.md"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                l.split_once(' ')
                    .map(|x| x.1)
                    .unwrap_or(l)
                    .split_once(' ')
                    .map(|x| x.1)
                    .unwrap_or(l)
                    .to_string()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        log(a.path()),
        log(b.path()),
        "{name}: escalation logs differ"
    );
}

fn dirty_repo(dir: &Path) {
    let wt = dir.join("wt");
    if !wt.exists() {
        fs::create_dir_all(&wt).unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&wt)
            .output()
            .unwrap();
        fs::write(wt.join("new.rs"), "// change\n").unwrap();
    }
}

fn clean_repo(dir: &Path) {
    let wt = dir.join("wt");
    fs::create_dir_all(&wt).unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(&wt)
        .output()
        .unwrap();
}

const SUB: &[(&str, &str)] = &[
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

#[test]
fn subagent_confidence_gate_parity() {
    check(
        "subagent",
        &[Step {
            hook: "subagent-confidence-gate",
            env: SUB,
            setup: clean_repo,
            payload: r#"{"session_id":"s","agent_id":"w1","cwd":"{dir}/wt","last_assistant_message":"read only"}"#,
        }],
    );
    check(
        "subagent-dirty",
        &[
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: dirty_repo,
                payload: r#"{"session_id":"s","agent_id":"w1","cwd":"{dir}/wt","last_assistant_message":"done. Confidence: 97%"}"#,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w2","cwd":"{dir}/wt","last_assistant_message":"no rating"}"#,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w3","cwd":"{dir}/wt","last_assistant_message":"Confidence: 60%"}"#,
            },
            // Stalled confidence releases early; then budget exhaustion for w2.
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w3","cwd":"{dir}/wt","last_assistant_message":"Confidence: 60%"}"#,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w2","cwd":"{dir}/wt","last_assistant_message":"no rating"}"#,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: SUB,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w2","cwd":"{dir}/wt","last_assistant_message":"no rating"}"#,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: &[("SUBAGENT_CONFIDENCE_THRESHOLD", "0")],
                setup: none,
                payload: r#"{"cwd":"{dir}/wt","last_assistant_message":"Confidence: 1%"}"#,
            },
        ],
    );
}

const TASK: &[(&str, &str)] = &[
    ("TEAM_CONFIDENCE_THRESHOLD", "96"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn confidence_marker(dir: &Path) {
    fs::create_dir_all(dir.join(".claude/team/confidence")).unwrap();
    fs::write(dir.join(".claude/team/confidence/t9.txt"), "97").unwrap();
}

#[test]
fn task_completed_parity() {
    check(
        "task-completed",
        &[
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t1","summary":"Confidence: 97%"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t2","summary":"Confidence: 50%"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t3","summary":"none"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t3","summary":"none"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t3","summary":"none"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: TASK,
                setup: confidence_marker,
                payload: r#"{"session_id":"s","task_id":"t9","summary":"done"}"#,
            },
        ],
    );
}

const PLAN: &[(&str, &str)] = &[("TEAM_PLAN_GATE", "1"), ("LOOP_GUARD_MAX_BLOCKS", "3")];

fn approve_plan(dir: &Path) {
    fs::create_dir_all(dir.join(".claude/team")).unwrap();
    fs::write(
        dir.join(".claude/team/plan.md"),
        "Intent\nStatus: APPROVED\n",
    )
    .unwrap();
}

#[test]
fn task_created_parity() {
    let ev = r#"{"session_id":"s","agent_type":"implementer"}"#;
    check(
        "task-created",
        &[
            Step {
                hook: "team-task-created",
                env: PLAN,
                setup: none,
                payload: ev,
            },
            Step {
                hook: "team-task-created",
                env: PLAN,
                setup: none,
                payload: ev,
            },
            Step {
                hook: "team-task-created",
                env: PLAN,
                setup: none,
                payload: ev,
            },
            Step {
                hook: "team-task-created",
                env: PLAN,
                setup: none,
                payload: ev,
            },
            Step {
                hook: "team-task-created",
                env: PLAN,
                setup: approve_plan,
                payload: ev,
            },
        ],
    );
}

const IDLE: &[(&str, &str)] = &[
    ("TEAM_RISK_ROUNDS", "1"),
    ("TEAM_READONLY_ROLES", "explorer reviewer"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn risky_plan(dir: &Path) {
    fs::create_dir_all(dir.join(".claude/team")).unwrap();
    fs::write(
        dir.join(".claude/team/plan.md"),
        "- Risk: data loss — Owner: implementer — Mitigation: pending\n\
         - Risk: drift — Owner: dbadmin — Mitigation: done\n",
    )
    .unwrap();
}

#[test]
fn teammate_idle_parity() {
    check(
        "idle",
        &[
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: risky_plan,
                payload: r#"{"session_id":"s","agent_type":"implementer"}"#,
            },
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: none,
                payload: r#"{"session_id":"s","agent_type":"explorer"}"#,
            },
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: none,
                payload: r#"{"session_id":"s","agent_type":"dbadmin"}"#,
            },
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: none,
                payload: r#"{"session_id":"s"}"#,
            },
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: none,
                payload: r#"{"session_id":"s","agent_type":"implementer"}"#,
            },
            Step {
                hook: "team-teammate-idle",
                env: IDLE,
                setup: none,
                payload: r#"{"session_id":"s","agent_type":"implementer"}"#,
            },
        ],
    );
}

const GATE: &[(&str, &str)] = &[("TEAM_APPROVAL_GATE", "1"), ("LOOP_GUARD_MAX_BLOCKS", "3")];
const GATE_NO_JQ: &[(&str, &str)] = &[
    ("TEAM_APPROVAL_GATE", "1"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
    ("OCGEN_NO_JQ", "1"),
];

fn unlock(dir: &Path) {
    ocgen::approval::grant(&dir.join(".home"), dir, 10).unwrap();
}

/// The approval scenario, for a given gate environment.
fn approval_steps(env: &'static [(&'static str, &'static str)]) -> Vec<Step> {
    let bash = |command: &'static str, session: &'static str| {
        // Leaked once per test step: payloads must be 'static for the step table.
        let payload = serde_json::json!({
            "session_id": session, "tool_name": "Bash", "tool_input": { "command": command }
        })
        .to_string();
        Step {
            hook: "team-approval-gate",
            env,
            setup: none,
            payload: Box::leak(payload.into_boxed_str()),
        }
    };
    let mut steps = vec![
        bash("git status", "s"),
        bash("git log --grep='push'", "s"),
        bash("cat scripts/deploy.sh", "s"),
        bash("grep -r execution-approved docs/", "s"),
        // Each blocked attempt below uses its own session so the loop guard's
        // budget doesn't interfere with the detection being compared.
        bash("terraform apply -auto-approve", "a1"),
        bash("terraform -chdir=infra apply", "a2"),
        bash("git -C . push origin main", "a3"),
        bash("git \"push\" origin", "a4"),
        bash("sh ./deploy.sh", "a5"),
        bash("make -C infra deploy", "a6"),
        bash("aws s3 rm s3://b/x --recursive", "a7"),
        bash("sshfs host:/x /mnt", "a8"),
        bash("ocgen approve", "a9"),
        bash("echo 9999999999 > ~/.claude/ocgen/approvals/x", "a10"),
        // A Windows-style path to the approval store.
        bash("echo 9 > C:\\Users\\x\\.claude\\ocgen\\approvals\\k", "a11"),
        // The budget: three blocks in one session, then deny + halt.
        bash("git push origin main", "b"),
        bash("git push origin main", "b"),
        bash("git push origin main", "b"),
    ];
    steps.push(Step {
        hook: "team-approval-gate",
        env,
        setup: none,
        payload: r#"{"session_id":"w1","tool_name":"Write","tool_input":{"file_path":"{dir}/.home/.claude/ocgen/approvals/x"}}"#,
    });
    // The same store, addressed with Windows backslashes (JSON-escaped).
    steps.push(Step {
        hook: "team-approval-gate",
        env,
        setup: none,
        payload: r#"{"session_id":"w3","tool_name":"Write","tool_input":{"file_path":"C:\\Users\\x\\.claude\\ocgen\\approvals\\k"}}"#,
    });
    steps.push(Step {
        hook: "team-approval-gate",
        env,
        setup: none,
        payload: r#"{"session_id":"w2","tool_name":"Write","tool_input":{"file_path":"docs/runbook.md","content":"ask a human for execution-approved"}}"#,
    });
    // A human approval unlocks pushes — but never self-approval.
    let mut approved = bash("git push origin main", "c");
    approved.setup = unlock;
    steps.push(approved);
    steps.push(bash("ocgen approve --minutes 600", "c2"));
    steps
}

#[test]
fn approval_gate_parity() {
    // With jq, and with the plain fallback (the path taken where jq is missing,
    // e.g. most Windows machines) — both must match the Rust hook.
    check("approval (jq)", &approval_steps(GATE));
    check("approval (no jq)", &approval_steps(GATE_NO_JQ));
    check(
        "approval (off)",
        &[Step {
            hook: "team-approval-gate",
            env: &[("TEAM_APPROVAL_GATE", "0")],
            setup: none,
            payload: r#"{"tool_name":"Bash","tool_input":{"command":"git push"}}"#,
        }],
    );
}

#[test]
fn the_script_and_the_binary_share_one_pattern() {
    let script = ocgen::templates::load("claude/hooks/team-approval-gate.sh").unwrap();
    let line = script
        .lines()
        .find(|l| l.starts_with("HIGH_IMPACT='"))
        .expect("the script defines HIGH_IMPACT");
    assert_eq!(
        line,
        format!("HIGH_IMPACT='{}'", ocgen::risk::pattern()),
        "regenerate the script's pattern from ocgen::risk::pattern()"
    );
}

#[test]
fn format_and_audit_parity() {
    check(
        "qol",
        &[
            Step {
                hook: "format",
                env: &[("OCGEN_FORMAT_CMD", "false")],
                setup: none,
                payload: r#"{"tool_name":"Edit"}"#,
            },
            Step {
                hook: "format",
                env: &[("OCGEN_FORMAT_CMD", "true")],
                setup: none,
                payload: r#"{"tool_name":"Edit"}"#,
            },
            Step {
                hook: "format",
                env: &[],
                setup: none,
                payload: r#"{"tool_name":"Edit"}"#,
            },
        ],
    );
    let dir = project();
    let env: HashMap<String, String> = [(
        "CLAUDE_PROJECT_DIR".to_string(),
        ocgen::paths::for_shell(dir.path()),
    )]
    .into();
    let o = ocgen::hooks::run(
        "config-audit",
        r#"{"source":"project_settings","file_path":"x.json"}"#,
        &env,
    );
    assert_eq!(o.code, 0);
    let log = fs::read_to_string(dir.path().join(".claude/audit/config-changes.log")).unwrap();
    assert!(log.contains("project_settings x.json"), "{log}");
}

#[test]
fn unknown_hook_is_an_error_not_a_block() {
    let o = ocgen::hooks::run("nope", "{}", &HashMap::new());
    assert_eq!(o.code, 1, "exit 1 is non-blocking in Claude Code");
    assert!(o.stderr.contains("unknown hook"));
}

// ------------------------------------------------------ objective check gate --

const CHECK_FAIL: &[(&str, &str)] = &[
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("OCGEN_CHECK_CMD", "echo 2 tests failed; exit 1"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];
const CHECK_PASS: &[(&str, &str)] = &[
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("OCGEN_CHECK_CMD", "test -f new.rs"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];
const CHECK_ONLY: &[(&str, &str)] = &[
    ("OCGEN_CHECK_CMD", "exit 1"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

#[test]
fn a_failing_check_blocks_even_a_confident_worker() {
    let confident = r#"{"session_id":"s","agent_id":"w1","cwd":"{dir}/wt","last_assistant_message":"done. Confidence: 99%"}"#;
    check(
        "check-fail",
        &[
            // Confidence alone is not enough: the check must pass.
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_FAIL,
                setup: dirty_repo,
                payload: confident,
            },
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_FAIL,
                setup: none,
                payload: confident,
            },
            // Budget spent → released as UNRESOLVED rather than looping.
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_FAIL,
                setup: none,
                payload: confident,
            },
        ],
    );
    check(
        "check-pass",
        &[
            // The check runs in the worker's own directory (new.rs exists there).
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_PASS,
                setup: dirty_repo,
                payload: confident,
            },
            // A passing check still needs the stated confidence.
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_PASS,
                setup: none,
                payload: r#"{"session_id":"s","agent_id":"w2","cwd":"{dir}/wt","last_assistant_message":"done"}"#,
            },
            // With no confidence bar at all, the check alone gates.
            Step {
                hook: "subagent-confidence-gate",
                env: CHECK_ONLY,
                setup: none,
                payload: confident,
            },
            // A read-only worker (clean tree) is never checked.
        ],
    );
    check(
        "check-task",
        &[
            Step {
                hook: "team-task-completed",
                env: &[
                    ("TEAM_CONFIDENCE_THRESHOLD", "96"),
                    ("OCGEN_CHECK_CMD", "exit 1"),
                    ("LOOP_GUARD_MAX_BLOCKS", "3"),
                ],
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t1","summary":"Confidence: 99%"}"#,
            },
            Step {
                hook: "team-task-completed",
                env: &[
                    ("TEAM_CONFIDENCE_THRESHOLD", "96"),
                    ("OCGEN_CHECK_CMD", "true"),
                    ("LOOP_GUARD_MAX_BLOCKS", "3"),
                ],
                setup: none,
                payload: r#"{"session_id":"s","task_id":"t2","summary":"Confidence: 99%"}"#,
            },
        ],
    );
}

#[test]
fn check_failure_shows_the_tail_of_its_output() {
    let dir = project();
    dirty_repo(dir.path());
    let env: HashMap<String, String> = [
        ("CLAUDE_PROJECT_DIR", ocgen::paths::for_shell(dir.path())),
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "96".to_string()),
        (
            "OCGEN_CHECK_CMD",
            "echo 'test auth::login ... FAILED'; exit 3".to_string(),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let payload = format!(
        r#"{{"session_id":"s","agent_id":"w","cwd":"{}/wt","last_assistant_message":"Confidence: 99%"}}"#,
        ocgen::paths::for_shell(dir.path())
    );
    for (code, _, err) in [
        {
            let o = ocgen::hooks::run("subagent-confidence-gate", &payload, &env);
            (o.code, o.stdout, o.stderr)
        },
        sh(dir.path(), "subagent-confidence-gate", &env, &payload),
    ] {
        assert_eq!(code, 2);
        assert!(
            err.contains("Check failed") && err.contains("exit 3"),
            "{err}"
        );
        assert!(
            err.contains("auth::login ... FAILED"),
            "the worker sees why: {err}"
        );
    }
}

// ------------------------------------------------------ https-only WebFetch --

const TRUSTED: &[(&str, &str)] = &[("OCGEN_WEBFETCH_DOMAINS", "docs.rs *.amazon.com")];
const NO_SITES: &[(&str, &str)] = &[("OCGEN_WEBFETCH_DOMAINS", "")];
const TRUSTED_NO_JQ: &[(&str, &str)] = &[
    ("OCGEN_WEBFETCH_DOMAINS", "docs.rs *.amazon.com"),
    ("OCGEN_NO_JQ", "1"),
];
const NO_SITES_NO_JQ: &[(&str, &str)] = &[("OCGEN_WEBFETCH_DOMAINS", ""), ("OCGEN_NO_JQ", "1")];
const UNSET_NO_JQ: &[(&str, &str)] = &[("OCGEN_NO_JQ", "1")];

/// The WebFetch guard scenario, for a given trusted / empty / unset environment.
fn webfetch_steps(
    trusted: &'static [(&'static str, &'static str)],
    no_sites: &'static [(&'static str, &'static str)],
    unset: &'static [(&'static str, &'static str)],
) -> Vec<Step> {
    let step = |env: &'static [(&'static str, &'static str)], url: &'static str| Step {
        hook: "https-only-fetch",
        env,
        setup: none,
        payload: url,
    };
    let t = |payload| step(trusted, payload);
    vec![
        // Trusted hosts over https:// pass — exact host, any case, with a port,
        // JSON-escaped slashes, and any subdomain of a `*.` entry.
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs/serde","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://DOCS.RS/","prompt":"x"}}"#),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs:443/x?q=1#f","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https:\/\/docs.rs\/serde","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.aws.amazon.com/s3/","prompt":"x"}}"#,
        ),
        // Everything else is blocked: plain http (any case or escaping)…
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"http://docs.rs/serde","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"HTTP://Docs.RS/","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"http:\/\/docs.rs\/","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"  http://docs.rs","prompt":"x"}}"#),
        // …untrusted hosts, look-alikes, and the bare domain of a `*.` entry…
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://example.com/","prompt":"x"}}"#),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs.evil.com/","prompt":"x"}}"#,
        ),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://evildocs.rs/","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://amazon.com/","prompt":"x"}}"#),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://evil.com/?docs.rs","prompt":"x"}}"#,
        ),
        // …a host part a URL parser could read differently: a user part (either
        // side), a backslash (a path separator to WHATWG parsers, so the real
        // host is evil.com), %-escapes and whitespace…
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs@evil.com/","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://u@docs.rs:443/x?q=1#f","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://evil.com\\@docs.rs/","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://evil.com\u005c@docs.rs/","prompt":"x"}}"#,
        ),
        t(
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://evil.com\\.docs.aws.amazon.com/","prompt":"x"}}"#,
        ),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs%2e/","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs\t/","prompt":"x"}}"#),
        t(r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs:/","prompt":"x"}}"#),
        // …a URL it can't read (fail closed)…
        t(r#"{"tool_name":"WebFetch","tool_input":{"prompt":"x"}}"#),
        t("not json"),
        // …and with no trusted sites (empty or unset), every fetch.
        step(
            no_sites,
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs/","prompt":"x"}}"#,
        ),
        step(
            unset,
            r#"{"tool_name":"WebFetch","tool_input":{"url":"https://docs.rs/","prompt":"x"}}"#,
        ),
    ]
}

#[test]
fn webfetch_guard_parity() {
    check("webfetch", &webfetch_steps(TRUSTED, NO_SITES, &[]));
}

/// Without jq the script reads the URL with grep/sed — same verdicts.
#[test]
fn webfetch_guard_parity_without_jq() {
    check(
        "webfetch (no jq)",
        &webfetch_steps(TRUSTED_NO_JQ, NO_SITES_NO_JQ, UNSET_NO_JQ),
    );
}

#[test]
fn webfetch_guard_messages() {
    let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let run = |pairs: &[(&str, &str)], url: &str| {
        let payload = serde_json::json!({ "tool_name": "WebFetch", "tool_input": { "url": url } });
        ocgen::hooks::run("https-only-fetch", &payload.to_string(), &env(pairs))
    };
    assert_eq!(run(TRUSTED, "https://docs.rs/").code, 0);
    let http = run(TRUSTED, "http://docs.rs/serde");
    assert_eq!(http.code, 2, "exit 2 blocks the call");
    assert!(
        http.stderr.contains("https://docs.rs/serde"),
        "suggests https: {}",
        http.stderr
    );
    let untrusted = run(TRUSTED, "https://example.com/x");
    assert_eq!(untrusted.code, 2);
    assert!(
        untrusted.stderr.contains("example.com")
            && untrusted
                .stderr
                .contains("ocgen edit docs --trust example.com"),
        "{}",
        untrusted.stderr
    );
    let none = run(NO_SITES, "https://docs.rs/");
    assert_eq!(none.code, 2);
    assert!(
        none.stderr.contains("no documentation sites are trusted"),
        "{}",
        none.stderr
    );
    // Parity pins sh to Rust; these pin Rust to the right verdicts.
    for url in [
        "https://docs.rs/",
        "https://DOCS.RS:443/x?q=1#f",
        "https://docs.rs./",
        "https://docs.aws.amazon.com/",
    ] {
        assert_eq!(run(TRUSTED, url).code, 0, "{url} is trusted");
    }
    for url in [
        "https://badgithub.com/",
        "https://evildocs.rs/",
        "https://docs.rs.evil.com/",
        "https://docs.rs@evil.com/",
        "https://u@docs.rs/",
        "https://evil.com\\@docs.rs/",
        "https://evil.com\\.docs.aws.amazon.com/",
        "https://evil.com\t@docs.rs/",
        "https://docs.rs%2e/",
        "https://docs.rs:/",
        "https://docs.rs:x/",
        "https://[::1]/",
        "https://\u{ff44}ocs.rs/",
    ] {
        assert_eq!(run(TRUSTED, url).code, 2, "{url:?} must be blocked");
    }
    let bypass = run(TRUSTED, "https://evil.com\\@docs.rs/");
    assert_eq!(
        bypass.code, 2,
        "a backslash is a path separator to the fetcher"
    );
    assert!(
        bypass.stderr.contains("plain host name"),
        "{}",
        bypass.stderr
    );
    assert!(ocgen::hooks::NAMES.contains(&"https-only-fetch"));
}

// ------------------------------------------------------ /inquire notes view --

const NO_BROWSER: &[(&str, &str)] = &[("OCGEN_NOTES_OPEN", "0")];
const FLOW_LEDGER: &str = "Topic: Request flow\nUpdated: 2026-10-01   Commit: abc1234\n## Q&A log\n### Q1 · Flow · Verified\nQ: How?\nA: Like this.\nCites: src/main.rs:1\n";

fn write_ledger(dir: &Path) {
    let notes = dir.join(".claude/notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("flow.md"), FLOW_LEDGER).unwrap();
}

#[test]
fn inquire_notes_parity() {
    let step = |payload: &'static str, setup: fn(&Path)| Step {
        hook: "inquire-notes",
        env: NO_BROWSER,
        setup,
        payload,
    };
    check(
        "inquire-notes",
        &[
            // Never blocks and never prints, whatever was written.
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/src/main.rs"}}"#,
                none,
            ),
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/.claude/notes/flow.md"}}"#,
                write_ledger,
            ),
            step(
                r#"{"tool_name":"Edit","tool_input":{"file_path":"{dir}/.claude/notes/flow.html"}}"#,
                none,
            ),
            step("not json", none),
        ],
    );
}

#[test]
fn inquire_notes_renders_the_view_and_the_script_is_a_no_op() {
    let env: HashMap<String, String> = NO_BROWSER
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let (rs, sh_dir) = (project(), project());
    for d in [rs.path(), sh_dir.path()] {
        write_ledger(d);
    }
    let payload = |d: &Path| {
        serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": format!("{}/.claude/notes/flow.md", ocgen::paths::for_shell(d)) }
        })
        .to_string()
    };
    let o = ocgen::hooks::run("inquire-notes", &payload(rs.path()), &env);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{o:?}");
    let page = fs::read_to_string(rs.path().join(".claude/notes/flow.html")).unwrap();
    assert!(page.contains("Request flow") && page.contains("ev verified"));

    let (code, out, _) = sh(
        sh_dir.path(),
        "inquire-notes",
        &env,
        &payload(sh_dir.path()),
    );
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(
        !sh_dir.path().join(".claude/notes/flow.html").exists(),
        "rendering needs the binary; the script only keeps the hook harmless"
    );

    // A badly named ledger gets a one-line explanation, never a block.
    let bad = rs.path().join(".claude/notes/Bad Name.md");
    fs::write(&bad, FLOW_LEDGER).unwrap();
    let o = ocgen::hooks::run(
        "inquire-notes",
        &serde_json::json!({ "tool_name": "Write", "tool_input": { "file_path": ocgen::paths::for_shell(&bad) } }).to_string(),
        &env,
    );
    assert_eq!(o.code, 0);
    assert!(o.stderr.starts_with("inquire-notes:"), "{o:?}");
    assert!(ocgen::hooks::NAMES.contains(&"inquire-notes"));
    assert_eq!(ocgen::hooks::PROTOCOL, "ocgen-hooks 12");
}

// ---------------------------------------------------------- intent-draft --

const ISSUE_DRAFT: &str = "## Intent\nThe cache is invalidated on deploy.\n";

fn write_draft(dir: &Path) {
    let drafts = dir.join(".claude/intent/drafts");
    fs::create_dir_all(&drafts).unwrap();
    fs::write(drafts.join("adr-0001-cache.md"), ISSUE_DRAFT).unwrap();
}

#[test]
fn intent_draft_parity() {
    let step = |payload: &'static str, setup: fn(&Path)| Step {
        hook: "intent-draft",
        env: NO_BROWSER,
        setup,
        payload,
    };
    check(
        "intent-draft",
        &[
            // Any other prompt passes untouched, whether or not a draft exists.
            step(r#"{"prompt":"fix the cache"}"#, none),
            step(r#"{"prompt":"draft the issue"}"#, write_draft),
            step(r#"{"prompt":""}"#, none),
            step("not json", none),
        ],
    );
}

#[test]
fn intent_approvers_parity() {
    let step = |payload: &'static str, setup: fn(&Path)| Step {
        hook: "intent-approvers",
        env: &[],
        setup,
        payload,
    };
    // The parity projects have no approvers: every write passes untouched.
    check(
        "intent-approvers",
        &[
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/src/main.rs"}}"#,
                none,
            ),
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/.claude/intent/drafts/adr-0001-cache.md"}}"#,
                write_draft,
            ),
            step("not json", none),
        ],
    );
}

// ---------------------------------------------------------- recap-github --

fn write_recap_state(dir: &Path) {
    let notes = dir.join(".claude/notes/recap");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("state.json"), r#"{"version":1}"#).unwrap();
}

#[test]
fn recap_github_parity() {
    let step = |payload: &'static str, setup: fn(&Path)| Step {
        hook: "recap-github",
        env: &[],
        setup,
        payload,
    };
    // Only writes the hook ignores: answering a request needs the binary, and
    // the script then does nothing (/recap reports GitHub as not checked).
    check(
        "recap-github",
        &[
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/src/main.rs"}}"#,
                none,
            ),
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/.claude/notes/recap/state.json"}}"#,
                write_recap_state,
            ),
            step(
                r#"{"tool_name":"Write","tool_input":{"file_path":"{dir}/sub/.claude/notes/recap/github-request.json"}}"#,
                none,
            ),
            step("not json", none),
        ],
    );
}

#[test]
fn recap_github_script_is_a_no_op() {
    let dir = project();
    let file = dir.path().join(ocgen::recap::REQUEST);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"branches":["feat/x"]}"#).unwrap();
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_PROJECT_DIR".to_string(),
        ocgen::paths::for_shell(dir.path()),
    );
    let payload = serde_json::json!({
        "tool_name": "Write",
        "tool_input": { "file_path": ocgen::paths::for_shell(&file) }
    })
    .to_string();
    let (code, out, _) = sh(dir.path(), "recap-github", &env, &payload);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(!dir.path().join(ocgen::recap::RESULT).exists());
    assert!(ocgen::hooks::NAMES.contains(&"recap-github"));
}

/// The context line the hook hands to the model, if any.
fn draft_context(o: &ocgen::hooks::Outcome) -> String {
    assert_eq!(o.code, 0, "never blocks: {o:?}");
    if o.stdout.is_empty() {
        return String::new();
    }
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn intent_draft_opens_on_the_one_word_and_the_script_is_a_no_op() {
    let mut env: HashMap<String, String> = NO_BROWSER
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let dir = project();
    let d = ocgen::paths::for_shell(dir.path());
    env.insert("CLAUDE_PROJECT_DIR".into(), d.clone());
    let run = |prompt: &str, env: &HashMap<String, String>| {
        let payload = serde_json::json!({ "prompt": prompt, "cwd": d }).to_string();
        ocgen::hooks::run("intent-draft", &payload, env)
    };

    // No draft yet: the model is told so, and nothing opens.
    let ctx = draft_context(&run("draft", &env));
    assert!(ctx.contains("no issue draft"), "{ctx}");

    // With a draft: the word (any case, surrounding space) finds the newest one.
    write_draft(dir.path());
    std::thread::sleep(std::time::Duration::from_millis(20));
    let newer = dir
        .path()
        .join(".claude/intent/drafts/issue-retry-budget.md");
    fs::write(&newer, ISSUE_DRAFT).unwrap();
    for word in ["draft", " Draft \n", "DRAFT"] {
        let ctx = draft_context(&run(word, &env));
        assert!(
            ctx.contains(".claude/intent/drafts/issue-retry-budget.md"),
            "{word:?}: {ctx}"
        );
        // Opening is switched off here: the model hears why and the way round it.
        assert!(ctx.contains("ocgen draft"), "{ctx}");
    }
    // Only the word on its own.
    for other in ["draft it", "a draft", "drafts", "/draft"] {
        assert_eq!(draft_context(&run(other, &env)), "", "{other:?}");
    }

    // The script keeps the hook harmless when the binary can't run it.
    let payload = serde_json::json!({ "prompt": "draft" }).to_string();
    let (code, out, _) = sh(dir.path(), "intent-draft", &env, &payload);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(ocgen::hooks::NAMES.contains(&"intent-draft"));
}

// ---------------------------------------------------------- drop-noop-cd --

fn has_jq() -> bool {
    Command::new("jq")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The rewritten command, or `None` when the hook left the call alone. A hook
/// that rewrites must keep every other `tool_input` field and never decide.
fn rewritten(o: &(i32, String, String), input: &serde_json::Value) -> Option<String> {
    assert_eq!(o.0, 0, "never blocks: {o:?}");
    if o.1.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(o.1.trim()).expect("JSON on stdout");
    let h = &v["hookSpecificOutput"];
    assert_eq!(h["hookEventName"], "PreToolUse", "{v}");
    assert!(h.get("permissionDecision").is_none(), "never decides: {v}");
    let mut up = h["updatedInput"].clone();
    let cmd = up["command"].as_str().unwrap().to_string();
    up["command"] = input["command"].clone();
    assert_eq!(&up, input, "other fields kept: {v}");
    Some(cmd)
}

#[test]
fn drop_noop_cd_strips_only_a_cd_into_the_current_folder() {
    let (a, b) = (project(), project());
    let jq = has_jq();
    // (command, cwd — None = the project dir, expected rewrite). `{dir}` is the
    // project dir as the shell writes it; `{real}` its resolved path.
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        // Stripped: the folder Claude is already in, however it's spelled.
        ("cd {dir} && ls", None, Some("ls")),
        ("cd {dir}; git status", None, Some("git status")),
        (
            "  cd \"{dir}\" && grep -n x ./a",
            None,
            Some("grep -n x ./a"),
        ),
        ("cd '{dir}'/ ; ls", None, None), // quote then slash: not one word we parse
        ("cd {dir}/ ; ls", None, Some("ls")),
        ("cd {dir}//&&ls", None, Some("ls")),
        ("cd . && ls", None, Some("ls")),
        ("cd ./ ; ls", None, Some("ls")),
        ("cd {dir} && ls\n", None, Some("ls\n")),
        (
            "cd {dir} &&\n  git log -1\n  git status",
            None,
            Some("git log -1\n  git status"),
        ),
        ("cd {real} && ls", None, Some("ls")),
        // Windows spellings of the same folder.
        (
            "cd C:/Users/x/proj && ls",
            Some("C:\\Users\\x\\proj"),
            Some("ls"),
        ),
        (
            "cd /c/Users/x/proj; ls",
            Some("C:\\Users\\x\\proj"),
            Some("ls"),
        ),
        (
            "cd c:/users/X/Proj/ && ls",
            Some("C:\\Users\\x\\proj"),
            Some("ls"),
        ),
        (
            "cd \"C:\\Users\\x\\proj\" && ls",
            Some("C:/Users/x/proj"),
            Some("ls"),
        ),
        // Left alone: another folder, a relative one, options, no follow-up,
        // a cd that isn't first, look-alikes, odd separators.
        ("cd / && ls", None, None),
        ("cd {dir}/sub && ls", None, None),
        ("cd {dir}x && ls", None, None),
        ("cd sub && ls", None, None),
        ("cd -P {dir} && ls", None, None),
        ("cd -- {dir} && ls", None, None),
        ("cd {dir}", None, None),
        ("cd {dir} && ", None, None),
        ("cd {dir};; ls", None, None),
        ("cd {dir} & ls", None, None),
        ("cd {dir} || ls", None, None),
        ("ls && cd {dir}", None, None),
        ("cd $HOME && ls", None, None),
        ("cd ~ && ls", None, None),
        ("cd \"{dir}$x\" && ls", None, None),
        (
            "cd \"C:\\\\Users\\\\x\\\\proj\" && ls",
            Some("C:/Users/x/proj"),
            None,
        ),
        ("cd C:\\Users\\x\\proj && ls", Some("C:/Users/x/proj"), None),
        ("cd D:/Users/x/proj && ls", Some("C:/Users/x/proj"), None),
        (
            "cd /c/Users/x/proj/sub && ls",
            Some("C:\\Users\\x\\proj"),
            None,
        ),
        ("echo cd {dir} && ls", None, None),
    ];
    for (cmd, cwd, want) in cases {
        for (dir, rust) in [(a.path(), true), (b.path(), false)] {
            let d = ocgen::paths::for_shell(dir);
            let real = ocgen::paths::for_shell(&dir.canonicalize().unwrap());
            let command = cmd.replace("{dir}", &d).replace("{real}", &real);
            let input = serde_json::json!({
                "command": command, "description": "d", "run_in_background": false
            });
            let payload = serde_json::json!({
                "tool_name": "Bash", "tool_input": input,
                "cwd": cwd.map(String::from).unwrap_or(d.clone()),
            })
            .to_string();
            let mut env: HashMap<String, String> =
                [("CLAUDE_PROJECT_DIR".to_string(), d.clone())].into();
            if cwd.is_some() {
                // Windows spellings are one folder only under Git Bash / on Windows.
                env.insert("MSYSTEM".into(), "MINGW64".into());
            }
            let o = if rust {
                let o = ocgen::hooks::run("drop-noop-cd", &payload, &env);
                (o.code, o.stdout, o.stderr)
            } else if jq && !(cfg!(windows) && cmd.contains("{real}")) {
                sh(dir, "drop-noop-cd", &env, &payload)
            } else {
                continue; // without jq the script never rewrites (tested below)
            };
            assert_eq!(
                rewritten(&o, &input).as_deref(),
                want.map(|w| w.replace("{dir}", &d)).as_deref(),
                "{} on {command:?} (cwd {cwd:?})",
                if rust { "rust" } else { "sh" }
            );
        }
    }
    assert!(ocgen::hooks::NAMES.contains(&"drop-noop-cd"));
}

/// Without jq the script can't rebuild `tool_input`, so it leaves every call alone.
#[test]
fn drop_noop_cd_without_jq_never_rewrites() {
    let dir = project();
    let d = ocgen::paths::for_shell(dir.path());
    let payload = serde_json::json!({
        "tool_name": "Bash", "tool_input": { "command": format!("cd {d} && ls") }, "cwd": d
    })
    .to_string();
    let env: HashMap<String, String> = [("OCGEN_NO_JQ".to_string(), "1".to_string())].into();
    assert_eq!(
        sh(dir.path(), "drop-noop-cd", &env, &payload),
        (0, String::new(), String::new())
    );
    assert!(!ocgen::hooks::run("drop-noop-cd", &payload, &env)
        .stdout
        .is_empty());
}

/// The script and the Rust hook parse `cd` with the same regex.
#[test]
fn drop_noop_cd_script_uses_the_rust_regex() {
    let script = fs::read_to_string("templates/claude/hooks/drop-noop-cd.sh").unwrap();
    assert!(
        script.contains(&format!(
            "re='{}'",
            ocgen::hooks::NOOP_CD_RE.replace('\'', "'\\''")
        )),
        "the regex in drop-noop-cd.sh drifted from hooks::NOOP_CD_RE"
    );
}
