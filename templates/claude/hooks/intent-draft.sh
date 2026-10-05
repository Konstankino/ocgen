#!/bin/sh
# Claude Code — UserPromptSubmit hook: the word `draft`, sent on its own, opens the
# /intent issue draft (.claude/intent/drafts/<name>.md) in a local browser editor —
# edit the Markdown, preview it as GitHub shows it, copy it. With several drafts it
# opens their list, with the one this session wrote last selected. The intent
# prefix in lowercase (`adr` for ADR) does the same for the intent files
# (docs/adr/ADR-0007-<slug>.md), read-only. As a PostToolUse hook
# (Write|Edit|MultiEdit|Bash) it remembers the draft or intent file written.
#
# The pages need the ocgen binary (`ocgen hook intent-draft`, used whenever a
# compatible ocgen is on PATH). Without one this script does nothing: the prompt
# reaches Claude as typed, and /intent tells the user to run `ocgen draft`.
cat >/dev/null
exit 0
