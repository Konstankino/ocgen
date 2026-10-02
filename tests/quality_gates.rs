//! The quality gates read the right input: the teammate's or worker's own words,
//! the right working tree, the right owner and a current plan approval.
//!
//! Every scenario runs step by step through both implementations — the Rust hook
//! (`ocgen hook <name>`) and the bundled `sh` script, each in its own fresh
//! project — and each step's exit code, stdout kind and first stderr line must
//! match, as well as the verdict the step pins. Fixtures follow the payloads
//! Claude Code 2.1.277 sends (TaskCompleted carries task_id, task_subject,
//! task_description, teammate_name and team_name; SubagentStop adds
//! last_assistant_message and background_tasks) and its transcript lines.

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
use serde_json::json;
use tempfile::TempDir;

/// An env value that removes the variable instead of setting it.
const UNSET: &str = "\u{0}unset";

fn project() -> TempDir {
    project_with(|_| {})
}

fn project_with(tune: impl FnOnce(&mut Project)) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "qg".into();
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
    p.claude.workflow.subagent_confidence = 96;
    p.claude.hooks_extra.format_cmd = "true".into();
    p.claude.hooks_extra.drop_noop_cd = true;
    tune(&mut p);
    p.scaffold(dir.path(), false).unwrap();
    git(dir.path(), &["init", "-q"]);
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

/// One hook run: (exit code, stdout, stderr).
type Out = (i32, String, String);

