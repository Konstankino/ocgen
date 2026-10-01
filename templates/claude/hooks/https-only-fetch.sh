#!/bin/sh
# Claude Code — PreToolUse hook (matcher: WebFetch): block plain-HTTP fetches.
# Documentation is fetched over https:// only; a WebFetch to an http:// URL is
# denied with a pointer to the https:// URL. Deterministic: no model judgment.
#   exit 2 -> block (stderr shown to the agent).   exit 0 -> allow.
payload=$(cat)

# The URL: jq when available, otherwise the "url" string value. JSON may escape
# "/" as "\/", so backslashes are dropped before comparing.
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    url=$(printf '%s' "$payload" | jq -r '.tool_input.url // empty' 2>/dev/null)
else
    url=$(printf '%s' "$payload" | tr '\n' ' ' | grep -oE '"url"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' |
        head -n 1 | sed -E 's/^"url"[[:space:]]*:[[:space:]]*"(.*)"$/\1/')
fi
url=$(printf '%s' "$url" | tr -d '\\' | sed -E 's/^[[:space:]]+//')
lower=$(printf '%s' "$url" | tr 'A-Z' 'a-z')
case "$lower" in
http://*)
    echo "Blocked: plain http:// is not allowed for WebFetch in this project — fetch https://${url#*://} instead, or skip the page if it has no HTTPS version." >&2
    exit 2
    ;;
esac
exit 0
