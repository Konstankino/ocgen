#!/bin/sh
# Claude Code Agent Teams — TaskCompleted hook (task gate).
# A task may only be completed when:
#   1. the project's check command passes (OCGEN_CHECK_CMD, e.g. `cargo test`); and
#   2. the teammate states a sufficient confidence (TEAM_CONFIDENCE_THRESHOLD=N).
# Either can be off (unset / 0). Both are set in .claude/settings.json.
# Parallel-safe: reads the confidence from THIS completion's own stdin event, so
# concurrent completions never read each other's value.
#   exit 0 -> allow completion.   exit 2 -> block and send feedback on stderr.
thr="${TEAM_CONFIDENCE_THRESHOLD:-0}"
case "$thr" in '' | *[!0-9]*) thr=0 ;; esac
check="${OCGEN_CHECK_CMD:-}"
[ "$thr" -gt 0 ] || [ -n "$check" ] || exit 0

payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"

# Loop guard (shared): caps how often this gate may block the same task.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

tid=$(printf '%s' "$payload" | grep -oiE '"(task_?id|id)"[[:space:]]*:[[:space:]]*"[^"]+"' | head -n 1 | grep -oE '"[^"]+"$' | tr -d '"')

# (a) Prefer a confidence the teammate stated in this completion ("Confidence: 97%").
score=$(printf '%s' "$payload" | grep -oiE 'confidence[^0-9]{0,4}[0-9]{1,3}' | grep -oE '[0-9]{1,3}' | head -n 1)

# (b) Fall back to a per-task marker keyed by THIS event's task id.
# (A redirect from a missing file errors in the shell itself, before `tr` runs,
# so check the file exists rather than relying on 2>/dev/null.)
marker="$proj/.claude/team/confidence/$tid.txt"
if [ -z "$score" ] && [ -n "$tid" ] && [ -f "$marker" ]; then
    score=$(tr -cd '0-9' <"$marker")
fi

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

# 1. The objective check, in the project directory.
if [ -n "$check" ]; then
    out=$(cd "$proj" 2>/dev/null && sh -c "$check" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "Check failed: \`$check\` (exit $rc). The task isn't complete until it passes — a" >&2
        echo "stated confidence doesn't override a failing check. Last output:" >&2
        printf '%s\n' "$out" | tail -n 15 >&2
        block "check \`$check\` failing (exit $rc)"
    fi
fi

# 2. The stated confidence.
if [ "$thr" -gt 0 ]; then
    if [ -z "$score" ]; then
        echo "No confidence found for this completion. State 'Confidence: NN%' in the completion" >&2
        echo "(or write .claude/team/confidence/<task-id>.txt); it must be >= ${thr}%." >&2
        block "confidence not stated (bar ${thr}%)"
    fi
    if [ "$score" -lt "$thr" ] 2>/dev/null; then
        echo "Confidence ${score}% is below the required ${thr}%." >&2
        echo "Keep working (or re-scope) until you are >= ${thr}% confident, then re-complete." >&2
        block "confidence ${score}% < ${thr}%" "$score"
    fi
fi
lg_on && lg_reset
exit 0