fn run_sh(dir: &Path, hook: &str, env: &HashMap<String, String>, payload: &str) -> Out {
    let mut cmd = Command::new("sh");
    cmd.arg(dir.join(format!(".claude/hooks/{hook}.sh")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The real environment (a shell needs more than PATH on Windows), minus
    // anything that would steer a gate.
    for k in [
        "TEAM_PLAN_GATE",
        "TEAM_RISK_ROUNDS",
        "TEAM_READONLY_ROLES",
        "TEAM_CONFIDENCE_THRESHOLD",
        "SUBAGENT_CONFIDENCE_THRESHOLD",
        "SUBAGENT_READONLY_ROLES",
        "LOOP_GUARD_MAX_BLOCKS",
        "OCGEN_CHECK_CMD",
        "OCGEN_CHECK_TIMEOUT",
        "OCGEN_FORMAT_CMD",
        "OCGEN_NO_JQ",
        "OCGEN_HOOK_PROBE",
        "OCGEN_TRANSCRIPT_SETTLE_MS",
        "OCGEN_SANDBOX",
        "OCGEN_SANDBOX_DENY_WRITE",
        "OCGEN_SANDBOX_DENY_READ",
        "OCGEN_SANDBOX_DENY_ENV",
        "MSYSTEM",
    ] {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        if v == UNSET {
            cmd.env_remove(k);
        } else {
            cmd.env(k, v);
        }
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

fn run_rs(hook: &str, env: &HashMap<String, String>, payload: &str) -> Out {
    let env: HashMap<String, String> = env
        .iter()
        .filter(|(_, v)| *v != UNSET)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let o = ocgen::hooks::run(hook, payload, &env);
    (o.code, o.stdout, o.stderr)
}

struct Step {
    hook: &'static str,
    env: Vec<(&'static str, String)>,
    /// `{dir}` is replaced with the project directory, as the shell writes it.
    payload: String,
    /// Filesystem changes applied to each project before this step.
    setup: Box<dyn Fn(&Path)>,
    expect: i32,
    /// Both implementations' stderr must contain this.
    err: &'static str,
    /// Both implementations' stdout must be a JSON object with exactly these keys.
    out: Option<&'static [&'static str]>,
}

fn step(hook: &'static str, env: &[(&'static str, &str)], payload: String, expect: i32) -> Step {
    Step {
        hook,
        env: env.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        payload,
        setup: Box::new(|_| {}),
        expect,
        err: "",
        out: None,
    }
}

impl Step {
    fn setup(mut self, f: impl Fn(&Path) + 'static) -> Self {
        self.setup = Box::new(f);
        self
    }
    fn err(mut self, s: &'static str) -> Self {
        self.err = s;
        self
    }
    fn out(mut self, keys: &'static [&'static str]) -> Self {
        self.out = Some(keys);
        self
    }
}

fn keys(o: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(o.trim()).ok()?;
    let mut k: Vec<String> = v.as_object()?.keys().cloned().collect();
    k.sort();
    Some(k)
}

/// Run `steps` through the script (first project) and the binary (second), with
/// `extra` added to every step's environment. Returns both projects.
fn parity(name: &str, steps: &[Step], extra: &[(&'static str, &str)]) -> (TempDir, TempDir) {
    let (a, b) = (project(), project());
    for (i, s) in steps.iter().enumerate() {
        let mut outs: Vec<Out> = Vec::new();
        for (dir, rust) in [(a.path(), false), (b.path(), true)] {
            (s.setup)(dir);
            let d = ocgen::paths::for_shell(dir);
            let mut env: HashMap<String, String> = s
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.replace("{dir}", &d)))
                .collect();
            for (k, v) in extra {
                env.insert(k.to_string(), v.to_string());
            }
            env.insert("CLAUDE_PROJECT_DIR".into(), d.clone());
            env.insert("HOME".into(), format!("{d}/.home"));
            let payload = s.payload.replace("{dir}", &d);
            outs.push(if rust {
                run_rs(s.hook, &env, &payload)
            } else {
                run_sh(dir, s.hook, &env, &payload)
            });
        }
        let (sh, rs) = (&outs[0], &outs[1]);
        let ctx = format!("{name} step {i} ({}):\nsh={sh:?}\nrust={rs:?}", s.hook);
        assert_eq!(sh.0, rs.0, "exit code differs — {ctx}");
        assert_eq!(rs.0, s.expect, "unexpected verdict — {ctx}");
        assert_eq!(keys(&sh.1), keys(&rs.1), "stdout differs — {ctx}");
        if let Some(want) = s.out {
            let want: Vec<String> = want.iter().map(|k| k.to_string()).collect();
            assert_eq!(keys(&rs.1), Some(want), "stdout kind — {ctx}");
        }
        assert_eq!(
            sh.2.lines().next(),
            rs.2.lines().next(),
            "first stderr line differs — {ctx}"
        );
        assert!(
            sh.2.contains(s.err) && rs.2.contains(s.err),
            "stderr lacks {:?} — {ctx}",
            s.err
        );
    }
    let log = |d: &Path| {
        fs::read_to_string(d.join(".claude/loop-guard/escalations.md"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.splitn(3, ' ').nth(2).unwrap_or(l).to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        log(a.path()),
        log(b.path()),
        "{name}: escalation logs differ"
    );
    (a, b)
}

/// The jq path and the plain fallback (most Windows machines have no jq).
const JQ: &[(&str, &str)] = &[];
const NO_JQ: &[(&str, &str)] = &[("OCGEN_NO_JQ", "1")];

// -------------------------------------------------------------- transcripts --

/// A transcript line the way Claude Code writes it: one content block per line,
/// keys in its order (`"type":"text","text":…`).
fn assistant_text(text: &str) -> String {
    let t = serde_json::to_string(text).unwrap();
    format!(
        r#"{{"parentUuid":"p1","isSidechain":false,"message":{{"model":"claude-opus-5","id":"msg_01Text","type":"message","role":"assistant","content":[{{"type":"text","text":{t}}}],"container":null,"stop_reason":"end_turn","stop_sequence":null,"stop_details":null,"usage":{{"input_tokens":2,"output_tokens":9}}}},"requestId":"req_01","type":"assistant","uuid":"u1","timestamp":"2026-10-02T10:00:01.000Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s","version":"2.1.277","gitBranch":"main"}}"#
    )
}

fn assistant_thinking() -> String {
    r#"{"parentUuid":"p0","isSidechain":false,"message":{"model":"claude-opus-5","id":"msg_01Text","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Confidence: 12% so far","signature":"sig"}],"stop_reason":null,"stop_sequence":null},"requestId":"req_01","type":"assistant","uuid":"u0","timestamp":"2026-10-02T10:00:00.000Z","sessionId":"s","version":"2.1.277"}"#.to_string()
}

fn assistant_tool_use() -> String {
    r#"{"parentUuid":"p2","isSidechain":false,"message":{"model":"claude-opus-5","id":"msg_02Tool","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test","description":"Confidence: 5%"}}],"stop_reason":"tool_use","stop_sequence":null},"type":"assistant","uuid":"u2","timestamp":"2026-10-02T10:00:02.000Z","sessionId":"s","version":"2.1.277"}"#.to_string()
}

fn user_tool_result(text: &str) -> String {
    let t = serde_json::to_string(text).unwrap();
    format!(
        r#"{{"parentUuid":"u2","isSidechain":false,"type":"user","message":{{"role":"user","content":[{{"tool_use_id":"toolu_1","type":"tool_result","content":[{{"type":"text","text":{t}}}]}}]}},"uuid":"u3","timestamp":"2026-10-02T10:00:03.000Z","sessionId":"s","version":"2.1.277"}}"#
    )
}

fn sidechain_text(text: &str, agent: &str) -> String {
    assistant_text(text).replacen(
        r#""isSidechain":false"#,
        &format!(r#""isSidechain":true,"agentId":"{agent}""#),
        1,
    )
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn transcript(rel: &'static str, lines: Vec<String>) -> impl Fn(&Path) + 'static {
    move |dir: &Path| write(dir, rel, &format!("{}\n", lines.join("\n")))
}

/// A TaskCompleted event as Claude Code 2.1.277 builds it.
fn task_event(tid: &str, subject: &str, desc: &str, tp: &str, sid: &str, team: &str) -> String {
    json!({
        "session_id": sid,
        "transcript_path": format!("{{dir}}/{tp}"),
        "cwd": "{dir}",
        "permission_mode": "default",
        "hook_event_name": "TaskCompleted",
        "task_id": tid,
        "task_subject": subject,
        "task_description": desc,
        "teammate_name": "impl-1",
        "team_name": team,
    })
    .to_string()
}

const TASK: &[(&str, &str)] = &[
    ("TEAM_CONFIDENCE_THRESHOLD", "96"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
    // The wait for Claude Code's transcript writes has its own test.
    ("OCGEN_TRANSCRIPT_SETTLE_MS", "0"),
];

/// The lead's session and its implicit team (`session-<first 8 of its id>`).
const LEAD: &str = "abcdef12-0000-4000-8000-000000000000";
const TEAM: &str = "session-abcdef12";
/// A teammate in its own session (tmux / split panes) of the lead's team.
const MATE: &str = "fedcba98-1111-4000-8000-000000000000";

/// An in-process teammate's own transcript and its `.meta.json`, filed the way
/// Claude Code 2.1.277 files them: beside the lead's transcript, under
/// `subagents/` (`sub` is an optional folder below it), named by the agent id
/// (`a<name>-<hex>`), with the teammate's name, team and role in the meta.
fn in_process(d: &Path, sub: &str, id: &str, name: &str, team: &str, role: &str, said: &str) {
    let base = format!("tr/{LEAD}/subagents/{sub}agent-{id}");
    let meta = json!({
        "agentType": name, "description": "Parser", "name": name, "spawnDepth": 0,
        "requestShape": "background", "requestNonInteractive": true, "model": "opus",
        "taskKind": "in_process_teammate", "teamName": team, "color": "blue",
        "planModeRequired": false, "customAgentType": role, "permissionMode": "auto",
    });
    write(d, &format!("{base}.meta.json"), &meta.to_string());
    write(
        d,
        &format!("{base}.jsonl"),
        &format!("{}\n", sidechain_text(said, id)),
    );
}

fn lead_said(d: &Path, said: &str) {
    write(
        d,
        &format!("tr/{LEAD}.jsonl"),
        &format!("{}\n", assistant_text(said)),
    );
}

fn task_steps() -> Vec<Step> {
    let in_team = |team: &'static str| {
        move |tid: &str, subject: &str, desc: &str, tp: &str, sid: &str, expect| {
            step(
                "team-task-completed",
                TASK,
                task_event(tid, subject, desc, tp, sid, team),
                expect,
            )
        }
    };
    let s = in_team(TEAM);
    let lead_tp = format!("tr/{LEAD}.jsonl");
    let done = "Done; the parser handles every fixture.";
    vec![
        // The lead-written subject/description is never scored…
        s(
            "t1",
            "Raise parser confidence to 97% on fixtures",
            "Teammate should state Confidence: NN% when done",
            "tr/t1.jsonl",
            MATE,
            2,
        )
        .setup(transcript("tr/t1.jsonl", vec![assistant_text(done)]))
        .err(".claude/team/confidence/session-abcdef12/t1.txt"),
        // …the teammate's final message is, whatever the description says.
        s(
            "t2",
            "Speed up the parser",
            "current confidence 40 runs pass",
            "tr/t2.jsonl",
            MATE,
            0,
        )
        .setup(transcript(
            "tr/t2.jsonl",
            vec![
                assistant_text("Confidence: 50% while starting"),
                assistant_thinking(),
                assistant_tool_use(),
                user_tool_result("test result: ok. Confidence: 10%"),
                assistant_thinking(),
                assistant_text("All 12 fixtures pass.\n\nConfidence: 99%"),
            ],
        )),
        // An honest, low score with a target in the same line is the low score.
        s("t3", "x", "y", "tr/t3.jsonl", MATE, 2)
            .setup(transcript(
                "tr/t3.jsonl",
                vec![assistant_text("Partly done.\nConfidence: 60% (target 96%)")],
            ))
            .err("Confidence 60% is below the required 96%"),
        // Non-ASCII separators read the same in both, in any locale.
        s("t4", "x", "y", "tr/t4.jsonl", MATE, 0).setup(transcript(
            "tr/t4.jsonl",
            vec![assistant_text("Merged.\n**Confidence** — 97%")],
        )),
        // The marker fallback is keyed by team and task, first integer only…
        s("t6", "x", "y", "tr/none.jsonl", MATE, 0).setup(|d: &Path| {
            write(
                d,
                ".claude/team/confidence/session-abcdef12/t6.txt",
                "Confidence: 97% verified 2026-10-02\n",
            )
        }),
        s("t7", "x", "y", "tr/none.jsonl", MATE, 2)
            .setup(|d: &Path| {
                write(
                    d,
                    ".claude/team/confidence/session-abcdef12/t7.txt",
                    "9720261002\n",
                )
            })
            .err("No confidence found"),
        // …and the old, collision-prone per-task path is not read.
        s("t8", "x", "y", "tr/none.jsonl", MATE, 2)
            .setup(|d: &Path| write(d, ".claude/team/confidence/t8.txt", "97\n"))
            .err("No confidence found"),
        // An in-process teammate: transcript_path is the lead's, and its own
        // words are in the transcript its meta names it in — not a teammate's
        // of another name, and not the lead's.
        s("t9", "x", "y", &lead_tp, LEAD, 0).setup(|d: &Path| {
            lead_said(d, "Plan looks good.\nConfidence: 10%");
            in_process(
                d,
                "",
                "aimpl-2-1111111111111111",
                "impl-2",
                TEAM,
                "implementer",
                "Confidence: 50%",
            );
            in_process(
                d,
                "",
                "aimpl-1-0123456789abcdef",
                "impl-1",
                TEAM,
                "implementer",
                "Tests pass.\nConfidence: 98%",
            );
        }),
        s("t10", "x", "y", &lead_tp, LEAD, 2)
            .setup(|d: &Path| {
                lead_said(d, "Plan looks good.\nConfidence: 99%");
                in_process(
                    d,
                    "",
                    "aimpl-1-0123456789abcdef",
                    "impl-1",
                    TEAM,
                    "implementer",
                    "Tests pass.\nConfidence: 70%",
                );
            })
            .err("Confidence 70% is below"),
        // Claude Code may file an agent one folder deeper.
        s("t11", "x", "y", &lead_tp, LEAD, 0).setup(|d: &Path| {
            let _ = fs::remove_dir_all(d.join(format!("tr/{LEAD}/subagents")));
            in_process(
                d,
                "wf-1/",
                "aimpl-1-0123456789abcdef",
                "impl-1",
                TEAM,
                "implementer",
                "Confidence: 97%",
            );
        }),
        // Without its own transcript, the lead's words never count for it —
        // in the session's own team…
        s("t12", "x", "y", &lead_tp, LEAD, 2)
            .setup(|d: &Path| {
                let _ = fs::remove_dir_all(d.join(format!("tr/{LEAD}/subagents")));
            })
            .err("No confidence found"),
        // …or in a named team, where the other teammates' transcripts show
        // that transcript_path is the lead's.
        in_team("parser-team")("t13", "x", "y", &lead_tp, LEAD, 2)
            .setup(|d: &Path| {
                in_process(
                    d,
                    "",
                    "aimpl-2-1111111111111111",
                    "impl-2",
                    "parser-team",
                    "implementer",
                    "Confidence: 99%",
                );
            })
            .err(".claude/team/confidence/parser-team/t13.txt"),
        // A teammate in its own session: transcript_path is its own.
        s("t14", "x", "y", &format!("tr/{MATE}.jsonl"), MATE, 0).setup(transcript(
            "tr/fedcba98-1111-4000-8000-000000000000.jsonl",
            vec![assistant_text("Implemented.\n- Confidence: 97%")],
        )),
    ]
}

#[test]
fn task_gate_reads_the_teammates_own_words() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, b) = parity(&format!("task ({variant})"), &task_steps(), extra);
        for d in [a.path(), b.path()] {
            assert!(
                !d.join(".claude/team/confidence/session-abcdef12/t6.txt")
                    .exists(),
                "a passed task's marker is removed ({variant})"
            );
            assert!(d
                .join(".claude/team/confidence/session-abcdef12/t7.txt")
                .exists());
        }
    }
}

/// Claude Code appends transcript lines on a timer (every 100 ms), so a
/// teammate's final message can land just after its TaskCompleted hook starts:
/// both implementations wait for it before they read.
#[test]
fn task_gate_waits_for_the_final_message_to_land() {
    for rust in [true, false] {
        let dir = project();
        let d = ocgen::paths::for_shell(dir.path());
        write(
            dir.path(),
            "tr/late.jsonl",
            &format!("{}\n", assistant_text("Starting.\nConfidence: 50%")),
        );
        let env: HashMap<String, String> = [
            ("TEAM_CONFIDENCE_THRESHOLD", "96"),
            ("OCGEN_TRANSCRIPT_SETTLE_MS", "1500"),
            ("CLAUDE_PROJECT_DIR", d.as_str()),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let payload = task_event("t1", "x", "y", "tr/late.jsonl", MATE, TEAM).replace("{dir}", &d);
        let root = dir.path().to_path_buf();
        let hook = std::thread::spawn(move || {
            if rust {
                run_rs("team-task-completed", &env, &payload)
            } else {
                run_sh(&root, "team-task-completed", &env, &payload)
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("tr/late.jsonl"))
            .unwrap();
        writeln!(
            f,
            "{}",
            assistant_text("All fixtures pass.\nConfidence: 99%")
        )
        .unwrap();
        drop(f);
        let o = hook.join().unwrap();
        assert_eq!(
            o.0,
            0,
            "{} read the final message: {o:?}",
            if rust { "rust" } else { "sh" }
        );
    }
}

/// An event without `team_name`, which Claude Code marks deprecated (to be
/// removed): its team is then the one Claude Code makes for the session,
/// `session-<first 8 of its id>` — what every teammate meta it writes names.
fn without_team(event: String) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&event).unwrap();
    v.as_object_mut().unwrap().remove("team_name");
    v.to_string()
}

fn no_team_steps() -> Vec<Step> {
    let s = |tid: &str, tp: &str, sid: &str, expect| {
        step(
            "team-task-completed",
            TASK,
            without_team(task_event(tid, "x", "y", tp, sid, "")),
            expect,
        )
    };
    let idle = |name: &str, expect| {
        let v = json!({
            "session_id": LEAD, "transcript_path": format!("{{dir}}/tr/{LEAD}.jsonl"),
            "cwd": "{dir}", "permission_mode": "default", "hook_event_name": "TeammateIdle",
            "teammate_name": name,
        });
        step("team-teammate-idle", IDLE, v.to_string(), expect)
    };
    let lead_tp = format!("tr/{LEAD}.jsonl");
    let fresh = |d: &Path| {
        let _ = fs::remove_dir_all(d.join(format!("tr/{LEAD}/subagents")));
        lead_said(d, "Plan looks good.\nConfidence: 99%");
    };
    vec![
        // An in-process teammate's words are in the transcript its meta files
        // under the session's team — never the lead's.
        s("n1", &lead_tp, LEAD, 2)
            .setup(move |d: &Path| {
                fresh(d);
                in_process(
                    d,
                    "",
                    "aimpl-1-0123456789abcdef",
                    "impl-1",
                    TEAM,
                    "implementer",
                    "Partly done, two fixtures still fail.\nConfidence: 40%",
                );
            })
            .err("Confidence 40% is below"),
        s("n2", &lead_tp, LEAD, 0).setup(|d: &Path| {
            in_process(
                d,
                "",
                "aimpl-1-0123456789abcdef",
                "impl-1",
                TEAM,
                "implementer",
                "Tests pass.\nConfidence: 98%",
            )
        }),
        // With only other teammates filed there, the lead's words still don't
        // count: the marker under the session's team is the fallback…
        s("n3", &lead_tp, LEAD, 2)
            .setup(move |d: &Path| {
                fresh(d);
                in_process(
                    d,
                    "",
                    "aimpl-2-1111111111111111",
                    "impl-2",
                    TEAM,
                    "implementer",
                    "Confidence: 99%",
                );
            })
            .err(".claude/team/confidence/session-abcdef12/n3.txt"),
        s("n4", &lead_tp, LEAD, 0)
            .setup(|d: &Path| write(d, ".claude/team/confidence/session-abcdef12/n4.txt", "97\n")),
        // …and teammates of a team by another name mark it the lead's too.
        s("n5", &lead_tp, LEAD, 2)
            .setup(move |d: &Path| {
                fresh(d);
                in_process(
                    d,
                    "",
                    "aimpl-2-1111111111111111",
                    "impl-2",
                    "parser-team",
                    "implementer",
                    "Confidence: 99%",
                );
            })
            .err("No confidence found"),
        // A teammate in its own session: transcript_path is its own.
        s("n6", &format!("tr/{MATE}.jsonl"), MATE, 0).setup(transcript(
            "tr/fedcba98-1111-4000-8000-000000000000.jsonl",
            vec![assistant_text("Implemented.\nConfidence: 97%")],
        )),
        // The risk gate finds an in-process teammate's role the same way: a
        // read-only one is exempt…
        idle("reviewer-1", 0).setup(|d: &Path| {
            write(
                d,
                ".claude/team/plan.md",
                "- Risk: x — Owner: reviewer-1 — Mitigation: pending\n\
                 - Risk: y — Owner: impl-1 — Mitigation: pending\n",
            );
            in_process(
                d,
                "",
                "areviewer-1-2222222222222222",
                "reviewer-1",
                TEAM,
                "reviewer",
                "Done.",
            );
        }),
        // …a writer is held.
        idle("impl-1", 2).err("You still own risks"),
    ]
}

/// Without `team_name` the gates take the session's own team, so an in-process
/// teammate is still judged on its own transcript and role, and the lead's
/// words never pass its task.
#[test]
fn task_gate_without_a_team_name_reads_the_teammate_not_the_lead() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        parity(&format!("no team ({variant})"), &no_team_steps(), extra);
    }
}

// ------------------------------------------------------- the worker's words --

fn dirty_wt(dir: &Path) {
    let wt = dir.join("wt");
    if !wt.exists() {
        fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q"]);
        fs::write(wt.join("new.rs"), "// change\n").unwrap();
    }
}

fn stop_event(agent: &str, agent_type: &str, msg: &str) -> String {
    json!({
        "session_id": "s",
        "transcript_path": "{dir}/tr/lead.jsonl",
        "cwd": "{dir}/wt",
        "permission_mode": "default",
        "agent_id": agent,
        "agent_type": agent_type,
        "hook_event_name": "SubagentStop",
        "stop_hook_active": false,
        "agent_transcript_path": format!("{{dir}}/tr/lead/subagents/agent-{agent}.jsonl"),
        "last_assistant_message": msg,
        "background_tasks": [{
            "id": "b1", "type": "local_bash", "status": "running",
            "description": "nightly eval",
            "command": "python eval.py --min-confidence 0.8"
        }],
        "session_crons": [],
    })
    .to_string()
}

fn start_event(agent: &str, agent_type: &str) -> String {
    json!({
        "session_id": "s",
        "transcript_path": "{dir}/tr/lead.jsonl",
        "cwd": "{dir}/wt",
        "permission_mode": "default",
        "agent_type": agent_type,
        "hook_event_name": "SubagentStart",
        "agent_id": agent,
    })
    .to_string()
}

const SUB: &[(&str, &str)] = &[
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn worker_message_steps() -> Vec<Step> {
    let s = |agent: &str, msg: &str, expect| {
        step(
            "subagent-confidence-gate",
            SUB,
            stop_event(agent, "general-purpose", msg),
            expect,
        )
        .setup(dirty_wt)
    };
    vec![
        // A running background task's command is not the worker's claim.
        s("w1", "Done, all tests pass.\nConfidence: 98%", 0),
        // Prose after the score doesn't override it.
        s(
            "w2",
            "Added retries.\nConfidence: 97%\nCaveat: low confidence in 3 edge cases.",
            0,
        ),
        s("w3", "Confidence: 60% (target 96%)", 2).err("Confidence 60% is below the required 96%"),
        // Not a percentage, or not a possible one: not a stated confidence.
        s("w4", "Confidence: 9/10", 2).err("did not state your confidence"),
        s("w5", "Confidence: 150%", 2).err("did not state your confidence"),
        s("w6", "Shipped.\n\n**Confidence:** 97%", 0),
        s("w7", "done.\nConfidence — 97%", 0),
        // The last stated score wins.
        s("w8", "Confidence: 97%\nre-checked: Confidence: 80%", 0),
        s("w9", "Confidence: 99%\nConfidence: 80%", 2).err("Confidence 80% is below"),
        s("w10", "Done.\n_Confidence_: 97%", 0),
        s("w11", "Done.\nmin_confidence: 99%", 2).err("did not state your confidence"),
    ]
}

#[test]
fn worker_gate_scores_only_the_final_message_in_any_locale() {
    for (variant, extra) in [
        ("jq", JQ),
        ("no jq", NO_JQ),
        (
            "utf-8 locale",
            &[("LANG", "en_US.UTF-8"), ("LC_ALL", "en_US.UTF-8")][..],
        ),
        (
            "no locale",
            &[("LANG", UNSET), ("LC_ALL", UNSET), ("LC_CTYPE", UNSET)][..],
        ),
        (
            "no locale, no jq",
            &[
                ("LANG", UNSET),
                ("LC_ALL", UNSET),
                ("LC_CTYPE", UNSET),
                ("OCGEN_NO_JQ", "1"),
            ][..],
        ),
    ] {
        parity(
            &format!("worker message ({variant})"),
            &worker_message_steps(),
            extra,
        );
    }
}

// ------------------------------------------------------- the worker's tree --

/// A repository with one commit and the lead's uncommitted edit in it.
fn lead_editing(dir: &Path) {
    let wt = dir.join("wt");
    if !wt.exists() {
        fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q"]);
        fs::write(wt.join("lib.rs"), "// v1\n").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-qm", "init"]);
        fs::write(wt.join("lib.rs"), "// the lead's half-done edit\n").unwrap();
    }
}

const WORKER: &[(&str, &str)] = &[
    ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
    ("SUBAGENT_READONLY_ROLES", "explorer reviewer verifier"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn worker_tree_steps() -> Vec<Step> {
    let start = |agent: &str, ty: &str| {
        step(
            "subagent-confidence-gate",
            WORKER,
            start_event(agent, ty),
            0,
        )
    };
    let stop = |agent: &str, ty: &str, msg: &str, expect| {
        step(
            "subagent-confidence-gate",
            WORKER,
            stop_event(agent, ty, msg),
            expect,
        )
    };
    vec![
        // A read-only role is never gated, even in a tree the lead dirtied.
        start("e1", "explorer").setup(lead_editing),
        stop("e1", "Explorer", "Finding: auth is in src/auth.rs:12", 0),
        // A writer that changed nothing since it started passes…
        start("g1", "general-purpose"),
        stop("g1", "general-purpose", "Nothing to change.", 0),
        // …one that changed the tree is gated, and stays gated until it passes.
        start("g2", "general-purpose"),
        stop("g2", "general-purpose", "done", 2)
            .setup(|d: &Path| fs::write(d.join("wt/worker.rs"), "// new\n").unwrap())
            .err("did not state your confidence"),
        stop("g2", "general-purpose", "done", 2).err("did not state your confidence"),
        stop("g2", "general-purpose", "Confidence: 97%", 0),
        // A writer that committed leaves a clean tree, and is gated all the same.
        start("g3", "general-purpose"),
        stop("g3", "general-purpose", "Committed.", 2)
            .setup(|d: &Path| {
                let wt = d.join("wt");
                fs::write(wt.join("more.rs"), "// more\n").unwrap();
                git(&wt, &["add", "-A"]);
                git(&wt, &["commit", "-qm", "work"]);
            })
            .err("did not state your confidence"),
        stop("g3", "general-purpose", "Committed.\nConfidence: 99%", 0),
        // Without a recorded start, a writer in a dirty tree is gated as before.
        stop("g4", "general-purpose", "done", 2)
            .setup(|d: &Path| fs::write(d.join("wt/x.rs"), "// x\n").unwrap()),
        stop("g4", "general-purpose", "Confidence: 97%", 0),
        // An in-process teammate stops as a subagent whose agent_type is its
        // name; its role is in its transcript's meta. A read-only one is never
        // gated, though the shared tree changed under it…
        start("areviewer-1-0123456789abcdef", "reviewer-1"),
        stop(
            "areviewer-1-0123456789abcdef",
            "reviewer-1",
            "Looks right.",
            0,
        )
        .setup(|d: &Path| {
            teammate_role(d, "areviewer-1-0123456789abcdef", "reviewer");
            fs::write(d.join("wt/peer.rs"), "// a peer's edit\n").unwrap();
        }),
        // …a writer is.
        start("aimpl-3-0123456789abcdef", "impl-3"),
        stop("aimpl-3-0123456789abcdef", "impl-3", "done", 2)
            .setup(|d: &Path| {
                teammate_role(d, "aimpl-3-0123456789abcdef", "implementer");
                fs::write(d.join("wt/impl.rs"), "// impl-3's edit\n").unwrap();
            })
            .err("did not state your confidence"),
        stop("aimpl-3-0123456789abcdef", "impl-3", "Confidence: 98%", 0),
    ]
}

/// The `.meta.json` beside a teammate's transcript (`agent_transcript_path`).
fn teammate_role(d: &Path, agent: &str, role: &str) {
    let meta = json!({
        "agentType": agent, "name": agent, "taskKind": "in_process_teammate",
        "teamName": TEAM, "customAgentType": role,
    });
    write(
        d,
        &format!("tr/lead/subagents/agent-{agent}.meta.json"),
        &meta.to_string(),
    );
}

#[test]
fn worker_gate_skips_read_only_roles_and_judges_each_writer_by_its_own_changes() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, b) = parity(
            &format!("worker tree ({variant})"),
            &worker_tree_steps(),
            extra,
        );
        for d in [a.path(), b.path()] {
            let left: Vec<String> = fs::read_dir(d.join(".claude/worker-baseline"))
                .map(|r| {
                    r.filter_map(Result::ok)
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| n != ".gitignore")
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                left.is_empty(),
                "baselines cleaned up ({variant}): {left:?}"
            );
        }
    }
}

/// With the team task gate on, a teammate is judged per task by TaskCompleted
/// (its own words, the team check under the lock) — not again at every turn end
/// by the worker gate, which would ask for a confidence mid-task and run the
/// check in the shared tree outside the lock. Other subagents still are.
#[test]
fn worker_gate_leaves_teammates_to_the_task_gate() {
    let env: Vec<(&str, &str)> = WORKER
        .iter()
        .copied()
        .chain([("TEAM_TASK_GATE", "1")])
        .collect();
    let ev = |agent: &str, ty: &str, msg: Option<&str>, expect| {
        let payload = match msg {
            None => start_event(agent, ty),
            Some(m) => stop_event(agent, ty, m),
        };
        step("subagent-confidence-gate", &env, payload, expect)
    };
    let steps = vec![
        ev("aimpl-3-0123456789abcdef", "impl-3", None, 0).setup(lead_editing),
        ev("aimpl-3-0123456789abcdef", "impl-3", Some("done"), 0).setup(|d: &Path| {
            teammate_role(d, "aimpl-3-0123456789abcdef", "implementer");
            fs::write(d.join("wt/impl.rs"), "// impl-3's edit\n").unwrap();
        }),
        ev("g5", "general-purpose", None, 0),
        ev("g5", "general-purpose", Some("done"), 2)
            .setup(|d: &Path| fs::write(d.join("wt/worker.rs"), "// new\n").unwrap())
            .err("did not state your confidence"),
        ev("g5", "general-purpose", Some("Confidence: 97%"), 0),
    ];
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, b) = parity(&format!("teammates ({variant})"), &steps, extra);
        for d in [a.path(), b.path()] {
            let left = fs::read_dir(d.join(".claude/worker-baseline"))
                .map(|r| {
                    r.filter_map(Result::ok)
                        .filter(|e| e.file_name() != ".gitignore")
                        .count()
                })
                .unwrap_or(0);
            assert_eq!(left, 0, "baselines cleaned up ({variant})");
        }
    }
}

