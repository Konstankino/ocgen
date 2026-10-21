#!/bin/sh
# Claude Code — PreToolUse hook (matcher: Bash|PowerShell): native Windows has
# no Claude Code sandbox, and ocgen's can't run there either (an AppContainer
# can't open the NUL device, which cargo, git and most tools open). So with the
# project's sandbox on, every command Claude runs there is refused — never run
# unsandboxed. Anywhere else it does nothing: Claude Code's own sandbox covers
# macOS, Linux and WSL2. Same as `ocgen hook shell-sandbox`.
cat >/dev/null
[ "${OCGEN_SANDBOX:-}" = 1 ] || exit 0
win=
case "$(uname -s 2>/dev/null)" in MINGW* | MSYS* | CYGWIN*) win=1 ;; esac
[ -z "${MSYSTEM:-}" ] || win=1
[ "${OS:-}" != Windows_NT ] || win=1
[ -n "$win" ] || exit 0
cat >&2 <<'EOF'
Blocked: ocgen's sandbox doesn't run on native Windows: the sandbox Windows offers a program (an AppContainer) can't open the NUL device, which cargo, git and most tools open, so they fail in it. Run Claude Code and ocgen in WSL2, where the sandbox works, or turn the sandbox off for this project (`ocgen edit team`); ocgen never runs a command unsandboxed.
EOF
exit 2
