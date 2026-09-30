#!/bin/sh
# Claude Code Agent Teams — PreToolUse execution-approval gate.
#
# Deterministic human-approval line for HIGH-IMPACT EXTERNAL actions (pushes,
# deploys, cloud mutations, publishes, remote shells). No agent may cross it until
# a human approves — from their own terminal, OUTSIDE the agent:
#
#     ocgen approve            # unlocks for 30 minutes, then re-locks by itself
#
# The approval lives outside the project (~/.claude/ocgen/approvals/<project>), so
# an agent can't create it as a side effect of normal work, and it expires.
# Decisions come only from pattern matching and that file — never from model
# judgment. Enabled via TEAM_APPROVAL_GATE=1 (set in .claude/settings.json).
#   exit 2 -> block (stderr shown to the agent).   exit 0 -> allow.
[ "${TEAM_APPROVAL_GATE:-0}" = "1" ] || exit 0

payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"

# Loop guard (shared). This gate NEVER allows a blocked action; after the budget
# it denies AND halts the agent (continue:false) so it stops retrying.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

# The approval file: keyed by the repository root (shared by every worktree), the
# same key `ocgen approve` computes.
root=$(git -C "$proj" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
if [ -n "$root" ]; then
    root=$(dirname "$root")
else
    # `pwd -W` is Git Bash's Windows form (C:/Users/x), the one ocgen computes.
    root=$(cd "$proj" 2>/dev/null && { pwd -W 2>/dev/null || pwd -P; })
fi
key=$(printf '%s' "$root" | tr -c 'A-Za-z0-9._-' '_' | tail -c 200)
marker="${HOME}/.claude/ocgen/approvals/${key}"
approved() {
    [ -f "$marker" ] || return 1
    exp=$(tr -cd '0-9' <"$marker")
    [ -n "$exp" ] && [ "$exp" -gt "$(date +%s)" ]
}

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

# The tool, its shell command and its target file. jq when available; otherwise
# jstr reads a JSON string value up to its real closing quote (escapes included).
jstr() {
    printf '%s' "$payload" | tr '\n' ' ' |
        sed -nE 's/.*"('"$1"')"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*/\2/p'
}
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    tool=$(printf '%s' "$payload" | jq -r '.tool_name // empty' 2>/dev/null)
    cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command // empty' 2>/dev/null)
    path=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' 2>/dev/null)
else
    tool=$(jstr tool_name)
    cmd=$(jstr command)
    path=$(jstr 'file_path|notebook_path')
fi
# Strip what a shell would strip, so `git "push"` can't slip through. Windows
# paths use backslashes, so compare the target with forward slashes.
cmd=$(printf '%s' "$cmd" | tr -d "\"'\\\\")
path=$(printf '%s' "$path" | tr '\\' '/' | tr -s '/')

self_approval() {
    echo "Blocked: only a HUMAN may approve execution, from a terminal outside the agent" >&2
    echo "(ocgen approve). No tool call may approve or touch the approval store." >&2
    deny "tried to approve itself"
}

# High-impact external actions. Generated from ocgen's risk pattern (a test keeps
# this line and the Rust constant identical) — tune it to your environment.
HIGH_IMPACT='(^|[^[:alnum:]_./-])(ssh|scp|sftp|rsync)([[:space:]]|$)|git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(push|merge)([[:space:]]|$)|gh[[:space:]]+(pr[[:space:]]+merge|release|repo[[:space:]]+delete|api[[:space:]])|aws[[:space:]]+[a-z0-9-]+[[:space:]]+[a-z0-9-]*(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send-)|(gcloud|az)[[:space:]].*(delete|create|update|deploy|remove|apply)|(terraform|tofu)([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(apply|destroy)([[:space:]]|$)|kubectl([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(apply|delete|create|replace|patch|scale|drain|cordon|uncordon)([[:space:]]|$)|helm([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(install|upgrade|uninstall|delete|rollback)([[:space:]]|$)|docker[[:space:]]+(push|rm|rmi)([[:space:]]|$)|(npm|pnpm|yarn|bun)[[:space:]]+(run[[:space:]]+)?(publish|deploy)([[:space:]]|$)|cargo[[:space:]]+publish|twine[[:space:]]+upload|curl[[:space:]].*(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--request[[:space:]]*(POST|PUT|DELETE|PATCH)|(-d|--data)[[:space:]])|wget[[:space:]].*--method=(POST|PUT|DELETE|PATCH)|(^|[;&|(][[:space:]]*|(sh|bash|zsh|python[0-9.]*|node|ruby)[[:space:]]+)([^[:space:]]*/)?(deploy|publish)[A-Za-z0-9_.-]*\.(sh|bash|py|js|ts|rb)([[:space:]]|$)|make([[:space:]]+[^[:space:]]+)*[[:space:]]+(deploy|publish)([[:space:]]|$)|ocgen[[:space:]]+approve|ocgen/?approvals'

case "$tool" in
Bash)
    if printf '%s' "$cmd" | grep -Eq 'ocgen[[:space:]]+approve|ocgen/?approvals'; then
        self_approval
    fi
    if printf '%s' "$cmd" | grep -Eq "$HIGH_IMPACT" && ! approved; then
        echo "BLOCKED by the execution-approval gate: this is a high-impact external" >&2
        echo "action (ssh / cloud mutation / git push|merge / deploy / publish / etc.)." >&2
        echo "A human must review, then approve from their own terminal (it expires by itself):" >&2
        echo "    ocgen approve" >&2
        echo "Without ocgen: echo \$(( \$(date +%s) + 1800 )) > \"$marker\"" >&2
        echo "No agent may approve." >&2
        deny "gated command without approval"
    fi
    ;;
*)
    case "$path" in
    *ocgen/approvals*) self_approval ;;
    esac
    ;;
esac
exit 0