/// A repository with no commits yet, so everything in it is untracked.
fn fresh_repo(dir: &Path) {
    let wt = dir.join("wt");
    if !wt.exists() {
        fs::create_dir_all(wt.join("src")).unwrap();
        git(&wt, &["init", "-q"]);
        fs::write(wt.join("src/main.rs"), "fn main() {}\n").unwrap();
    }
}

fn untracked_steps() -> Vec<Step> {
    let start = |agent: &str| {
        step(
            "subagent-confidence-gate",
            WORKER,
            start_event(agent, "general-purpose"),
            0,
        )
    };
    let stop = |agent: &str, msg: &str, expect| {
        step(
            "subagent-confidence-gate",
            WORKER,
            stop_event(agent, "general-purpose", msg),
            expect,
        )
    };
    let edit = |rel: &'static str, body: &'static str| {
        move |d: &Path| {
            let p = d.join("wt").join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
    };
    vec![
        // No commits yet: a worker that rewrites a file git doesn't track…
        start("u1").setup(fresh_repo),
        stop("u1", "Rewrote main.rs.", 2)
            .setup(edit("src/main.rs", "fn main() { run() }\n"))
            .err("did not state your confidence"),
        stop("u1", "Rewrote main.rs.\nConfidence: 97%", 0),
        // …changed the tree; one that only read it did not.
        start("u2"),
        stop("u2", "Read main.rs.", 0),
        // A file staged (and changed again) before the first commit.
        start("u3").setup(|d: &Path| {
            git(&d.join("wt"), &["add", "src/main.rs"]);
            fs::write(d.join("wt/src/main.rs"), "fn main() { run(); }\n").unwrap();
        }),
        stop("u3", "done", 2)
            .setup(edit("src/main.rs", "fn main() { run(); stop(); }\n"))
            .err("did not state your confidence"),
        stop("u3", "Confidence: 98%", 0),
        // With commits: a file inside a directory git doesn't track yet, with a
        // nested repository and a folder of ignored output beside it…
        start("u4").setup(|d: &Path| {
            let wt = d.join("wt");
            git(&wt, &["add", "-A"]);
            git(&wt, &["commit", "-qm", "init"]);
            fs::write(wt.join(".gitignore"), "target/\n").unwrap();
            fs::create_dir_all(wt.join("a-dep")).unwrap();
            git(&wt.join("a-dep"), &["init", "-q"]);
            fs::create_dir_all(wt.join("src/newmod")).unwrap();
            fs::write(wt.join("src/newmod/lib.rs"), "// the lead's new module\n").unwrap();
        }),
        // …is changed by a worker that edits it…
        stop("u4", "done", 2)
            .setup(edit("src/newmod/lib.rs", "// rewritten\n"))
            .err("did not state your confidence"),
        stop("u4", "Confidence: 97%", 0),
        // …or adds one beside it…
        start("u5"),
        stop("u5", "done", 2)
            .setup(edit("src/newmod/more.rs", "// more\n"))
            .err("did not state your confidence"),
        stop("u5", "Confidence: 97%", 0),
        // …but not by ignored build output.
        start("u6"),
        stop("u6", "Built it.", 0).setup(edit("target/out.bin", "bin")),
    ]
}

/// A worker that changes only files git doesn't track yet — new files, a new
/// directory, any file before the first commit — changed its tree: the record
/// covers their content, not just their names.
#[test]
fn worker_gate_sees_changes_to_untracked_files() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        parity(&format!("untracked ({variant})"), &untracked_steps(), extra);
    }
}

