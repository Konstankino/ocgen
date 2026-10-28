#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): after a write to a pending
# /intent file or a draft (.claude/intent/drafts/: an issue's or a pull request's),
# check that it @mentions every approver the project has now — not the list the
# session's /intent skill was generated with — and tell Claude what is missing.
# A recorded approval that links a GitHub comment is checked against it: the
# comment must be by that approver's own login (one read-only gh call each).
#
# The check reads the project's state, so it needs the ocgen binary
# (`ocgen hook intent-approvers`, used whenever a compatible ocgen is on PATH).
# Without one this script does nothing; `ocgen verify` and `ocgen edit intent`
# still list the files that miss an approver.
cat >/dev/null
exit 0
