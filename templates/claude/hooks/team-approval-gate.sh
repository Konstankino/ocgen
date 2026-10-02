#!/bin/sh
# Claude Code Agent Teams — PreToolUse execution-approval gate (every tool).
#
# A human-approval line for HIGH-IMPACT EXTERNAL actions (pushes, deploys, cloud
# mutations, publishes, remote shells): a tool call whose command matches stays
# blocked until a human approves — from their own terminal, OUTSIDE the agent:
#
#     ocgen approve            # unlocks for 30 minutes, then re-locks by itself
#
# The approval lives outside the project (~/.claude/ocgen/approvals/<project>), so
# an agent can't create it as a side effect of normal work, and it expires.
# Decisions come only from pattern matching and that file — never from model
# judgment. It matches command TEXT, so it is a guard-rail, not a boundary: the
# sandbox is what keeps an agent from pushing or deploying by other means.
# Enabled via TEAM_APPROVAL_GATE=1 (set in .claude/settings.json).
#   exit 2 -> block (stderr shown to the agent).   exit 0 -> allow.
[ "${TEAM_APPROVAL_GATE:-0}" = "1" ] || exit 0

payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"

# Loop guard (shared). This gate NEVER allows a blocked action; after the budget
# it denies AND halts the agent (continue:false) so it stops retrying.
lg_on() { return 1; }
lg_lib="$(dirname "$0")/loop-guard.sh"
[ -f "$lg_lib" ] && . "$lg_lib"

# The longest approval `ocgen approve` grants (24 h) plus slack, in seconds. An
# expiry further out wasn't written by it, so it is no approval (same number as
# ocgen's approval::MAX_AHEAD_SECS; a test compares them).
max_ahead=86700

