#!/bin/sh
# Claude Code — PostToolUse (Write|Edit|MultiEdit), Stop and ExitWorktree hook:
# carry a worktree's notes (.claude/notes, .claude/briefs, .claude/intent/drafts)
# into the main checkout. They are git-ignored, so the main checkout never sees
# them, and Claude Code counts them as no change: removing the worktree deletes
# them, mostly with no hook to run first (`:q` in a `claude --worktree` session,
# an archived desktop session). So in a worktree each note is copied as soon as
# it is written, over ocgen's own earlier copy but never over a note the main
# checkout has in another version (that one stays; the worktree's goes beside
# it). Before ExitWorktree removes a worktree, the hook stages the notes the main
# checkout still lacks and asks the user; once the approved removal is done, it
# copies them in.
#
# This needs the ocgen binary (`ocgen hook worktree-notes`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: notes stay
# in the worktree, and a removal goes ahead as Claude Code would do it.
cat >/dev/null
exit 0
