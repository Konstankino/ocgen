#!/bin/sh
# Claude Code Agent Teams — TaskCreated hook (plan-approval gate).
# Governance is driven by env vars set in .claude/settings.json:
#   TEAM_PLAN_GATE=1 -> block new tasks until the shared plan is approved.
# The plan is approved when .claude/team/plan.md has a line "Status: APPROVED"
# (/team-plan writes "Status: APPROVED session=<the approving session>"). The first
# event that sees a fresh line stamps it with the team run and a checksum of the
# plan without its Status lines:
#   Status: APPROVED session=… [ocgen: run=<team> plan=<crc>-<bytes>]
# A later team run, or any edit to the plan, leaves the approval stale until the
# line is written afresh — and so does a fresh line from another session. Closing
# a risk (its "Mitigation:" status) or ticking a task's checkbox is the team's
# progress, not an edit to the plan.
# The run is the event's team; the lead's own events carry none, so for them it
# is the team Claude Code makes for the lead's session, session-<first 8 of its
# id>. An event also belongs to that session's team (an in-process teammate
# shares the lead's session).
#   exit 0 -> allow creation.   exit 2 -> block and send feedback on stderr.
LC_ALL=C
export LC_ALL
[ "${TEAM_PLAN_GATE:-0}" = "1" ] || exit 0

payload=$(cat)

# field <name> -> the first string value of "name": "..."
field() {
    printf '%s' "$payload" | grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -n 1 |
        sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/'
}

# safe <text> -> a filename-safe token
safe() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_' | cut -c1-80; }

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

# The verdict: "" = approved, else why not.
why=unapproved
n=
[ ! -f "$plan" ] || n=$(grep -n '^Status: APPROVED' "$plan" | tail -n 1 | cut -d: -f1)
if [ -n "$n" ]; then
    line=$(sed -n "${n}p" "$plan")
    # Without the state the team changes as it works: a task's checkbox, and a
    # risk's mitigation (its line from "Mitigation:" on).
    sum=$(grep -v '^Status:' "$plan" |
        sed -E -e 's/^([[:space:]]*(([-*+]|[0-9]+[.)])[[:space:]]+)?)\[[xX ]\]/\1[ ]/' \
            -e 's/[Mm][Ii][Tt][Ii][Gg][Aa][Tt][Ii][Oo][Nn]:.*/Mitigation:/' |
        cksum | awk '{ print $1 "-" $2 }')
    sid=$(field session_id)
    own=
    [ -z "$sid" ] || own=$(safe "session-$(printf '%s' "$sid" | cut -c1-8)")
    run=$(safe "$(field team_name)")
    [ -n "$run" ] || run=$own
    [ -n "$run" ] || run=none
    # ours <run> -> whether a recorded run is this event's
    ours() { [ "$1" = "$run" ] || { [ -n "$own" ] && [ "$1" = "$own" ]; }; }
    stamp=$(printf '%s\n' "$line" | sed -n 's/.*\[ocgen: run=\([^] ][^] ]*\) plan=\([0-9][0-9]*-[0-9][0-9]*\)\].*/\1 \2/p')
    if [ -n "$stamp" ]; then
        if ! ours "${stamp% *}"; then
            why=other-run
        elif [ "${stamp#* }" != "$sum" ]; then
            why=changed
        else
            why=
        fi
    else
        by=$(printf '%s\n' "$line" | sed -n 's/.*session=\([A-Za-z0-9_-][A-Za-z0-9_-]*\).*/\1/p')
        if [ -n "$by" ] && ! ours "session-$(printf '%s' "$by" | cut -c1-8)"; then
            why=other-session
        else
            why=
            # A fresh approval: stamp it (never from `ocgen verify`'s probe).
            if [ "${OCGEN_HOOK_PROBE:-}" != 1 ]; then
                awk -v n="$n" -v s=" [ocgen: run=$run plan=$sum]" \
                    'NR == n { sub(/[[:space:]]+$/, ""); $0 = $0 s } { print }' "$plan" >"$plan.ocgen-$$" &&
                    mv "$plan.ocgen-$$" "$plan"
            fi
        fi
    fi
fi

if [ -n "$why" ]; then
    if lg_on && [ "$(lg_bump)" -ge "$LG_MAX" ]; then
        lg_escalate "kept creating tasks before the plan was approved"
        echo "STOP retrying: the plan is still not approved. Tell the lead that" >&2
        echo ".claude/team/plan.md needs 'Status: APPROVED' from the user, then go idle." >&2
        exit 2
    fi
    case "$why" in
    other-run)
        echo "Plan approval is stale: .claude/team/plan.md was approved for another team run." >&2
        echo "Run /team-plan for this goal and get it approved again before creating execution tasks." >&2
        ;;
    changed)
        echo "Plan approval is stale: .claude/team/plan.md changed after it was approved." >&2
        echo "Get the changed plan approved, then replace its Status line with a fresh 'Status: APPROVED'." >&2
        ;;
    other-session)
        echo "Plan approval is stale: .claude/team/plan.md was approved in another session." >&2
        echo "Run /team-plan in this session and get it approved again before creating execution tasks." >&2
        ;;
    *)
        echo "Plan not approved yet. Run /team-plan and get the plan APPROVED in" >&2
        echo ".claude/team/plan.md before creating execution tasks." >&2
        ;;
    esac
    exit 2
fi
lg_on && lg_reset
exit 0
