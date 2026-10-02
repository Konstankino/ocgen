#!/bin/sh
# Claude Code — PreToolUse hook (matcher: Bash): drop a no-op `cd`.
# A leading `cd <folder> && …` (or `;`) into the folder Claude is already in
# changes nothing, but after a `cd` Claude Code can't tell where relative paths
# point, so under permissions.blockReadsOutsideWorkingDirectories every such
# command asks. Claude Code only skips a `cd` spelled exactly like its working
# directory; this also matches a trailing "/", C:\ vs C:/ vs /c/, Windows case
# and the resolved path. The rest of the command goes back as updatedInput and is
# permission-checked as usual — this hook never allows, asks or blocks.
# Needs jq to rebuild the tool input; without it, every call is left alone.
payload=$(cat)
[ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1 || exit 0

# Same pattern as the Rust hook (hooks::NOOP_CD_RE): one plain or simply quoted
# target, then && or ;, then the rest.
re='\A[ \t\r\n]*cd[ \t]+(?:"(?<dq>[^"$`\r\n]*)"|'\''(?<sq>[^'\''\r\n]*)'\''|(?<bare>[^ \t\r\n"'\''$`\\;&|<>()*?\[\]~{}#]+))[ \t]*(?:&&|;)[ \t\r\n]*(?<rest>[^ \t\r\n;&|][\s\S]*)\z'
target=$(printf '%s' "$payload" | jq -r --arg re "$re" \
    '.tool_input.command // "" | capture($re) | .dq // .sq // .bare' 2>/dev/null) || exit 0
[ -n "$target" ] || exit 0
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty' 2>/dev/null)
[ -n "$cwd" ] || cwd=${CLAUDE_PROJECT_DIR:-}

# A folder path for comparing: "/" separators, MSYS /c/… as c:/…, no trailing
# "/" (but "/" and "c:/" stay), Windows paths lowercased.
norm() {
    p=$(printf '%s' "$1" | tr '\\' '/' | sed -E 's#^/([A-Za-z])(/+|$)#\1:/#; s#(.)/+$#\1#')
    case "$p" in
    [A-Za-z]:) p="$p/" ;;
    esac
    case "$p" in
    [A-Za-z]:*) p=$(printf '%s' "$p" | tr 'A-Z' 'a-z') ;;
    esac
    printf '%s' "$p"
}

same=
case "$target" in
. | ./) same=1 ;;
*'\\'*) ;;
*)
    if [ -n "$cwd" ]; then
        t=$(norm "$target")
        c=$(norm "$cwd")
        case "$t" in
        /* | [a-z]:/*)
            if [ "$t" = "$c" ]; then
                same=1
            else
                # Same folder once resolved (symlinks such as /tmp → /private/tmp)?
                rt=$(CDPATH= cd -- "$target" 2>/dev/null && pwd -P) &&
                    rc=$(CDPATH= cd -- "$cwd" 2>/dev/null && pwd -P) &&
                    [ "$(norm "$rt")" = "$(norm "$rc")" ] && same=1
            fi
            ;;
        esac
    fi
    ;;
esac
[ -n "$same" ] || exit 0

printf '%s' "$payload" | jq -c --arg re "$re" \
    '{hookSpecificOutput: {hookEventName: "PreToolUse",
      updatedInput: (.tool_input | .command = (.command | capture($re).rest))}}' 2>/dev/null
exit 0
