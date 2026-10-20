#!/bin/sh
# Claude Code — PreToolUse hook (matcher: Bash|PowerShell): native Windows has
# no Claude Code sandbox, so with the project's sandbox on, ocgen runs each
# command Claude runs in its own (an AppContainer), and checks the project's
# deny and ask rules against the command as written first.
#
# Only the ocgen binary can do that (`ocgen hook shell-sandbox`, used whenever a
# compatible ocgen is on PATH). Without one, on native Windows with the sandbox
# on, this script refuses the command: ocgen never runs it unsandboxed.
# Anywhere else it does nothing — Claude Code's own sandbox covers macOS,
# Linux and WSL2.
cat >/dev/null
[ "${OCGEN_SANDBOX:-}" = 1 ] || exit 0
win=
case "$(uname -s 2>/dev/null)" in MINGW* | MSYS* | CYGWIN*) win=1 ;; esac
[ -z "${MSYSTEM:-}" ] || win=1
[ "${OS:-}" != Windows_NT ] || win=1
[ -n "$win" ] || exit 0
echo "Blocked: the project's sandbox is on, and on native Windows only the ocgen binary can run this command in it — install the ocgen that generated this project so the hooks use it; ocgen never runs the command unsandboxed." >&2
exit 2
