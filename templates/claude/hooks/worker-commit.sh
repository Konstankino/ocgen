#!/bin/sh
# Claude Code — SubagentStop hook: commit what a worktree-isolated worker left
# uncommitted, on its own branch. The implementer and /fanout workers have no
# shell to commit with, and Claude Code refuses the coordinator's git commands in
# another worktree, so its merge would find nothing. Only Claude Code's agent
# worktrees count (`worktree-agent-*` branches): never the main checkout, nor a
# session's worktree. Git-ignored files stay out; the repository's own git hooks
# run. Never blocks a worker.
#
# Also PreToolUse (Bash, PowerShell): before the coordinator merges a worker's
# branch, the same commit for whatever is still uncommitted there; it then holds
# that one merge, saying why (see below).
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

# PreToolUse: the coordinator is about to merge a worker's branch. A worker whose
# stop left its changes uncommitted (this hook failed or never ran there, or
# didn't see the worker's worktree) would merge as nothing, and Claude Code
# refuses git aimed at another worktree, the user's `!` command included. Hooks
# aren't held to that: commit what the worker left, on its branch, and hold this
# one call so the coordinator runs the merge again (and a workstreams board's gate
# reads the new tip). The merge is ocgen's risk::SUBAGENT_MERGE (a test keeps
# this line identical); any other command is left alone.
SUBAGENT_MERGE='^[[:space:]]*git[[:space:]]+merge(([[:space:]]+--(no-ff|ff-only|no-edit))*[[:space:]]+(refs/heads/)?worktree-agent-[[:alnum:]_-]+([[:space:]]+--(no-ff|ff-only|no-edit))*|[[:space:]]+--abort)[[:space:]]*$'
nl='
'
if [ "$(json_str hook_event_name)" = PreToolUse ]; then
    if [ -n "$jq" ]; then
        cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command | strings' 2>/dev/null)
    else
        cmd=$(printf '%s' "$payload" | grep -oE '"command"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' | head -n 1 |
            sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//' | json_unescape)
    fi
    [ -n "$cmd" ] || exit 0
    ! printf '%s' "$cmd" | tr -d '\t\n\r' | grep -q '[[:cntrl:]]' || exit 0
    # As the shell runs it: continued lines joined, quotes and backslashes gone.
    ncmd=$(printf '%s\n' "$cmd" | awk '{ if (sub(/\\$/, "")) printf "%s", $0; else print }' |
        tr -d "\"'\\\\")
    case $ncmd in *"$nl"*) exit 0 ;; esac
    printf '%s\n' "$ncmd" | grep -Eq "$SUBAGENT_MERGE" || exit 0
    branch=$(printf '%s\n' "$ncmd" | grep -oE 'worktree-agent-[[:alnum:]_-]+' | head -n 1)
    [ -n "$branch" ] || exit 0 # --abort
    cwd=$(json_str cwd)
    [ -n "$cwd" ] || cwd="${CLAUDE_PROJECT_DIR:-.}"
    # The worktree that has the branch checked out (none once it is removed).
    wt=$(git -C "$cwd" worktree list --porcelain 2>/dev/null |
        awk -v ref="branch refs/heads/$branch" '{ sub(/\r$/, "") } /^worktree / { p = substr($0, 10) } $0 == ref { print p; exit }')
    [ -n "$wt" ] || exit 0
    [ -n "$(git -C "$wt" status --porcelain --untracked-files=all 2>/dev/null)" ] || exit 0
    out=$({ git -C "$wt" add -A && git -C "$wt" commit -q -m "chore(subagent): the subagent's changes" \
        -m "Committed by ocgen's worker-commit hook before the coordinator's merge: the subagent stopped with them uncommitted."; } </dev/null 2>&1)
    if [ $? -eq 0 ]; then
        sha=$(git -C "$wt" rev-parse --short HEAD 2>/dev/null)
        echo "ocgen committed the changes the subagent left uncommitted on $branch ($sha): its stop didn't commit them, so this merge would have taken nothing. Run the merge again to take them." >&2
    else
        git -C "$wt" reset -q >/dev/null 2>&1
        echo "ocgen could not commit the changes the subagent left uncommitted on $branch (git commit failed), so this merge would take none of them. git said:" >&2
        printf '%s\n' "$out" | tail -n 20 >&2
        echo "They are still uncommitted in its worktree ($wt). Send the cause to the worker to fix, then merge again; never ask the user to merge or copy files by hand." >&2
    fi
    exit 2
fi

# The hook input's `cwd` follows the subagent (its own worktree when isolated).
cwd=$(json_str cwd)
[ -n "$cwd" ] || cwd="${CLAUDE_PROJECT_DIR:-.}"
git_dir=$(git -C "$cwd" rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 0
common=$(git -C "$cwd" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
branch=$(git -C "$cwd" symbolic-ref --quiet --short HEAD 2>/dev/null)
[ "$git_dir" != "$common" ] || exit 0
case "$branch" in worktree-agent-*) ;; *) exit 0 ;; esac
who=$(safe "$(json_str agent_type)")
[ -n "$who" ] || who=subagent

# `worker-commit.sh mark`, run by the worker gate on a worker the loop guard
# released: an empty commit on top that says so, its body the release message
# (OCGEN_RELEASED), so the branch shows the work was never verified.
if [ "${1:-}" = mark ]; then
    git -C "$cwd" commit -q --allow-empty -m "chore($who): UNRESOLVED, released by the loop guard" \
        -m "${OCGEN_RELEASED:-}" </dev/null >/dev/null 2>&1
    exit 0
fi
[ -n "$(git -C "$cwd" status --porcelain --untracked-files=all 2>/dev/null)" ] || exit 0

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
