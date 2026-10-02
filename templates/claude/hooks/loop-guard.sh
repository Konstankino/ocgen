#!/bin/sh
# Claude Code — shared loop guard, sourced by ocgen's blocking hooks.
#
# A hook that blocks (exit 2) has no built-in upper bound, so an agent that
# cannot satisfy a gate would be sent back forever. This caps how many times one
# gate may block the same agent on the same subject (a task, a role, a worker):
#   LOOP_GUARD_MAX_BLOCKS=N  (set in .claude/settings.json)  0/unset = off.
# When the budget is spent (or a stated confidence stops rising), the calling
# hook escalates: quality gates release the agent with the result marked
# UNRESOLVED; approval gates stay closed and tell the agent to stop.
# Escalations are appended to .claude/loop-guard/escalations.md (self-ignored).
LG_MAX="${LOOP_GUARD_MAX_BLOCKS:-0}"
case "$LG_MAX" in '' | *[!0-9]*) LG_MAX=0 ;; esac
LG_DIR="${CLAUDE_PROJECT_DIR:-.}/.claude/loop-guard"

lg_on() { [ "$LG_MAX" -gt 0 ]; }

# lg_field <json> <name> -> the first string value of "name": "..."
lg_field() {
    printf '%s' "$1" | grep -oE "\"$2\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -n 1 |
        sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/'
}

# lg_safe <text> -> a filename- and JSON-safe token (byte by byte, in any locale,
# as the Rust hook makes it)
lg_safe() { printf '%s' "$1" | LC_ALL=C tr -c 'A-Za-z0-9._-' '_' | LC_ALL=C cut -c1-80; }

# lg_key <payload> <gate> <subject> -> sets LG_GATE, LG_SUBJECT, LG_KEY
lg_key() {
    sid=$(lg_safe "$(lg_field "$1" session_id)")
    [ -n "$sid" ] || sid=nosession
    LG_GATE="$2"
    LG_SUBJECT=$(lg_safe "$3")
    [ -n "$LG_SUBJECT" ] || LG_SUBJECT=unknown
    LG_KEY="$LG_DIR/$sid/$LG_GATE-$LG_SUBJECT"
}

lg_prepare() {
    mkdir -p "$(dirname "$LG_KEY")" 2>/dev/null || return 1
    [ -f "$LG_DIR/.gitignore" ] || printf '*\n' >"$LG_DIR/.gitignore"
}

# lg_bump -> increment and print this subject's consecutive block count
lg_bump() {
    lg_prepare || {
        echo 0
        return
    }
    n=$(cat "$LG_KEY.count" 2>/dev/null)
    n=$((${n:-0} + 1))
    printf '%s\n' "$n" >"$LG_KEY.count"
    echo "$n"
}

# lg_stalled <score> -> true when a stated confidence did not rise since the last
# block (records the score either way). No score -> never "stalled".
lg_stalled() {
    [ -n "$1" ] || return 1
    prev=$(cat "$LG_KEY.score" 2>/dev/null)
    printf '%s\n' "$1" >"$LG_KEY.score"
    [ -n "$prev" ] && [ "$1" -le "$prev" ] 2>/dev/null
}

# lg_reset -> the gate passed; the next attempt gets a fresh budget
lg_reset() { rm -f "$LG_KEY.count" "$LG_KEY.score" "$LG_KEY.escalated" 2>/dev/null; }

# lg_escalate <detail> -> log once per episode to escalations.md
lg_escalate() {
    lg_prepare || return 0
    [ -f "$LG_KEY.escalated" ] && return 0
    : >"$LG_KEY.escalated"
    printf -- '- %s [%s] %s: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$LG_GATE" "$LG_SUBJECT" \
        "$(printf '%s' "$1" | tr '\n' ' ')" >>"$LG_DIR/escalations.md"
}

# lg_release <message> -> quality gate gives way: warn the user and allow (exit 0)
lg_release() {
    printf '{"systemMessage": "%s"}\n' "$(printf '%s' "$1" | tr -d '"\\' | tr '\n' ' ')"
    exit 0
}
