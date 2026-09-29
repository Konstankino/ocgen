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

# (a) Prefer a confidence the teammate stated in this completion ("Confidence: 97%").
score=$(printf '%s' "$payload" | grep -oiE 'confidence[^0-9]{0,4}[0-9]{1,3}' | grep -oE '[0-9]{1,3}' | head -n 1)

# (b) Fall back to a per-task marker keyed by THIS event's task id.
if [ -z "$score" ]; then
    tid=$(printf '%s' "$payload" | grep -oiE '"(task_?id|id)"[[:space:]]*:[[:space:]]*"[^"]+"' | head -n 1 | grep -oE '"[^"]+"$' | tr -d '"')
    [ -n "$tid" ] && score=$(tr -cd '0-9' < "$proj/.claude/team/confidence/$tid.txt" 2>/dev/null)
fi

if [ -z "$score" ]; then
    echo "No confidence found for this completion. State 'Confidence: NN%' in the completion" >&2
    echo "(or write .claude/team/confidence/<task-id>.txt); it must be >= ${thr}%." >&2
    exit 2
fi
if [ "$score" -lt "$thr" ] 2>/dev/null; then
    echo "Confidence ${score}% is below the required ${thr}%." >&2
    echo "Keep working (or re-scope) until you are >= ${thr}% confident, then re-complete." >&2
    exit 2
fi
exit 0
