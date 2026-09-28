#!/bin/sh
# Claude Code Agent Teams — TaskCreated hook (plan-approval gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_PLAN_GATE=1 -> block new tasks until the shared plan is approved.
# The plan is approved when .claude/team/plan.md contains a line "Status: APPROVED".
#   exit 0 -> allow creation.   exit 2 -> block and send feedback on stderr.
[ "${TEAM_PLAN_GATE:-0}" = "1" ] || exit 0

plan="${CLAUDE_PROJECT_DIR:-.}/.claude/team/plan.md"
if [ ! -f "$plan" ] || ! grep -q '^Status: APPROVED' "$plan"; then
    echo "Plan not approved yet. Run /team-plan and get the plan APPROVED in" >&2
    echo ".claude/team/plan.md before creating execution tasks." >&2
    exit 2
fi
exit 0
