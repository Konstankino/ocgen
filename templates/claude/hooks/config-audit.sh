#!/bin/sh
# Claude Code — ConfigChange hook: log every settings/skills change made during a
# session to .claude/audit/config-changes.log (self-git-ignored).
#
# While a safety gate is on (the approval gate or the WebFetch guard), a change
# that would weaken it is also blocked, and Claude Code keeps the settings it
# loaded: disableAllHooks, TEAM_APPROVAL_GATE switched off, a WebFetch site added,
# or — in the project's own ocgen-generated settings.json — the gate's env, its
# hooks or this hook removed, or the file deleted. It is judged from the changed
# file's new text against the gate settings in force (this hook's environment),
# so it stops the plain ways, not a determined rewrite (a gate's hook narrowed
# with a matcher, or its command changed): the sandbox and the permission rules
# are the boundary there. A human who meant it restarts Claude Code. Policy
# settings (Claude Code can't block those) and skills are only logged.
#   exit 2 -> block (stderr shown).   exit 0 -> allow.
payload=$(cat)
proj="${CLAUDE_PROJECT_DIR:-.}"

# field <name> -> a top-level string of the event: jq when available, otherwise
# the raw value with \\, \" and \/ decoded.
field() {
    if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
        printf '%s' "$payload" | jq -r --arg k "$1" '.[$k] | strings' 2>/dev/null
    else
        printf '%s' "$payload" | tr '\n' ' ' |
            sed -nE 's/.*"'"$1"'"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*/\1/p' |
            sed -e 's/\\\\/\\/g' -e 's/\\"/"/g' -e 's#\\/#/#g'
    fi
}
src=$(field source)
file=$(field file_path)

# vals <key> -> every value of "key" in the new text, one "v:<value>" per line
# (quotes dropped), wherever it appears.
vals() {
    printf '%s' "$text" | grep -oE '"'"$1"'"[[:space:]]*:[[:space:]]*("([^"\\]|\\.)*"|[^,}[:space:]]+)' |
        sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*//; s/^"(.*)"$/\1/; s/^/v:/'
}

# weakened -> what the change would weaken, or nothing (ocgen's
# hooks::weakened_gate).
weakened() {
    case "$src" in policy_settings | skills) return ;; esac
    [ -n "$file" ] || return
    approval=
    [ "${TEAM_APPROVAL_GATE:-}" = "1" ] && approval=1
    [ -n "$approval" ] || [ -n "${OCGEN_WEBFETCH_DOMAINS+set}" ] || return
    case "$file" in
    /* | ?:*) f=$file ;;
    *) f="$proj/$file" ;;
    esac
    # The project's own settings.json, written by ocgen with the gate env.
    ours=
    [ "$src" = project_settings ] && [ -f "$proj/.claude/hooks/config-audit.sh" ] && ours=1
    if [ ! -f "$f" ]; then
        [ -n "$ours" ] && echo "deletes the project settings"
        return
    fi
    text=$(tr '\n\r' '  ' <"$f")
    if printf '%s' "$text" | grep -qE '"disableAllHooks"[[:space:]]*:[[:space:]]*true'; then
        echo "turns off every hook"
        return
    fi
    if [ -n "$approval" ]; then
        sw=$(vals TEAM_APPROVAL_GATE)
        if { [ -n "$sw" ] && printf '%s\n' "$sw" | grep -qv '^v:1$'; } ||
            { [ -n "$ours" ] && [ -z "$sw" ]; }; then
            echo "turns off the approval gate"
            return
        fi
        case "$text" in
        *team-approval-gate*) ;;
        *)
            [ -n "$ours" ] && echo "removes the approval gate hook" && return
            ;;
        esac
    fi
    if [ -n "${OCGEN_WEBFETCH_DOMAINS+set}" ]; then
        trusted=" $(printf '%s' "$OCGEN_WEBFETCH_DOMAINS" | LC_ALL=C tr 'A-Z' 'a-z' | tr -s '[:space:]' ' ') "
        set -f
        for d in $(vals OCGEN_WEBFETCH_DOMAINS | sed 's/^v://' | LC_ALL=C tr 'A-Z' 'a-z'); do
            case "$trusted" in
            *" $d "*) ;;
            *)
                echo "trusts another WebFetch site"
                return
                ;;
            esac
        done
        case "$text" in
        *https-only-fetch*) ;;
        *)
            [ -n "$ours" ] && echo "removes the WebFetch guard" && return
            ;;
        esac
    fi
    case "$text" in
    *config-audit*) ;;
    *) [ -n "$ours" ] && echo "removes this audit hook" ;;
    esac
}
what=$(weakened)

dir="$proj/.claude/audit"
if mkdir -p "$dir" 2>/dev/null; then
    [ -f "$dir/.gitignore" ] || printf '*\n' >"$dir/.gitignore"
    line="$(date -u +%Y-%m-%dT%H:%M:%SZ) ${src:-unknown} ${file:--}"
    [ -n "$what" ] && line="$line blocked: $what"
    printf '%s\n' "$line" >>"$dir/config-changes.log"
fi
if [ -n "$what" ]; then
    echo "Blocked: this settings change would weaken a safety gate ($what), so it is not applied to this session." >&2
    echo "If a human made it on purpose, restart Claude Code to load it. An agent may not change the gates." >&2
    exit 2
fi
exit 0
