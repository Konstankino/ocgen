#!/bin/sh
# Claude Code — PreToolUse workstreams merge gate (every tool).
#
# While /workstreams has a board open (.claude/workstreams/board.md whose first
# `Status:` line isn't CLOSED), a `git merge` may bring in exactly one branch,
# named in full (refs/heads/<name>), that the board marks `Gate: ready`, at the
# tip the board recorded (`Tip:`); `--abort`, `--continue` and `--quit` pass.
# Anything else is refused: a short name, a commit, two branches, a word after
# -m or -s, a branch not on the board, one still pending, one that moved since
# it was checked, a merge the gate can't read. Merging unchecked work is the
# accident the board exists to prevent, and the skill text that says so is cut
# at compaction; this gate is not. Decisions come only from the board and git —
# never from model judgment.
#   exit 2 -> block (stderr shown to the agent).   exit 0 -> allow.
payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"
board="$proj/.claude/workstreams/board.md"
[ -f "$board" ] || exit 0
# The first Status: line decides (a finished board is moved aside, never appended to).
if LC_ALL=C awk '/^Status:/ { sub(/\r$/, ""); print; exit }' "$board" | LC_ALL=C grep -Eq '^Status:[[:space:]]*CLOSED'; then
    exit 0
fi

# Loop guard (shared). This gate NEVER allows a refused merge; after the budget
# it denies AND halts the agent (continue:false) so it stops retrying.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

# deny <reason> -> block this call; once the budget is spent, halt the agent too.
deny() {
    if lg_on; then
        who=$(lg_field "$payload" agent_type)
        [ -n "$who" ] || who=main
        lg_key "$payload" workstreams-gate "$who"
        if [ "$(lg_bump)" -ge "$LG_MAX" ]; then
            lg_escalate "halted after repeated refused merges: $1"
            msg="Halted by the loop guard: repeated merges the workstreams board refused ($1). Check the branch with /workstreams, or close the board; see .claude/loop-guard/escalations.md"
            printf '{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "%s"}, "continue": false, "stopReason": "%s"}\n' "$msg" "$msg"
            exit 0
        fi
    fi
    exit 2
}

# refuse <reason> <detail> -> the gate's message, then deny.
refuse() {
    echo "BLOCKED by the workstreams merge gate: $1" >&2
    echo "$2" >&2
    echo "While .claude/workstreams/board.md is open, git merge takes exactly one branch, named refs/heads/<name>," >&2
    echo "that the board marks Gate: ready, at the tip it recorded. /workstreams checks a branch and marks it" >&2
    echo "ready; closing the board (Status: CLOSED) lifts this." >&2
    deny "$1"
}

# The call's shell command (any tool: Bash, Monitor, PowerShell, MCP…). jq when
# available; otherwise jall reads every string value of a key, each up to its
# real closing quote (one per line), and unjson decodes them.
jall() {
    printf '%s' "$payload" | tr '\n' ' ' |
        grep -oE '"('"$1"')"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' |
        sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//'
}
# repeated <key> -> whether the payload holds the key more than once
repeated() {
    [ -n "$(printf '%s' "$payload" | tr '\n' ' ' | grep -oE '"('"$1"')"[[:space:]]*:' | sed -n 2p)" ]
}
# A repeated "command" key could hide the real one: refused, jq or not (the
# binary reads the same text).
if repeated command; then
    refuse "the call repeats the command key" "a second command could hide the real one from the gate"
fi
# unjson: decode a JSON string body — \\ first (so "\\n" stays a backslash and an
# n), then \n to a newline, \t and \r to spaces (all the same to the patterns),
# \" and \/ to themselves. \b, \f and \uXXXX are left as written: in a command
# they mean a control character (JSON escapes nothing else that way), which is
# refused below (fail closed).
p1=$(printf '\001')
p2=$(printf '\002')
unjson() {
    LC_ALL=C sed -e 's/\\\\/'"$p2"'/g' -e 's/\\n/'"$p1"'/g' -e 's/\\[tr]/ /g' \
        -e 's/\\"/"/g' -e 's#\\/#/#g' | LC_ALL=C tr "$p1$p2" '\n\\'
}
unsure= # set when the command may hide a word from the pattern: refused (fail closed)
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command | strings' 2>/dev/null)
else
    raw=$(jall command)
    case "$(printf '%s' "$raw" | sed 's/\\\\//g')" in
    *'\b'* | *'\f'* | *'\u'*) unsure=1 ;;
    esac
    cmd=$(printf '%s' "$raw" | unjson)
fi
[ -n "$cmd" ] || exit 0

# A control character (other than tab/newline/CR) could hide a merge.
if [ -n "$unsure" ] || printf '%s' "$cmd" | LC_ALL=C tr -d '\t\n\r' | LC_ALL=C grep -q '[[:cntrl:]]'; then
    refuse "the command holds a control character" "it could hide a merge from the gate"
