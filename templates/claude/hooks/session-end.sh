#!/bin/sh
# Claude Code — SessionEnd hook: drop the loop guard's state for the session that
# ended (.claude/loop-guard/<session>/). Its counts matter only while the session
# runs; .claude/loop-guard/escalations.md, the log a human reads, stays. Never
# blocks.
LC_ALL=C
export LC_ALL
payload=$(cat)
[ "${OCGEN_HOOK_PROBE:-}" != 1 ] || exit 0 # `ocgen verify`: change nothing
sid=$(printf '%s' "$payload" | grep -oE '"session_id"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 |
    sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/')
# As the loop guard names its folder (loop-guard.sh, lg_safe).
sid=$(printf '%s' "$sid" | tr -c 'A-Za-z0-9._-' '_' | cut -c1-80)
# No id, or one that would name the loop guard's folder or its parent.
case "$sid" in *[!.]*) ;; *) exit 0 ;; esac
dir="${CLAUDE_PROJECT_DIR:-.}/.claude/loop-guard/$sid"
[ ! -d "$dir" ] || rm -rf "$dir"
exit 0
