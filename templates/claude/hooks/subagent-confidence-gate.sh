#!/bin/sh
# Claude Code — SubagentStop confidence gate.
# Enforces per-worker confidence on subagents that WROTE files (e.g. worktree-
# isolated /fanout workers), pairing isolated writes with an enforced quality bar.
# Driven by SUBAGENT_CONFIDENCE_THRESHOLD (set in .claude/settings.json). Only a
# worker that changed files and did not state a sufficient confidence is blocked;
# read-only workers pass. exit 2 -> keep the subagent working (stderr shown to it).
thr="${SUBAGENT_CONFIDENCE_THRESHOLD:-0}"
[ "$thr" -gt 0 ] 2>/dev/null || exit 0

payload=$(cat)

# Loop guard (shared): caps how often this gate may send the same worker back.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

# The subagent's working directory (its own worktree when isolated); the hook
# input's `cwd` follows the subagent, unlike ${CLAUDE_PROJECT_DIR}.
cwd=$(printf '%s' "$payload" | grep -oE '"cwd"[[:space:]]*:[[:space:]]*"[^"]+"' | head -n 1 | grep -oE '"[^"]+"$' | tr -d '"')
[ -n "$cwd" ] || cwd="${CLAUDE_PROJECT_DIR:-.}"

# Only gate workers that actually changed files. A fresh worktree starts clean, so
# any change is this worker's; a read-only worker leaves the tree clean and passes.
[ -n "$(git -C "$cwd" status --porcelain 2>/dev/null)" ] || exit 0

# Read the confidence the worker stated in its final message (last one wins).
score=$(printf '%s' "$payload" | grep -oiE 'confidence[^0-9]{0,4}[0-9]{1,3}' | grep -oE '[0-9]{1,3}' | tail -n 1)

if lg_on; then
    worker=$(lg_field "$payload" agent_id)
    [ -n "$worker" ] || worker=$(lg_field "$payload" agent_type)
    lg_key "$payload" subagent-confidence "${worker:-subagent}"
fi

# Block, unless the loop guard says this worker is stuck: then release it with
# the result marked UNRESOLVED and escalate to a human.
block() {
    if lg_on; then
        n=$(lg_bump)
        stalled=0
        lg_stalled "$score" && stalled=1
        if [ "$n" -ge "$LG_MAX" ] || [ "$stalled" = 1 ]; then
            why="blocked ${n}x"
            [ "$stalled" = 1 ] && why="confidence stopped rising (${n} blocks)"
            lg_escalate "${why}; last confidence ${score:-not stated}%, bar ${thr}%"
            lg_release "UNRESOLVED: worker ${LG_SUBJECT} stopped by the loop guard (${why}, confidence ${score:-not stated}% < ${thr}%). Review its changes; see .claude/loop-guard/escalations.md"
        fi
        echo "Loop guard: block ${n} of ${LG_MAX}. If a different approach won't get you there," >&2
        echo "say what blocks you instead of retrying the same thing." >&2
    fi
    exit 2
}

if [ -z "$score" ]; then
    echo "You changed files but did not state your confidence. Finish only when you are" >&2
    echo ">= ${thr}% confident, and state 'Confidence: NN%' in your final message." >&2
    block
fi
if [ "$score" -lt "$thr" ] 2>/dev/null; then
    echo "Confidence ${score}% is below the required ${thr}%. Keep working (or re-scope)" >&2
    echo "until you are >= ${thr}% confident, then state 'Confidence: NN%' and finish." >&2
    block
fi
lg_on && lg_reset
exit 0
