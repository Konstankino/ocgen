#!/bin/sh
# Claude Code Agent Teams — TeammateIdle hook.
# Runs when a teammate is about to go idle.
#   exit 0  -> allow the teammate to go idle (default; safe no-op).
#   exit 2  -> keep it working and send feedback on stderr, e.g.:
#              echo "You still have open tasks — keep going." >&2; exit 2
exit 0
