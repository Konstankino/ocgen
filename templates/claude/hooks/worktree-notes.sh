#!/bin/sh
# Claude Code — PreToolUse / PostToolUse hook (ExitWorktree): carry a worktree's
# notes (.claude/notes, .claude/briefs, .claude/intent/drafts) into the main
# checkout when the worktree is removed. They are git-ignored, so the main
# checkout never sees them, and Claude Code counts them as no change. Before a
# `remove`, the hook stages the notes the main checkout lacks and asks the user;
# once the approved removal is done, it copies them in, never over a note the
# main checkout already has.
#
# This needs the ocgen binary (`ocgen hook worktree-notes`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing, and the
# removal goes ahead as Claude Code would do it.
cat >/dev/null
exit 0
