#!/bin/sh
# Claude Code — PostToolUse hook (Edit|Write): run the project's formatter after
# Claude edits a file — in the project directory, or for a file in another
# worktree of its repository (a worktree-isolated worker's own) in the project's
# folder there.
# OCGEN_FORMAT_CMD is set on the hook command in settings. With the project's
# sandbox on, the formatter runs in it (see below).
# Never blocks: a failing formatter is reported, not fatal.
[ -n "${OCGEN_FORMAT_CMD:-}" ] || exit 0
payload=$(cat)
if [ -z "${OCGEN_NO_JQ:-}" ] && command -v jq >/dev/null 2>&1; then
    file=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty | strings' 2>/dev/null)
else
    file=$(printf '%s' "$payload" | grep -oE '"file_path"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' | head -n 1 |
        sed -E 's/^"file_path"[[:space:]]*:[[:space:]]*"//; s/"$//; s/\\(["\\/])/\1/g')
fi
proj="${CLAUDE_PROJECT_DIR:-.}"
# common <dir> -> the repository <dir> belongs to, shared by all its worktrees
common() {
    c=$(git -C "$1" rev-parse --git-common-dir 2>/dev/null) || return 0
    case "$c" in
    /* | [A-Za-z]:/*) ;;
    *) c="$1/$c" ;;
    esac
    (CDPATH= cd -- "$c" 2>/dev/null && pwd -P)
}
# --- sandbox (the same in format.sh, subagent-confidence-gate.sh and
# team-task-completed.sh, and in the Rust hook) ---
# Claude Code runs hooks outside its sandbox, so with the project's sandbox on
# (OCGEN_SANDBOX=1) the project code a hook starts itself — the check, the
# formatter — runs in an OS sandbox of its own: no writes to the paths in
# OCGEN_SANDBOX_DENY_WRITE (nor to the withheld credentials), no reads of
# OCGEN_SANDBOX_DENY_READ, and OCGEN_SANDBOX_DENY_ENV unset. `~/` is the home
# folder; a relative path names one in the project and the same one in the
# command's folder. Everything else stays as the user has it. Seatbelt on macOS;
# bubblewrap on Linux, where a missing bwrap means the command doesn't run at
# all — never unsandboxed. Native Windows has no sandbox: commands run as before.

# sb_ready -> true when a project command may run, else sb_err says why not.
# Call it before leaving the hook's folder: it notes the project.
sb_ready() {
    sb_err=
    [ "${OCGEN_SANDBOX:-}" = 1 ] || return 0
    sb_os=$(uname -s 2>/dev/null)
    case "$sb_os" in
    Darwin)
        [ -f /usr/bin/sandbox-exec ] ||
            sb_err="the project's sandbox is on, but /usr/bin/sandbox-exec is missing, and ocgen never runs this command unsandboxed"
        ;;
    Linux)
        sb_bwrap=$(command -v bwrap) ||
            sb_err="the project's sandbox is on, but bubblewrap (bwrap) isn't installed, and ocgen never runs this command unsandboxed — install bubblewrap (e.g. sudo apt install bubblewrap); Claude Code's own sandbox needs it too"
        ;;
    *) return 0 ;;
    esac
    sb_proj=$(sb_real "${CLAUDE_PROJECT_DIR:-.}")
    [ -z "$sb_err" ]
}

# sb_real <path> -> the path as the kernel sees it: its longest existing part
# with every symlink resolved, then the rest as written
sb_real() {
    sb_p=$1
    case "$sb_p" in /*) ;; *) sb_p="$(pwd -P)/$sb_p" ;; esac
    sb_p=$(printf '%s\n' "$sb_p" | sed 's#//*#/#g; s#\(.\)/$#\1#')
    sb_rest=
    while [ ! -e "$sb_p" ] && [ "$sb_p" != / ]; do
        sb_rest="/${sb_p##*/}$sb_rest"
        sb_p=${sb_p%/*}
        [ -n "$sb_p" ] || sb_p=/
    done
    sb_n=0
    while [ -L "$sb_p" ] && [ "$sb_n" -lt 40 ]; do
        sb_l=$(readlink "$sb_p")
        case "$sb_l" in /*) sb_p=$sb_l ;; *) sb_p="${sb_p%/*}/$sb_l" ;; esac
        sb_n=$((sb_n + 1))
    done
    if [ -d "$sb_p" ]; then
        sb_p=$(cd -P -- "$sb_p" 2>/dev/null && pwd -P)
    else
        sb_l=$(cd -P -- "${sb_p%/*}/" 2>/dev/null && pwd -P)
        sb_p="${sb_l%/}/${sb_p##*/}"
    fi
    printf '%s%s\n' "${sb_p%/}" "$sb_rest"
}

