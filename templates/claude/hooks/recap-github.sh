#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): when /recap writes its
# GitHub request (.claude/notes/recap/github-request.json), look up what's new on
# the issues and the branches' PRs it names, with the user's own gh login, and
# write it to .claude/notes/recap/github.json.
#
# The lookup needs the ocgen binary (`ocgen hook recap-github`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: no answer
# is written for the request, and /recap reports GitHub as not checked.
cat >/dev/null
exit 0
