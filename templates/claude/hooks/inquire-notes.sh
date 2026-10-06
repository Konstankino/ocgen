#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): keep each /inquire ledger's,
# /recap report's and /intent reading copy's HTML view (<name>.html beside the .md)
# in step with its Markdown and show it in the browser, refreshing a tab that
# already shows it. Each such document is kept in both project languages
# (<name>.md and <name>.<language>.md): after a write, the hook tells Claude which
# language version fell behind, and as a Stop / SubagentStop hook it holds the end
# of a turn once while one is still owed. As a UserPromptSubmit hook, the word
# `note` or `notes`, sent on its own, opens the ledger — or their list, with the
# one this session wrote last selected.
#
# Rendering, the live view and the language check need the ocgen binary
# (`ocgen hook inquire-notes`, used whenever a compatible ocgen is on PATH).
# Without one this script does nothing: the Markdown stays the source of truth,
# the prompt reaches Claude as typed, and `ocgen verify` reports that the HTML view
# is unavailable.
cat >/dev/null
exit 0