# sb_paths <entries> <base> [rel] -> the paths the entries name, one per line,
# each once: `~/x` in the home folder, `/x` as is, anything else in <base>; with
# `rel`, only the relative ones. On Linux an entry in .claude/ stands for .claude
# itself: bubblewrap can only protect what exists, and a whole read-only .claude
# also keeps a missing settings file from being created.
sb_paths() (
    set -f
    unset IFS
    for sb_e in $1; do
        case "$sb_e" in
        '~' | '~/'*)
            [ -z "${3:-}" ] && [ -n "${HOME:-}" ] || continue
            sb_real "$HOME${sb_e#?}"
            ;;
        /*)
            [ -z "${3:-}" ] || continue
            sb_real "$sb_e"
            ;;
        *)
            sb_r=${sb_e#./}
            if [ "$sb_os" = Linux ]; then
                case "$sb_r" in .claude/*) sb_r=.claude ;; esac
            fi
            sb_real "$2/$sb_r"
            ;;
        esac
    done | awk '!seen[$0]++'
)

# sb_lists <dir> -> sets sb_w (to write-protect, in the project), sb_x (the same
# in <dir>, when it isn't the project) and sb_r (to hide)
sb_lists() {
    sb_w=$(sb_paths "${OCGEN_SANDBOX_DENY_WRITE:-}" "$sb_proj")
    sb_x=
    sb_r=$(sb_paths "${OCGEN_SANDBOX_DENY_READ:-}" "$sb_proj")
    if [ "$1" != "$sb_proj" ]; then
        sb_x=$(sb_paths "${OCGEN_SANDBOX_DENY_WRITE:-}" "$1" rel)
        sb_r=$(printf '%s\n%s\n' "$sb_r" "$(sb_paths "${OCGEN_SANDBOX_DENY_READ:-}" "$1" rel)" |
            sed '/^$/d' | awk '!seen[$0]++')
    fi
}

# sb_ancestors -> every folder above each path on stdin (shallowest first,
# once), but not /
sb_ancestors() {
    awk '{ n = split($0, c, "/"); p = ""; for (i = 2; i < n; i++) { p = p "/" c[i]; if (!seen[p]++) print p } }'
}

# sb_clause <operation> <filter> <paths> -> one Seatbelt deny clause (none
# without paths)
sb_clause() {
    [ -n "$3" ] || return 0
    printf '(deny %s' "$1"
    printf '%s\n' "$3" | sed 's/[\\"]/\\&/g' | while IFS= read -r sb_l; do
        printf ' (%s "%s")' "$2" "$sb_l"
    done
    printf ')'
}

# sb_profile <dir> -> the Seatbelt profile for a command run in <dir>: writes
# denied to the protected paths and the withheld credentials, reads denied to
# the credentials, and no folder above any of them renamed or removed (which
# would move a protected path out of the way)
sb_profile() {
    sb_lists "$1"
    sb_all=$(printf '%s\n%s\n%s\n' "$sb_w" "$sb_x" "$sb_r" | sed '/^$/d' | awk '!seen[$0]++')
    printf '(version 1)(allow default)'
    sb_clause 'file-write*' subpath "$sb_all"
    sb_clause file-write-unlink literal "$(printf '%s\n' "$sb_all" | sb_ancestors)"
    sb_clause 'file-read*' subpath "$sb_r"
}

# sb_unset -> unset the withheld variables (OCGEN_SANDBOX_DENY_ENV)
sb_unset() {
    set -f
    unset IFS
    for sb_v in ${OCGEN_SANDBOX_DENY_ENV:-}; do
        case "$sb_v" in '' | [0-9]* | *[!A-Za-z0-9_]*) ;; *) unset "$sb_v" ;; esac
    done
}

# sb_exec <command> [<launcher>...] -> after sb_ready, exec the command through
# sh -c in this folder, in the sandbox when the project's is on, the launcher
# (setsid) first. Linux: the whole system as it is; every folder above a
# protected path bound onto itself (a mount point can't be renamed away); the
# protected paths that exist read-only — then, when this folder lies inside one
# (a worker's worktree in .claude/), this folder writable again and its own
# protected paths read-only; last, an empty folder over each withheld
# credential folder and /dev/null over each file.
sb_exec() {
    sb_cmd=$1
    shift
    [ "${OCGEN_SANDBOX:-}" = 1 ] || exec "$@" sh -c "$sb_cmd"
    case "$sb_os" in Darwin | Linux) ;; *) exec "$@" sh -c "$sb_cmd" ;; esac
    sb_d=$(sb_real .)
    if [ "$sb_os" = Darwin ]; then
        sb_prof=$(sb_profile "$sb_d")
        sb_unset
        exec "$@" /usr/bin/sandbox-exec -p "$sb_prof" /bin/sh -c "$sb_cmd"
    fi
    sb_lists "$sb_d"
    set -f
    IFS='
'
    set -- "$@" "$sb_bwrap" --dev-bind / /
    for sb_t in $(printf '%s\n%s\n%s\n' "$sb_w" "$sb_x" "$sb_r" | sed '/^$/d' | sb_ancestors); do
        [ ! -d "$sb_t" ] || set -- "$@" --bind "$sb_t" "$sb_t"
    done
    sb_reopen=
    for sb_t in $sb_w; do
        [ -e "$sb_t" ] || continue
        set -- "$@" --ro-bind "$sb_t" "$sb_t"
        [ "$sb_d" = "$sb_proj" ] || case "$sb_d/" in "$sb_t"/*) sb_reopen=1 ;; esac
    done
    [ -z "$sb_reopen" ] || set -- "$@" --bind "$sb_d" "$sb_d"
    for sb_t in $sb_x; do
        [ ! -e "$sb_t" ] || set -- "$@" --ro-bind "$sb_t" "$sb_t"
    done
    for sb_t in $sb_r; do
        if [ -d "$sb_t" ]; then
            set -- "$@" --tmpfs "$sb_t"
        elif [ -e "$sb_t" ]; then
            set -- "$@" --ro-bind /dev/null "$sb_t"
        fi
    done
    sb_unset
    exec "$@" /bin/sh -c "$sb_cmd"
}
# --- end sandbox ---

# The project directory, unless the edited file is in another worktree of the
# project's repository (a worktree-isolated worker's): then the project's folder
# in that worktree. A file in an unrelated repository is not formatted there.
dir=
if [ -n "$file" ]; then
    case "$file" in
    [A-Za-z]:\\*) file=$(printf '%s' "$file" | tr '\\' '/') ;;
    esac
    case "$file" in
    /* | [A-Za-z]:/*) ;;
    *) file="$proj/$file" ;;
    esac
    fdir=$(dirname -- "$file")
    top=$(git -C "$fdir" rev-parse --show-toplevel 2>/dev/null)
    if [ -n "$top" ] && [ "$top" != "$(git -C "$proj" rev-parse --show-toplevel 2>/dev/null)" ]; then
        here=$(common "$fdir")
        if [ -n "$here" ] && [ "$here" = "$(common "$proj")" ]; then
            dir=$top
            prefix=$(git -C "$proj" rev-parse --show-prefix 2>/dev/null)
            [ -z "$prefix" ] || [ ! -d "$top/${prefix%/}" ] || dir="$top/${prefix%/}"
        fi
    fi
fi
if ! sb_ready; then
    echo "format hook: '$OCGEN_FORMAT_CMD' not run: $sb_err" >&2
    exit 0
fi
cd "${dir:-$proj}" 2>/dev/null || exit 0
if ! (sb_exec "$OCGEN_FORMAT_CMD") >/dev/null 2>&1; then
    echo "format hook: '$OCGEN_FORMAT_CMD' failed (ignored)" >&2
fi
exit 0
