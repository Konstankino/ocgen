#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): when Claude writes its
# GitHub request (.claude/notes/github/request.json), read what it asks for with
# the user's own gh login — only what reads, and only on the github.com
# repositories of the project's remotes — and write each answer under
# .claude/notes/github/.
#
# The reading needs the ocgen binary (`ocgen hook github-read`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: no answer
# is written, and Claude asks the user to run the gh command instead.
cat >/dev/null
exit 0
