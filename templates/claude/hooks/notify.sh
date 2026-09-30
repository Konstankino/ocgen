#!/bin/sh
# Claude Code — Notification / StopFailure hook: a desktop notification when
# Claude needs your input or a turn fails. Never blocks.
payload=$(cat)
msg=$(printf '%s' "$payload" | grep -oE '"message"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 |
    sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/')
if [ -z "$msg" ]; then
    ev=$(printf '%s' "$payload" | grep -oE '"hook_event_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 |
        sed -E 's/^.*:[[:space:]]*"([^"]*)"$/\1/')
    msg="Claude Code: ${ev:-needs your attention}"
fi
msg=$(printf '%s' "$msg" | tr -d '"\\' | cut -c1-200)
if command -v osascript >/dev/null 2>&1; then
    osascript -e "display notification \"$msg\" with title \"Claude Code\"" >/dev/null 2>&1
elif command -v notify-send >/dev/null 2>&1; then
    notify-send "Claude Code" "$msg" >/dev/null 2>&1
else
    printf '\a' >/dev/tty 2>/dev/null
fi
exit 0
