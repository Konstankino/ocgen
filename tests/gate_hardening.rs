//! Hardening of the execution-approval gate and the hooks around it. Every gate
//! case runs three ways — the Rust hook (`ocgen hook`), the bundled `sh` script
//! with jq, and the script's plain fallback without jq — and all three must agree.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use ocgen::agent;
use ocgen::approval;
use ocgen::claude::Team;
use ocgen::manifest::Manifest;
use ocgen::render::Project;
use ocgen::target::Target;
use serde_json::{json, Value};
use tempfile::TempDir;

fn gated() -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = "gate".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p.claude.team = Team {
        enabled: true,
        mode: "in-process".into(),
        hooks: true,
        plan_gate: false,
        confidence_threshold: 0,
        risk_rounds: false,
        approval_gate: true,
    };
    p.claude.workflow.intent = true;
    p.claude.intent.trusted_domains = vec!["docs.rs".into()];
    p.claude.hooks_extra.config_audit = true;
    p
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// A generated, gated project inside its own git repository (the approval key
/// then comes from git in every implementation).
fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    gated().scaffold(dir.path(), false).unwrap();
    dir
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Imp {
    Rust,
    Sh,
    ShNoJq,
}
const ALL: [Imp; 3] = [Imp::Rust, Imp::Sh, Imp::ShNoJq];

type Out = (i32, String, String);

fn home_of(dir: &Path) -> std::path::PathBuf {
    dir.join(".home")
}

