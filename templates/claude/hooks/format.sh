#!/bin/sh
# Claude Code — PostToolUse hook (Edit|Write): run the project's formatter after
# Claude edits a file. OCGEN_FORMAT_CMD is set on the hook command in settings.
# Never blocks: a failing formatter is reported, not fatal.
[ -n "${OCGEN_FORMAT_CMD:-}" ] || exit 0
cat >/dev/null
cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
if ! sh -c "$OCGEN_FORMAT_CMD" >/dev/null 2>&1; then
    echo "format hook: '$OCGEN_FORMAT_CMD' failed (ignored)" >&2
fi
exit 0
