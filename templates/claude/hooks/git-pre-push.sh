#!/bin/sh
# ocgen:pre-push — execution-approval gate for `git push` (installed by ocgen).
#
# Blocks a push made UNDER CLAUDE CODE unless a human has approved it
# (`ocgen approve`). It sees every push — from a script, a Makefile or an
# interpreter — so it holds even when the command text doesn't look like a push.
# Your own pushes, from your own terminal, are never affected.
# To uninstall, delete .git/hooks/pre-push.
[ -n "${CLAUDECODE:-}" ] || exit 0

root=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
[ -n "$root" ] || exit 0
root=$(dirname "$root")
# Only projects that turned the approval gate on.
grep -q '"TEAM_APPROVAL_GATE": *"1"' "$root/.claude/settings.json" 2>/dev/null || exit 0

key=$(printf '%s' "$root" | tr -c 'A-Za-z0-9._-' '_' | tail -c 200)
marker="${HOME}/.claude/ocgen/approvals/${key}"
if [ -f "$marker" ]; then
    exp=$(tr -cd '0-9' <"$marker")
    [ -n "$exp" ] && [ "$exp" -gt "$(date +%s)" ] && exit 0
fi
echo "Push blocked by the execution-approval gate: a human must approve first," >&2
echo "from their own terminal:  ocgen approve   (it expires by itself)." >&2
exit 1
