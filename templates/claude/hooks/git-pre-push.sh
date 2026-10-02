#!/bin/sh
# ocgen:pre-push — execution-approval backstop for `git push` (installed by ocgen).
#
# Blocks a push made UNDER CLAUDE CODE unless a human has approved it
# (`ocgen approve`), also when the command text doesn't look like a push (a
# script, a Makefile, an interpreter). It is a backstop, not a boundary: git
# skips it on `git push --no-verify` or with another core.hooksPath, and it only
# knows it runs under Claude Code from $CLAUDECODE. What really stops a push is
# the sandbox (no credentials, no network) or branch protection on the server.
# Your own pushes, from your own terminal, are never affected.
# To uninstall, delete .git/hooks/pre-push.
[ -n "${CLAUDECODE:-}" ] || exit 0

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
[ -n "$common" ] || exit 0
root=$(dirname "$common")

# Only when a project in this repository turned the approval gate on: the
# repository root, the projects ocgen recorded below (one "# ocgen:root <dir>"
# line each, kept up to date by ocgen), and — for a copy chained from another
# hook — the project this script lives in (<project>/.claude/hooks/).
here=$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd)
gated=
nl='
'
set -f
IFS=$nl
for dir in $root $(sed -n 's/^# ocgen:root //p' "$0" 2>/dev/null) $here; do
    grep -q '"TEAM_APPROVAL_GATE": *"1"' "$dir/.claude/settings.json" 2>/dev/null && gated=1
done
[ -n "$gated" ] || exit 0

# The approval: keyed by the repository root, byte by byte (LC_ALL=C), exactly as
# `ocgen approve` computes it. An expiry further out than the longest approval
# ocgen grants (24 h, plus slack) wasn't written by it.
max_ahead=86700
key=$(printf '%s' "$root" | LC_ALL=C tr -c 'A-Za-z0-9._-' '_' | LC_ALL=C tail -c 200)
marker="${HOME}/.claude/ocgen/approvals/${key}"
if [ -f "$marker" ]; then
    exp=$(tr -cd '0-9' <"$marker")
    now=$(date +%s)
    [ -n "$exp" ] && [ ${#exp} -le 12 ] && [ "$exp" -gt "$now" ] &&
        [ "$exp" -le $((now + max_ahead)) ] && exit 0
fi
echo "Push blocked by the execution-approval backstop: a human must approve first," >&2
echo "from their own terminal:  ocgen approve   (it expires by itself)." >&2
exit 1
# Gated ocgen projects in this repository:
