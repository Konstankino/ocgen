#!/bin/sh
# Claude Code — SubagentStart / SubagentStop worker gate.
# A subagent that CHANGED its work tree (e.g. a worktree-isolated /fanout worker)
# may only finish when:
#   1. the project's check command passes (OCGEN_CHECK_CMD, e.g. `cargo test`) —
#      an objective signal, run in the worker's own directory; and
#   2. its final message states a sufficient confidence (SUBAGENT_CONFIDENCE_THRESHOLD):
#      a line like "Confidence: 97%" — the last such line counts.
# Either can be off (unset / 0). Both are set in .claude/settings.json.
# Read-only roles (SUBAGENT_READONLY_ROLES: no Edit/Write tool) are never gated,
# nor are in-process teammates while the team task gate (TEAM_TASK_GATE=1) judges
# them per task at TaskCompleted.
# An in-process teammate stops as a subagent named after itself, so its role is
# read from the .meta.json beside its transcript.
# At SubagentStart the hook records the tree the worker starts from (HEAD, status,
# diff, untracked files' content); at SubagentStop it gates only a worker whose
# tree changed since — so the lead's own uncommitted edits don't count, and a
# worker that committed, or only touched files git doesn't track yet, still does.
# exit 2 -> keep the subagent working (stderr shown to it).
# Character classes behave as in the Rust hook (`ocgen hook`) in any locale; the
# check itself runs in the caller's locale, as the Rust hook runs it.
lc_was=${LC_ALL-ocgen-unset}
LC_ALL=C
export LC_ALL
thr="${SUBAGENT_CONFIDENCE_THRESHOLD:-0}"
case "$thr" in '' | *[!0-9]*) thr=0 ;; esac
check="${OCGEN_CHECK_CMD:-}"
[ "$thr" -gt 0 ] || [ -n "$check" ] || exit 0
lim="${OCGEN_CHECK_TIMEOUT:-300}"
case "$lim" in '' | *[!0-9]*) lim=300 ;; esac

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

# tree_state <dir> -> sets ts_top, ts_head, ts_sum and ts_dirty; fails outside a
# git work tree. ts_sum is one cksum of everything uncommitted: `git status
# --porcelain --untracked-files=all`, the staged and the unstaged diff (both work
# before the first commit), and the blob ids of the untracked, not ignored files
# — regular files it can open only, since `hash-object` stops at the first path
# it can't read (a nested repository, a dangling link, a file nobody may read) —
# so editing a file git doesn't track yet changes it too.
tree_state() {
    ts_top=$(git -C "$1" rev-parse --show-toplevel 2>/dev/null) || return 1
    ts_head=$(git -C "$1" rev-parse --verify -q HEAD 2>/dev/null)
    ts_sum=$({
        git -C "$1" --no-optional-locks status --porcelain --untracked-files=all
        git -C "$1" --no-optional-locks diff --cached --no-ext-diff --no-color
        git -C "$1" --no-optional-locks diff --no-ext-diff --no-color
        git -C "$ts_top" --no-optional-locks ls-files -o --exclude-standard -z | tr '\0' '\n' |
            while IFS= read -r f; do
                # (`true`, not `:`: a special built-in's failed redirection
                # ends a POSIX shell.)
                [ -f "$ts_top/$f" ] && true 2>/dev/null <"$ts_top/$f" && printf '%s\n' "$f"
            done | git -C "$ts_top" hash-object --no-filters --stdin-paths
    } 2>/dev/null | cksum)
    ts_dirty=
    [ -z "$(git -C "$1" --no-optional-locks status --porcelain --untracked-files=all 2>/dev/null)" ] || ts_dirty=1
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

# Where SubagentStart recorded the tree this worker started from (self-ignored).
agent=$(safe "$(json_str agent_id)")
bl=
[ -z "$agent" ] || bl="$proj/.claude/worker-baseline/$agent"
forget() { [ -z "$bl" ] || [ -n "$probe" ] || rm -f "$bl"; }

# Read-only roles can't have changed anything. An in-process teammate stops as a
# subagent named after itself; its role is in the .meta.json beside its
# transcript (customAgentType).
role=$(json_str agent_type | tr 'A-Z' 'a-z')
meta=$(json_str agent_transcript_path)
case "$meta" in
[A-Za-z]:\\*) meta=$(printf '%s' "$meta" | tr '\\' '/') ;;
esac
# meta_str <key> -> a string field of that .meta.json (empty when none).
meta_str() {
    if [ ! -f "$meta" ]; then
        :
    elif [ -n "$jq" ]; then
        jq -r --arg k "$1" '.[$k] // empty | strings' "$meta" 2>/dev/null
    else
        grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"\\\\]*\"" "$meta" | head -n 1 |
            sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//'
    fi
}
custom=
kind=
case "$meta" in
*.jsonl)
    meta="${meta%.jsonl}.meta.json"
    custom=$(meta_str customAgentType)
    kind=$(meta_str taskKind)
    ;;
