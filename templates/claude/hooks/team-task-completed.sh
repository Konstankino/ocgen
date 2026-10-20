#!/bin/sh
# Claude Code Agent Teams — TaskCompleted hook (task gate).
# A task may only be completed when:
#   1. the project's check command passes (OCGEN_CHECK_CMD, e.g. `cargo test`) — or
#      the task's own narrower check, a `Check: <project check> <args>` line in its
#      description; and
#   2. the teammate states a sufficient confidence (TEAM_CONFIDENCE_THRESHOLD=N).
# Either can be off (unset / 0). Both are set in .claude/settings.json.
# The event carries only the lead-written task (subject, description), which is
# never scored. The teammate's own words are the last assistant message in its
# transcript; failing that, .claude/team/confidence/<team-name>/<task-id>.txt,
# removed once the task passes.
# Teammates share one directory, so checks run one at a time (a lock under
# .claude/team/), and a failure may be another teammate's unfinished edit.
#   exit 0 -> allow completion.   exit 2 -> block and send feedback on stderr.
# Character classes behave as in the Rust hook (`ocgen hook`) in any locale; the
# check itself runs in the caller's locale, as the Rust hook runs it.
lc_was=${LC_ALL-ocgen-unset}
LC_ALL=C
export LC_ALL
thr="${TEAM_CONFIDENCE_THRESHOLD:-0}"
case "$thr" in '' | *[!0-9]*) thr=0 ;; esac
check="${OCGEN_CHECK_CMD:-}"
[ "$thr" -gt 0 ] || [ -n "$check" ] || exit 0
lim="${OCGEN_CHECK_TIMEOUT:-300}"
case "$lim" in '' | *[!0-9]*) lim=300 ;; esac
# A minute short of this hook's own timeout (hooks::CHECK_TIMEOUT_MAX): Claude
# Code lets the change through when it times a gate out.
[ "$lim" -le 3540 ] || lim=3540

payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"
probe=
[ "${OCGEN_HOOK_PROBE:-}" != 1 ] || probe=1 # `ocgen verify`: decide, change nothing
jq=
[ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1 && jq=1

# json_unescape -> decode JSON string escapes (\n, \", \\, \uXXXX, …) on stdin.
json_unescape() {
    awk '
    function hex(s,   i, c, n) {
        n = 0
        for (i = 1; i <= 4; i++) {
            c = index("0123456789abcdef", tolower(substr(s, i, 1)))
            if (c == 0) return -1
            n = n * 16 + c - 1
        }
        return n
    }
    function utf8(n) {
        if (n < 128) return sprintf("%c", n)
        if (n < 2048) return sprintf("%c%c", 192 + int(n / 64), 128 + n % 64)
        return sprintf("%c%c%c", 224 + int(n / 4096), 128 + int(n / 64) % 64, 128 + n % 64)
    }
    {
        s = $0; o = ""
        while ((i = index(s, "\\")) > 0) {
            o = o substr(s, 1, i - 1); e = substr(s, i + 1, 1); s = substr(s, i + 2)
            if (e == "n") o = o "\n"
            else if (e == "t") o = o "\t"
            else if (e == "r") o = o "\r"
            else if (e == "u") { v = hex(s); if (v > 0) o = o utf8(v); if (v >= 0) s = substr(s, 5) }
            else if (e != "b" && e != "f") o = o e
        }
        printf "%s%s", (NR > 1 ? "\n" : ""), o s
    }'
}

# json_str <name> -> the event's top-level string field <name>, decoded (jq, or a
# best-effort grep where jq is missing).
json_str() {
    if [ -n "$jq" ]; then
        printf '%s' "$payload" | jq -r --arg k "$1" '.[$k] // empty | strings' 2>/dev/null
    else
        printf '%s' "$payload" | grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"([^\"\\\\]|\\\\.)*\"" | head -n 1 |
            sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//' | json_unescape
    fi
}

# safe <text> -> a filename-safe token
safe() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_' | cut -c1-80; }

# conf_of < text -> the confidence it states: the last line that starts with
# "Confidence" and a percentage ("Confidence: 97%", "- **Confidence** — 97%",
# "_Confidence_: 97%"), 0-100 — only markup and punctuation around the word.
conf_of() {
    grep -iE '^[^0-9A-Za-z]*confidence[^0-9A-Za-z]{0,8}[0-9]{1,3}[[:blank:]]*%' | tail -n 1 |
        grep -oiE '^[^0-9A-Za-z]*confidence[^0-9A-Za-z]{0,8}[0-9]{1,3}' | grep -oE '[0-9]+$' |
        sed 's/^0*\([0-9]\)/\1/' | { read -r n && [ "$n" -le 100 ] && echo "$n"; }
}

# last_text <transcript> -> the text of its last assistant message (Claude Code
# writes one content block per JSONL line).
last_text() {
    [ -f "$1" ] || return 0
    if [ -n "$jq" ]; then
        jq -rR 'fromjson? | select(type == "object" and .type == "assistant")
            | [.message.content[]? | select(type == "object" and .type == "text") | .text | strings]
            | select(length > 0) | join("\n") | @json' "$1" 2>/dev/null | tail -n 1 | jq -r . 2>/dev/null
    else
        grep -F '"type":"assistant"' "$1" | grep -F '"type":"text","text":"' | tail -n 1 |
            grep -oE '"type":"text","text":"([^"\\]|\\.)*"' |
            sed -E 's/^"type":"text","text":"//; s/"$//' | json_unescape
    fi
}

# caller_locale -> restore the LC_ALL this hook was started with
caller_locale() {
    if [ "$lc_was" = ocgen-unset ]; then unset LC_ALL; else LC_ALL=$lc_was; fi
}

# kill_tree <pid> -> kill a process and its descendants (children first, so none
# is orphaned before it is found), where `ps` can list parents.
kill_tree() {
    for c in $(ps -A -o pid= -o ppid= 2>/dev/null | awk -v p="$1" '$2 == p { print $1 }'); do
        kill_tree "$c"
    done
    kill -KILL "$1" 2>/dev/null
}

# --- sandbox (the same in format.sh, subagent-confidence-gate.sh and
# team-task-completed.sh) ---
# With the project's sandbox on (OCGEN_SANDBOX=1), the project code a hook starts
# itself — the check, the formatter — runs in ocgen's own sandbox, which denies
# whatever the hook env's lists don't grant. Only the ocgen binary can apply it,
# so this script never runs that code, sandboxed or not: the hook command uses
# the binary whenever it is installed.

# sb_ready -> true when a project command may run (the sandbox is off), else
# sb_err says why not.
sb_ready() {
    sb_err="the project's sandbox is on, and only the ocgen binary can run this command in it — install the ocgen that generated this project so the hooks use it; ocgen never runs it unsandboxed"
    [ "${OCGEN_SANDBOX:-}" != 1 ]
}
# --- end sandbox ---

# run_check <dir> <command> <seconds> -> sets rc (the exit code, "timeout after
# <OCGEN_CHECK_TIMEOUT>s", or "not run" when the project's sandbox is on: only
# the ocgen binary runs it then) and out (the last 15 lines of output, or why not
# run). The check runs in its own process group where the system allows
# (setsid, or job control), and a
# watchdog kills the whole group — else its process tree — when its time is up.
# Output goes to a file, so a process the check leaves running can't hold the
# hook open; the jobs are run from a subshell whose own notices ("Killed") go
# nowhere, and its exit code comes back in a file.
run_check() {
    if ! sb_ready; then
        rc="not run"
        out=$sb_err
        return
    fi
    log=$(mktemp 2>/dev/null) || log="${TMPDIR:-/tmp}/ocgen-check-$$"
    : >"$log"
    (
        if command -v setsid >/dev/null; then
            (cd "$1" && caller_locale && exec setsid sh -c "$2") </dev/null >"$log" 2>&1 &
        else
            set -m
            (cd "$1" && caller_locale && exec sh -c "$2") </dev/null >"$log" 2>&1 &
            set +m
        fi
        pid=$!
        (
            trap 'kill "$s"; exit 0' TERM
            sleep "$3" &
            s=$!
            wait "$s"
            : >"$log.timeout"
            w=$(cat "/proc/$pid/winpid") && taskkill //F //T //PID "$w" >/dev/null
            kill -KILL -"$pid" || kill_tree "$pid"
        ) </dev/null >/dev/null &
        dog=$!
        wait "$pid"
        echo "$?" >"$log.rc"
        kill "$dog"
        wait "$dog"
    ) 2>/dev/null
    rc=$(cat "$log.rc" 2>/dev/null)
    [ -n "$rc" ] || rc="could not run"
    [ ! -f "$log.timeout" ] || rc="timeout after ${lim}s"
    out=$(tail -n 15 "$log")
    rm -f "$log" "$log.rc" "$log.timeout"
}

# locked_check -> run_check in the shared project directory, one teammate at a
# time: a mkdir lock, taken over once older than the timeout plus a minute (or,
# never stamped, after a few seconds). Git never sees it. Waiting for it counts
# against the same timeout as the check.
locked_check() {
    lock="$proj/.claude/team/check.lock"
    start=$(date +%s)
    held=
    unstamped=
    if mkdir -p "$proj/.claude/team" 2>/dev/null; then
        while :; do
            if mkdir "$lock" 2>/dev/null; then
                held=1
                printf '*\n' >"$lock/.gitignore" # runtime state, not a change to anyone's tree
                date +%s >"$lock/at"
                break
            fi
            [ -d "$lock" ] || break # can't lock here: run unlocked
            at=$(cat "$lock/at" 2>/dev/null)
            now=$(date +%s)
            case "$at" in
            '' | *[!0-9]*)
                # Its holder stamps it at once, so it died first.
                [ -n "$unstamped" ] || unstamped=$now
                if [ $((now - unstamped)) -ge 3 ]; then
                    rm -rf "$lock"
                    unstamped=
                    continue
                fi
                ;;
            *)
                unstamped=
                if [ $((now - at)) -gt $((lim + 60)) ]; then
                    rm -rf "$lock"
                    continue
                fi
                ;;
            esac
            if [ $((now - start)) -ge "$lim" ]; then
                rc="timeout after ${lim}s"
                out="Another teammate's check held .claude/team/check.lock the whole time."
                return
            fi
            sleep 1
        done
    fi
    left=$((lim - ($(date +%s) - start)))
    [ "$left" -gt 0 ] || left=0
    run_check "$proj" "$check" "$left"
    [ -z "$held" ] || rm -rf "$lock"
}

