#!/bin/sh
# Claude Code Agent Teams — TaskCreated hook (plan-approval gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_PLAN_GATE=1 -> block new tasks until the shared plan is approved.
# The plan is approved when .claude/team/plan.md contains a line "Status: APPROVED".
#   exit 0 -> allow creation.   exit 2 -> block and send feedback on stderr.
[ "${TEAM_PLAN_GATE:-0}" = "1" ] || exit 0

payload=$(cat)

# Loop guard (shared). This gate NEVER releases — an unapproved plan must not
# start work — but after the budget it tells the agent to stop retrying.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

plan="${CLAUDE_PROJECT_DIR:-.}/.claude/team/plan.md"
if lg_on; then
    who=$(lg_field "$payload" agent_type)
    [ -n "$who" ] || who=$(lg_field "$payload" agent_id)
    lg_key "$payload" plan-gate "${who:-lead}"
fi
if [ ! -f "$plan" ] || ! grep -q '^Status: APPROVED' "$plan"; then
    if lg_on && [ "$(lg_bump)" -ge "$LG_MAX" ]; then
        lg_escalate "kept creating tasks before the plan was approved"
        echo "STOP retrying: the plan is still not approved. Tell the lead that" >&2
        echo ".claude/team/plan.md needs 'Status: APPROVED' from the user, then go idle." >&2
        exit 2
    fi
    echo "Plan not approved yet. Run /team-plan and get the plan APPROVED in" >&2
    echo ".claude/team/plan.md before creating execution tasks." >&2
    exit 2
fi
lg_on && lg_reset
exit 0
