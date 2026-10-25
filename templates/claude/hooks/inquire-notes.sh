#!/bin/sh
# Claude Code — PostToolUse hook (Write|Edit|MultiEdit): keep each /inquire ledger's,
# /recap report's, /intent reading copy's and the /workstreams board's HTML view
# (<name>.html beside the .md) in step with its Markdown and show it in the
# browser, refreshing a tab that already shows it. Each such document but the
# board (English only, parsed by a hook) is kept in both project languages
# (<name>.md and <name>.<language>.md): after a write, the hook tells Claude which
# language version fell behind, and as a Stop / SubagentStop hook it holds the end
# of a turn once while one is still owed. A /brief brief (.claude/briefs/<slug>.md)
# has no static page: after a write the hook shows it in the browser editor, under
# its coverage strip, and remembers it as the session's brief; a file there whose
# name is no slug gets a note asking Claude to rename it, or to fold a language
# version written beside a brief back into its one file. As a UserPromptSubmit
# hook, the word `note` or `notes`, sent on its own, opens the ledger — or their
# list, with the one this session wrote last selected — and the word `brief` the
# brief this session wrote, else the only one, else their list. At SessionStart
# and CwdChanged it names the folders Claude Code's file watcher watches (the
# briefs' among them), and on FileChanged it remembers the brief that changed.
#
# Rendering, the live view, the words and the language check need the ocgen
# binary (`ocgen hook inquire-notes`, used whenever a compatible ocgen is on PATH).
# Without one this script does nothing: the Markdown stays the source of truth,
# the prompt reaches Claude as typed, and `ocgen verify` reports that the HTML view
# is unavailable.
cat >/dev/null
exit 0