tid=$(printf '%s' "$payload" | grep -oiE '"(task_?id|id)"[[:space:]]*:[[:space:]]*"[^"]+"' | head -n 1 | grep -oE '"[^"]+"$' | tr -d '"')
sid=$(json_str session_id)
mate=$(json_str teammate_name)
named=$(json_str team_name)
# The event's team; without team_name (Claude Code marks it deprecated), the team
# Claude Code makes for the session — the one every in-process teammate's meta
# names: session-<first 8 of its id>.
team=$named
[ -n "$team" ] || [ -z "$sid" ] || team="session-$(printf '%s' "$sid" | cut -c1-8)"
key=$(safe "$team")
[ -n "$key" ] || key=team
marker_rel=".claude/team/confidence/$key/$(if [ -n "$tid" ]; then safe "$tid"; else echo '<task-id>'; fi).txt"
marker="$proj/$marker_rel"

# meta_says <meta.json> -> "own" when it names this teammate in this team, "team"
# when it names any other teammate (of this team or another: only a lead has
# teammates), else nothing (a plain subagent's meta names no team).
meta_says() {
    if [ -n "$jq" ]; then
        jq -r --arg n "$mate" --arg t "$team" \
            'select(.teamName | strings | length > 0)
            | if .teamName == $t and .name == $n then "own" else "team" end' "$1" 2>/dev/null
    elif grep -qF "\"teamName\":\"$team\"" "$1"; then
        if grep -qF "\"name\":\"$mate\"" "$1"; then echo own; else echo team; fi
    elif grep -qE '"teamName":"[^"]' "$1"; then
        echo team
    fi
}

# (a) The teammate's own words. An in-process teammate shares the lead's session,
# so transcript_path is the lead's; Claude Code files the teammate's own
# transcript beside it (<transcript>/subagents/, or a folder below) as
# agent-<agent id>.jsonl, with an agent-<agent id>.meta.json naming the teammate
# and its team — the newest one wins. A teammate in its own session writes
# transcript_path itself. The lead's transcript never speaks for a teammate: not
# when the event names the lead session's implicit team (session-<first 8 of its
# session id>), nor when any teammates are filed beside it.
score=
tp=$(json_str transcript_path)
case "$tp" in
[A-Za-z]:\\*) tp=$(printf '%s' "$tp" | tr '\\' '/') ;;
esac
if [ "$thr" -gt 0 ] && [ -n "$tp" ]; then
    src=
    lead=
    if [ -n "$mate" ] && [ -n "$team" ]; then
        for m in "${tp%.jsonl}"/subagents/*.meta.json "${tp%.jsonl}"/subagents/*/*.meta.json; do
            [ -f "$m" ] || continue
            case "$(meta_says "$m")" in
            own)
                f="${m%.meta.json}.jsonl"
                if [ ! -f "$f" ]; then
                    lead=1
                elif [ -z "$src" ] || [ "$f" -nt "$src" ]; then
                    src=$f
                fi
                ;;
            team) lead=1 ;;
            esac
        done
    fi
    if [ -z "$src" ] && [ -z "$lead" ] && { [ -z "$named" ] || [ "$named" != "session-$(printf '%s' "$sid" | cut -c1-8)" ]; }; then
        src=$tp
    fi
    if [ -n "$src" ]; then
        # Claude Code appends transcript lines on a timer, so the final message
        # may land just after this hook starts.
        ms="${OCGEN_TRANSCRIPT_SETTLE_MS:-300}"
        case "$ms" in '' | *[!0-9]*) ms=300 ;; esac
        [ "$ms" -eq 0 ] || sleep "$(awk -v m="$ms" 'BEGIN { printf "%.3f", m / 1000 }')" 2>/dev/null || sleep 1
        words=$(last_text "$src")
        score=$(printf '%s\n' "$words" | conf_of)
    fi
