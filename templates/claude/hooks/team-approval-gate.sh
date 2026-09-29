#!/bin/sh
# Claude Code Agent Teams — PreToolUse execution-approval gate.
#
# Deterministic human-approval line for HIGH-IMPACT EXTERNAL actions (substantial
# side effects on outside services). No agent may cross this line until a human
# reviews and unlocks execution by creating the marker file — from their own
# terminal, OUTSIDE the agent:
#
#     touch "$CLAUDE_PROJECT_DIR/.claude/team/execution-approved"
#
# Remove that file to re-lock. Decisions here come only from pattern matching and
# the marker check — never from model judgment. Enabled via TEAM_APPROVAL_GATE=1
# (set in .claude/settings.json). Wired on PreToolUse for Bash/file-write tools.
#   exit 2 -> block (stderr shown to the agent).   exit 0 -> allow.
[ "${TEAM_APPROVAL_GATE:-0}" = "1" ] || exit 0

payload=$(cat)
marker="${CLAUDE_PROJECT_DIR:-.}/.claude/team/execution-approved"

# Loop guard (shared). This gate NEVER allows a blocked action; after the budget
# it denies AND halts the agent (continue:false) so it stops retrying.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

# deny <reason> -> block this call; once the budget is spent, halt the agent too.
deny() {
    if lg_on; then
        who=$(lg_field "$payload" agent_type)
        [ -n "$who" ] || who=main
        lg_key "$payload" approval-gate "$who"
        if [ "$(lg_bump)" -ge "$LG_MAX" ]; then
            lg_escalate "halted after repeated high-impact attempts: $1"
            msg="Halted by the loop guard: repeated attempts at a gated high-impact action ($1). A human must review and approve from their own terminal; see .claude/loop-guard/escalations.md"
            printf '{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "%s"}, "continue": false, "stopReason": "%s"}\n' "$msg" "$msg"
            exit 0
        fi
    fi
    exit 2
}

# Self-protection: an agent must never create or modify the approval marker
# itself (that would let it approve its own high-impact actions). Any tool call
# that references the marker path is blocked outright.
if printf '%s' "$payload" | grep -q 'execution-approved'; then
    echo "Blocked: the execution-approval marker may only be created by a HUMAN," >&2
    echo "from a terminal outside the agent — never through a tool call." >&2
    deny "tried to create the approval marker"
fi

# Deterministic denylist of high-impact external / substantial-side-effect
# commands. Tune this to your environment — it is intentionally fail-closed.
HIGH_IMPACT='(^|[^[:alnum:]_./-])(ssh|scp|sftp|rsync)([[:space:]]|$)|git[[:space:]]+push|git[[:space:]]+merge|gh[[:space:]]+(pr[[:space:]]+merge|release|repo[[:space:]]+delete|api[[:space:]])|aws[[:space:]]+[a-z0-9-]+[[:space:]]+[a-z0-9-]*(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send-)|(gcloud|az)[[:space:]].*(delete|create|update|deploy|remove|apply)|terraform[[:space:]]+(apply|destroy)|kubectl[[:space:]]+(apply|delete|create|replace|patch|scale|drain|cordon|uncordon)|helm[[:space:]]+(install|upgrade|uninstall|delete|rollback)|docker[[:space:]]+(push|rm|rmi)|(npm|pnpm|yarn)[[:space:]]+publish|cargo[[:space:]]+publish|twine[[:space:]]+upload|curl[[:space:]].*(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--request[[:space:]]*(POST|PUT|DELETE|PATCH)|(-d|--data)[[:space:]])|wget[[:space:]].*--method=(POST|PUT|DELETE|PATCH)'

# Only Bash carries a shell command worth pattern-matching.
case "$payload" in
*'"tool_name":"Bash"'* | *'"tool_name": "Bash"'*)
    if printf '%s' "$payload" | grep -Eq "$HIGH_IMPACT"; then
        if [ ! -f "$marker" ]; then
            echo "BLOCKED by the execution-approval gate: this is a high-impact external" >&2
            echo "action (ssh / cloud mutation / git push|merge / deploy / publish / etc.)." >&2
            echo "A human must review, then unlock execution from their own terminal:" >&2
            echo "    touch \"$marker\"" >&2
            echo "No agent may create that marker. Delete it afterwards to re-lock." >&2
            deny "gated command without approval"
        fi
    fi
    ;;
esac
exit 0
