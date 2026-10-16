#!/bin/sh
# Claude Code — PostToolUse hook (Edit|Write): run the project's formatter after
# Claude edits a file — in the project directory, or for a file in another
# worktree of its repository (a worktree-isolated worker's own) in the project's
# folder there.
# OCGEN_FORMAT_CMD is set on the hook command in settings. With the project's
# sandbox on, only the ocgen binary runs the formatter (see below).
# Never blocks: a failing formatter is reported, not fatal.
[ -n "${OCGEN_FORMAT_CMD:-}" ] || exit 0
payload=$(cat)
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    file=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty | strings' 2>/dev/null)
else
    file=$(printf '%s' "$payload" | grep -oE '"file_path"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' | head -n 1 |
        sed -E 's/^"file_path"[[:space:]]*:[[:space:]]*"//; s/"$//; s/\\(["\\/])/\1/g')
fi
proj="${CLAUDE_PROJECT_DIR:-.}"
# common <dir> -> the repository <dir> belongs to, shared by all its worktrees
common() {
    c=$(git -C "$1" rev-parse --git-common-dir 2>/dev/null) || return 0
    case "$c" in
    /* | [A-Za-z]:/*) ;;
    *) c="$1/$c" ;;
    esac
    (CDPATH= cd -- "$c" 2>/dev/null && pwd -P)
}
# --- sandbox (the same in format.sh, subagent-confidence-gate.sh and
# team-task-completed.sh) ---
# With the project's sandbox on (OCGEN_SANDBOX=1), the project code a hook starts
# itself — the check, the formatter — runs in ocgen's own sandbox, which denies
# whatever the hook env's lists don't grant. Only the ocgen binary can apply it,
# so this script never runs that code, sandboxed or not: the hook command uses
# the binary whenever it is installed.

# sb_ready -> true when a project command may run (the sandbox is off), else
# sb_err says why not.
sb_ready() {
    sb_err="the project's sandbox is on, and only the ocgen binary can run this command in it — install the ocgen that generated this project so the hooks use it; ocgen never runs it unsandboxed"
    [ "${OCGEN_SANDBOX:-}" != 1 ]
}
# --- end sandbox ---

# The project directory, unless the edited file is in another worktree of the
# project's repository (a worktree-isolated worker's): then the project's folder
# in that worktree. A file in an unrelated repository is not formatted there.
dir=
if [ -n "$file" ]; then
    case "$file" in
    [A-Za-z]:\\*) file=$(printf '%s' "$file" | tr '\\' '/') ;;
    esac
    case "$file" in
    /* | [A-Za-z]:/*) ;;
    *) file="$proj/$file" ;;
    esac
    fdir=$(dirname -- "$file")
    top=$(git -C "$fdir" rev-parse --show-toplevel 2>/dev/null)
    if [ -n "$top" ] && [ "$top" != "$(git -C "$proj" rev-parse --show-toplevel 2>/dev/null)" ]; then
        here=$(common "$fdir")
        if [ -n "$here" ] && [ "$here" = "$(common "$proj")" ]; then
            dir=$top
            prefix=$(git -C "$proj" rev-parse --show-prefix 2>/dev/null)
            [ -z "$prefix" ] || [ ! -d "$top/${prefix%/}" ] || dir="$top/${prefix%/}"
        fi
    fi
fi
if ! sb_ready; then
    echo "format hook: '$OCGEN_FORMAT_CMD' not run: $sb_err" >&2
    exit 0
fi
cd "${dir:-$proj}" 2>/dev/null || exit 0
if ! (exec sh -c "$OCGEN_FORMAT_CMD") >/dev/null 2>&1; then
    echo "format hook: '$OCGEN_FORMAT_CMD' failed (ignored)" >&2
fi
exit 0
