//! What counts as a high-impact external action (push, deploy, cloud mutation,
//! publish, remote shell). One pattern, shared verbatim with the approval-gate
//! script, applied to the command after stripping quotes and backslashes so that
//! `git "push"` or `g\it push` can't slip through.

use regex::Regex;
use std::sync::OnceLock;

/// Options (and their arguments) between a program and its subcommand, e.g.
/// `git -C . push`, `terraform -chdir=infra apply`, `kubectl -n prod delete`.
const OPT: &str = r"([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*";

/// The pattern's alternatives. `{OPT}` is replaced with [`OPT`]. Kept in sync with
/// `templates/claude/hooks/team-approval-gate.sh` (a test compares them).
pub const HIGH_IMPACT_PARTS: [&str; 17] = [
    r"(^|[^[:alnum:]_./-])(ssh|scp|sftp|rsync)([[:space:]]|$)",
    r"git{OPT}[[:space:]]+(push|merge)([[:space:]]|$)",
    r"gh[[:space:]]+(pr[[:space:]]+merge|release|repo[[:space:]]+delete|api[[:space:]])",
    r"aws[[:space:]]+[a-z0-9-]+[[:space:]]+[a-z0-9-]*(delete|terminate|create|put|update|modify|remove|rm|mv|cp|sync|stop|start|reboot|run-instances|deregister|detach|attach|associate|disassociate|scale|apply|invoke|publish|import|export|restore|purge|send-)",
    r"(gcloud|az)[[:space:]].*(delete|create|update|deploy|remove|apply)",
    r"(terraform|tofu){OPT}[[:space:]]+(apply|destroy)([[:space:]]|$)",
    r"kubectl{OPT}[[:space:]]+(apply|delete|create|replace|patch|scale|drain|cordon|uncordon)([[:space:]]|$)",
    r"helm{OPT}[[:space:]]+(install|upgrade|uninstall|delete|rollback)([[:space:]]|$)",
    r"docker[[:space:]]+(push|rm|rmi)([[:space:]]|$)",
    r"(npm|pnpm|yarn|bun)[[:space:]]+(run[[:space:]]+)?(publish|deploy)([[:space:]]|$)",
    r"cargo[[:space:]]+publish",
    r"twine[[:space:]]+upload",
    r"curl[[:space:]].*(-X[[:space:]]*(POST|PUT|DELETE|PATCH)|--request[[:space:]]*(POST|PUT|DELETE|PATCH)|(-d|--data)[[:space:]])",
    r"wget[[:space:]].*--method=(POST|PUT|DELETE|PATCH)",
    // A deploy/publish script being RUN (first word of a command, or after an
    // interpreter) — not merely read or grepped.
    r"(^|[;&|(][[:space:]]*|(sh|bash|zsh|python[0-9.]*|node|ruby)[[:space:]]+)([^[:space:]]*/)?(deploy|publish)[A-Za-z0-9_.-]*\.(sh|bash|py|js|ts|rb)([[:space:]]|$)",
    r"make([[:space:]]+[^[:space:]]+)*[[:space:]]+(deploy|publish)([[:space:]]|$)",
    r"ocgen[[:space:]]+approve|ocgen/?approvals",
];

/// The full alternation, as used by both implementations.
pub fn pattern() -> String {
    HIGH_IMPACT_PARTS
        .iter()
        .map(|p| p.replace("{OPT}", OPT))
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
pub fn high_impact(command: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(&pattern()).expect("high-impact pattern compiles"));
    re.find(&normalize(command))
        .map(|m| m.as_str().trim().to_string())
}
