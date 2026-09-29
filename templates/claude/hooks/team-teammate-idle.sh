#!/bin/sh
# Claude Code Agent Teams — TeammateIdle hook (ownership-scoped risk gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_RISK_ROUNDS=1     -> hold a teammate while a risk IT OWNS is still pending.
#   TEAM_READONLY_ROLES    -> space-separated roles with no Write/Edit tool; exempt,
#                            since they cannot do mitigation work.
# In .claude/team/plan.md each risk is ONE line carrying an "Owner: <role>" and a
# "Mitigation: <status>" (pending / done / accepted). A teammate is held only while
# a risk it owns is "Mitigation: pending" -- so a teammate that finished its own
# risks idles freely even while others' risks remain open (no livelock, no pressure
# to fabricate closures for risks it doesn't own).
#   exit 0 -> allow idle.   exit 2 -> keep working and send feedback on stderr.
[ "${TEAM_RISK_ROUNDS:-0}" = "1" ] || exit 0

payload=$(cat)

# Loop guard (shared): caps how often this gate may hold the same teammate.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"
role=$(printf '%s' "$payload" | grep -oiE '"agent_type"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 | grep -oE '"[^"]*"$' | tr -d '"')
[ -n "$role" ] || exit 0  # can't identify the teammate -> don't livelock it

# Read-only roles cannot mitigate -> exempt.
role_lc=$(printf '%s' "$role" | tr 'A-Z' 'a-z')
for r in ${TEAM_READONLY_ROLES:-}; do
    [ "$role_lc" = "$(printf '%s' "$r" | tr 'A-Z' 'a-z')" ] && exit 0
done

plan="${CLAUDE_PROJECT_DIR:-.}/.claude/team/plan.md"
[ -f "$plan" ] || exit 0

# Among the pending-mitigation lines, which are owned by THIS role?
owned=$(grep -i 'mitigation:[[:space:]]*pending' "$plan" | grep -iE "owner:[[:space:]]*${role}([^a-z0-9_-]|\$)")
lg_on && lg_key "$payload" risk-idle "$role"
if [ -n "$owned" ]; then
    if lg_on; then
        n=$(lg_bump)
        if [ "$n" -ge "$LG_MAX" ]; then
            # Let it idle rather than pressure it into faking a closure.
            lg_escalate "held ${n}x; still pending: $(printf '%s' "$owned" | head -n 3)"
            lg_release "UNRESOLVED: ${role} could not close its pending risks after ${n} holds; released by the loop guard. See .claude/loop-guard/escalations.md"
        fi
    fi
    echo "You still own risks marked 'Mitigation: pending' in .claude/team/plan.md." >&2
    echo "Close each one you own (mark it done or accepted) before going idle." >&2
    echo "If you truly can't mitigate one, say why — never mark it done to get past this." >&2
    exit 2
fi
lg_on && lg_reset
exit 0
