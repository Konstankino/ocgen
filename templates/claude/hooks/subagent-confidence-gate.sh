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

# The subagent's working directory (its own worktree when isolated); the hook
# input's `cwd` follows the subagent, unlike ${CLAUDE_PROJECT_DIR}.
cwd=$(printf '%s' "$payload" | grep -oE '"cwd"[[:space:]]*:[[:space:]]*"[^"]+"' | head -n 1 | grep -oE '"[^"]+"$' | tr -d '"')
[ -n "$cwd" ] || cwd="${CLAUDE_PROJECT_DIR:-.}"

# Only gate workers that actually changed files. A fresh worktree starts clean, so
# any change is this worker's; a read-only worker leaves the tree clean and passes.
[ -n "$(git -C "$cwd" status --porcelain 2>/dev/null)" ] || exit 0

# Read the confidence the worker stated in its final message (last one wins).
score=$(printf '%s' "$payload" | grep -oiE 'confidence[^0-9]{0,4}[0-9]{1,3}' | grep -oE '[0-9]{1,3}' | tail -n 1)

if [ -z "$score" ]; then
    echo "You changed files but did not state your confidence. Finish only when you are" >&2
    echo ">= ${thr}% confident, and state 'Confidence: NN%' in your final message." >&2
    exit 2
fi
if [ "$score" -lt "$thr" ] 2>/dev/null; then
    echo "Confidence ${score}% is below the required ${thr}%. Keep working (or re-scope)" >&2
    echo "until you are >= ${thr}% confident, then state 'Confidence: NN%' and finish." >&2
    exit 2
fi
exit 0