# Whether a human approval is in force. The file is keyed by the repository root
# (shared by every worktree), computed byte by byte (LC_ALL=C) exactly like
# `ocgen approve` does — also for a non-ASCII path.
approved() {
    root=$(git -C "$proj" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
    if [ -n "$root" ]; then
        root=$(dirname "$root")
    else
        # `pwd -W` is Git Bash's Windows form (C:/Users/x), the one ocgen computes.
        root=$(cd "$proj" 2>/dev/null && { pwd -W 2>/dev/null || pwd -P; })
    fi
    key=$(printf '%s' "$root" | LC_ALL=C tr -c 'A-Za-z0-9._-' '_' | LC_ALL=C tail -c 200)
    marker="${HOME}/.claude/ocgen/approvals/${key}"
    [ -f "$marker" ] || return 1
    exp=$(tr -cd '0-9' <"$marker")
    now=$(date +%s)
    [ -n "$exp" ] && [ ${#exp} -le 12 ] && [ "$exp" -gt "$now" ] && [ "$exp" -le $((now + max_ahead)) ]
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

# The call's shell command (any tool: Bash, Monitor, PowerShell, MCP…), its
# working directory and its target files. jq when available; otherwise jall reads
# every string value of a key, each up to its real closing quote (one per line),
# and unjson decodes them. Unlike jq, jall also sees a key repeated in a nested
# object (an MCP tool's options), so every value is checked — and a repeated
# "command", which could hide the real one, is gated (fail closed).
jall() {
    printf '%s' "$payload" | tr '\n' ' ' |
        grep -oE '"('"$1"')"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' |
        sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//'
}
# repeated <key> -> whether the payload holds the key more than once
repeated() {
    [ -n "$(printf '%s' "$payload" | tr '\n' ' ' | grep -oE '"('"$1"')"[[:space:]]*:' | sed -n 2p)" ]
}
# unjson: decode a JSON string body — \\ first (so "\\n" stays a backslash and an
# n), then \n to a newline, \t and \r to spaces (all the same to the patterns),
# \" and \/ to themselves. \b, \f and \uXXXX are left as written: in a command
# they mean a control character (JSON escapes nothing else that way), which is
# gated below (fail closed).
p1=$(printf '\001')
p2=$(printf '\002')
unjson() {
    LC_ALL=C sed -e 's/\\\\/'"$p2"'/g' -e 's/\\n/'"$p1"'/g' -e 's/\\[tr]/ /g' \
        -e 's/\\"/"/g' -e 's#\\/#/#g' | LC_ALL=C tr "$p1$p2" '\n\\'
}
unsure= # set when the command may hide a word from the pattern: gated (fail closed)
nl='
'
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    cmd=$(printf '%s' "$payload" | jq -r '.tool_input.command | strings' 2>/dev/null)
    cwd=$(printf '%s' "$payload" | jq -r '.cwd | strings' 2>/dev/null)
    paths=$(printf '%s' "$payload" |
        jq -r '.tool_input | objects | (.file_path, .notebook_path, .path) | strings' 2>/dev/null)
else
    raw=$(jall command)
    case "$(printf '%s' "$raw" | sed 's/\\\\//g')" in
    *'\b'* | *'\f'* | *'\u'*) unsure=1 ;;
    esac
    ! repeated command || unsure=1
    cmd=$(printf '%s' "$raw" | unjson)
    cwd=$(jall cwd | unjson)
    paths=
    for k in file_path notebook_path path; do
        p=$(jall "$k" | unjson)
        [ -n "$p" ] && paths="$paths$p$nl"
    done
fi

self_approval() {
    echo "Blocked: only a HUMAN may approve execution, from a terminal outside the agent" >&2
    echo "(ocgen approve). No tool call may approve or touch the approval store." >&2
    deny "tried to approve itself"
}

# High-impact external actions. Generated from ocgen's risk pattern (a test keeps
# this line and the Rust constant identical; the binary applies the same one).
HIGH_IMPACT='(^|[^[:alnum:]_./-])(ssh|scp|sftp|rsync)([[:space:];&|)<>`]|$)|git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(push|merge)([[:space:];&|)<>`]|$)|git[[:space:]].*alias\.[^=[:space:]]+[[:space:]=]+[^[:space:]]*(push|merge)|gh[[:space:]]+(pr[[:space:]]+merge|release|repo[[:space:]]+delete|api[[:space:]]|workflow[[:space:]]+run)|aws[[:space:]]+[a-z0-9-]+[[:space:]]+[a-z0-9-]*(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send-)|(gcloud|az)[[:space:]].*(delete|create|update|deploy|remove|apply)|(terraform|tofu)([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(apply|destroy)([[:space:];&|)<>`]|$)|pulumi([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(up|update|destroy)([[:space:];&|)<>`]|$)|kubectl([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(apply|delete|create|replace|patch|scale|drain|cordon|uncordon|set|rollout[[:space:]]+(restart|undo))([[:space:];&|)<>`]|$)|helm([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+(install|upgrade|uninstall|delete|rollback)([[:space:];&|)<>`]|$)|docker([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+((image|container|compose)([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+)?(push|rm|rmi)([[:space:];&|)<>`]|$)|docker[[:space:]].*[[:space:]]--push([[:space:];&|)<>`]|$)|(npm|pnpm|yarn|bun)([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*([[:space:]]+workspace[[:space:]]+[^[:space:]]+)?[[:space:]]+(run[[:space:]]+)?(publish|deploy)([[:space:];&|)<>`]|$)|cargo([[:space:]]+\+[^[:space:]]+)?([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+publish|twine[[:space:]]+upload|curl[[:space:]](([^|&]|&[^&])*[[:space:]])?(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--request[[:space:]=]*(POST|PUT|DELETE|PATCH)|-[A-Za-z]*[dT]|-[A-Za-z]*F[[:space:]]*[^=[:space:]]+=|--(data|data-binary|data-raw|data-urlencode|data-ascii|json|upload-file|form|form-string)([[:space:]=]|$))|wget[[:space:]].*(--method=(POST|PUT|DELETE|PATCH)|--(post|body)-(data|file))|(^|[;&|(][[:space:]]*|(sh|bash|zsh|python[0-9.]*|node|ruby)[[:space:]]+)([^[:space:]]*/)?(deploy|publish)[A-Za-z0-9_.-]*\.(sh|bash|py|js|ts|rb)([[:space:];&|)<>`]|$)|make([[:space:]]+[^[:space:]]+)*[[:space:]]+(deploy|publish)([[:space:];&|)<>`]|$)|ocgen[[:space:]]+approve|ocgen/?approvals'

# Self-approval: `ocgen approve`, or any spelling of the approval store, matched
# without case on the whole command as one line (ocgen's risk::SELF_APPROVAL; a
# test keeps them identical).
SELF_APPROVAL='ocgen[[:space:]]+approve|ocgen[/.]*approv(als|~)|(\.claude|claude~)[/.0-9]*ocgen([^-_[:alnum:]]|$)|(\.claude|claude~).*[^-_[:alnum:]]ocgen[^-_[:alnum:]].*approv(als|~)|(\.claude|claude~).*[^[:space:];&|()<>]approv(als|~)|[^[:space:];&|()<>]approv(als|~).*(\.claude|claude~)|(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?(\[|[*?{$`])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$).*approv(als|~)|approv(als|~).*(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?(\[|[*?{$`])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$)|((^|[^-_[:alnum:]])(cd|pushd)[[:space:]]+|[[:alnum:]_][[:space:]]*=[[:space:]]*)[^[:space:];&|()<>`]*(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?[[:space:];&|()<>`].*[^-_[:alnum:]](o|oc|ocg|ocge)?(\[|[*?{`]|\$[^(])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$).*approv(als|~)'

if [ -n "$cmd" ]; then
    # Join lines continued with a trailing backslash (as the shell does), then
    # strip what a shell would strip, so `git "push"` can't slip through.
    ncmd=$(printf '%s\n' "$cmd" | awk '{ if (sub(/\\$/, "")) printf "%s", $0; else print }' |
        tr -d "\"'\\\\")
    # (The long s, \305\277, reads as s: APFS and NTFS fold it so.)
    longs=$(printf '\305\277')
    if printf '%s' "$ncmd" | tr '\n\r' '  ' | LC_ALL=C sed "s/$longs/s/g" |
        LC_ALL=C grep -Eiq "$SELF_APPROVAL"; then
        self_approval
    fi
    # A control character (other than tab/newline/CR) could hide a word: gated.
    if printf '%s' "$cmd" | LC_ALL=C tr -d '\t\n\r' | LC_ALL=C grep -q '[[:cntrl:]]'; then
        unsure=1
    fi
    if { [ -n "$unsure" ] || printf '%s\n' "$ncmd" | LC_ALL=C grep -Eq "$HIGH_IMPACT"; } && ! approved; then
        echo "BLOCKED by the execution-approval gate: this is a high-impact external" >&2
        echo "action (ssh / cloud mutation / git push|merge / deploy / publish / etc.)." >&2
        echo "A human must review, then approve from their own terminal (it expires by itself):" >&2
        echo "    ocgen approve" >&2
        echo "No agent may approve, and no tool call may touch the approval store." >&2
        deny "gated command without approval"
    fi
fi

# canon <path> <cwd> -> the path for matching (ocgen's hooks::canonical_path): "\"
# as "/", made absolute against cwd when relative, "//" squeezed, "." and ".."
# resolved lexically, ASCII-lowercased (APFS and NTFS ignore case).
canon() {
    c=$(printf '%s' "$1" | tr '\\' '/')
    case "$c" in
    /* | '~'* | ?:*) ;;
    *) [ -n "$2" ] && c="$(printf '%s' "$2" | tr '\\' '/')/$c" ;;
    esac
    out=
    c_ifs=$IFS
    IFS=/
    for seg in $c; do
        case "$seg" in
        '' | .) ;;
        ..) out=${out%/*} ;;
        *) out="$out/$seg" ;;
        esac
    done
    IFS=$c_ifs
    printf '%s' "$out" | LC_ALL=C tr 'A-Z' 'a-z'
}

# Any tool's target file or folder: never the approval store. "approv" also
# covers the names a case-insensitive disk takes for approvals (the Windows short
# name APPROV~1, a long s for the s). A relative path is tried against every
# working directory read (without jq, a nested "cwd" is one too).
set -f
IFS=$nl
for p in $paths; do
    for wd in ${cwd:-.}; do
        case "$(canon "$p" "$wd")" in
        *ocgen/approv*) self_approval ;;
        esac
    done
done
exit 0
