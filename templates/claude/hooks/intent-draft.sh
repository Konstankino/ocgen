#!/bin/sh
# Claude Code — UserPromptSubmit hook: the word `draft`, sent on its own, opens the
# newest /intent issue draft (.claude/intent/drafts/<name>.md) in a local browser
# editor — edit the Markdown, preview it as GitHub shows it, copy it.
#
# The editor needs the ocgen binary (`ocgen hook intent-draft`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: the prompt
# reaches Claude as typed, and /intent tells the user to run `ocgen draft`.
cat >/dev/null
exit 0
