#!/bin/sh
# Claude Code Agent Teams — TaskCompleted hook (confidence gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_CONFIDENCE_THRESHOLD=N -> a teammate must be >= N% (0..100) confident before
#                                  a task may be completed. 0/unset = off.
# Parallel-safe: reads the confidence from THIS completion's own stdin event, so
# concurrent completions never read each other's value.
#   exit 0 -> allow completion.   exit 2 -> block and send feedback on stderr.
thr="${TEAM_CONFIDENCE_THRESHOLD:-0}"
[ "$thr" -gt 0 ] 2>/dev/null || exit 0

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
if [ -z "$score" ] && [ -n "$tid" ]; then
    score=$(tr -cd '0-9' < "$proj/.claude/team/confidence/$tid.txt" 2>/dev/null)
fi

# Budgets are per task, so parallel completions never share a counter.
lg_on && lg_key "$payload" task-confidence "${tid:-task}"

# Block, unless the loop guard says this task is stuck: then allow completion
# with the result marked UNRESOLVED and escalate to a human.
block() {
    if lg_on; then
        n=$(lg_bump)
        stalled=0
        lg_stalled "$score" && stalled=1
        if [ "$n" -ge "$LG_MAX" ] || [ "$stalled" = 1 ]; then
            why="blocked ${n}x"
            [ "$stalled" = 1 ] && why="confidence stopped rising (${n} blocks)"
            lg_escalate "${why}; last confidence ${score:-not stated}%, bar ${thr}%"
            lg_release "UNRESOLVED: task ${LG_SUBJECT} completed by the loop guard (${why}, confidence ${score:-not stated}% < ${thr}%). Needs review; see .claude/loop-guard/escalations.md"
        fi
        echo "Loop guard: block ${n} of ${LG_MAX}. If a different approach won't get you there," >&2
        echo "say what blocks you instead of retrying the same thing." >&2
    fi
    exit 2
}

if [ -z "$score" ]; then
    echo "No confidence found for this completion. State 'Confidence: NN%' in the completion" >&2
    echo "(or write .claude/team/confidence/<task-id>.txt); it must be >= ${thr}%." >&2
    block
fi
if [ "$score" -lt "$thr" ] 2>/dev/null; then
    echo "Confidence ${score}% is below the required ${thr}%." >&2
    echo "Keep working (or re-scope) until you are >= ${thr}% confident, then re-complete." >&2
    block
fi
lg_on && lg_reset
exit 0
