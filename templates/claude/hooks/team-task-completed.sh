#!/bin/sh
# Claude Code Agent Teams — TaskCompleted hook.
# Runs when a task is being marked complete.
#   exit 0  -> allow completion (default; safe no-op).
#   exit 2  -> block completion and send feedback on stderr, e.g. run the tests
#              first and reject if they fail:
#              if ! cargo test --quiet; then echo "tests failing" >&2; exit 2; fi
exit 0