fi

# (b) Fall back to the marker for THIS team and task: its first integer.
if [ "$thr" -gt 0 ] && [ -z "$score" ] && [ -n "$tid" ] && [ -f "$marker" ]; then
    score=$(grep -oE '[0-9]+' "$marker" | head -n 1)
    case "$score" in
    [0-9] | [0-9][0-9] | [0-9][0-9][0-9]) score=$(printf '%s\n' "$score" | sed 's/^0*\([0-9]\)/\1/') ;;
    *) score= ;;
    esac
    [ -z "$score" ] || [ "$score" -le 100 ] || score=
fi

# A task may narrow the project check — never replace it: the check runs outside
# the agent's permissions, so only the project check plus plain arguments counts.
if [ -n "$check" ]; then
    line=$(json_str task_description | sed -n 's/^[[:blank:]]*Check:[[:blank:]]*//p' | head -n 1 | sed 's/[[:space:]]*$//')
    case "$line" in
    "$check "*)
        [ -n "$(printf '%s' "${line#"$check "}" | tr -d 'A-Za-z0-9_./:=,@+% -')" ] || check=$line
        ;;
    esac
fi

# Loop guard (shared): caps how often this gate may block the same task.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"
# Budgets are per task, so parallel completions never share a counter.
lg_on && lg_key "$payload" task-confidence "${tid:-task}"

# block <reason> [<score>] -> refuse completion, unless the loop guard says this
# task is stuck: then allow it, marked UNRESOLVED, and escalate to a human.
block() {
    if lg_on; then
        n=$(lg_bump)
        stalled=0
        [ -n "${2:-}" ] && lg_stalled "$2" && stalled=1
        if [ "$n" -ge "$LG_MAX" ] || [ "$stalled" = 1 ]; then
            why="blocked ${n}x"
            [ "$stalled" = 1 ] && why="confidence stopped rising (${n} blocks)"
            lg_escalate "${why}; $1"
            lg_release "UNRESOLVED: task ${LG_SUBJECT} completed by the loop guard (${why}; $1). Needs review; see .claude/loop-guard/escalations.md"
        fi
        echo "Loop guard: block ${n} of ${LG_MAX}. If a different approach won't get you there," >&2
        echo "say what blocks you instead of retrying the same thing." >&2
    fi
    exit 2
}

# 1. The objective check, in the shared project directory.
if [ -n "$check" ]; then
    locked_check
    if [ "$rc" != 0 ]; then
        echo "Check failed: \`$check\` (exit $rc). The task isn't complete until it passes — a" >&2
        echo "stated confidence doesn't override a failing check. Teammates share this directory," >&2
        echo "so the failure may come from another teammate's unfinished edits: if it isn't yours," >&2
        echo "say so instead of editing their files. Last output:" >&2
        printf '%s\n' "$out" >&2
        block "check \`$check\` failing (exit $rc)"
    fi
fi

# 2. The stated confidence.
if [ "$thr" -gt 0 ]; then
    if [ -z "$score" ]; then
        echo "No confidence found for this completion. End your final message with a line" >&2
        echo "'Confidence: NN%' (>= ${thr}%), or write it to ${marker_rel}." >&2
        block "confidence not stated (bar ${thr}%)"
    fi
    if [ "$score" -lt "$thr" ]; then
        echo "Confidence ${score}% is below the required ${thr}%." >&2
        echo "Keep working (or re-scope) until you are >= ${thr}% confident, then re-complete." >&2
        block "confidence ${score}% < ${thr}%" "$score"
    fi
fi
[ -n "$probe" ] || [ -z "$tid" ] || rm -f "$marker"
lg_on && lg_reset
exit 0
