#!/bin/sh
# Claude Code — PreToolUse hook (matcher: WebFetch): the WebFetch guard.
# A fetch is allowed only over https:// and only to a trusted documentation site
# (OCGEN_WEBFETCH_DOMAINS, space-separated, set from `ocgen edit intent`; a
# leading "*." trusts every subdomain, not the bare domain). An empty or unset
# list trusts nothing, and a URL that can't be read is blocked too (fail closed).
# Deterministic: no model judgment.
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
https://*) ;;
http://*)
    echo "Blocked: plain http:// is not allowed for WebFetch in this project — fetch https://${url#*://} instead, or skip the page if it has no HTTPS version." >&2
    exit 2
    ;;
"")
    echo "Blocked: could not read the WebFetch URL, so it can't be checked against the trusted documentation sites." >&2
    exit 2
    ;;
*)
    echo "Blocked: only https:// URLs may be fetched in this project." >&2
    exit 2
    ;;
esac

# The host: drop the scheme, then path/query/fragment, user part, port and a trailing dot.
host=${lower#https://}
host=${host%%/*}
host=${host%%\?*}
host=${host%%#*}
host=${host##*@}
host=${host%%:*}
host=${host%.}

if [ -z "$(printf '%s' "${OCGEN_WEBFETCH_DOMAINS:-}" | tr -d '[:space:]')" ]; then
    echo "Blocked: no documentation sites are trusted in this project, so WebFetch is off. Ask the user to trust one with \`ocgen edit intent --trust-domain $host\`, or skip it." >&2
    exit 2
fi
set -f # "*.example.com" entries must not glob
for d in $OCGEN_WEBFETCH_DOMAINS; do
    d=$(printf '%s' "$d" | tr 'A-Z' 'a-z')
    case "$d" in
    \*.*)
        suffix=${d#\*}
        case "$host" in
        *"$suffix") [ "$host" != "${suffix#.}" ] && exit 0 ;;
        esac
        ;;
    *) [ "$host" = "$d" ] && exit 0 ;;
    esac
done
echo "Blocked: $host is not a trusted documentation site for this project (trusted: $OCGEN_WEBFETCH_DOMAINS). Ask the user to trust it with \`ocgen edit intent --trust-domain $host\`, or skip it." >&2
exit 2
