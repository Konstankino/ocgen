#!/usr/bin/env python3
"""Stand-in for the `claude` binary, for the deliver-bench tests.

Logs every call to the file in BENCH_STUB_LOG, then answers by its arguments:
- `--version`: a version line;
- `--output-format json`: the scripted user, one JSON result: FINISHED once the prompt holds the final
  report, else a REPLY (BENCH_STUB_REPLY replaces it on a call with a spec). `--bare` is refused unless
  ANTHROPIC_API_KEY is set, as the real binary does: bare mode never reads a subscription login;
- a `/deliver` prompt: stream-json ending in a question;
- `--resume`: copies the overlay in BENCH_STUB_OVERLAY into the cwd, then stream-json ending in a
  final report. Every result names a new session id, so a caller has to read it from the result.

BENCH_STUB_MODE selects: error_max_turns (a result with `errors` and no `result`), wrong_model,
no_deliver, never_finishes, hang (emits the init event, then sleeps), user_down (every scripted-user
call fails), user_down_later (only calls with a spec fail, so the bench's check before the first fixture
passes), ask_tool (asks through AskUserQuestion on the first call), other_message_model (assistant
messages name another model), survivor (starts a child that would outlive the call and writes its pid
to BENCH_STUB_SURVIVOR_PID). BENCH_STUB_BARE_FAIL_ONCE names a marker file: the call with a spec that
finds it missing creates it and fails. BENCH_STUB_HANG_IN hangs an agent call whose cwd holds that text.

An option it doesn't know fails the call, as the real binary does. Python rather than the repo's
small sh stand-ins, because it has to emit JSON and copy files.
"""

import json
import os
import shutil
import subprocess
import sys
import time

VALUE_FLAGS = {"--model", "--output-format", "--permission-mode", "--settings", "--resume"}
SWITCHES = {"-p", "--bare", "--verbose", "--version"}

VERSION = "9.9.9 (Claude Code stub)"
QUESTION = "Which rounding rule should the totals use?"
REPLY_TEXT = "use the rule the team agreed on"
FINAL_MARK = "STUB-FINAL-REPORT"
FINAL_REPORT = FINAL_MARK + ": done, all tests pass.\nConfidence: 90%"
ERROR_TEXT = "Reached the maximum number of turns"
DENIAL = {"tool_name": "Bash", "tool_use_id": "stub-tool-1", "tool_input": {"command": "git push"}}
FIRST_SESSION = "stub-session-1"
BARE_NEEDS_KEY = "bare mode needs ANTHROPIC_API_KEY; it never reads a subscription login"
USER_DOWN_TEXT = "service unavailable"
TOOL_QUESTION = "Should matching ignore case?"
MESSAGE_MODEL = "claude-from-the-frontmatter"


def parse(argv):
    """The flags, the positional words and any unknown options in `argv`."""
    flags, words, unknown = {}, [], []
    rest = iter(argv)
    for arg in rest:
        if arg == "--":
            words.extend(rest)
        elif arg in VALUE_FLAGS:
            flags[arg] = next(rest, "")
        elif arg in SWITCHES:
            flags[arg] = True
        elif arg.startswith("-"):
            unknown.append(arg)
        else:
            words.append(arg)
    return flags, words, unknown


def log_call(argv, flags, prompt, stdin_text):
    path = os.environ.get("BENCH_STUB_LOG")
    if not path:
        return
    cwd = os.getcwd()
    parents = [cwd]
    while os.path.dirname(parents[-1]) != parents[-1]:
        parents.append(os.path.dirname(parents[-1]))
    entry = {
        "argv": argv,
        "flags": flags,
        "prompt": prompt,
        "stdin": stdin_text,
        "cwd": cwd,
        "path": os.environ.get("PATH", ""),
        "api_key": bool(os.environ.get("ANTHROPIC_API_KEY")),
        "disable_claude_mds": os.environ.get("CLAUDE_CODE_DISABLE_CLAUDE_MDS"),
        "cwd_entries": sorted(os.listdir(cwd)),
        "in_git_repo": any(os.path.exists(os.path.join(p, ".git")) for p in parents),
    }
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry) + "\n")


def emit(event):
    print(json.dumps(event), flush=True)


def init_event(flags, mode, session):
    model = "claude-other-1" if mode == "wrong_model" else flags.get("--model", "")
    commands, skills = ["compact", "deliver"], ["deliver", "team-plan"]
    if mode == "no_deliver":
        commands, skills = ["compact"], ["team-plan"]
    return {
        "type": "system",
        "subtype": "init",
        "session_id": session,
        "model": model,
        "slash_commands": commands,
        "skills": skills,
        "agents": ["implementer", "reviewer"],
        "permissionMode": flags.get("--permission-mode", ""),
    }


