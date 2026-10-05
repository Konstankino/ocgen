#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): keep each /inquire ledger's
# HTML view (.claude/notes/<topic>.html) in step with its Markdown and show it in
# the browser, refreshing a tab that already shows it. As a UserPromptSubmit hook,
# the word `note` or `notes`, sent on its own, opens the ledger — or their list,
# with the one this session wrote last selected.
#
# Rendering and the live view need the ocgen binary (`ocgen hook inquire-notes`,
# used whenever a compatible ocgen is on PATH). Without one this script does
# nothing: the Markdown ledger stays the source of truth, the prompt reaches Claude
# as typed, and `ocgen verify` reports that the HTML view is unavailable.
cat >/dev/null
exit 0
