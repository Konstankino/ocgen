#!/bin/sh
# Claude Code Agent Teams — TeammateIdle hook (risk-mitigation gate, role-scoped).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_RISK_ROUNDS=1     -> keep teammates working while any risk in the plan's
#                            register is still pending mitigation.
#   TEAM_READONLY_ROLES    -> space-separated agent roles that CANNOT mitigate (no
#                            Write/Edit tool). They are exempt from the gate so a
#                            read-only worker (explorer/reviewer/…) is never held
#                            hostage by a risk it has no way to address.
# In .claude/team/plan.md, a risk is open while its line reads "Mitigation: pending"
# (case-insensitive); change it to "Mitigation: done" or "Mitigation: accepted".
#   exit 0 -> allow idle.   exit 2 -> keep working and send feedback on stderr.
[ "${TEAM_RISK_ROUNDS:-0}" = "1" ] || exit 0

payload=$(cat)

# Exempt read-only roles: this idling teammate cannot do mitigation work.
role=$(printf '%s' "$payload" | grep -oiE '"agent_type"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 | grep -oE '"[^"]*"$' | tr -d '"')
if [ -n "$role" ]; then
    role_lc=$(printf '%s' "$role" | tr 'A-Z' 'a-z')
    for r in ${TEAM_READONLY_ROLES:-}; do
        [ "$role_lc" = "$(printf '%s' "$r" | tr 'A-Z' 'a-z')" ] && exit 0
    done
fi

plan="${CLAUDE_PROJECT_DIR:-.}/.claude/team/plan.md"
if [ -f "$plan" ] && grep -qi '^[[:space:]]*[-*]\{0,1\}[[:space:]]*Mitigation:[[:space:]]*pending' "$plan"; then
    echo "Risks still need mitigation rounds. Address each 'Mitigation: pending' entry" >&2
    echo "in .claude/team/plan.md (mark it done or accepted) before going idle." >&2
    exit 2
fi
exit 0