/// `hash-object` stops at the first file it can't open, so an untracked file
/// nobody may read (another user's key, a container's output) must not hide
/// the content of the files listed after it.
#[cfg(unix)]
#[test]
fn an_unreadable_untracked_file_hides_no_other_change() {
    use std::os::unix::fs::PermissionsExt as _;
    let steps = || {
        vec![
            step(
                "subagent-confidence-gate",
                WORKER,
                start_event("u9", "general-purpose"),
                0,
            )
            .setup(|d: &Path| {
                fresh_repo(d);
                let new = d.join("wt/new");
                fs::create_dir_all(&new).unwrap();
                let key = new.join("a-key.pem");
                fs::write(&key, "secret\n").unwrap();
                fs::set_permissions(&key, fs::Permissions::from_mode(0o000)).unwrap();
                fs::write(new.join("b.rs"), "// the lead's\n").unwrap();
            }),
            step(
                "subagent-confidence-gate",
                WORKER,
                stop_event("u9", "general-purpose", "Rewrote b.rs."),
                2,
            )
            .setup(|d: &Path| fs::write(d.join("wt/new/b.rs"), "// the worker's\n").unwrap())
            .err("did not state your confidence"),
        ]
    };
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let _ = parity(&format!("unreadable ({variant})"), &steps(), extra);
    }
}

/// Both implementations record the same tree byte for byte, so a worker started
/// under one and stopped under the other is judged the same — before the first
/// commit and after, with names git quotes and paths `hash-object` can't read.
#[test]
fn both_implementations_record_the_same_tree() {
    let dir = project();
    let wt = dir.path().join("wt");
    fs::create_dir_all(wt.join("src/new mod")).unwrap();
    git(&wt, &["init", "-q"]);
    fs::write(wt.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(wt.join("src/new mod/lib.rs"), "// lib\n").unwrap();
    fs::write(wt.join(".gitignore"), "target/\n").unwrap();
    fs::create_dir_all(wt.join("target")).unwrap();
    fs::write(wt.join("target/out.bin"), "bin").unwrap();
    fs::create_dir_all(wt.join("a-dep")).unwrap();
    git(&wt.join("a-dep"), &["init", "-q"]);
    // Names Windows can't hold, or that Git Bash reads in its own charset.
    #[cfg(unix)]
    {
        fs::write(wt.join("src/naïve.rs"), "// a name git quotes\n").unwrap();
        fs::write(wt.join("src/say \"q\".rs"), "// quoted name\n").unwrap();
        std::os::unix::fs::symlink("missing", wt.join("a-dangling")).unwrap();
        std::os::unix::fs::symlink("src", wt.join("a-dirlink")).unwrap();
    }
    let d = ocgen::paths::for_shell(dir.path());
    let record = |agent: &str, rust: bool, no_jq: bool| {
        let mut env: HashMap<String, String> = [
            ("SUBAGENT_CONFIDENCE_THRESHOLD", "96"),
            ("CLAUDE_PROJECT_DIR", d.as_str()),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        if no_jq {
            env.insert("OCGEN_NO_JQ".into(), "1".into());
        }
        let payload = start_event(agent, "general-purpose").replace("{dir}", &d);
        let o = if rust {
            run_rs("subagent-confidence-gate", &env, &payload)
        } else {
            run_sh(dir.path(), "subagent-confidence-gate", &env, &payload)
        };
        assert_eq!(o.0, 0, "{o:?}");
        fs::read_to_string(dir.path().join(".claude/worker-baseline").join(agent)).unwrap()
    };
    let all = || {
        [
            record("r", true, false),
            record("s", false, false),
            record("n", false, true),
        ]
    };
    for stage in ["no commits", "staged", "committed, untracked dir"] {
        match stage {
            "staged" => git(&wt, &["add", "src/main.rs"]),
            "committed, untracked dir" => {
                git(&wt, &["commit", "-qm", "init"]);
                fs::write(wt.join("src/new mod/more.rs"), "// more\n").unwrap();
            }
            _ => {}
        }
        let [r, s, n] = all();
        assert_eq!(r, s, "{stage}: rust vs sh");
        assert_eq!(r, n, "{stage}: rust vs sh without jq");
        // The untracked content is in the sum: editing it moves the record.
        fs::write(wt.join("src/new mod/lib.rs"), format!("// {stage}\n")).unwrap();
        assert_ne!(all()[0], r, "{stage}: an untracked edit moves the sum");
    }
}

/// Both twins read a number setting the way sh's `case *[!0-9]*` does: plain
/// digits only. Anything else (`+99`, ` 99`, `99%`) is unset — the same verdict
/// from the script and the binary.
#[test]
fn number_settings_read_alike_in_both_twins() {
    for bad in ["+99", " 99", "99%"] {
        let env = [("SUBAGENT_CONFIDENCE_THRESHOLD", bad)];
        let steps = vec![step(
            "subagent-confidence-gate",
            &env,
            stop_event("n1", "general-purpose", "done"),
            0,
        )
        .setup(|d: &Path| {
            lead_editing(d);
            fs::write(d.join("wt/x.rs"), "// x\n").unwrap();
        })];
        for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
            parity(&format!("threshold {bad:?} ({variant})"), &steps, extra);
        }
    }
}

// ----------------------------------------------------------- the team check --

fn checked_task(tid: &str, desc: &str) -> String {
    json!({
        "session_id": MATE, "transcript_path": "{dir}/tr/none.jsonl", "cwd": "{dir}",
        "hook_event_name": "TaskCompleted", "task_id": tid, "task_subject": "x",
        "task_description": desc, "teammate_name": "impl-1", "team_name": TEAM,
    })
    .to_string()
}

const CHECK: &[(&str, &str)] = &[
    ("OCGEN_CHECK_CMD", "test -f ok"),
    ("OCGEN_CHECK_TIMEOUT", "2"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn team_check_runs_the_tasks_own_check_one_at_a_time() {
    let s = |tid: &str, desc: &str, expect| {
        step(
            "team-task-completed",
            CHECK,
            checked_task(tid, desc),
            expect,
        )
    };
    let steps = vec![
        // A task narrows the project check with its own `Check:` line.
        s("t1", "Fix the parser.\nCheck: test -f ok -a -f extra\n", 2)
            .setup(|d: &Path| fs::write(d.join("ok"), "").unwrap())
            .err("Check failed: `test -f ok -a -f extra` (exit 1)"),
        s("t1", "Fix the parser.\nCheck: test -f ok -a -f extra\n", 2)
            .err("another teammate's unfinished edits"),
        s("t2", "No check line.", 0),
        // Only the project check plus plain arguments: anything else is ignored.
        s("t3", "Check: test -f ok; touch pwned", 0),
        s("t4", "Check: touch pwned", 0),
        // A stale lock (its holder died) is taken over.
        s("t5", "x", 0).setup(|d: &Path| write(d, ".claude/team/check.lock/at", "1\n")),
        // A live one is waited for — within the same time budget as the check.
        s("t6", "x", 2)
            .setup(|d: &Path| write(d, ".claude/team/check.lock/at", &format!("{}\n", now())))
            .err("Check failed: `test -f ok` (exit timeout after 2s)"),
    ];
    let (a, b) = parity("team check", &steps, JQ);
    for d in [a.path(), b.path()] {
        assert!(!d.join("pwned").exists(), "a task line can't run commands");
    }
}

/// The lock is runtime state: git never sees it (so it doesn't count as a
/// change to a worker's tree), and a lock whose holder died before it could
/// stamp it is taken over rather than waited on forever.
#[test]
fn the_team_check_lock_stays_out_of_git_and_never_wedges() {
    let env = [
        (
            "OCGEN_CHECK_CMD",
            "! git status --porcelain --untracked-files=all | grep check.lock",
        ),
        ("OCGEN_CHECK_TIMEOUT", "20"),
    ];
    let steps = vec![
        step("team-task-completed", &env, checked_task("t1", "x"), 0),
        step("team-task-completed", &env, checked_task("t2", "x"), 0)
            .setup(|d: &Path| fs::create_dir_all(d.join(".claude/team/check.lock")).unwrap()),
    ];
    let started = std::time::Instant::now();
    parity("team check lock", &steps, JQ);
    assert!(
        started.elapsed().as_secs() < 20,
        "an unstamped lock is taken over well before the timeout"
    );
}

/// The scripts read text in the C locale (as the Rust hook does), but the check
/// itself runs in the caller's locale — as the Rust hook runs it.
#[test]
fn the_check_runs_in_the_callers_locale() {
    let dir = project();
    let d = ocgen::paths::for_shell(dir.path());
    dirty_wt(dir.path());
    let payload = stop_event("w1", "general-purpose", "Confidence: 99%").replace("{dir}", &d);
    let task = checked_task("t1", "x").replace("{dir}", &d);
    for lc in [None, Some("en_US.UTF-8")] {
        let check = match lc {
            None => "test \"${LC_ALL-unset}\" = unset".to_string(),
            Some(v) => format!("test \"$LC_ALL\" = {v}"),
        };
        let env: HashMap<String, String> = [
            ("CLAUDE_PROJECT_DIR", d.as_str()),
            ("OCGEN_CHECK_CMD", check.as_str()),
            ("LC_ALL", lc.unwrap_or(UNSET)),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (hook, payload) in [
            ("subagent-confidence-gate", &payload),
            ("team-task-completed", &task),
        ] {
            let o = run_sh(dir.path(), hook, &env, payload);
            assert_eq!(o.0, 0, "{hook} with LC_ALL={lc:?}: {o:?}");
        }
    }
}

#[test]
fn a_hung_check_times_out_the_same_way_and_takes_its_children_with_it() {
    let check = "sleep 30 & echo $! > gc.pid; sleep 30";
    let env = [
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "0"),
        ("OCGEN_CHECK_CMD", check),
        ("OCGEN_CHECK_TIMEOUT", "1"),
    ];
    let started = std::time::Instant::now();
    let steps = vec![step(
        "subagent-confidence-gate",
        &env,
        stop_event("w1", "general-purpose", "Confidence: 99%"),
        2,
    )
    .setup(dirty_wt)
    .err("(exit timeout after 1s)")];
    let (a, b) = parity("hung check", &steps, JQ);
    assert!(
        started.elapsed().as_secs() < 20,
        "both returned near the timeout, not when the check would have"
    );
    if cfg!(unix) {
        for d in [a.path(), b.path()] {
            let pid = fs::read_to_string(d.join("wt/gc.pid")).unwrap();
            let alive = Command::new("kill")
                .args(["-0", pid.trim()])
                .output()
                .unwrap()
                .status
                .success();
            if alive {
                let _ = Command::new("kill").args(["-9", pid.trim()]).output();
            }
            assert!(!alive, "the check's child was killed too ({})", d.display());
        }
    }
}

// ------------------------------------------------------------- risk owners --

const IDLE: &[(&str, &str)] = &[
    ("TEAM_RISK_ROUNDS", "1"),
    ("TEAM_READONLY_ROLES", "explorer reviewer"),
    ("LOOP_GUARD_MAX_BLOCKS", "3"),
];

fn idle_event(name: Option<&str>, role: Option<&str>) -> String {
    let mut v = json!({
        "session_id": "s", "transcript_path": "{dir}/tr/lead.jsonl", "cwd": "{dir}",
        "permission_mode": "default", "hook_event_name": "TeammateIdle", "team_name": TEAM,
    });
    if let Some(n) = name {
        v["teammate_name"] = json!(n);
    }
    if let Some(r) = role {
        v["agent_type"] = json!(r);
    }
    v.to_string()
}

fn register(d: &Path) {
    write(
        d,
        ".claude/team/plan.md",
        "## Risks\n\
         - Risk: data loss — Owner: impl-1 — Mitigation: pending\n\
         - Risk: drift — Owner: impl-2 — Mitigation: done\n\
         - Risk: slow build — Owner: implementer — Mitigation: pending\n",
    );
}

fn risk_steps() -> Vec<Step> {
    let s = |name, role, expect| step("team-teammate-idle", IDLE, idle_event(name, role), expect);
    vec![
        // Owners are teammates: impl-1 is held, its same-role peer is not…
        s(Some("impl-1"), Some("implementer"), 2)
            .setup(register)
            .err("You still own risks"),
        // …and a role-named owner holds no named teammate, but says so.
        s(Some("impl-2"), Some("implementer"), 0).out(&["systemMessage"]),
        // An event with only a role: the role is the identity (older plans).
        s(None, Some("implementer"), 2),
        // No identity at all: the gate says it didn't check, rather than nothing.
        s(None, None, 0).out(&["systemMessage"]),
        s(Some("explorer-1"), Some("explorer"), 0),
        // The loop guard counts per teammate.
        s(Some("impl-1"), Some("implementer"), 2),
        s(Some("impl-1"), Some("implementer"), 0).out(&["systemMessage"]),
        // In-process teammates as Claude Code 2.1.277 reports them: a name and
        // no role (agent_type is the lead's), the role in the meta beside the
        // teammate's transcript. A role-named owner is reported for its role…
        in_process_idle("impl-2", 0)
            .setup(|d: &Path| {
                in_process(
                    d,
                    "",
                    "aimpl-2-1111111111111111",
                    "impl-2",
                    TEAM,
                    "implementer",
                    "Done.",
                )
            })
            .out(&["systemMessage"]),
        // …and a read-only role is exempt even from risks it owns.
        in_process_idle("reviewer-1", 0).setup(|d: &Path| {
            let p = d.join(".claude/team/plan.md");
            let body = fs::read_to_string(&p).unwrap();
            fs::write(
                p,
                format!("{body}- Risk: x — Owner: reviewer-1 — Mitigation: pending\n"),
            )
            .unwrap();
            in_process(
                d,
                "",
                "areviewer-1-2222222222222222",
                "reviewer-1",
                TEAM,
                "reviewer",
                "Done.",
            );
        }),
        // Without its meta, it is just a name.
        in_process_idle("reviewer-1", 2).setup(|d: &Path| {
            let _ = fs::remove_dir_all(d.join(format!("tr/{LEAD}/subagents")));
        }),
    ]
}

fn in_process_idle(name: &str, expect: i32) -> Step {
    let v = json!({
        "session_id": LEAD, "transcript_path": format!("{{dir}}/tr/{LEAD}.jsonl"), "cwd": "{dir}",
        "permission_mode": "default", "hook_event_name": "TeammateIdle",
        "teammate_name": name, "team_name": TEAM,
    });
    step("team-teammate-idle", IDLE, v.to_string(), expect)
}

#[test]
fn risk_gate_holds_the_teammate_that_owns_the_risk() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, _) = parity(&format!("risk owners ({variant})"), &risk_steps(), extra);
        let log = fs::read_to_string(a.path().join(".claude/loop-guard/escalations.md")).unwrap();
        assert!(log.contains("impl-1") && log.contains("data loss"), "{log}");
    }
}