def answer(text, session, denials, flags, mode):
    model = MESSAGE_MODEL if mode == "other_message_model" else flags.get("--model", "")
    emit({
        "type": "assistant",
        "session_id": session,
        "message": {"role": "assistant", "model": model, "content": [{"type": "text", "text": text}]},
    })
    emit({
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "result": text,
        "session_id": session,
        "num_turns": 2,
        "total_cost_usd": 0.01,
        "permission_denials": denials,
    })


def next_session(resumed):
    """The id this call reports, or None when `resumed` isn't one the stub gave out."""
    if resumed is None:
        return FIRST_SESSION
    prefix, _, number = resumed.rpartition("-")
    if prefix != FIRST_SESSION.rpartition("-")[0] or not number.isdigit():
        return None
    return f"{prefix}-{int(number) + 1}"


def agent(flags, prompt, mode):
    resuming = "--resume" in flags
    session = next_session(flags.get("--resume"))
    if session is None:
        print(f"error: no conversation with session id {flags['--resume']}", file=sys.stderr)
        return 1
    if not resuming and not prompt.startswith("/deliver "):
        print(f"error: the first call must start /deliver: {prompt!r}", file=sys.stderr)
        return 2
    print("stub: warming up", flush=True)
    emit({"type": "system", "subtype": "hook_started", "session_id": session})
    emit(init_event(flags, mode, session))
    hang_in = os.environ.get("BENCH_STUB_HANG_IN")
    if mode == "hang" or (hang_in and hang_in in os.getcwd()):
        time.sleep(120)
        return 0
    if mode == "error_max_turns":
        emit({
            "type": "result",
            "subtype": "error_max_turns",
            "is_error": True,
            "session_id": session,
            "num_turns": 50,
            "total_cost_usd": 0.5,
            "permission_denials": [],
            "errors": [ERROR_TEXT],
        })
        return 1
    if mode == "survivor":
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        with open(os.environ["BENCH_STUB_SURVIVOR_PID"], "a", encoding="utf-8") as handle:
            handle.write(f"{child.pid}\n")
    if mode == "ask_tool" and not resuming:
        emit({
            "type": "assistant",
            "session_id": session,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "ask-1",
                    "name": "AskUserQuestion",
                    "input": {"questions": [{"question": TOOL_QUESTION}]},
                }],
            },
        })
    if resuming and mode != "never_finishes":
        overlay = os.environ.get("BENCH_STUB_OVERLAY")
        if overlay:
            shutil.copytree(overlay, os.getcwd(), dirs_exist_ok=True)
        answer(FINAL_REPORT, session, [DENIAL], flags, mode)
    else:
        answer(QUESTION, session, [], flags, mode)
    return 0


def scripted_user(flags, prompt, mode):
    if "--bare" in flags and not os.environ.get("ANTHROPIC_API_KEY"):
        print(f"error: {BARE_NEEDS_KEY}", file=sys.stderr)
        return 1
    has_spec = "Spec:" in prompt
    marker = os.environ.get("BENCH_STUB_BARE_FAIL_ONCE")
    fails_once = has_spec and marker and not os.path.exists(marker)
    if mode == "user_down" or (mode == "user_down_later" and has_spec) or fails_once:
        if fails_once:
            open(marker, "w", encoding="utf-8").close()
        print(f"error: {USER_DOWN_TEXT}", file=sys.stderr)
        return 1
    if mode != "never_finishes" and FINAL_MARK in prompt:
        reply = "FINISHED"
    elif mode == "never_finishes" or not has_spec:
        reply = "REPLY: " + REPLY_TEXT
    else:
        reply = os.environ.get("BENCH_STUB_REPLY", "REPLY: " + REPLY_TEXT)
    print(json.dumps({
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "result": reply,
        "session_id": "stub-user",
        "total_cost_usd": 0.001,
    }))
    return 0


def main():
    argv = sys.argv[1:]
    stdin_text = sys.stdin.read() if sys.stdin is not None and not sys.stdin.isatty() else ""
    flags, words, unknown = parse(argv)
    prompt = "\n".join(part for part in (" ".join(words), stdin_text) if part)
    log_call(argv, flags, prompt, stdin_text)
    if unknown:
        print(f"error: unknown option {unknown[0]}", file=sys.stderr)
        return 2
    if "--version" in flags:
        print(VERSION)
        return 0
    if "-p" not in flags:
        print("error: the stub only answers -p calls", file=sys.stderr)
        return 2
    mode = os.environ.get("BENCH_STUB_MODE", "")
    if "--bare" in flags or flags.get("--output-format") == "json":
        return scripted_user(flags, prompt, mode)
    return agent(flags, prompt, mode)


if __name__ == "__main__":
    sys.exit(main())
