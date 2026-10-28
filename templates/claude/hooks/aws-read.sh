#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): when Claude writes its
# AWS request (.claude/notes/aws/request.json), check that the project's dev
# profile is the dev account, then run the calls that only read metadata with
# the user's own AWS CLI and hand Claude the output in the hook's note, saved
# nowhere.
#
# The reading needs the ocgen binary (`ocgen hook aws-read`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: no answer
# comes, and Claude marks the claims that need AWS as unconfirmed.
cat >/dev/null
exit 0
