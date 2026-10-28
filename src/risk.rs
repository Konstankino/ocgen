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
    // An aws word that starts with a mutating verb (`cp`, `delete-bucket`), after
    // any options: `--profile dev` before the service can't hide one, and a read
    // that merely contains a verb (`list-attached-role-policies`) isn't one.
    r"(^|[^[:alnum:]_.-])aws([[:space:]]+[^[:space:];&|)<>`]+)*[[:space:]]+(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send)(-[a-z0-9-]*)?{END}",
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

/// A `git merge` call: `git`, its options, `merge`, then the arguments up to a
/// shell operator or the end of the line. `{OPT}` as in [`HIGH_IMPACT_PARTS`].
/// Kept in sync with `templates/claude/hooks/workstreams-merge-gate.sh` (a test
/// compares them).
pub const MERGE_CALL: &str = r"git{OPT}[[:space:]]+merge([[:space:]]+[^;&|)<>`]*)?([;&|)<>`]|$)";

/// The merge-call pattern as both implementations use it.
pub fn merge_pattern() -> String {
    MERGE_CALL.replace("{OPT}", OPT)
}

/// The arguments of every `git merge` in `command`, one list per call: the
/// words after `merge`, up to a shell operator, on the normalized line (quotes
/// and backslashes stripped, continued lines joined, as in [`high_impact`]).
/// Empty when nothing merges. A quoted value loses its quotes, so `-m "done"`
/// reads as `-m done`: the gate that uses this counts `done` as a branch.
pub fn merge_args(command: &str) -> Vec<Vec<String>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // The arguments' group, named: `{OPT}` holds groups of its own.
        let named = merge_pattern().replacen("merge(", "merge(?P<args>", 1);
        Regex::new(&named).expect("merge pattern compiles")
    });
    // A carriage return reads as a space, as the script's `tr` makes it.
    normalize(&command.replace("\\\n", ""))
        .replace('\r', " ")
        .split('\n')
        .flat_map(|line| {
            re.captures_iter(line)
                .map(|c| {
                    c.name("args")
                        .map(|m| m.as_str())
                        .unwrap_or("")
                        .split_whitespace()
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A git command with a `merge` word that [`MERGE_CALL`] didn't read as a call —
/// an option value with a space, as in `git -C "My Projects/x" merge …` once the
/// quotes are gone. Kept in sync with the gate script's `UNREAD`.
pub const UNREAD_MERGE: &str =
    r"(^|[^[:alnum:]_./-])git[[:space:]].*[[:space:]]merge([[:space:]]|$)";

/// Whether some line of `command` runs git with a `merge` word that
/// [`merge_args`] didn't read as a call (see [`UNREAD_MERGE`]): the gate that
/// uses this refuses it, fail closed.
pub fn unread_merge(command: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(UNREAD_MERGE).expect("unread-merge pattern compiles"));
    normalize(&command.replace("\\\n", ""))
        .replace('\r', " ")
        .split('\n')
        .any(|line| re.is_match(line))
}

/// Whether `command` holds an ASCII control character other than tab, newline or
/// carriage return. No ordinary command needs one, and it could hide a word from
/// the pattern, so the gate treats such a command as high-impact (fail closed).
pub fn has_control(command: &str) -> bool {
    command
        .chars()
        .any(|c| c.is_ascii_control() && !matches!(c, '\t' | '\n' | '\r'))
}

#[cfg(test)]
mod tests {
    use super::merge_args;

    #[test]
    fn merge_args_reads_each_git_merge_call() {
        assert!(merge_args("git status").is_empty());
        assert!(merge_args("git log --grep=merge").is_empty());
        assert!(merge_args("echo merge").is_empty());
        assert_eq!(merge_args("git merge"), vec![Vec::<String>::new()]);
        assert_eq!(
            merge_args("git merge --no-ff --no-edit refs/heads/x"),
            vec![vec!["--no-ff", "--no-edit", "refs/heads/x"]]
        );
        assert_eq!(
            merge_args("git -C wt merge refs/heads/x"),
            vec![vec!["refs/heads/x"]]
        );
        assert_eq!(
            merge_args("git merge refs/heads/x -m \"all done\""),
            vec![vec!["refs/heads/x", "-m", "all", "done"]]
        );
        assert_eq!(
            merge_args("git fetch origin && git merge refs/heads/x; git merge --abort"),
            vec![vec!["refs/heads/x"], vec!["--abort"]]
        );
        assert_eq!(
            merge_args("git merge refs/heads/x \\\n  refs/heads/y\ngit merge refs/heads/z"),
            vec![vec!["refs/heads/x", "refs/heads/y"], vec!["refs/heads/z"]]
        );
        assert_eq!(
            merge_args("(git merge refs/heads/x)"),
            vec![vec!["refs/heads/x"]]
        );
        assert_eq!(
            merge_args("git merge refs/heads/x\r"),
            vec![vec!["refs/heads/x"]]
        );
    }

    #[test]
    fn a_merge_hidden_by_a_spaced_option_value_is_still_seen() {
        use super::unread_merge;
        let hidden = "git -C \"/Users/dev/My Projects/x\" merge refs/heads/y";
        assert!(merge_args(hidden).is_empty());
        assert!(unread_merge(hidden));
        assert!(!unread_merge("git log --grep=merge"));
        assert!(!unread_merge("echo merge"));
        assert!(!unread_merge("git merge-base HEAD x"));
        assert!(
            unread_merge("git log --oneline merge"),
            "a plain merge word: fail closed"
        );
    }
}
