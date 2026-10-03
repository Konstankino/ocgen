//! What counts as a high-impact external action (push, deploy, cloud mutation,
//! publish, remote shell). One pattern, shared verbatim with the approval-gate
//! script, applied to the command after stripping quotes and backslashes so that
//! `git "push"` or `g\it push` can't slip through.
//!
//! It is a text match: it sees the command as written, not what the shell makes
//! of variables, aliases or scripts. A guard-rail, not a boundary. The boundary
//! is the sandbox: sandboxed commands can't write the approval store and run
//! without the credentials ocgen knows about, so a push or publish that needs
//! them fails however it is phrased — except one an OS keychain hands git
//! through a credential helper, which the sandbox can't hide (git starts with
//! no helper, but a command can name one: see `claude::GIT_HELPER_RESET`). For
//! pushes, server-side branch protection is what really stops one.

use regex::Regex;
use std::sync::OnceLock;

/// Options (and their arguments) between a program and its subcommand, e.g.
/// `git -C . push`, `terraform -chdir=infra apply`, `kubectl -n prod delete`.
const OPT: &str = r"([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*";

/// Where a subcommand word ends: whitespace, a shell operator (`git push;ls`,
/// `$(git push)`, `git push>log`) or the end of the line.
const END: &str = r"([[:space:];&|)<>`]|$)";

/// The pattern's alternatives. `{OPT}` and `{END}` are replaced with [`OPT`] and
/// [`END`]. Kept in sync with `templates/claude/hooks/team-approval-gate.sh` (a
/// test compares them).
pub const HIGH_IMPACT_PARTS: [&str; 20] = [
    r"(^|[^[:alnum:]_./-])(ssh|scp|sftp|rsync){END}",
    r"git{OPT}[[:space:]]+(push|merge){END}",
    // A git alias that pushes (`git -c alias.p=push p`, `git config alias.p push`).
    r"git[[:space:]].*alias\.[^=[:space:]]+[[:space:]=]+[^[:space:]]*(push|merge)",
    r"gh[[:space:]]+(pr[[:space:]]+merge|release|repo[[:space:]]+delete|api[[:space:]]|workflow[[:space:]]+run)",
    r"aws[[:space:]]+[a-z0-9-]+[[:space:]]+[a-z0-9-]*(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send-)",
    r"(gcloud|az)[[:space:]].*(delete|create|update|deploy|remove|apply)",
    r"(terraform|tofu){OPT}[[:space:]]+(apply|destroy){END}",
    r"pulumi{OPT}[[:space:]]+(up|update|destroy){END}",
    r"kubectl{OPT}[[:space:]]+(apply|delete|create|replace|patch|scale|drain|cordon|uncordon|set|rollout[[:space:]]+(restart|undo)){END}",
    r"helm{OPT}[[:space:]]+(install|upgrade|uninstall|delete|rollback){END}",
    r"docker{OPT}[[:space:]]+((image|container|compose){OPT}[[:space:]]+)?(push|rm|rmi){END}",
    r"docker[[:space:]].*[[:space:]]--push{END}",
    r"(npm|pnpm|yarn|bun){OPT}([[:space:]]+workspace[[:space:]]+[^[:space:]]+)?[[:space:]]+(run[[:space:]]+)?(publish|deploy){END}",
    r"cargo([[:space:]]+\+[^[:space:]]+)?{OPT}[[:space:]]+publish",
    r"twine[[:space:]]+upload",
    // An HTTP request that sends something: a mutating method, a body, a form or
    // a file (`-d`/`-T` may be bundled, as in `-sd @p`). Only curl's own words
    // count: the scan stops at a pipe or `&&`, so `curl … | cut -d: -f2` is a read.
    r"curl[[:space:]](([^|&]|&[^&])*[[:space:]])?(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--request[[:space:]=]*(POST|PUT|DELETE|PATCH)|-[A-Za-z]*[dT]|-[A-Za-z]*F[[:space:]]*[^=[:space:]]+=|--(data|data-binary|data-raw|data-urlencode|data-ascii|json|upload-file|form|form-string)([[:space:]=]|$))",
    r"wget[[:space:]].*(--method=(POST|PUT|DELETE|PATCH)|--(post|body)-(data|file))",
    // A deploy/publish script being RUN (first word of a command, or after an
    // interpreter) — not merely read or grepped.
    r"(^|[;&|(][[:space:]]*|(sh|bash|zsh|python[0-9.]*|node|ruby)[[:space:]]+)([^[:space:]]*/)?(deploy|publish)[A-Za-z0-9_.-]*\.(sh|bash|py|js|ts|rb){END}",
    r"make([[:space:]]+[^[:space:]]+)*[[:space:]]+(deploy|publish){END}",
    r"ocgen[[:space:]]+approve|ocgen/?approvals",
];