/// Run hook `hook` of the project in `dir` one way, with the project's env plus `extra`.
fn run(imp: Imp, dir: &Path, hook: &str, extra: &[(&str, &str)], payload: &str) -> Out {
    let d = ocgen::paths::for_shell(dir);
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("CLAUDE_PROJECT_DIR".into(), d.clone());
    env.insert("HOME".into(), format!("{d}/.home"));
    for (k, v) in extra {
        env.insert(k.to_string(), v.to_string());
    }
    if imp == Imp::Rust {
        let o = ocgen::hooks::run(hook, payload, &env);
        return (o.code, o.stdout, o.stderr);
    }
    let mut cmd = Command::new("sh");
    cmd.arg(dir.join(format!(".claude/hooks/{hook}.sh")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The real environment (a shell needs more than PATH on Windows), minus
    // anything that would steer a gate.
    for k in [
        "TEAM_APPROVAL_GATE",
        "LOOP_GUARD_MAX_BLOCKS",
        "OCGEN_NO_JQ",
        "OCGEN_WEBFETCH_DOMAINS",
        "CLAUDECODE",
    ] {
        cmd.env_remove(k);
    }
    for (k, v) in &env {
        cmd.env(k, v);
    }
    if imp == Imp::ShNoJq {
        cmd.env("OCGEN_NO_JQ", "1");
    }
    let mut child = cmd.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const GATE: &[(&str, &str)] = &[("TEAM_APPROVAL_GATE", "1")];

fn bash(command: &str) -> String {
    json!({ "session_id": "s", "tool_name": "Bash", "tool_input": { "command": command } })
        .to_string()
}

fn tool(name: &str, input: Value) -> String {
    json!({ "session_id": "s", "tool_name": name, "tool_input": input }).to_string()
}

/// Every implementation exits with `code` on `payload`; returns their outputs.
fn expect(dir: &Path, payload: &str, code: i32) -> Vec<Out> {
    let outs: Vec<Out> = ALL
        .iter()
        .map(|i| run(*i, dir, "team-approval-gate", GATE, payload))
        .collect();
    for (imp, o) in ALL.iter().zip(&outs) {
        assert_eq!(
            o.0, code,
            "{imp:?} exit {} (want {code}) for {payload}\nstderr: {}",
            o.0, o.2
        );
    }
    outs
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ------------------------------------------------------------------- A1 --

#[test]
fn the_block_message_names_no_store_and_no_recipe() {
    let dir = project();
    for payload in [
        bash("git push origin main"),
        bash("ocgen approve"),
        tool(
            "Write",
            json!({ "file_path": "/x/.claude/ocgen/approvals/k", "content": "9" }),
        ),
    ] {
        for (imp, o) in ALL.iter().zip(expect(dir.path(), &payload, 2)) {
            assert!(o.2.contains("ocgen approve"), "{imp:?}: {}", o.2);
            for leak in [".claude/ocgen", "approvals/", "date +%s", "echo $(("] {
                assert!(!o.2.contains(leak), "{imp:?} leaks {leak:?}: {}", o.2);
            }
        }
    }
}

#[test]
fn an_approval_further_out_than_ocgen_grants_is_ignored() {
    let dir = project();
    let home = home_of(dir.path());
    // The longest real approval works…
    approval::grant(&home, dir.path(), approval::MAX_MINUTES).unwrap();
    assert!(approval::remaining(&home, dir.path()).is_some());
    expect(dir.path(), &bash("git push origin main"), 0);
    // …a longer one can't be granted…
    assert!(approval::grant(&home, dir.path(), approval::MAX_MINUTES + 1).is_err());
    // …and a hand-written far-future expiry is not an approval.
    let marker = approval::marker_path(&home, dir.path());
    for forged in [
        format!("{}\n", now() + 10 * 365 * 24 * 3600),
        format!("{}\n", now() + approval::MAX_AHEAD_SECS + 120),
        "9999999999\n".to_string(),
        // Not how ocgen writes an expiry (the scripts read at most 12 digits).
        format!("000{}\n", now() + 600),
        "99999999999999999999999999\n".to_string(),
    ] {
        fs::write(&marker, &forged).unwrap();
        assert!(
            approval::remaining(&home, dir.path()).is_none(),
            "forged {forged}"
        );
        expect(dir.path(), &bash("git push origin main"), 2);
    }
}

#[test]
fn the_scripts_cap_approvals_like_ocgen() {
    let want = format!("max_ahead={}", approval::MAX_AHEAD_SECS);
    for script in ["team-approval-gate.sh", "git-pre-push.sh"] {
        let body = ocgen::templates::load_embedded(&format!("claude/hooks/{script}")).unwrap();
        assert!(
            body.lines().any(|l| l.trim() == want),
            "{script} must define {want}"
        );
    }
}

#[test]
fn the_script_and_the_binary_share_the_self_approval_pattern() {
    let script = ocgen::templates::load_embedded("claude/hooks/team-approval-gate.sh").unwrap();
    let line = script
        .lines()
        .find(|l| l.starts_with("SELF_APPROVAL='"))
        .expect("the script defines SELF_APPROVAL");
    assert_eq!(
        line,
        format!("SELF_APPROVAL='{}'", ocgen::risk::SELF_APPROVAL)
    );
}

#[test]
fn self_approval_is_blocked_however_the_store_is_spelled() {
    let dir = project();
    for cmd in [
        "cd ~/.claude/ocgen && echo 9999999999 > approvals/KEY",
        "echo 9999999999 > ~/.claude/ocgen/./approvals/KEY",
        "echo 9999999999 > ~/.claude/ocgen//approvals/KEY",
        "echo 9999999999 > ~/.claude/OCGEN/approvals/KEY",
        "echo 1 > $HOME/.Claude/Ocgen/Approvals/KEY",
        "d=~/.claude/ocg; echo 1 > ${d}en/approvals/KEY",
        "d=~/.claude/ocg\necho 1 > ${d}en/approvals/KEY",
        "cd ~/.claude && cd ocgen && echo 1 > approvals/KEY",
        "echo 1 > C:\\Users\\x\\.claude\\ocgen\\approvals\\k",
        // Windows 8.3 short names of .claude and approvals.
        "echo 1 > C:\\Users\\x\\CLAUDE~1\\ocgen\\APPROV~1\\k",
        "cd /c/Users/x/CLAUDE~1/ocgen && echo 1 > APPROV~1/k",
        // APFS and NTFS fold the long s (U+017F) to an s.
        "cd ~/.claude && cd ocgen && echo 1 > approval\u{17f}/KEY",
        "ocgen approve",
        "OCGEN   Approve --minutes 600",
    ] {
        for (imp, o) in ALL.iter().zip(expect(dir.path(), &bash(cmd), 2)) {
            assert!(
                o.2.starts_with("Blocked: only a HUMAN"),
                "{imp:?} on {cmd:?}: {}",
                o.2
            );
        }
    }
    // Ordinary work that merely mentions ocgen or ~/.claude is fine.
    for cmd in [
        "ocgen verify",
        "ocgen doctor",
        "ls ~/.claude/projects",
        "cat .claude/settings.json",
        "grep -rn approvals src/",
        "git status",
    ] {
        expect(dir.path(), &bash(cmd), 0);
    }
}

/// The store's name must not be blocked where it isn't the store: the project's
/// own `.claude/.ocgen-state.json` is committed with everything else, and a
/// command may name `.claude` and the word "approvals" without going near
/// `~/.claude/ocgen/approvals/`. Each false block would also charge the loop
/// guard, which halts the agent at its budget.
#[test]
fn work_on_the_projects_own_claude_folder_is_not_self_approval() {
    let dir = project();
    let ordinary = [
        "cat .claude/.ocgen-state.json | jq .claude.team",
        "git add .claude/.ocgen-state.json .claude/settings.json",
        "git checkout -- .claude/.ocgen-state.json",
        "git diff .claude/.ocgen-state.json",
        "grep -n approvals .claude/hooks/team-approval-gate.sh",
        "grep -rn approvals .claude/",
        "ls .claude",
        "ls .claude/hooks && ocgen verify",
        "cat .claude/settings.json | grep ocgen",
        "cat .claude/output-styles/ocgen-concise.md",
        "git add .claude/rules/ocgen-team.md && git commit -m \"approvals: say who approves\"",
        // A whole `.claude/` (or a glob under it) beside the word "approvals".
        "git add .claude/ src/ && git commit -m \"approvals expire\"",
        "git add .claude/ && git commit -m \"$(cat <<'EOF'\nfix: approvals expire\nEOF\n)\"",
        "ls .claude && grep -rn approvals src/",
        "cd .claude && grep -rn approvals hooks/*.sh",
        "grep -n approvals .claude/hooks/*.sh",
        "find .claude -name '*.sh' | xargs grep -l approvals",
        "rg -n approvals -g '*.sh' .claude/",
        "cat .claude/*.json | grep approvals",
    ];
    let guarded = [("TEAM_APPROVAL_GATE", "1"), ("LOOP_GUARD_MAX_BLOCKS", "3")];
    for cmd in ordinary {
        assert!(!ocgen::risk::self_approval(cmd), "{cmd}");
        for imp in ALL {
            for _ in 0..4 {
                let o = run(imp, dir.path(), "team-approval-gate", &guarded, &bash(cmd));
                assert_eq!((o.0, o.1.as_str()), (0, ""), "{imp:?} on {cmd:?}: {}", o.2);
            }
        }
    }
    // The narrower pattern still sees the store reached in steps, split by a
    // variable, or put together from a copy made elsewhere.
    for cmd in [
        "cd ~/.claude && cd ocgen && cd approvals && echo 1 > K",
        "cd ~/.claude/ocgen. && echo 1 > approvals/K",
        "ls ~/.claude/ocgen",
        "cp -r /tmp/approvals ~/.claude/ocg$x",
        "a=approvals; d=~/.claude/ocg; echo 1 > ${d}en/$a/K",
    ] {
        assert!(ocgen::risk::self_approval(cmd), "{cmd}");
        for (imp, o) in ALL.iter().zip(expect(dir.path(), &bash(cmd), 2)) {
            assert!(
                o.2.starts_with("Blocked: only a HUMAN"),
                "{imp:?} on {cmd:?}: {}",
                o.2
            );
        }
    }
}

/// `ocgen` counts only as a whole name, but the shell can also reach the store's
/// folder by a glob or a variable (`~/.claude/oc*`, `d=~/.claude/ocg;
/// cd ${d}en`), or by a `..` written with Windows separators: then "approvals"
/// as a plain word is enough, as it was before the pattern was narrowed. A glob
/// that can't match `ocgen` (`.claude/hooks/*.sh`) stays ordinary work.
#[test]
fn a_store_folder_reached_by_a_glob_or_a_variable_is_self_approval() {
    let dir = project();
    for cmd in [
        "cd ~/.claude/oc[g]en && echo 9 > approvals/K",
        "cd ~/.claude/ocg* && echo 9 > approvals/K",
        "cd ~/.claude/*/ && echo 9 > approvals/K",
        "pushd ~/.claude/o?gen && echo 9 > approvals/K",
        "cd ~/.claude/$x && echo 9 > approvals/K",
        "d=~/.claude/ocg; cd ${d}en; echo 9 > approvals/K",
        "d=~/.claude/; cd ${d}ocg*; echo 9 > approvals/K",
        "cd ~/.claude; cd ocg?n; echo 9 > approvals/K",
        "cd ~/.claude && cd oc* && echo 9 > approvals/K",
        "CD ~/.CLAUDE/OC* && echo 9 > APPROVALS/K",
        "cd ~/.claude\ncd ./oc*\necho 9 > approvals/K",
        "cd ~/.claude;\tcd oc*;\techo 9 > approvals/K",
        "cd /tmp && cp -r approvals ~/.claude/oc*/",
        "cd C:\\Users\\x\\.claude\\x\\..\\ocgen && echo 9 > approvals\\K",
        "cd C:\\Users\\x\\.claude; cd .\\oc*; echo 9 > approvals\\K",
        "$d = \"$HOME\\.claude\\ocg\"; cd \"${d}en\"; Set-Content approvals\\K 9",
    ] {
        assert!(ocgen::risk::self_approval(cmd), "{cmd}");
        for (imp, o) in ALL.iter().zip(expect(dir.path(), &bash(cmd), 2)) {
            assert!(
                o.2.starts_with("Blocked: only a HUMAN"),
                "{imp:?} on {cmd:?}: {}",
                o.2
            );
        }
    }
}

/// The shell joins a line continued with a trailing backslash, so a store path
/// split that way is the store path: the binary joins the lines before the
/// self-approval check, like the script — which matters once an approval is in
/// force and the high-impact pattern no longer stops the write.
#[test]
fn a_store_path_split_across_continued_lines_is_self_approval() {
    let dir = project();
    let split = "echo 1 > ~/.cl\\\naude/ocg\\\nen/appr\\\novals/KEY";
    assert!(ocgen::risk::self_approval(split));
    for approved in [false, true] {
        if approved {
            approval::grant(&home_of(dir.path()), dir.path(), 10).unwrap();
        }
        for (imp, o) in ALL.iter().zip(expect(dir.path(), &bash(split), 2)) {
            assert!(
                o.2.starts_with("Blocked: only a HUMAN"),
                "{imp:?} (approved: {approved}): {}",
                o.2
            );
        }
    }
}

/// Without jq the script reads keys from the raw text, where a nested object can
/// repeat one (an MCP tool's options): a repeated key never hides the real
/// `tool_input` value that jq and the binary read. Every value is checked, and
/// a repeated command the script can't place is gated (fail closed).
#[test]
fn without_jq_a_nested_key_never_hides_the_real_one() {
    let dir = project();
    let home = ocgen::paths::for_shell(&home_of(dir.path()));
    let store = format!("{home}/.claude/ocgen/approvals/K");
    for input in [
        json!({ "command": "git push origin main", "opts": { "command": "ls" } }),
        json!({ "command": "ocgen approve", "opts": { "command": "ls" } }),
        json!({ "file_path": store, "opts": { "file_path": "notes.md" } }),
        json!({ "notebook_path": store, "opts": { "notebook_path": "a.ipynb" } }),
        json!({ "path": store, "opts": { "path": "src" } }),
    ] {
        expect(dir.path(), &tool("mcp__x__run", input), 2);
    }
    // A nested cwd doesn't move where a relative path points either.
    let p = json!({ "session_id": "s", "cwd": format!("{home}/.claude/ocgen"),
                    "tool_name": "mcp__x__write",
                    "tool_input": { "file_path": "approvals/K", "cwd": "/tmp" } })
    .to_string();
    expect(dir.path(), &p, 2);
    // A repeated command that is harmless where jq and the binary read it: only
    // the plain script gates it — until a human approves, like any gated call.
    let benign = tool(
        "mcp__x__run",
        json!({ "command": "ls", "opts": { "command": "pwd" } }),
    );
    for imp in ALL {
        let o = run(imp, dir.path(), "team-approval-gate", GATE, &benign);
        let want = if imp == Imp::ShNoJq { 2 } else { 0 };
        assert_eq!(o.0, want, "{imp:?}: {}", o.2);
    }
    approval::grant(&home_of(dir.path()), dir.path(), 10).unwrap();
    expect(dir.path(), &benign, 0);
    // One of each key, as every built-in tool sends them, is read as before.
    let d = ocgen::paths::for_shell(dir.path());
    expect(
        dir.path(),
        &tool(
            "Write",
            json!({ "file_path": format!("{d}/src/x.rs"), "content": "\"path\": \"x\"" }),
        ),
        0,
    );
}

#[test]
fn file_tools_are_checked_on_the_canonical_path() {
    let dir = project();
    let home = ocgen::paths::for_shell(&home_of(dir.path()));
    let blocked = [
        json!({ "file_path": format!("{home}/.claude/ocgen/./approvals/K") }),
        json!({ "file_path": format!("{home}/.claude/OCGEN/approvals/K") }),
        json!({ "file_path": format!("{home}/.claude/ocgen//approvals/K") }),
        json!({ "file_path": format!("{home}/.claude/ocgen/x/../approvals/K") }),
        json!({ "file_path": format!("{home}/.claude/Ocgen/Approvals/K") }),
        json!({ "file_path": "C:\\Users\\x\\.claude\\ocgen\\\\approvals\\K" }),
        json!({ "file_path": "C:\\Users\\x\\.claude\\OCGEN\\.\\approvals\\K" }),
        json!({ "file_path": "C:\\Users\\x\\CLAUDE~1\\ocgen\\APPROV~1\\K" }),
        json!({ "file_path": format!("{home}/.claude/ocgen/approval\u{17f}/K") }),
        json!({ "notebook_path": format!("{home}/.claude/ocgen/./approvals/K") }),
        json!({ "path": format!("{home}/.claude/ocgen/approvals") }),
    ];
    for input in blocked {
        for name in ["Write", "Edit", "MultiEdit", "NotebookEdit", "Read"] {
            expect(dir.path(), &tool(name, input.clone()), 2);
        }
    }
    // A relative path is resolved against the call's working directory.
    for (cwd, file) in [
        (format!("{home}/.claude/ocgen"), "approvals/K"),
        (format!("{home}/.claude/x"), "../ocgen/approvals/K"),
    ] {
        let p = json!({ "session_id": "s", "cwd": cwd, "tool_name": "Write",
                         "tool_input": { "file_path": file, "content": "9" } })
        .to_string();
        expect(dir.path(), &p, 2);
    }
    // Ordinary reads and writes stay allowed.
    let d = ocgen::paths::for_shell(dir.path());
    for (name, input) in [
        (
            "Write",
            json!({ "file_path": "docs/runbook.md", "content": "ask a human" }),
        ),
        ("Edit", json!({ "file_path": format!("{d}/src/main.rs") })),
        ("Read", json!({ "file_path": format!("{d}/README.md") })),
        ("Grep", json!({ "pattern": "approvals", "path": d })),
        (
            "Glob",
            json!({ "pattern": "**/*.rs", "path": format!("{d}/src") }),
        ),
        (
            "Read",
            json!({ "file_path": format!("{home}/.claude/projects/x.jsonl") }),
        ),
    ] {
        expect(dir.path(), &tool(name, input), 0);
    }
}

// ------------------------------------------------------------------- A3 --

#[test]
fn any_tool_that_runs_a_command_is_gated() {
    let dir = project();
    for (name, command) in [
        ("Monitor", "git push origin main"),
        ("PowerShell", "git push origin main"),
        ("mcp__shell__run", "terraform apply -auto-approve"),
        ("Monitor", "./deploy.sh 2>&1 | grep --line-buffered done"),
    ] {
        expect(
            dir.path(),
            &tool(name, json!({ "command": command, "description": "x" })),
            2,
        );
    }
    let o = expect(
        dir.path(),
        &tool("Monitor", json!({ "command": "ocgen approve" })),
        2,
    );
    assert!(o[0].2.starts_with("Blocked: only a HUMAN"), "{}", o[0].2);
    expect(
        dir.path(),
        &tool("Monitor", json!({ "command": "tail -f build.log" })),
        0,
    );
}

#[test]
fn the_gate_is_registered_for_every_tool() {
    let dir = project();
    let s: Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    let groups = s["hooks"]["PreToolUse"].as_array().unwrap();
    let gate = groups
        .iter()
        .find(|g| g.to_string().contains("team-approval-gate"))
        .expect("a PreToolUse group runs the approval gate");
    assert!(
        gate.get("matcher").is_none(),
        "no matcher, so every tool is checked: {gate}"
    );
}

// ------------------------------------------------------------------- A6 --

#[test]
fn newline_separated_commands_are_matched_line_by_line() {
    let dir = project();
    for cmd in [
        "cd infra\n./deploy.sh",
        "echo ok\n./publish.py",
        "echo ok\nssh host uptime",
        "git status\ngit push origin main",
        "echo ok\r\nssh prod uptime",
    ] {
        assert!(
            ocgen::risk::high_impact(&ocgen::risk::normalize(cmd)).is_some(),
            "risk: {cmd:?}"
        );
        expect(dir.path(), &bash(cmd), 2);
    }
    expect(dir.path(), &bash("echo deploy\ncat notes.md"), 0);
}

/// A backslash-newline continues the command (the shell joins the lines), while
/// a plain newline ends it: the binary and the script must both see it that way,
/// so a word on the next line never completes a match on this one.
#[test]
fn continued_lines_are_joined_and_other_lines_kept_apart() {
    let dir = project();
    for cmd in [
        "terraform -chdir=infra \\\n  apply -auto-approve",
        "docker \\\n  push org/app:1",
        "git -C repo \\\n  push origin main",
        "curl https://api.example.com/x \\\n  --data-binary @payload.json",
    ] {
        assert!(ocgen::risk::high_impact(cmd).is_some(), "risk: {cmd:?}");
        expect(dir.path(), &bash(cmd), 2);
    }
    for cmd in [
        // `git` alone, then a command named `push`: no push happens.
        "git\npush origin",
        "kubectl -n\napply -f x.yaml",
        "curl https://example.com\n-d @p",
        "gcloud\ndelete x",
    ] {
        assert!(ocgen::risk::high_impact(cmd).is_none(), "risk: {cmd:?}");
        expect(dir.path(), &bash(cmd), 0);
    }
}

#[test]
fn json_escapes_are_decoded_without_jq() {
    let dir = project();
    for cmd in [
        "git\tpush origin main",
        "terraform\tapply",
        "cargo\tpublish",
        "kubectl\tdelete pod x",
        "git \"push\" origin",
        // A control character hides nothing: it is gated (fail closed).
        "git\u{1}push origin main",
        "echo \u{7}",
    ] {
        expect(dir.path(), &bash(cmd), 2);
    }
    for cmd in [
        // A backslash followed by `n` is not a newline.
        "echo a\\nssh host",
        "printf '%s\\n' \"quoted\"",
        "echo \"a\\\\b\"",
    ] {
        expect(dir.path(), &bash(cmd), 0);
    }
}

/// The approval key is computed byte by byte in every implementation, so an
/// approval for a project under a non-ASCII path is found by the scripts too.
#[cfg(unix)]
#[test]
fn approval_keys_match_for_non_ascii_paths() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("dév-проект");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    gated().scaffold(&repo, false).unwrap();
    let home = home_of(&repo);
    let push = bash("git push origin main");
    let utf8 = [
        ("TEAM_APPROVAL_GATE", "1"),
        ("LC_ALL", "en_US.UTF-8"),
        ("LANG", "en_US.UTF-8"),
    ];
    approval::grant(&home, &repo, 10).unwrap();
    for imp in ALL {
        let o = run(imp, &repo, "team-approval-gate", &utf8, &push);
        assert_eq!(o.0, 0, "{imp:?} must find the approval: {}", o.2);
    }
    let pre_push = Command::new("sh")
        .arg(repo.join(".git/hooks/pre-push"))
        .current_dir(&repo)
        .env("HOME", &home)
        .env("CLAUDECODE", "1")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LANG", "en_US.UTF-8")
        .output()
        .unwrap();
    assert_eq!(
        pre_push.status.code(),
        Some(0),
        "pre-push must find the approval: {}",
        String::from_utf8_lossy(&pre_push.stderr)
    );
}

// ------------------------------------------------------------------- A2 --

#[test]
fn common_publish_and_deploy_phrasings_are_gated() {
    let dir = project();
    let gated = [
        "pnpm -r publish",
        "pnpm --filter web publish",
        "npm --workspace pkg publish",
        "npm -w pkg publish --access public",
        "yarn --cwd pkg publish",
        "bun publish",
        "cargo +nightly publish",
        "cargo -Z unstable-options publish",
        "cargo --locked publish",
        "docker image push org/app:1",
        "docker compose push",
        "docker compose -f prod.yml push",
        "docker buildx build --push -t org/app .",
        "docker build --push .",
        "pulumi up --yes",
        "pulumi -C infra destroy",
        "kubectl rollout restart deploy/x",
        "kubectl -n prod rollout undo deploy/x",
        "kubectl set image deploy/x app=org/app:2",
        "gh workflow run deploy.yml",
        "curl --json {} https://api.example.com/x",
        "curl -T build.tgz https://uploads.example.com/",
        "curl --upload-file build.tgz https://uploads.example.com/",
        "curl --data-binary @payload.json https://api.example.com/deploy",
        "curl --data-raw x=1 https://api.example.com/x",
        "curl --data-urlencode q=1 https://api.example.com/x",
        "curl -F file=@x https://api.example.com/x",
        "curl --form file=@x https://api.example.com/x",
        "curl -sd @p https://api.example.com/x",
        "curl --request=PUT https://api.example.com/x",
        "git -c alias.p=push p origin main",
        "git -c \"alias.p=push --no-verify\" p origin main",
        "git config alias.pp push && git pp",
        "git push;echo done",
        "git push&&echo done",
        "echo $(git push)",
        "terraform apply|tee log",
        "yarn workspace web publish",
        "curl -s \"https://api.example.com/x?a=1&b=2\" -d @p",
        "curl -H \"Content-Type: application/json; charset=utf-8\" -d @body https://x",
    ];
    for cmd in gated {
        assert!(
            ocgen::risk::high_impact(&ocgen::risk::normalize(cmd)).is_some(),
            "should be high-impact: {cmd}"
        );
        expect(dir.path(), &bash(cmd), 2);
    }
    let ordinary = [
        "cargo test",
        "cargo +nightly test",
        "cargo build --release",
        "npm test",
        "pnpm -r test",
        "npm run build",
        "docker build -t org/app .",
        "docker image ls",
        "docker compose up -d",
        "kubectl get pods",
        "kubectl rollout status deploy/x",
        "curl https://example.com",
        "curl -fsSL https://example.com/install.sh -o install.sh",
        "curl -sS -D - https://example.com",
        "git status",
        "git log --grep=push",
        "git -c color.ui=false log",
        "gh workflow list",
        "gh run view 42",
        "pulumi preview",
        // What follows curl's own options isn't a curl option: a `-d`/`-T` there
        // belongs to the next command of the pipeline or list.
        "curl -s https://api.github.com/repos/o/r/releases/latest | grep tag_name | cut -d: -f2",
        "curl -sS https://example.com/a.tgz | tar -xz && ls -ld a",
        "curl -LO https://example.com/a.zip && unzip a.zip -d out",
        "curl -s https://example.com/list | sort -T /tmp",
    ];
    for cmd in ordinary {
        assert!(
            ocgen::risk::high_impact(&ocgen::risk::normalize(cmd)).is_none(),
            "should NOT be high-impact: {cmd}"
        );
        expect(dir.path(), &bash(cmd), 0);
    }
}

#[test]
fn ask_rules_cover_the_same_phrasings() {
    let ask = ocgen::claude::HIGH_IMPACT_ASK;
    for want in [
        "Bash(pnpm * publish*)",
        "Bash(npm * publish*)",
        "Bash(cargo * publish*)",
        "Bash(docker * push*)",
        "Bash(docker *--push*)",
        "Bash(pulumi up*)",
        "Bash(pulumi destroy*)",
        "Bash(kubectl rollout restart*)",
        "Bash(kubectl rollout undo*)",
        "Bash(kubectl set *)",
        "Bash(gh workflow run*)",
        "Bash(curl *--json*)",
        "Bash(curl *--data*)",
        "Bash(curl *--upload-file*)",
        "Bash(curl *-T *)",
        "Bash(curl *--form*)",
        "Bash(curl *-F *)",
        "Bash(git -c alias*)",
    ] {
        assert!(ask.contains(&want), "missing ask rule {want}");
    }
    for rule in ask {
        assert!(
            ocgen::validate::permission_rule(rule).is_ok(),
            "invalid rule {rule}"
        );
    }
}

// ------------------------------------------------------------------- A5 --

fn pre_push(repo: &Path, home: &Path, under_claude: bool) -> (i32, String) {
    let mut cmd = Command::new("sh");
    cmd.arg(repo.join(".git/hooks/pre-push"))
        .current_dir(repo)
        .env("HOME", home)
        .env_remove("CLAUDECODE");
    if under_claude {
        cmd.env("CLAUDECODE", "1");
    }
    let o = cmd.output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn pre_push_gates_projects_below_the_repository_root() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    let api = repo.path().join("services/api");
    fs::create_dir_all(&api).unwrap();
    gated().scaffold(&api, false).unwrap();

    let hook = fs::read_to_string(repo.path().join(".git/hooks/pre-push")).unwrap();
    assert!(hook.contains("ocgen:pre-push"), "{hook}");
    let api_root = ocgen::paths::for_shell(&fs::canonicalize(&api).unwrap());
    assert!(
        hook.lines()
            .any(|l| l == format!("# ocgen:root {api_root}")),
        "the gated project is recorded: {hook}"
    );

    assert_eq!(pre_push(repo.path(), home.path(), false).0, 0, "a human");
    let (code, err) = pre_push(repo.path(), home.path(), true);
    assert_eq!(code, 1, "an agent's push from a monorepo needs approval");
    assert!(err.contains("ocgen approve"), "{err}");
    assert!(!err.contains("approvals/"), "{err}");
    approval::grant(home.path(), &api, 10).unwrap();
    assert_eq!(pre_push(repo.path(), home.path(), true).0, 0, "approved");
    approval::revoke(home.path(), &api).unwrap();

    // A second gated project in the same repository is added, not swapped in.
    let web = repo.path().join("web");
    fs::create_dir_all(&web).unwrap();
    gated().scaffold(&web, false).unwrap();
    let hook = fs::read_to_string(repo.path().join(".git/hooks/pre-push")).unwrap();
    let web_root = ocgen::paths::for_shell(&fs::canonicalize(&web).unwrap());
    for root in [&api_root, &web_root] {
        assert!(
            hook.lines().any(|l| l == format!("# ocgen:root {root}")),
            "{root} recorded: {hook}"
        );
    }
    assert_eq!(
        hook.matches(&format!("# ocgen:root {api_root}\n")).count(),
        1,
        "no duplicates: {hook}"
    );
    // Turning the gate off in one project leaves the other gated.
    fs::write(api.join(".claude/settings.json"), "{}\n").unwrap();
    assert_eq!(pre_push(repo.path(), home.path(), true).0, 1);
    fs::write(web.join(".claude/settings.json"), "{}\n").unwrap();
    assert_eq!(pre_push(repo.path(), home.path(), true).0, 0, "none gated");
}

#[test]
fn pre_push_is_honest_and_keeps_a_users_edits() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    gated().scaffold(repo.path(), false).unwrap();
    let path = repo.path().join(".git/hooks/pre-push");
    let hook = fs::read_to_string(&path).unwrap();
    // It says what it is: a backstop that `--no-verify` skips.
    assert!(hook.contains("--no-verify"), "{hook}");
    assert!(hook.contains("backstop"), "{hook}");

    // An edited ocgen hook is backed up before it is rewritten.
    let edited = format!("{hook}echo my-extra-check\n");
    fs::write(&path, &edited).unwrap();
    gated().install_pre_push(repo.path()).unwrap();
    assert_eq!(
        fs::read_to_string(repo.path().join(".git/hooks/pre-push.ocgen-bak")).unwrap(),
        edited
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), hook);
    // An unedited one is just refreshed (no backup churn).
    fs::remove_file(repo.path().join(".git/hooks/pre-push.ocgen-bak")).unwrap();
    gated().install_pre_push(repo.path()).unwrap();
    assert!(!repo.path().join(".git/hooks/pre-push.ocgen-bak").exists());
    // Someone else's hook is never touched, even one that isn't text.
    let theirs = b"\x7fELF\x02\x01\xff\xfe not ocgen".to_vec();
    fs::write(&path, &theirs).unwrap();
    let r = gated().install_pre_push(repo.path()).unwrap();
    assert!(matches!(r, ocgen::render::PrePush::Foreign(_)), "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), theirs);
}

// ------------------------------------------------------------- A4 / D1 --

#[test]
fn hook_scripts_come_from_the_binary_not_the_override_dir() {
    for path in [
        "claude/hooks/team-approval-gate.sh",
        "claude/hooks/git-pre-push.sh",
        "claude/hooks/https-only-fetch.sh",
        "claude/statusline.sh",
    ] {
        assert!(!ocgen::templates::is_overridable(path), "{path}");
        assert_eq!(
            ocgen::templates::load(path).unwrap(),
            ocgen::templates::load_embedded(path).unwrap()
        );
    }
    assert!(ocgen::templates::is_overridable("seeds.toml"));
    assert!(ocgen::templates::is_overridable("claude/agent.md.j2"));
    assert!(!ocgen::templates::editable_paths()
        .iter()
        .any(|p| p.starts_with("claude/hooks/")));
}

// Redirects the config dir via HOME, which only `dirs` honors on Unix.
#[cfg(unix)]
#[test]
fn templates_init_skips_hooks_and_existing_overrides() {
    let home = tempfile::tempdir().unwrap();
    let ocgen = || {
        let mut c = assert_cmd::Command::cargo_bin("ocgen").unwrap();
        c.env("HOME", home.path());
        c
    };
    let over = home.path().join(".config/ocgen/templates");
    fs::create_dir_all(over.join("claude/hooks")).unwrap();
    let mine = format!(
        "{}\n# mine\n",
        ocgen::templates::load_embedded("seeds.toml").unwrap()
    );
    fs::write(over.join("seeds.toml"), &mine).unwrap();
    // A stale copy of a hook script from an older `templates init`.
    fs::write(over.join("claude/hooks/team-approval-gate.sh"), "exit 0\n").unwrap();

    let out = ocgen().args(["templates", "init"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("seeds.toml"),
        "names what it skipped: {stdout}"
    );
    assert!(stdout.contains("--force"), "{stdout}");
    assert_eq!(fs::read_to_string(over.join("seeds.toml")).unwrap(), mine);
    assert!(over.join("manifest.toml").is_file());
    assert!(!over.join("claude/hooks/git-pre-push.sh").exists());
    assert!(!over.join("claude/statusline.sh").exists());

    let out = ocgen()
        .args(["templates", "init", "--force"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(over.join("seeds.toml")).unwrap(),
        ocgen::templates::load_embedded("seeds.toml").unwrap()
    );
    assert!(!over.join("claude/hooks/git-pre-push.sh").exists());

    let out = ocgen().args(["templates", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("claude/hooks/team-approval-gate.sh"))
        .unwrap_or_else(|| panic!("listed: {stdout}"));
    assert!(line.contains("ignored"), "{line}");

    // …and `templates edit` says why it won't take one.
    let out = ocgen()
        .args(["templates", "edit", "claude/hooks/team-approval-gate.sh"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ocgen binary"), "{stderr}");

    // Regenerating writes the embedded script, whatever the override dir holds.
    let proj = tempfile::tempdir().unwrap();
    git(proj.path(), &["init", "-q"]);
    gated().scaffold(proj.path(), false).unwrap();
    let doctor = ocgen().arg("doctor").arg(proj.path()).output().unwrap();
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert_eq!(
        fs::read_to_string(proj.path().join(".claude/hooks/team-approval-gate.sh")).unwrap(),
        ocgen::templates::load_embedded("claude/hooks/team-approval-gate.sh").unwrap()
    );
}

// ---------------------------------------------------------- config-audit --

const AUDIT: &[(&str, &str)] = &[
    ("TEAM_APPROVAL_GATE", "1"),
    ("OCGEN_WEBFETCH_DOMAINS", "docs.rs"),
];

/// Write `content` (or delete the file for `None`) at `rel` in the project, then
/// run the ConfigChange hook for it every way; all must exit `code`.
fn audit(dir: &Path, source: &str, rel: &str, content: Option<&str>, code: i32) -> Vec<Out> {
    let file = dir.join(rel);
    match content {
        Some(c) => {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, c).unwrap()
        }
        None => {
            let _ = fs::remove_file(&file);
        }
    }
    let payload = json!({
        "session_id": "s", "hook_event_name": "ConfigChange", "source": source,
        "file_path": ocgen::paths::for_shell(&file),
    })
    .to_string();
    let outs: Vec<Out> = ALL
        .iter()
        .map(|i| run(*i, dir, "config-audit", AUDIT, &payload))
        .collect();
    for (imp, o) in ALL.iter().zip(&outs) {
        assert_eq!(
            o.0, code,
            "{imp:?} exit {} (want {code}) for {source} {rel}: {content:?}\nstderr: {}",
            o.0, o.2
        );
    }
    let first: Vec<Option<&str>> = outs.iter().map(|o| o.2.lines().next()).collect();
    assert!(
        first.windows(2).all(|w| w[0] == w[1]),
        "same message: {first:?}"
    );
    outs
}

#[test]
fn config_audit_blocks_changes_that_weaken_a_gate() {
    let dir = project();
    let d = dir.path();
    let settings = fs::read_to_string(d.join(".claude/settings.json")).unwrap();
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&settings).unwrap();
        f(&mut v);
        serde_json::to_string_pretty(&v).unwrap()
    };
    let proj = "project_settings";
    let local = "local_settings";
    let s = ".claude/settings.json";
    let l = ".claude/settings.local.json";

    // Unchanged, or stricter: allowed (and logged).
    audit(d, proj, s, Some(&settings), 0);
    audit(
        d,
        proj,
        s,
        Some(&edit(&|v| v["env"]["OCGEN_WEBFETCH_DOMAINS"] = json!(""))),
        0,
    );
    audit(
        d,
        local,
        l,
        Some(r#"{"permissions":{"allow":["Bash(npm test)"]}}"#),
        0,
    );
    audit(d, "skills", ".claude/skills/x/SKILL.md", Some("x"), 0);
    let log = fs::read_to_string(d.join(".claude/audit/config-changes.log")).unwrap();
    assert!(log.contains("project_settings"), "{log}");

    // Weaker: blocked, with a message that says why.
    for (source, rel, content) in [
        (
            proj,
            s,
            edit(&|v| v["env"]["TEAM_APPROVAL_GATE"] = json!("0")),
        ),
        (
            proj,
            s,
            edit(&|v| {
                v["env"]
                    .as_object_mut()
                    .unwrap()
                    .remove("TEAM_APPROVAL_GATE");
            }),
        ),
        (
            proj,
            s,
            edit(&|v| {
                let pre = v["hooks"]["PreToolUse"].as_array_mut().unwrap();
                pre.retain(|g| !g.to_string().contains("team-approval-gate"));
            }),
        ),
        (
            proj,
            s,
            edit(&|v| v["env"]["OCGEN_WEBFETCH_DOMAINS"] = json!("docs.rs evil.com")),
        ),
        (
            proj,
            s,
            edit(&|v| {
                v["hooks"].as_object_mut().unwrap().remove("ConfigChange");
            }),
        ),
        (proj, s, edit(&|v| v["disableAllHooks"] = json!(true))),
        (local, l, r#"{"disableAllHooks": true}"#.to_string()),
        (
            local,
            l,
            r#"{"env": {"TEAM_APPROVAL_GATE": "0"}}"#.to_string(),
        ),
        (
            "user_settings",
            ".home-settings.json",
            r#"{"env":{"OCGEN_WEBFETCH_DOMAINS":"*.com"}}"#.to_string(),
        ),
    ] {
        let outs = audit(d, source, rel, Some(&content), 2);
        assert!(
            outs[0].2.contains("weaken") && outs[0].2.contains("restart"),
            "{}",
            outs[0].2
        );
    }
    // Deleting the project's settings drops every gate.
    audit(d, proj, s, None, 2);
    fs::write(d.join(s), &settings).unwrap();
    let log = fs::read_to_string(d.join(".claude/audit/config-changes.log")).unwrap();
    assert!(log.contains("blocked"), "{log}");
}

#[test]
fn config_audit_only_logs_when_no_gate_is_on() {
    let dir = project();
    let payload = json!({ "source": "local_settings", "file_path": ".claude/settings.local.json" })
        .to_string();
    fs::write(
        dir.path().join(".claude/settings.local.json"),
        r#"{"disableAllHooks": true}"#,
    )
    .unwrap();
    for imp in ALL {
        let o = run(imp, dir.path(), "config-audit", &[], &payload);
        assert_eq!(o.0, 0, "{imp:?}: {}", o.2);
    }
}
