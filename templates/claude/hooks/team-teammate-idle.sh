#!/bin/sh
# Claude Code Agent Teams — TeammateIdle hook (ownership-scoped risk gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_RISK_ROUNDS=1     -> hold a teammate while a risk IT OWNS is still pending.
#   TEAM_READONLY_ROLES    -> space-separated roles with no Write/Edit tool; exempt,
#                            since they cannot do mitigation work.
# In .claude/team/plan.md each risk is ONE line carrying an "Owner: <teammate-name>"
# and a "Mitigation: <status>" (pending / done / accepted). A teammate is held only
# while a risk it owns is "Mitigation: pending" -- so a teammate that finished its
# own risks idles freely even while others' risks remain open (no livelock, no
# pressure to fabricate closures for risks it doesn't own).
# The teammate is the event's teammate_name; its role (agent_type) only when the
# event has no name. An in-process teammate's role is read from its transcript's
# .meta.json. A role-named owner ("Owner: implementer", from an older
# /team-plan, when several teammates may share the role) holds no named teammate,
# but is reported to the user rather than passing unseen.
#   exit 0 -> allow idle.   exit 2 -> keep working and send feedback on stderr.
LC_ALL=C
export LC_ALL
[ "${TEAM_RISK_ROUNDS:-0}" = "1" ] || exit 0

payload=$(cat)

# field <name> -> the first string value of "name": "..."
field() {
    printf '%s' "$payload" | grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -n 1 |
        sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/'
}
lower() { printf '%s' "$1" | tr 'A-Z' 'a-z'; }
# ere <text> -> <text> as a literal in an extended regex
ere() { printf '%s' "$1" | sed 's/[][\.*^$+?(){}|]/\\&/g'; }
# say <message> -> tell the user (systemMessage) and allow
say() {
    printf '{"systemMessage": "%s"}\n' "$(printf '%s' "$1" | tr -d '"\\' | tr '\n' ' ')"
    exit 0
}

# Loop guard (shared): caps how often this gate may hold the same teammate.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

name=$(field teammate_name)
role=$(field agent_type)
who=${name:-$role}

# An in-process teammate's role: Claude Code files its transcript beside the
# lead's (transcript_path), under subagents/ (or a folder below), with a
# .meta.json naming the teammate, its team and its role (customAgentType); the
# newest transcript's counts. Without team_name (Claude Code marks it
# deprecated), the team is the one Claude Code makes for the session:
# session-<first 8 of its id>.
team=$(field team_name)
sid=$(field session_id)
[ -n "$team" ] || [ -z "$sid" ] || team="session-$(printf '%s' "$sid" | cut -c1-8)"
tp=$(field transcript_path | sed 's#\\\\#/#g')
mrole=
if [ -n "$tp" ] && [ -n "$name" ] && [ -n "$team" ]; then
    src=
    for m in "${tp%.jsonl}"/subagents/*.meta.json "${tp%.jsonl}"/subagents/*/*.meta.json; do
        [ -f "$m" ] || continue
        grep -qF "\"teamName\":\"$team\"" "$m" && grep -qF "\"name\":\"$name\"" "$m" || continue
        r=$(grep -oE '"customAgentType":"[^"\\]*"' "$m" | head -n 1 | sed 's/^"customAgentType":"//; s/"$//')
        f="${m%.meta.json}.jsonl"
        if [ ! -f "$f" ]; then
            [ -n "$mrole" ] || mrole=$r
        elif [ -z "$src" ] || [ "$f" -nt "$src" ]; then
            src=$f
            mrole=$r
        fi
    done
fi
[ -z "$mrole" ] || role=$mrole
[ -n "$who" ] || say "Risk gate: this TeammateIdle event names no teammate (no teammate_name or agent_type), so its pending risks in .claude/team/plan.md were not checked."

# Read-only roles cannot mitigate -> exempt.
for r in ${TEAM_READONLY_ROLES:-}; do
    [ -n "$role" ] && [ "$(lower "$role")" = "$(lower "$r")" ] && exit 0
done

plan="${CLAUDE_PROJECT_DIR:-.}/.claude/team/plan.md"
[ -f "$plan" ] || exit 0

# Among the pending-mitigation lines, which are owned by THIS teammate?
owned=$(grep -i 'mitigation:[[:space:]]*pending' "$plan" | grep -iE "owner:[[:space:]]*$(ere "$who")([^a-z0-9_-]|\$)")
lg_on && lg_key "$payload" risk-idle "$who"
if [ -n "$owned" ]; then
    if lg_on; then
        n=$(lg_bump)
        if [ "$n" -ge "$LG_MAX" ]; then
            # Let it idle rather than pressure it into faking a closure.
            lg_escalate "held ${n}x; still pending: $(printf '%s' "$owned" | head -n 3)"
            lg_release "UNRESOLVED: ${who} could not close its pending risks after ${n} holds; released by the loop guard. See .claude/loop-guard/escalations.md"
        fi
    fi
    echo "You still own risks marked 'Mitigation: pending' in .claude/team/plan.md." >&2
    echo "Close each one you own (mark it done or accepted) before going idle." >&2
    echo "If you truly can't mitigate one, say why — never mark it done to get past this." >&2
    exit 2
fi
lg_on && lg_reset
if [ -n "$name" ] && [ -n "$role" ] && [ "$(lower "$role")" != "$(lower "$name")" ]; then
    n=$(grep -i 'mitigation:[[:space:]]*pending' "$plan" | grep -ciE "owner:[[:space:]]*$(ere "$role")([^a-z0-9_-]|\$)")
    [ "${n:-0}" -eq 0 ] || say "Risk gate: $n pending risk(s) in .claude/team/plan.md name the role $role as Owner, not a teammate, so they hold no one. Write Owner: <teammate-name>."
fi
exit 0