/// The loop guard's subject (its state file and escalation line) is the same
/// bytes from the Rust hook and from `loop-guard.sh`, whatever the locale.
#[test]
fn loop_guard_subjects_match_in_any_locale() {
    let dir = project();
    write(
        dir.path(),
        ".claude/team/plan.md",
        "- Risk: drift — Owner: імпл-1 — Mitigation: pending\n",
    );
    let d = ocgen::paths::for_shell(dir.path());
    let env: HashMap<String, String> = [
        ("TEAM_RISK_ROUNDS", "1"),
        ("LOOP_GUARD_MAX_BLOCKS", "1"),
        ("CLAUDE_PROJECT_DIR", d.as_str()),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let o = run_rs(
        "team-teammate-idle",
        &env,
        r#"{"session_id":"s","teammate_name":"імпл-1"}"#,
    );
    assert_eq!(o.0, 0, "released at once: {o:?}");
    let log = fs::read_to_string(dir.path().join(".claude/loop-guard/escalations.md")).unwrap();
    // The name goes through a file: Windows would re-encode it as an argument.
    write(dir.path(), "name.txt", "імпл-1");
    let sh = Command::new("sh")
        .args(["-c", r#". "$1"; lg_safe "$(cat "$2")""#, "sh"])
        .arg(format!("{d}/.claude/hooks/loop-guard.sh"))
        .arg(format!("{d}/name.txt"))
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .unwrap();
    // As the hooks read it, through `$(…)`: GNU cut ends its output with a
    // newline, BSD cut doesn't.
    let subject = String::from_utf8_lossy(&sh.stdout)
        .trim_end_matches('\n')
        .to_string();
    assert!(
        log.contains(&format!("[risk-idle] {subject}:")),
        "sh subject {subject:?} vs {log}"
    );
}

// ----------------------------------------------------------- plan approval --

const PLAN_GATE: &[(&str, &str)] = &[("TEAM_PLAN_GATE", "1")];

/// A TaskCreated event. The lead's own carries no team_name (Claude Code 2.1.277
/// names a team only from inside a teammate); a teammate's carries its team.
fn created(sid: &str, team: Option<&str>) -> String {
    let mut v = json!({
        "session_id": sid, "transcript_path": "{dir}/tr/x.jsonl", "cwd": "{dir}",
        "permission_mode": "default", "hook_event_name": "TaskCreated",
        "task_id": "1", "task_subject": "Parser", "task_description": "Fix it",
    });
    if let Some(t) = team {
        v["teammate_name"] = json!("impl-1");
        v["team_name"] = json!(t);
    }
    v.to_string()
}

fn plan(status: &'static str) -> impl Fn(&Path) + 'static {
    move |d: &Path| {
        write(
            d,
            ".claude/team/plan.md",
            &format!(
                "# Intent\nShip the parser.\n\n## Tasks\n1. Parser — Owner: impl-1\n{status}\n"
            ),
        )
    }
}

const FRIDAY: &str = "cccccccc-3333-4000-8000-000000000000";

fn plan_steps() -> Vec<Step> {
    let s = |sid: &str, team: Option<&str>, expect| {
        step("team-task-created", PLAN_GATE, created(sid, team), expect)
    };
    vec![
        // The lead creates the first task: it stamps the approval with its run.
        s(LEAD, None, 0).setup(plan(
            "Status: APPROVED session=abcdef12-0000-4000-8000-000000000000",
        )),
        s(LEAD, None, 0),
        // An in-process teammate (the lead's session, its implicit team)…
        s(LEAD, Some(TEAM), 0),
        // …and a teammate in its own session, in the same team, share the run.
        s(MATE, Some(TEAM), 0),
        // A later session's lead, or its team, can't reuse it.
        s(FRIDAY, None, 2).err("Plan approval is stale"),
        s(FRIDAY, Some("session-cccccccc"), 2).err("Plan approval is stale"),
        // Editing the plan voids the approval…
        s(LEAD, None, 2)
            .setup(|d: &Path| {
                let p = d.join(".claude/team/plan.md");
                let body = fs::read_to_string(&p).unwrap();
                fs::write(p, body.replace("1. Parser", "1. Parser and lexer")).unwrap();
            })
            .err("changed after it was approved"),
        // …until it is approved again; whoever creates a task first stamps it.
        s(LEAD, Some(TEAM), 0).setup(|d: &Path| {
            let p = d.join(".claude/team/plan.md");
            let body: String = fs::read_to_string(&p)
                .unwrap()
                .lines()
                .filter(|l| !l.starts_with("Status:"))
                .map(|l| format!("{l}\n"))
                .collect();
            fs::write(p, format!("{body}Status: APPROVED session={LEAD}\n")).unwrap();
        }),
        s(LEAD, None, 0),
        // Approved in another session, never seen by this gate.
        s(LEAD, None, 2)
            .setup(plan(
                "Status: APPROVED session=dddddddd-4444-4000-8000-000000000000",
            ))
            .err("approved in another session"),
        // A plain line (an older /team-plan) is taken by the first run that sees it.
        s(LEAD, None, 0).setup(plan("Status: APPROVED")),
        s(MATE, Some(TEAM), 0),
        s(FRIDAY, None, 2),
        // An unsubstituted placeholder counts as no session.
        s(LEAD, None, 0).setup(plan("Status: APPROVED session=${CLAUDE_SESSION_ID}")),
    ]
}

#[test]
fn plan_approval_is_tied_to_the_plan_and_the_team_run() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, b) = parity(&format!("plan ({variant})"), &plan_steps(), extra);
        let read = |d: &Path| fs::read_to_string(d.join(".claude/team/plan.md")).unwrap();
        assert_eq!(read(a.path()), read(b.path()), "both stamp the plan alike");
        assert!(read(a.path()).contains("[ocgen: run=session-abcdef12 plan="));
    }
}

/// Replace `from` with `to` in the plan.
fn plan_edit(from: &'static str, to: &'static str) -> impl Fn(&Path) + 'static {
    move |d: &Path| {
        let p = d.join(".claude/team/plan.md");
        let body = fs::read_to_string(&p).unwrap();
        assert!(body.contains(from), "{from:?} in {body}");
        fs::write(p, body.replacen(from, to, 1)).unwrap();
    }
}

