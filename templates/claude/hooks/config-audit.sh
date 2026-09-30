#!/bin/sh
# Claude Code — ConfigChange hook: log every settings/skills change made during a
# session to .claude/audit/config-changes.log (self-git-ignored). Never blocks.
payload=$(cat)
dir="${CLAUDE_PROJECT_DIR:-.}/.claude/audit"
mkdir -p "$dir" 2>/dev/null || exit 0
[ -f "$dir/.gitignore" ] || printf '*\n' >"$dir/.gitignore"
src=$(printf '%s' "$payload" | grep -oE '"source"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 |
    sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/')
file=$(printf '%s' "$payload" | grep -oE '"file_path"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 |
    sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/')
printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${src:-unknown}" "${file:--}" >>"$dir/config-changes.log"
exit 0
