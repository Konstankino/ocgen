#!/bin/sh
# Claude Code Agent Teams — TaskCompleted hook (confidence gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_CONFIDENCE_THRESHOLD=N -> a teammate must record confidence >= N (0..100)
#                                  before a task may be completed. 0/unset = off.
# Before completing, a teammate writes its self-assessed confidence (a number) to
# .claude/team/confidence/<task-id>.txt. This hook reads the most recent such file.
#   exit 0 -> allow completion.   exit 2 -> block and send feedback on stderr.
thr="${TEAM_CONFIDENCE_THRESHOLD:-0}"
[ "$thr" -gt 0 ] 2>/dev/null || exit 0

dir="${CLAUDE_PROJECT_DIR:-.}/.claude/team/confidence"
latest=$(ls -t "$dir" 2>/dev/null | head -n 1)
if [ -z "$latest" ]; then
    echo "No confidence recorded. Before completing, write your confidence (0-100) to" >&2
    echo ".claude/team/confidence/<task-id>.txt; it must be >= ${thr}%." >&2
    exit 2
fi

score=$(tr -cd '0-9' < "$dir/$latest")
if [ -z "$score" ] || [ "$score" -lt "$thr" ] 2>/dev/null; then
    echo "Confidence ${score:-unknown}% is below the required ${thr}%." >&2
    echo "Keep working (or re-scope) until you are >= ${thr}% confident, then update" >&2
    echo ".claude/team/confidence/<task-id>.txt." >&2
    exit 2
fi
exit 0