/// The plan's runtime state is not the plan: a risk's mitigation status (which
/// the risk gate makes its owner close) and a task's checkbox change as the team
/// works, and keep the approval. The text of a task or a risk is the plan.
fn plan_state_steps() -> Vec<Step> {
    let s = |sid: &str, team: Option<&str>, expect| {
        step("team-task-created", PLAN_GATE, created(sid, team), expect)
    };
    let approve = |d: &Path| {
        let p = d.join(".claude/team/plan.md");
        let body: String = fs::read_to_string(&p)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("Status:"))
            .map(|l| format!("{l}\n"))
            .collect();
        fs::write(p, format!("{body}Status: APPROVED session={LEAD}\n")).unwrap();
    };
    vec![
        s(LEAD, None, 0).setup(|d: &Path| {
            write(
                d,
                ".claude/team/plan.md",
                &format!(
                    "# Intent\nShip the parser.\n\n## Tasks\n\
                     - [ ] 1. Parser — Owner: impl-1\n\
                     - [ ] 2. Lexer — Owner: impl-2\n\n## Risks\n\
                     - Risk: data loss — Owner: impl-1 — Mitigation: pending\n\
                     - Risk: drift — Owner: impl-2 — mitigation: pending\n\
                     Status: APPROVED session={LEAD}\n"
                ),
            )
        }),
        // The risk gate holds impl-1 until it closes its risk…
        in_process_idle("impl-1", 2).err("You still own risks"),
        in_process_idle("impl-1", 0).setup(plan_edit(
            "Mitigation: pending",
            "Mitigation: done — retries added in src/io.rs",
        )),
        // …which leaves the approval in force, for the lead and for teammates.
        s(LEAD, None, 0),
        s(MATE, Some(TEAM), 0).setup(plan_edit("mitigation: pending", "MITIGATION: accepted")),
        // Ticking (or unticking) a task is progress, not a new plan.
        s(LEAD, Some(TEAM), 0).setup(plan_edit("- [ ] 1. Parser", "- [x] 1. Parser")),
        s(LEAD, None, 0).setup(plan_edit("- [ ] 2. Lexer", "- [X] 2. Lexer")),
        s(LEAD, None, 0).setup(plan_edit("- [x] 1. Parser", "- [ ] 1. Parser")),
        // Editing a task's text voids the approval…
        s(LEAD, None, 2)
            .setup(plan_edit("1. Parser —", "1. Parser and lexer —"))
            .err("changed after it was approved"),
        s(LEAD, None, 0).setup(approve),
        // …and so does editing a risk's.
        s(LEAD, None, 2)
            .setup(plan_edit("Risk: data loss", "Risk: data corruption"))
            .err("changed after it was approved"),
        s(LEAD, None, 0).setup(approve),
        // So does reassigning a task.
        s(LEAD, None, 2)
            .setup(plan_edit("Lexer — Owner: impl-2", "Lexer — Owner: impl-1"))
            .err("changed after it was approved"),
    ]
}

#[test]
fn closing_a_risk_or_ticking_a_task_keeps_the_plan_approved() {
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let (a, b) = parity(
            &format!("plan state ({variant})"),
            &plan_state_steps(),
            extra,
        );
        let read = |d: &Path| fs::read_to_string(d.join(".claude/team/plan.md")).unwrap();
        assert_eq!(read(a.path()), read(b.path()), "both stamp the plan alike");
    }
}

