#!/bin/sh
# Claude Code — SubagentStop hook: commit what a worktree-isolated worker left
# uncommitted, on its own branch. The implementer and /fanout workers have no
# shell to commit with, and Claude Code refuses the coordinator's git commands in
# another worktree, so its merge would find nothing. Only Claude Code's agent
# worktrees count (`worktree-agent-*` branches): never the main checkout, nor a
# session's worktree. Git-ignored files stay out; the repository's own git hooks
# run. Never blocks.
LC_ALL=C
export LC_ALL
payload=$(cat)
[ "${OCGEN_HOOK_PROBE:-}" != 1 ] || exit 0 # `ocgen verify`: change nothing
jq=
[ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1 && jq=1

# json_unescape -> decode JSON string escapes (\n, \", \\, \uXXXX, …) on stdin.
json_unescape() {
    awk '
    function hex(s,   i, c, n) {
        n = 0
        for (i = 1; i <= 4; i++) {
            c = index("0123456789abcdef", tolower(substr(s, i, 1)))
            if (c == 0) return -1
            n = n * 16 + c - 1
        }
        return n
    }
    function utf8(n) {
        if (n < 128) return sprintf("%c", n)
        if (n < 2048) return sprintf("%c%c", 192 + int(n / 64), 128 + n % 64)
        return sprintf("%c%c%c", 224 + int(n / 4096), 128 + int(n / 64) % 64, 128 + n % 64)
    }
    {
        s = $0; o = ""
        while ((i = index(s, "\\")) > 0) {
            o = o substr(s, 1, i - 1); e = substr(s, i + 1, 1); s = substr(s, i + 2)
            if (e == "n") o = o "\n"
            else if (e == "t") o = o "\t"
            else if (e == "r") o = o "\r"
            else if (e == "u") { v = hex(s); if (v > 0) o = o utf8(v); if (v >= 0) s = substr(s, 5) }
            else if (e != "b" && e != "f") o = o e
        }
        printf "%s%s", (NR > 1 ? "\n" : ""), o s
    }'
}

# json_str <name> -> the event's top-level string field <name>, decoded (jq, or a
# best-effort grep where jq is missing).
json_str() {
    if [ -n "$jq" ]; then
        printf '%s' "$payload" | jq -r --arg k "$1" '.[$k] // empty | strings' 2>/dev/null
    else
        printf '%s' "$payload" | grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"([^\"\\\\]|\\\\.)*\"" | head -n 1 |
            sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//' | json_unescape
    fi
}

# safe <text> -> a filename-safe token
safe() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_' | cut -c1-80; }

# The hook input's `cwd` follows the subagent (its own worktree when isolated).
cwd=$(json_str cwd)
[ -n "$cwd" ] || cwd="${CLAUDE_PROJECT_DIR:-.}"
git_dir=$(git -C "$cwd" rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 0
common=$(git -C "$cwd" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
branch=$(git -C "$cwd" symbolic-ref --quiet --short HEAD 2>/dev/null)
[ "$git_dir" != "$common" ] || exit 0
case "$branch" in worktree-agent-*) ;; *) exit 0 ;; esac
[ -n "$(git -C "$cwd" status --porcelain --untracked-files=all 2>/dev/null)" ] || exit 0

who=$(safe "$(json_str agent_type)")
[ -n "$who" ] || who=subagent
subject="chore($who): the subagent's changes"
body="Committed by ocgen's worker-commit hook when the subagent stopped, so its branch merges."
# Another git command holding the index (the worker gate runs beside this hook)
# is waited out briefly; any other failure is final, so the repository's commit
# hooks run once.
committed=
tries=0
while [ "$tries" -lt 5 ]; do
    if git -C "$cwd" add -A >/dev/null 2>&1 &&
        git -C "$cwd" commit -q -m "$subject" -m "$body" </dev/null >/dev/null 2>&1; then
        committed=1
        break
    fi
    [ -e "$git_dir/index.lock" ] || break
    sleep 1
    tries=$((tries + 1))
done
br=$(printf '%s' "$branch" | sed 's/\\/\\\\/g; s/"/\\"/g')
if [ -n "$committed" ]; then
    say="ocgen committed $who's changes on $br; merge that branch to take them."
else
    git -C "$cwd" reset -q >/dev/null 2>&1
    say="ocgen could not commit $who's changes on $br (git commit failed); they are still uncommitted in its worktree."
fi
printf '{"systemMessage":"%s"}\n' "$say"
exit 0