/// A command that would approve itself or touch the approval store
/// (`~/.claude/ocgen/approvals/`). Matched case-insensitively on the normalized
/// command joined into one line, so beyond the literal spellings it catches the
/// store reached in steps (`cd ~/.claude/ocgen && … > approvals/K`, `cd
/// ~/.claude && cd ocgen && … approvals`), through `.` or `//` segments, in
/// another case, by its Windows short names (`CLAUDE~1`, `APPROV~1`), or with
/// its name split by a variable (`d=~/.claude/ocg; … ${d}en/approvals/K`, via
/// `.claude` plus `approvals` inside a path or a word). `ocgen` counts only as
/// a whole name and `approvals` only inside a path, so the project's own
/// `.claude/.ocgen-state.json`, or `grep approvals .claude/…`, is ordinary work
/// — unless the folder after `.claude` may still turn out to be `ocgen`: a
/// glob or a variable there (`~/.claude/oc*`, `~/.claude/$x`), or a `cd` to
/// (or a variable set to) `.claude` or a start of `ocgen`, later followed by
/// one (`cd ~/.claude; cd oc*`, `d=~/.claude/ocg; cd ${d}en`); then a plain
/// `approvals` counts too. Pure indirection (`a=approv; …${a}als`, `$(…)`) and
/// a symlink made in an earlier call are beyond any text match; the sandbox's
/// `denyWrite` is the boundary there. Kept in sync with the gate script's
/// `SELF_APPROVAL` (a test compares them).
pub const SELF_APPROVAL: &str = r"ocgen[[:space:]]+approve|ocgen[/.]*approv(als|~)|(\.claude|claude~)[/.0-9]*ocgen([^-_[:alnum:]]|$)|(\.claude|claude~).*[^-_[:alnum:]]ocgen[^-_[:alnum:]].*approv(als|~)|(\.claude|claude~).*[^[:space:];&|()<>]approv(als|~)|[^[:space:];&|()<>]approv(als|~).*(\.claude|claude~)|(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?(\[|[*?{$`])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$).*approv(als|~)|approv(als|~).*(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?(\[|[*?{$`])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$)|((^|[^-_[:alnum:]])(cd|pushd)[[:space:]]+|[[:alnum:]_][[:space:]]*=[[:space:]]*)[^[:space:];&|()<>`]*(\.claude|claude~)[/.0-9]*(o|oc|ocg|ocge)?[[:space:];&|()<>`].*[^-_[:alnum:]](o|oc|ocg|ocge)?(\[|[*?{`]|\$[^(])[^./[:space:];&|()<>`]*([/[:space:];&|()<>`]|$).*approv(als|~)";

/// The full alternation, as used by both implementations.
pub fn pattern() -> String {
    HIGH_IMPACT_PARTS
        .iter()
        .map(|p| p.replace("{OPT}", OPT).replace("{END}", END))
        .collect::<Vec<_>>()
        .join("|")
}

/// Strip the characters a shell would remove before running the command. (This
/// also turns a Windows `ocgen\approvals` into `ocgenapprovals`, which the
/// pattern's `ocgen/?approvals` still matches.)
pub fn normalize(command: &str) -> String {
    command
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '\\'))
        .collect()
}

/// `Some(matched text)` when `command` performs a high-impact external action.
/// Lines continued with a trailing backslash are joined first, as the shell joins
/// them; then each line is matched on its own, exactly like `grep -E` in the gate
/// script — `^` and `$` hold at every line, and no match spans two commands.
pub fn high_impact(command: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(&pattern()).expect("high-impact pattern compiles"));
    normalize(&command.replace("\\\n", ""))
        .split('\n')
        .find_map(|line| re.find(line))
        .map(|m| m.as_str().trim().to_string())
}

/// Whether `command` would approve itself or touch the approval store (see
/// [`SELF_APPROVAL`]). Continued lines are joined first, as in [`high_impact`],
/// and the long s (`ſ`) is read as `s`, as APFS and NTFS do when they compare
/// names without case.
pub fn self_approval(command: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(&format!("(?i){SELF_APPROVAL}")).expect("self-approval pattern compiles")
    });
    re.is_match(
        &normalize(&command.replace("\\\n", ""))
            .replace('ſ', "s")
            .replace(['\n', '\r'], " "),
    )
}

/// Whether `command` holds an ASCII control character other than tab, newline or
/// carriage return. No ordinary command needs one, and it could hide a word from
/// the pattern, so the gate treats such a command as high-impact (fail closed).
pub fn has_control(command: &str) -> bool {
    command
        .chars()
        .any(|c| c.is_ascii_control() && !matches!(c, '\t' | '\n' | '\r'))
}