#[test]
fn a_verify_probe_never_stamps_the_plan() {
    let dir = project();
    plan("Status: APPROVED")(dir.path());
    let d = ocgen::paths::for_shell(dir.path());
    let env: HashMap<String, String> = [
        ("TEAM_PLAN_GATE", "1"),
        ("OCGEN_HOOK_PROBE", "1"),
        ("CLAUDE_PROJECT_DIR", d.as_str()),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let probe = r#"{"session_id":"ocgen-verify","agent_type":"ocgen-verify-probe"}"#;
    let before = fs::read_to_string(dir.path().join(".claude/team/plan.md")).unwrap();
    assert_eq!(run_rs("team-task-created", &env, probe).0, 0);
    assert_eq!(run_sh(dir.path(), "team-task-created", &env, probe).0, 0);
    let after = fs::read_to_string(dir.path().join(".claude/team/plan.md")).unwrap();
    assert_eq!(before, after, "a probe decides but changes nothing");
}

#[test]
fn verify_probes_the_gates_without_touching_the_plan() {
    let dir = project();
    let bin = assert_cmd::cargo::cargo_bin("ocgen");
    let path = std::env::join_paths(std::iter::once(bin.parent().unwrap().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    for status in [
        "Status: APPROVED",
        "Status: APPROVED session=abcdef12 [ocgen: run=session-abcdef12 plan=1-1]",
    ] {
        plan(status)(dir.path());
        let before = fs::read_to_string(dir.path().join(".claude/team/plan.md")).unwrap();
        // Through the binary (this build is first on PATH) and through the scripts.
        for path in [Some(&path), None] {
            let mut cmd = Command::new(&bin);
            cmd.args(["verify", "--no-claude"]).arg(dir.path());
            if let Some(p) = path {
                cmd.env("PATH", p);
            }
            let out = cmd.output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                !stdout.contains("TaskCreated: exit") && !stdout.contains("TaskCompleted: exit"),
                "{status} ({}): {stdout}",
                if path.is_some() { "binary" } else { "scripts" }
            );
        }
        let after = fs::read_to_string(dir.path().join(".claude/team/plan.md")).unwrap();
        assert_eq!(before, after, "verify changed the plan");
    }
}

/// verify accepts the plan gate's refusal of its own probe only when it is that
/// refusal: a script that can't even run still fails the check.
#[test]
fn verify_still_catches_a_broken_plan_gate_under_an_approved_plan() {
    let dir = project();
    plan("Status: APPROVED session=abcdef12 [ocgen: run=session-abcdef12 plan=1-1]")(dir.path());
    let script = dir.path().join(".claude/hooks/team-task-created.sh");
    fs::write(&script, "#!/bin/sh\nif then\n").unwrap();
    let out = Command::new(assert_cmd::cargo::cargo_bin("ocgen"))
        .args(["verify", "--no-claude"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // An approved plan can't make a broken gate look healthy: the script check
    // fails it, and the probe won't run a script ocgen didn't generate.
    assert!(!out.status.success(), "{stdout}");
    assert!(
        stdout.contains("team-task-created.sh has a syntax error"),
        "{stdout}"
    );
    assert!(
        stdout.contains("not running a hand-edited command"),
        "{stdout}"
    );
}

// ------------------------------------------------------------------ format --

#[test]
fn format_runs_in_the_edited_files_repository() {
    let env = [("OCGEN_FORMAT_CMD", "echo x > formatted-here")];
    let edit = |file: &str| {
        json!({
            "session_id": "s", "cwd": "{dir}", "hook_event_name": "PostToolUse",
            "tool_name": "Edit", "tool_input": { "file_path": file },
            "tool_response": { "filePath": file },
        })
        .to_string()
    };
    // A /fanout worker's checkout: a worktree of the project's repository.
    let worktree = |d: &Path| {
        let wt = d.join(".claude/worktrees/w1");
        if !wt.exists() {
            git(d, &["add", "-A"]);
            git(d, &["commit", "-qm", "init"]);
            git(d, &["worktree", "add", "-q", ".claude/worktrees/w1"]);
            fs::create_dir_all(wt.join("src")).unwrap();
        }
    };
    let steps = vec![
        step(
            "format",
            &env,
            edit("{dir}/.claude/worktrees/w1/src/lib.rs"),
            0,
        )
        .setup(worktree),
        // An unrelated repository is not the project's to format: the project.
        step("format", &env, edit("{dir}/vendor/other/lib.rs"), 0).setup(|d: &Path| {
            let other = d.join("vendor/other");
            fs::create_dir_all(&other).unwrap();
            git(&other, &["init", "-q"]);
        }),
        // Outside any repository (or with no path): the project directory.
        step("format", &env, edit("{dir}/../outside.rs"), 0),
        step(
            "format",
            &env,
            json!({ "tool_name": "Edit" }).to_string(),
            0,
        ),
    ];
    let (a, b) = parity("format", &steps, JQ);
    for (d, variant) in [(a.path(), "sh"), (b.path(), "rust")] {
        assert!(
            d.join(".claude/worktrees/w1/formatted-here").exists(),
            "{variant}: the worktree was formatted"
        );
        assert!(
            !d.join("vendor/other/formatted-here").exists(),
            "{variant}: an unrelated repository was left alone"
        );
        assert!(
            d.join("formatted-here").exists(),
            "{variant}: the project was formatted when no worktree of it owns the file"
        );
    }
    let (a, b) = parity(
        "format (no jq)",
        &[step(
            "format",
            &env,
            edit("{dir}/.claude/worktrees/w1/src/lib.rs"),
            0,
        )
        .setup(worktree)],
        NO_JQ,
    );
    for d in [a.path(), b.path()] {
        assert!(d.join(".claude/worktrees/w1/formatted-here").exists());
        assert!(!d.join("formatted-here").exists());
    }
}

/// A project in a subfolder of its repository (a monorepo) is formatted in its
/// own folder, as before — and so is the same folder of a worktree.
#[test]
fn format_runs_in_the_project_folder_of_a_monorepo() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    let rel = "services/api";
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "api".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.hooks_extra.format_cmd = "true".into();
    let proj = root.path().join(rel);
    p.scaffold(&proj, false).unwrap();
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(proj.join("src/lib.rs"), "\n").unwrap();
    git(root.path(), &["add", "-A"]);
    git(root.path(), &["commit", "-qm", "init"]);
    let wt = root.path().join("wt");
    git(
        root.path(),
        &["worktree", "add", "-q", wt.to_str().unwrap()],
    );
    let d = ocgen::paths::for_shell(&proj);
    for (file, want) in [
        (proj.join("src/lib.rs"), proj.clone()),
        (wt.join(rel).join("src/lib.rs"), wt.join(rel)),
    ] {
        let payload = json!({
            "session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Edit",
            "tool_input": { "file_path": ocgen::paths::for_shell(&file) },
        })
        .to_string();
        for (rust, no_jq) in [(true, false), (false, false), (false, true)] {
            let _ = fs::remove_file(want.join("formatted-here"));
            let mut env: HashMap<String, String> = [
                ("CLAUDE_PROJECT_DIR".to_string(), d.clone()),
                (
                    "OCGEN_FORMAT_CMD".to_string(),
                    "echo x > formatted-here".to_string(),
                ),
            ]
            .into();
            if no_jq {
                env.insert("OCGEN_NO_JQ".into(), "1".into());
            }
            let o = if rust {
                run_rs("format", &env, &payload)
            } else {
                run_sh(&proj, "format", &env, &payload)
            };
            assert_eq!(o.0, 0, "{o:?}");
            assert!(
                want.join("formatted-here").exists(),
                "rust={rust} no_jq={no_jq}: {} formats in {}",
                file.display(),
                want.display()
            );
            assert!(!root.path().join("formatted-here").exists());
            assert!(!wt.join("formatted-here").exists());
        }
    }
}

// ------------------------------------------------------------ drop-noop-cd --

fn has_jq() -> bool {
    Command::new("jq")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The rewritten command, or `None` when the hook left the call alone.
fn noop_cd(dir: &Path, rust: bool, cmd: &str, cwd: &str, msys: bool) -> Option<String> {
    let payload = json!({
        "tool_name": "Bash", "tool_input": { "command": cmd }, "cwd": cwd
    })
    .to_string();
    let mut env: HashMap<String, String> = [(
        "CLAUDE_PROJECT_DIR".to_string(),
        ocgen::paths::for_shell(dir),
    )]
    .into();
    if msys {
        env.insert("MSYSTEM".into(), "MINGW64".into());
    }
    let o = if rust {
        run_rs("drop-noop-cd", &env, &payload)
    } else {
        run_sh(dir, "drop-noop-cd", &env, &payload)
    };
    assert_eq!(o.0, 0, "never blocks: {o:?}");
    if o.1.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(o.1.trim()).unwrap();
    Some(
        v["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

#[test]
fn drop_noop_cd_reads_windows_paths_only_on_windows() {
    let dir = project();
    let sh_too = has_jq();
    for rust in [true, false] {
        if !rust && !sh_too {
            continue;
        }
        if !cfg!(windows) {
            // A case-sensitive Unix filesystem: /w/App is not /w/app, and `/x/`
            // is a folder, not a drive.
            for (cmd, cwd) in [
                ("cd /w/App && rm -rf build", "/w/app"),
                ("cd /x/PROJ && rm -rf build", "/x/proj"),
                ("cd /c/Users/x/proj; ls", "C:\\Users\\x\\proj"),
                ("cd \"/a\\b\" && rm -rf build", "/a/b"),
            ] {
                assert_eq!(
                    noop_cd(dir.path(), rust, cmd, cwd, false),
                    None,
                    "{} on {cmd:?} in {cwd:?}",
                    if rust { "rust" } else { "sh" }
                );
            }
        }
        // Under Git Bash the same spellings are one Windows folder.
        for (cmd, cwd) in [
            ("cd /c/Users/x/proj; ls", "C:\\Users\\x\\proj"),
            ("cd c:/users/X/Proj/ && ls", "C:\\Users\\x\\proj"),
            ("cd \"C:\\Users\\x\\proj\" && ls", "C:/Users/x/proj"),
        ] {
            assert_eq!(
                noop_cd(dir.path(), rust, cmd, cwd, true).as_deref(),
                Some("ls"),
                "{} on {cmd:?} in {cwd:?} (MSYSTEM)",
                if rust { "rust" } else { "sh" }
            );
        }
    }
}

// ----------------------------------------------------------------- wiring --

#[test]
fn settings_wire_the_worker_baseline_and_read_only_roles() {
    let dir = project();
    let s: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let ro = s["env"]["SUBAGENT_READONLY_ROLES"]
        .as_str()
        .unwrap_or_default();
    for role in ["explorer", "reviewer", "Explore", "Plan"] {
        assert!(
            ro.split_whitespace().any(|r| r == role),
            "{role} is read-only: {ro:?}"
        );
    }
    assert!(!ro.split_whitespace().any(|r| r == "implementer"), "{ro:?}");
    for ev in ["SubagentStart", "SubagentStop"] {
        let cmd = s["hooks"][ev][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_default();
        assert!(
            cmd.contains("subagent-confidence-gate"),
            "{ev} runs the worker gate: {cmd}"
        );
    }
    assert_eq!(ocgen::hooks::PROTOCOL, "ocgen-hooks 9");
}

#[test]
fn rules_say_where_confidence_and_owners_are_read() {
    let dir = project();
    let team = fs::read_to_string(dir.path().join(".claude/rules/ocgen-team.md")).unwrap();
    assert!(team.contains("last message"), "{team}");
    assert!(
        team.contains(".claude/team/confidence/<team-name>/<task-id>.txt"),
        "{team}"
    );
    assert!(team.contains("Owner: <teammate-name>"), "{team}");
    let plan = fs::read_to_string(dir.path().join(".claude/skills/team-plan/SKILL.md")).unwrap();
    assert!(plan.contains("Owner: <teammate-name>"), "{plan}");
    assert!(
        plan.contains("Status: APPROVED session=${CLAUDE_SESSION_ID}"),
        "{plan}"
    );
    assert!(plan.contains("Check:"), "{plan}");
    let fanout = fs::read_to_string(dir.path().join(".claude/skills/fanout/SKILL.md")).unwrap();
    assert!(
        fanout.contains("end its final message with a line `Confidence: NN%`"),
        "{fanout}"
    );
    assert!(
        !team.contains("The task check"),
        "no check command, no bullet"
    );
    let dir = project_with(|p| p.claude.workflow.check_cmd = "cargo test".into());
    let team = fs::read_to_string(dir.path().join(".claude/rules/ocgen-team.md")).unwrap();
    assert!(
        team.contains("one teammate at a time") && team.contains("`Check: cargo test <arguments>`"),
        "{team}"
    );
}

// ---------------------------------------------------------------- sandbox --
//
// Claude Code doesn't sandbox hooks, so with the project's sandbox on the hooks
// sandbox the project code they run themselves: the check and the formatter.

#[test]
fn every_script_that_runs_project_code_carries_the_same_sandbox() {
    let block = |script: &str| {
        let body = ocgen::templates::load_embedded(&format!("claude/hooks/{script}")).unwrap();
        let start = body.find("# --- sandbox").expect(script);
        let end = body.find("# --- end sandbox ---").expect(script);
        body[start..end].to_string()
    };
    let gate = block("subagent-confidence-gate.sh");
    assert_eq!(block("team-task-completed.sh"), gate);
    assert_eq!(block("format.sh"), gate);
    // The same reasons as the Rust hook gives.
    assert!(gate.contains(ocgen::hooks::NO_BWRAP));
    assert!(gate.contains(ocgen::hooks::NO_SEATBELT));
}

/// The sandbox env settings.json gives the hooks (credentials withheld).
fn sandbox_env() -> Vec<(&'static str, String)> {
    use ocgen::claude::{CREDENTIAL_ENV, CREDENTIAL_PATHS, HOOK_DENY_WRITE};
    vec![
        ("OCGEN_SANDBOX", "1".into()),
        ("OCGEN_SANDBOX_DENY_WRITE", HOOK_DENY_WRITE.join(" ")),
        ("OCGEN_SANDBOX_DENY_READ", CREDENTIAL_PATHS.join(" ")),
        ("OCGEN_SANDBOX_DENY_ENV", CREDENTIAL_ENV.join(" ")),
    ]
}

/// A worker step whose check is `check`, in the sandbox.
fn sandboxed_check(check: &str, expect: i32) -> Step {
    let mut s = step(
        "subagent-confidence-gate",
        &[
            ("SUBAGENT_CONFIDENCE_THRESHOLD", "0"),
            ("OCGEN_CHECK_TIMEOUT", "20"),
        ],
        stop_event("w1", "general-purpose", "Confidence: 99%"),
        expect,
    )
    .setup(|d: &Path| {
        dirty_wt(d);
        write(d, ".home/.ssh/id_test", "secret\n");
        write(d, ".home/.claude/ocgen/approvals/old", "1\n");
    });
    s.env.push(("OCGEN_CHECK_CMD", check.to_string()));
    s.env.extend(sandbox_env());
    s
}

#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_check_cannot_forge_an_approval_or_read_credentials() {
    let steps = vec![
        // Its own work goes ahead.
        sandboxed_check("echo built > out.txt && test -f out.txt", 0),
        // The approval store, however reached: written, created, renamed away.
        sandboxed_check(
            "echo 9999999999 > ../.home/.claude/ocgen/approvals/old",
            2,
        )
        .err("Operation not permitted"),
        sandboxed_check(
            "mkdir -p ../.home/.claude/ocgen/approvals && echo 1 > ../.home/.claude/ocgen/approvals/new",
            2,
        ),
        sandboxed_check("mv ../.home/.claude ../.home/moved", 2),
        sandboxed_check("mv ../.home ../moved-home", 2),
        // A withheld credential can't be read, nor its folder moved to read it.
        sandboxed_check("cat ../.home/.ssh/id_test", 2),
        sandboxed_check("mv ../.home/.ssh ../.home/k", 2),
        // What Claude Code and git run unsandboxed, in the project and in the
        // worker's own folder.
        sandboxed_check("echo x >> ../.claude/settings.json", 2),
        sandboxed_check("echo x > ../.claude/settings.local.json", 2),
        sandboxed_check("echo x > ../.claude/statusline.sh", 2),
        sandboxed_check("echo x > ../.git/hooks/pre-push", 2),
        sandboxed_check("mkdir -p .claude/hooks && echo x > .claude/hooks/a.sh", 2),
        sandboxed_check("mv ../.claude ../c2", 2),
    ];
    for (name, extra) in [("sandbox", JQ), ("sandbox (no jq)", NO_JQ)] {
        let (a, b) = parity(name, &steps, extra);
        for d in [a.path(), b.path()] {
            let store = d.join(".home/.claude/ocgen/approvals");
            assert_eq!(fs::read_to_string(store.join("old")).unwrap(), "1\n");
            assert!(!store.join("new").exists());
            assert!(d.join(".home/.ssh/id_test").exists());
            assert!(!d.join(".claude/settings.local.json").exists());
            assert!(!d.join("wt/.claude/hooks/a.sh").exists());
            assert!(
                d.join("wt/out.txt").exists(),
                "the check's own write landed"
            );
        }
    }
}

/// `ocgen hook` as Claude Code runs it: a process of its own, with `env` set.
#[cfg(target_os = "macos")]
fn run_bin(hook: &str, env: &[(&str, &str)], payload: &str) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ocgen"));
    cmd.args(["hook", hook])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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

#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_check_runs_without_the_withheld_env_and_still_times_out() {
    let dir = project();
    dirty_wt(dir.path());
    let d = ocgen::paths::for_shell(dir.path());
    let payload = stop_event("w1", "general-purpose", "Confidence: 99%").replace("{dir}", &d);
    let sb = sandbox_env();
    let run = |check: &str, sandbox: bool, rust: bool| {
        let mut env: Vec<(&str, &str)> = vec![
            ("CLAUDE_PROJECT_DIR", d.as_str()),
            ("SUBAGENT_CONFIDENCE_THRESHOLD", "0"),
            ("OCGEN_CHECK_CMD", check),
            ("OCGEN_CHECK_TIMEOUT", "1"),
            ("GH_TOKEN", "ghp_test"),
            ("NPM_TOKEN", "npm_test"),
        ];
        if sandbox {
            env.extend(sb.iter().map(|(k, v)| (*k, v.as_str())));
        }
        if rust {
            run_bin("subagent-confidence-gate", &env, &payload)
        } else {
            let env: HashMap<String, String> = env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            run_sh(dir.path(), "subagent-confidence-gate", &env, &payload)
        }
    };
    let check = r#"test -z "${GH_TOKEN+x}${NPM_TOKEN+x}""#;
    for rust in [false, true] {
        let who = if rust { "rust" } else { "sh" };
        assert_eq!(
            run(check, true, rust).0,
            0,
            "{who}: withheld in the sandbox"
        );
        assert_eq!(run(check, false, rust).0, 2, "{who}: there without it");
        // The timeout still takes the whole sandboxed check down.
        let started = std::time::Instant::now();
        let o = run("sleep 30 & echo $! > gc.pid; sleep 30", true, rust);
        assert!(started.elapsed().as_secs() < 20, "{who}: timed out");
        assert!(o.2.contains("(exit timeout after 1s)"), "{who}: {o:?}");
        let pid = fs::read_to_string(dir.path().join("wt/gc.pid")).unwrap();
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success();
        if alive {
            let _ = Command::new("kill").args(["-9", pid.trim()]).output();
        }
        assert!(!alive, "{who}: the check's child was killed too");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_team_check_and_the_formatter_run_sandboxed_too() {
    let mut team = step(
        "team-task-completed",
        &[("OCGEN_CHECK_TIMEOUT", "20")],
        checked_task("t1", "x"),
        2,
    )
    .setup(|d: &Path| write(d, ".home/.claude/ocgen/approvals/old", "1\n"))
    .err("Operation not permitted");
    team.env.push((
        "OCGEN_CHECK_CMD",
        "echo 9999999999 > .home/.claude/ocgen/approvals/old".into(),
    ));
    team.env.extend(sandbox_env());
    let edit = json!({
        "session_id": "s", "cwd": "{dir}", "hook_event_name": "PostToolUse",
        "tool_name": "Edit", "tool_input": { "file_path": "{dir}/src/lib.rs" },
    })
    .to_string();
    let mut fmt = step("format", &[], edit, 0)
        .setup(|d: &Path| write(d, ".home/.claude/ocgen/approvals/old", "1\n"))
        .err("format hook:");
    fmt.env.push((
        "OCGEN_FORMAT_CMD",
        "echo formatted > fmt.txt; echo 9999999999 > .home/.claude/ocgen/approvals/old".into(),
    ));
    fmt.env.extend(sandbox_env());
    let (a, b) = parity("sandboxed team check and formatter", &[team, fmt], JQ);
    for d in [a.path(), b.path()] {
        let old = d.join(".home/.claude/ocgen/approvals/old");
        assert_eq!(fs::read_to_string(old).unwrap(), "1\n");
        assert!(d.join("fmt.txt").exists(), "the formatter ran");
    }
}

/// A folder on PATH with the tools the worker gate uses, and `extra` scripts
/// (`name`, body) — so a test controls what `uname` says and whether `bwrap`
/// exists.
#[cfg(unix)]
fn tool_dir(extra: &[(&str, &str)]) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    for tool in [
        "sh", "cat", "awk", "grep", "head", "sed", "tr", "cut", "git", "cksum", "tail", "ps",
        "sleep", "mktemp", "rm", "mkdir", "date", "dirname", "setsid", "readlink", "env", "touch",
        "jq",
    ] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let path = String::from_utf8_lossy(&found.stdout).trim().to_string();
        if path.starts_with('/') {
            std::os::unix::fs::symlink(&path, bin.path().join(tool)).unwrap();
        }
    }
    for (name, body) in extra {
        let p = bin.path().join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

/// A fake `uname` that says Linux.
#[cfg(unix)]
const LINUX_UNAME: &str = "#!/bin/sh\necho Linux\n";

/// A fake `bwrap`: logs its arguments, one per line, to `bwrap.log` in the
/// folder it runs in, then runs the command after the options (from `/bin/sh`).
#[cfg(unix)]
const FAKE_BWRAP: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > bwrap.log\nwhile [ $# -gt 0 ] && [ \"$1\" != /bin/sh ]; do shift; done\nexec \"$@\"\n";

/// A worker isolated in a worktree under the project's `.claude/`, as Claude
/// Code makes them, plus the home and project files the sandbox protects.
#[cfg(unix)]
fn isolated_worker(d: &Path) {
    let wt = d.join(".claude/worktrees/w1");
    if !wt.exists() {
        fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q"]);
        fs::write(wt.join("new.rs"), "// change\n").unwrap();
    }
    write(d, ".home/.claude/ocgen/approvals/old", "1\n");
    write(d, ".home/.ssh/id_test", "secret\n");
    write(d, ".home/.npmrc", "//registry.npmjs.org/:_authToken=x\n");
}

/// Linux: the sh twin's bubblewrap command line is exactly the one the Rust
/// twin builds (`hooks::bwrap_args`) — checked on any Unix with a fake
/// `uname` and `bwrap`; on Linux the Rust hook runs it too.
#[cfg(unix)]
#[test]
fn linux_runs_the_check_in_bubblewrap_the_same_way() {
    use ocgen::hooks::{bwrap_args, SandboxSpec};
    let bin = tool_dir(&[("uname", LINUX_UNAME), ("bwrap", FAKE_BWRAP)]);
    let path = ocgen::paths::for_shell(bin.path());
    let sb = sandbox_env();
    for (variant, extra) in [("jq", JQ), ("no jq", NO_JQ)] {
        let dir = project();
        isolated_worker(dir.path());
        let d = ocgen::paths::for_shell(dir.path());
        let wt = dir.path().join(".claude/worktrees/w1");
        let log = wt.join("bwrap.log");
        let payload = stop_event("w1", "general-purpose", "Confidence: 99%")
            .replace("{dir}/wt", "{dir}/.claude/worktrees/w1")
            .replace("{dir}", &d);
        let mut env: HashMap<String, String> = [
            ("CLAUDE_PROJECT_DIR", d.clone()),
            ("HOME", format!("{d}/.home")),
            ("PATH", path.clone()),
            ("SUBAGENT_CONFIDENCE_THRESHOLD", "0".into()),
            ("OCGEN_CHECK_CMD", "echo ran > ran.txt".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        env.extend(sb.iter().map(|(k, v)| (k.to_string(), v.clone())));
        for (k, v) in extra {
            env.insert(k.to_string(), v.to_string());
        }
        let want: Vec<String> = bwrap_args(&SandboxSpec {
            deny_write: &env["OCGEN_SANDBOX_DENY_WRITE"],
            deny_read: &env["OCGEN_SANDBOX_DENY_READ"],
            home: &env["HOME"],
            project: dir.path(),
            dir: &wt,
        })
        .into_iter()
        .chain(["/bin/sh", "-c", "echo ran > ran.txt"].map(String::from))
        .collect();
        // One twin at a time: both write the same bwrap.log.
        let mut twins = vec!["sh"];
        if cfg!(target_os = "linux") {
            twins.push("rust");
        }
        for who in twins {
            let o = match who {
                "sh" => run_sh(dir.path(), "subagent-confidence-gate", &env, &payload),
                _ => run_rs("subagent-confidence-gate", &env, &payload),
            };
            assert_eq!(o.0, 0, "{variant} {who}: {o:?}");
            let got: Vec<String> = fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(String::from)
                .collect();
            assert_eq!(got, want, "{variant} {who}");
            fs::remove_file(&log).unwrap();
        }
        assert!(wt.join("ran.txt").exists());
        // The command line protects the store and the project's .claude (the
        // worker's folder in it stays writable), and hides the credentials.
        let real = |p: &Path| fs::canonicalize(p).unwrap().to_string_lossy().into_owned();
        let (home, p, w) = (real(&dir.path().join(".home")), real(dir.path()), real(&wt));
        let has = |opt: &str, arg: &str| want.windows(2).any(|x| x[0] == opt && x[1] == arg);
        assert!(has("--ro-bind", &format!("{home}/.claude")), "{want:?}");
        assert!(has("--ro-bind", &format!("{p}/.claude")), "{want:?}");
        let ro = want.iter().position(|a| *a == format!("{p}/.claude"));
        let rw = want.iter().rposition(|a| *a == w);
        assert!(rw > ro, "the worker's folder is re-opened after: {want:?}");
        assert!(has("--tmpfs", &format!("{home}/.ssh")), "{want:?}");
        assert!(
            want.windows(3).any(|x| x[0] == "--ro-bind"
                && x[1] == "/dev/null"
                && x[2] == format!("{home}/.npmrc")),
            "{want:?}"
        );
    }
}

/// Without bubblewrap a Linux check doesn't run at all — never unsandboxed.
#[cfg(unix)]
#[test]
fn linux_without_bubblewrap_refuses_to_run_the_check() {
    let bin = tool_dir(&[("uname", LINUX_UNAME)]);
    let dir = project();
    isolated_worker(dir.path());
    let d = ocgen::paths::for_shell(dir.path());
    let payload = stop_event("w1", "general-purpose", "Confidence: 99%").replace("{dir}", &d);
    dirty_wt(dir.path());
    let mut env: HashMap<String, String> = [
        ("CLAUDE_PROJECT_DIR", d.clone()),
        ("HOME", format!("{d}/.home")),
        ("PATH", ocgen::paths::for_shell(bin.path())),
        ("SUBAGENT_CONFIDENCE_THRESHOLD", "0".into()),
        ("OCGEN_CHECK_CMD", "echo ran > ran.txt".into()),
        ("OCGEN_NO_JQ", "1".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    env.extend(sandbox_env().into_iter().map(|(k, v)| (k.to_string(), v)));
    let mut runs = vec![(
        "sh",
        run_sh(dir.path(), "subagent-confidence-gate", &env, &payload),
    )];
    if cfg!(target_os = "linux") {
        runs.push(("rust", run_rs("subagent-confidence-gate", &env, &payload)));
    }
    let first = runs[0].1 .2.lines().next().map(String::from);
    for (who, o) in &runs {
        assert_eq!(o.0, 2, "{who}: {o:?}");
        assert!(o.2.contains("(exit not run)"), "{who}: {o:?}");
        assert!(
            o.2.contains("bubblewrap (bwrap) isn't installed"),
            "{who}: {o:?}"
        );
        assert_eq!(o.2.lines().next().map(String::from), first, "{who}");
    }
    assert!(
        !dir.path().join("wt/ran.txt").exists(),
        "never run unsandboxed"
    );

    // The formatter is skipped with the same reason (it never blocks).
    let edit = json!({ "tool_name": "Edit", "tool_input": { "file_path": "{dir}/x.rs" } })
        .to_string()
        .replace("{dir}", &d);
    env.insert("OCGEN_FORMAT_CMD".into(), "echo x > fmt.txt".into());
    let o = run_sh(dir.path(), "format", &env, &edit);
    assert_eq!(o.0, 0);
    assert!(
        o.2.starts_with("format hook: 'echo x > fmt.txt' not run: the project's sandbox is on"),
        "{o:?}"
    );
    if cfg!(target_os = "linux") {
        assert_eq!(run_rs("format", &env, &edit).2, o.2);
    }
    assert!(!dir.path().join("fmt.txt").exists());
}

/// Linux with a working bubblewrap (skipped without one): the check can't reach
/// the store, the credentials or the project's Claude Code settings.
#[cfg(target_os = "linux")]
#[test]
fn bubblewrap_keeps_the_check_off_the_store_and_the_settings() {
    let usable = Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !usable {
        eprintln!("skipped: no usable bwrap here");
        return;
    }
    let steps = vec![
        sandboxed_check("echo built > out.txt && test -f out.txt", 0),
        sandboxed_check("echo 9999999999 > ../.home/.claude/ocgen/approvals/old", 2),
        sandboxed_check("cat ../.home/.ssh/id_test", 2),
        sandboxed_check("echo x >> ../.claude/settings.json", 2),
        sandboxed_check("echo x > ../.claude/settings.local.json", 2),
        sandboxed_check("mv ../.home/.claude ../.home/moved", 2),
    ];
    let (a, b) = parity("bubblewrap", &steps, NO_JQ);
    for d in [a.path(), b.path()] {
        let old = d.join(".home/.claude/ocgen/approvals/old");
        assert_eq!(fs::read_to_string(old).unwrap(), "1\n");
        assert!(!d.join(".claude/settings.local.json").exists());
        assert!(d.join("wt/out.txt").exists());
    }
}

/// Native Windows has no sandbox: the check runs as it always has.
#[cfg(windows)]
#[test]
fn windows_runs_the_check_as_before_with_the_sandbox_on() {
    let mut ok = sandboxed_check("echo built > out.txt", 0);
    ok.env.retain(|(k, _)| *k != "OCGEN_CHECK_TIMEOUT");
    let (a, b) = parity("windows sandbox", &[ok], NO_JQ);
    for d in [a.path(), b.path()] {
        assert!(d.join("wt/out.txt").exists());
    }
}
