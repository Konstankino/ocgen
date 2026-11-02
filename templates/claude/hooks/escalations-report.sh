#!/bin/sh
# Claude Code — PostToolUse (Agent) and Stop hook: tell the lead about the agents
# the loop guard released unresolved in its session. The release itself only
# warns the user (systemMessage), and a worker's last message still states its
# confidence, so the lead would take the work as done. After an Agent call every
# release not yet noted goes in as context; a background worker returns before
# it stops, so its release waits for the end of the turn, where every release not
# yet named there holds the stop once. Each is recorded before it is told, in the
# session's loop-guard folder (gone when the session ends), so none is told twice
# through one channel and no stop is held twice for it.
LC_ALL=C
export LC_ALL
payload=$(cat)
[ "${OCGEN_HOOK_PROBE:-}" != 1 ] || exit 0 # `ocgen verify`: change nothing
dir="${CLAUDE_PROJECT_DIR:-.}/.claude/loop-guard"
log="$dir/escalations.md"
[ -f "$log" ] || exit 0

# field <name> -> the first string value of "name": "..." (a session id and an
# event name hold no escapes)
field() {
    printf '%s' "$payload" | grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -n 1 |
        sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/'
}

sid=$(field session_id)
sid=$(printf '%s' "$sid" | tr -c 'A-Za-z0-9._-' '_' | cut -c1-80)
[ -n "$sid" ] || exit 0
stop=
record="$dir/$sid/escalations-noted"
if [ "$(field hook_event_name)" = Stop ]; then
    stop=1
    record="$dir/$sid/escalations-reported"
fi

# The releases of this session not yet told through this channel: a releasing
# gate, and " | session=<sid>" first in the fields after the line's last " | ".
# A line written before escalations named their session is nobody's.
new=$(awk -v sid="$sid" -v rec="$record" '
    BEGIN { while ((getline t < rec) > 0) told[t] = 1 }
    { sub(/\r$/, "") }
    !/^- [^ ]+ \[(subagent-confidence|task-confidence|risk-idle)\] / { next }
    {
        s = $0; i = 0
        while ((j = index(substr(s, i + 1), " | ")) > 0) i += j
        if (i == 0) next
        split(substr(s, i + 3), f, " ")
        if (f[1] == "session=" sid && !(s in told)) print s
    }' "$log")
[ -n "$new" ] || exit 0

# Recorded first: a stop that comes back passes. Unrecorded, the stop could be
# held for good, so nothing is told.
{ mkdir -p "$dir/$sid" && printf '%s\n' "$new" >>"$record"; } 2>/dev/null || exit 0

n=$(printf '%s\n' "$new" | grep -c '')
text="ocgen: the loop guard released $n agent(s) of this session UNRESOLVED: their gate never passed, whatever their last message says.
$new

Treat that work as unverified: don't present it as done, name each one as unresolved in your report, and review a branch named here before you merge it."

# json <text> -> <text> as the inside of a JSON string
tab=$(printf '\t')
json() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e "s/$tab/\\\\t/g" |
        awk '{ printf "%s%s", (NR > 1 ? "\\n" : ""), $0 }'
}

if [ -n "$stop" ]; then
    printf '{"decision":"block","reason":"%s"}\n' \
        "$(json "$text If you already said so, or you are waiting for the user, say so in one line and stop.")"
else
    printf '{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"%s"}}\n' \
        "$(json "$text")"
fi
exit 0