*) meta= ;;
esac
custom=$(printf '%s' "$custom" | tr 'A-Z' 'a-z')
for r in ${SUBAGENT_READONLY_ROLES:-}; do
    r=$(printf '%s' "$r" | tr 'A-Z' 'a-z')
    if { [ -n "$role" ] && [ "$role" = "$r" ]; } || { [ -n "$custom" ] && [ "$custom" = "$r" ]; }; then
        forget
        exit 0
    fi
done
# With the team task gate on, TaskCompleted judges a teammate per task; its
# turn ends aren't a worker finishing.
if [ "${TEAM_TASK_GATE:-}" = 1 ] && [ "$kind" = in_process_teammate ]; then
    forget
    exit 0
fi

# The subagent's working directory (its own worktree when isolated); the hook
# input's `cwd` follows the subagent, unlike ${CLAUDE_PROJECT_DIR}.
cwd=$(json_str cwd)
[ -n "$cwd" ] || cwd="$proj"

if [ "$(json_str hook_event_name)" = SubagentStart ]; then
    if [ -n "$bl" ] && [ -z "$probe" ] && tree_state "$cwd" && mkdir -p "${bl%/*}" 2>/dev/null; then
        [ -f "${bl%/*}/.gitignore" ] || printf '*\n' >"${bl%/*}/.gitignore"
        printf '%s\n%s\n%s\n' "$ts_top" "$ts_head" "$ts_sum" >"$bl"
    fi
    exit 0
fi

# Did this worker change its tree? In the same tree: HEAD, status or diff moved.
# In another one (an isolated worktree made after the record): uncommitted
# changes, or a HEAD past the starting commit. Without a record, any uncommitted
# change counts.
changed=
if [ -n "$bl" ] && [ -f "$bl" ] && tree_state "$cwd"; then
    b_head=$(sed -n 2p "$bl")
    if [ "$(sed -n 1p "$bl")" = "$ts_top" ]; then
        { [ "$(sed -n 3p "$bl")" = "$ts_sum" ] && [ "$b_head" = "$ts_head" ]; } || changed=1
    else
        { [ -z "$ts_dirty" ] && [ "$b_head" = "$ts_head" ]; } || changed=1
    fi
elif [ -n "$(git -C "$cwd" --no-optional-locks status --porcelain --untracked-files=all 2>/dev/null)" ]; then
    changed=1
fi
if [ -z "$changed" ]; then
    forget
    exit 0
fi

# Only the worker's own final message counts — not the rest of the event
# (running background tasks, crons).
score=$(json_str last_assistant_message | conf_of)

# Loop guard (shared): caps how often this gate may send the same worker back.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"
if lg_on; then
    worker=$(lg_field "$payload" agent_id)
    [ -n "$worker" ] || worker=$(lg_field "$payload" agent_type)
    lg_key "$payload" subagent-confidence "${worker:-subagent}"
fi

# block <reason> [<score>] -> send the worker back, unless the loop guard says it
# is stuck: then release it with the result marked UNRESOLVED and escalate. A
# score enables "confidence stopped rising" detection.
block() {
    if lg_on; then
        n=$(lg_bump)
        stalled=0
        [ -n "${2:-}" ] && lg_stalled "$2" && stalled=1
        if [ "$n" -ge "$LG_MAX" ] || [ "$stalled" = 1 ]; then
            why="blocked ${n}x"
            [ "$stalled" = 1 ] && why="confidence stopped rising (${n} blocks)"
            lg_escalate "${why}; $1"
            forget
            lg_release "UNRESOLVED: worker ${LG_SUBJECT} stopped by the loop guard (${why}; $1). Review its changes; see .claude/loop-guard/escalations.md"
        fi
        echo "Loop guard: block ${n} of ${LG_MAX}. If a different approach won't get you there," >&2
        echo "say what blocks you instead of retrying the same thing." >&2
    fi
    exit 2
}

# 1. The objective check, in the worker's own directory.
if [ -n "$check" ]; then
    run_check "$cwd" "$check" "$lim"
    if [ "$rc" != 0 ]; then
        echo "Check failed: \`$check\` (exit $rc). Fix it before finishing — a stated confidence" >&2
        echo "doesn't override a failing check. Last output:" >&2
        printf '%s\n' "$out" >&2
        block "check \`$check\` failing (exit $rc)"
    fi
fi

# 2. The stated confidence.
if [ "$thr" -gt 0 ]; then
    if [ -z "$score" ]; then
        echo "You changed files but did not state your confidence. Finish only when you are" >&2
        echo ">= ${thr}% confident, and end your final message with a line 'Confidence: NN%'." >&2
        block "confidence not stated (bar ${thr}%)"
    fi
    if [ "$score" -lt "$thr" ]; then
        echo "Confidence ${score}% is below the required ${thr}%. Keep working (or re-scope)" >&2
        echo "until you are >= ${thr}% confident, then state 'Confidence: NN%' and finish." >&2
        block "confidence ${score}% < ${thr}%" "$score"
    fi
fi
forget
lg_on && lg_reset
exit 0