fi

# Join lines continued with a trailing backslash (as the shell does), strip what
# a shell would strip, so `git "merge"` can't slip through, and read a carriage
# return as the space the binary reads it as.
ncmd=$(printf '%s\n' "$cmd" | awk '{ if (sub(/\\$/, "")) printf "%s", $0; else print }' |
    tr -d "\"'\\\\" | tr '\r' ' ')

# A git merge call. Generated from ocgen's risk::MERGE_CALL (a test keeps this
# line and the Rust constant identical; the binary applies the same one).
MERGE='git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+merge([[:space:]]+[^;&|)<>`]*)?([;&|)<>`]|$)'
# …and the part before its arguments.
PREFIX='^git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+merge'
# A git command with a `merge` word the pattern didn't read as a call (an option
# value with a space, as in `git -C "My Projects/x" merge …`): refused, fail closed.
UNREAD='(^|[^[:alnum:]_./-])git[[:space:]].*[[:space:]]merge([[:space:]]|$)'
calls=$(printf '%s\n' "$ncmd" | LC_ALL=C grep -oE "$MERGE")
if [ -z "$calls" ]; then
    if printf '%s\n' "$ncmd" | LC_ALL=C grep -Eq "$UNREAD"; then
        refuse "a git merge the gate can't read" "an option value with a space (a path after -C) hides the call: run the merge from this checkout, by the branch's full name"
    fi
    exit 0
fi

# The board's workstream entries, one per line: a `- ` line with any lines
# wrapped under it joined by spaces (a heading or a blank line ends an entry),
# a trailing carriage return dropped.
entries() {
    LC_ALL=C awk '
        function flush() { if (e != "") print e; e = "" }
        { sub(/\r$/, "") }
        /^- / { flush(); e = $0; next }
        /^#/ || /^[[:space:]]*$/ { flush(); next }
        { if (e != "") e = e " " $0 }
        END { flush() }' "$board"
}

# field <entry> <key> -> the word after the first <key> in the entry.
field() {
    printf '%s\n' "$1" | LC_ALL=C awk -v key="$2" '{ for (i = 1; i < NF; i++) if ($i == key) { print $(i + 1); exit } }'
}

# check_merge <args…> -> return when this merge may run; refuse otherwise.
check_merge() {
    if [ $# -eq 1 ]; then
        case $1 in --abort | --continue | --quit) return 0 ;; esac
    fi
    got="$*"
    n=0
    ref=
    for a in "$@"; do
        case $a in
        -*) ;;
        *)
            n=$((n + 1))
            ref=$a
            ;;
        esac
    done
    [ "$n" -eq 1 ] || refuse "name exactly one branch as refs/heads/<name>" "got: git merge $got (an option's value counts as a branch: use --no-ff --no-edit and nothing else)"
    case $ref in
    refs/heads/?*) ;;
    *) refuse "$ref is not a full branch name (refs/heads/<name>)" "got: git merge $got" ;;
    esac
    # The entries naming the branch; the one marked ready wins, else the first.
    lines=$(entries | LC_ALL=C awk -v ref="$ref" '{ for (i = 1; i < NF; i++) if ($i == "Branch:" && $(i + 1) == ref) { print; break } }')
    line=$(printf '%s\n' "$lines" | LC_ALL=C awk '{ for (i = 1; i < NF; i++) if ($i == "Gate:" && $(i + 1) == "ready") { print; exit } }')
    [ -n "$line" ] || line=$(printf '%s\n' "$lines" | head -n 1)
    [ -n "$line" ] || refuse "$ref is not on the board" "only a workstream branch the board marks ready merges while it is open"
    gate=$(field "$line" Gate:)
    [ -n "$gate" ] || gate=none
    [ "$gate" = ready ] || refuse "$ref is not marked ready (Gate: $gate)" "check it first: /workstreams step 4 marks a checked branch ready"
    tip=$(field "$line" Tip:)
    if [ ${#tip} -lt 7 ] || printf '%s' "$tip" | LC_ALL=C grep -q '[^0-9a-fA-F]'; then
        refuse "$ref has no checked tip on the board" "the board line needs Tip: <sha> from the check"
    fi
    now=$(git -C "$proj" rev-parse --verify -q "$ref^{commit}" 2>/dev/null)
    [ -n "$now" ] || refuse "cannot read the tip of $ref" "git rev-parse failed in $proj"
    case $now in
    "$tip"*) ;;
    *) refuse "$ref moved since it was checked" "board tip $tip, now $now; check it again (/workstreams step 4)" ;;
    esac
}

nl='
'
set -f
IFS=$nl
for call in $calls; do
    unset IFS
    args=$(printf '%s' "$call" | LC_ALL=C sed -E -e "s/$PREFIX//" -e 's/[;&|)<>`]$//')
    # shellcheck disable=SC2086
    set -- $args
    check_merge "$@"
    IFS=$nl
done
exit 0
