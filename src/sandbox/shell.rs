//! Native Windows: Claude Code has no sandbox there, so ocgen sandboxes the
//! Bash and PowerShell commands Claude runs itself. A PreToolUse hook
//! (`shell-sandbox`) rewrites each command into one that runs it in the
//! project's AppContainer (`ocgen __sandbox --bash|--powershell`, see
//! [`super::helper_main`]), with the original command left as it was in the
//! middle, readable ([`wrap`]).
//!
//! Claude Code checks its permission rules against the rewritten command,
//! which none of them match. So the hook checks them against the original
//! first ([`decide`]), the way Claude Code treats a command its own sandbox
//! runs (`sandbox.autoAllowBashIfSandboxed`): a deny rule denies it, an ask
//! rule asks, and anything else runs — sandboxed — without a prompt. Only
//! deny and ask rules count, so the command is split generously
//! ([`subcommands`]): a rule matching more than the shell would run asks or
//! denies once too often, never lets anything through.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A shell Claude Code runs commands with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    PowerShell,
}

impl Shell {
    /// The shell a Claude Code tool runs its `command` in.
    pub fn of_tool(name: &str) -> Option<Self> {
        match name {
            "Bash" => Some(Shell::Bash),
            "PowerShell" => Some(Shell::PowerShell),
            _ => None,
        }
    }

    /// The helper's flag for it.
    pub fn flag(self) -> &'static str {
        match self {
            Shell::Bash => "--bash",
            Shell::PowerShell => "--powershell",
        }
    }
}

/// The variable a PowerShell wrapper hands the command to the helper in.
pub const COMMAND_VAR: &str = "OCGEN_SANDBOX_COMMAND";

/// The heredoc delimiter a Bash wrapper starts from.
const DELIMITER: &str = "OCGEN_CMD";

// ------------------------------------------------------------- wrapping --

/// `s` in single quotes for sh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `s` in single quotes for PowerShell.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The command that runs `cmd` in the project's sandbox, through the helper
/// `exe`, with `cmd` itself unchanged and readable in it: in Bash a quoted
/// heredoc (nothing in it is expanded), in PowerShell a verbatim here-string
/// in [`COMMAND_VAR`] — or, when a line of `cmd` would end that, its base64.
pub fn wrap(shell: Shell, exe: &Path, project: &Path, cmd: &str) -> String {
    match shell {
        Shell::Bash => {
            let lines: Vec<&str> = cmd.lines().collect();
            let mut delim = DELIMITER.to_string();
            let mut n = 1;
            while lines.contains(&delim.as_str()) {
                delim = format!("{DELIMITER}_{n}");
                n += 1;
            }
            format!(
                "{} __sandbox --bash --project {} <<'{delim}'\n{cmd}\n{delim}",
                sh_quote(&crate::paths::for_shell(exe)),
                sh_quote(&crate::paths::for_shell(project)),
            )
        }
        Shell::PowerShell => {
            let exe = ps_quote(&crate::paths::plain(exe).to_string_lossy());
            let project = ps_quote(&crate::paths::plain(project).to_string_lossy());
            if cmd.lines().any(|l| l.trim_start().starts_with("'@")) {
                return format!(
                    "$env:{COMMAND_VAR} = '{}'\n& {exe} __sandbox --powershell --base64 --project {project}",
                    base64(cmd.as_bytes())
                );
            }
            format!(
                "$env:{COMMAND_VAR} = @'\n{cmd}\n'@\n& {exe} __sandbox --powershell --project {project}"
            )
        }
    }
}

/// The PowerShell drive the helper roots at the project: in an AppContainer,
/// PowerShell can't make the project's own path its location (it looks into
/// every folder above it, which the container can't), so it works in
/// `Project:\` instead, where relative paths are the project's.
pub const PROJECT_DRIVE: &str = "Project";

/// What the PowerShell helper runs: `cmd` in `dir` of `project` (through
/// [`PROJECT_DRIVE`]), with UTF-8 output, exiting the way Claude Code reads a
/// PowerShell command's result — the last program's exit code, else 0 or 1
/// by whether the last statement succeeded. It never runs `cmd` anywhere
/// else: if it can't get there, it says so and ends with 125.
pub fn powershell_script(cmd: &str, project: &Path, dir: &Path) -> String {
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let below: PathBuf = dir
        .components()
        .skip(project.components().count())
        .collect();
    let at = Path::new(&format!("{PROJECT_DRIVE}:\\")).join(below);
    format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n\
         $OutputEncoding = [System.Text.Encoding]::UTF8\n\
         try {{ $null = New-PSDrive -Name {PROJECT_DRIVE} -PSProvider FileSystem -Root {root} -Scope Global -ErrorAction Stop; \
         Set-Location -LiteralPath {at} -ErrorAction Stop }} \
         catch {{ [Console]::Error.WriteLine('ocgen sandbox: PowerShell can''t start in ' + {dirq} + ': ' + $_.Exception.Message); exit 125 }}\n\
         {cmd}\n\
         $ocgenOk = $?\n\
         exit $(if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($ocgenOk) {{ 0 }} else {{ 1 }})",
        root = quote(project),
        at = quote(&at),
        dirq = quote(dir),
    )
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64, padded.
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// [`base64`] read back; `None` for anything else.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        let mut pad = 0;
        for &c in chunk {
            let v = match c {
                b'=' => {
                    pad += 1;
                    0
                }
                _ if pad > 0 => return None,
                _ => B64.iter().position(|&x| x == c)? as u32,
            };
            n = (n << 6) | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad.min(2)]);
    }
    Some(out)
}

// ------------------------------------------------------------- matching --

/// One permission rule for a shell tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// As written: `Bash(git push:*)`.
    pub text: String,
    /// The settings file it is in, to name it by.
    pub from: String,
    shell: Shell,
    /// What is in the parentheses; `None` for the whole tool.
    pattern: Option<String>,
}

impl Rule {
    /// The rule `text` says, when it is one for a shell tool.
    pub fn parse(text: &str, from: &str) -> Option<Self> {
        let t = text.trim();
        let (tool, pattern) = match t.split_once('(') {
            Some((tool, rest)) => (tool, Some(rest.strip_suffix(')')?.trim().to_string())),
            None => (t, None),
        };
        Some(Rule {
            text: t.to_string(),
            from: from.to_string(),
            shell: Shell::of_tool(tool)?,
            pattern: pattern.filter(|p| !p.is_empty() && p != "*"),
        })
    }

    /// Whether the rule covers a command of `shell` (PowerShell commands are
    /// held to the Bash rules too).
    fn applies_to(&self, shell: Shell) -> bool {
        self.shell == shell || self.shell == Shell::Bash
    }

    /// Whether the rule matches one of `candidates` (see [`subcommands`]).
    fn matches(&self, candidates: &[String], fold: bool) -> bool {
        let Some(pat) = &self.pattern else {
            return true;
        };
        let norm = |s: &str| {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if fold {
                s.to_lowercase()
            } else {
                s
            }
        };
        let pat = norm(pat);
        candidates.iter().any(|c| pattern_matches(&pat, &norm(c)))
    }
}

/// Whether `cmd` matches a rule's pattern: `prefix:*` a command starting with
/// the prefix, `*` anything (a trailing ` *` the bare command too), else the
/// whole command.
fn pattern_matches(pat: &str, cmd: &str) -> bool {
    if let Some(prefix) = pat.strip_suffix(":*") {
        return cmd.starts_with(prefix.trim_end());
    }
    if !pat.contains('*') {
        return cmd == pat;
    }
    if let Some(bare) = pat.strip_suffix(" *") {
        if !bare.contains('*') && cmd == bare {
            return true;
        }
    }
    let re = pat
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    regex::Regex::new(&format!("(?s)^{re}$")).is_ok_and(|r| r.is_match(cmd))
}

/// The commands `cmd` runs, as permission rules see them — generously: each
/// part between `;`, `&`, `|`, newlines, parentheses, braces and command
/// substitutions; each also without leading variable assignments and the
/// wrappers Claude Code looks through (`time`, `timeout 5`, `env`, `nohup`…),
/// without quotes and backslashes, and with a program named by its path
/// named by its file name too; and the whole command. More candidates than
/// the shell runs only make a deny or ask rule match more often.
pub fn subcommands(shell: Shell, cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |s: String| {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    add(cmd.to_string());
    let parts = match shell {
        Shell::Bash => split_bash(cmd),
        Shell::PowerShell => split_powershell(cmd),
    };
    for part in parts {
        let peeled = peel(shell, &part);
        add(part);
        let plain = crate::risk::normalize(&peeled);
        for c in [peeled, plain] {
            if let Some(short) = by_name(&c) {
                add(short);
            }
            add(c);
        }
    }
    out
}

/// The parts of a Bash command between control operators, parentheses,
/// braces and command substitutions (`$(…)` and backticks run inside double
/// quotes too).
fn split_bash(cmd: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();
    let flush = |cur: &mut String, parts: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
        }
        cur.clear();
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') => {
                quote = None;
                cur.push(c);
            }
            (Some('\''), _) => cur.push(c),
            (_, '\\') => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some('"'), '"') => {
                quote = None;
                cur.push(c);
            }
            (_, '`' | ')') => flush(&mut cur, &mut parts),
            (_, '$') if chars.peek() == Some(&'(') => {
                chars.next();
                flush(&mut cur, &mut parts);
            }
            (Some(_), _) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, ';' | '&' | '|' | '\n' | '\r' | '(' | '{' | '}') => flush(&mut cur, &mut parts),
            (None, _) => cur.push(c),
        }
    }
    flush(&mut cur, &mut parts);
    parts
}

/// The parts of a PowerShell command between statements, pipes, `&&`/`||`,
/// the call operator, parentheses, braces and subexpressions (`$(…)` runs
/// inside double quotes too).
fn split_powershell(cmd: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();
    let flush = |cur: &mut String, parts: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
        }
        cur.clear();
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') => {
                quote = None;
                cur.push(c);
            }
            (Some('\''), _) => cur.push(c),
            (_, '`') => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some('"'), '"') => {
                quote = None;
                cur.push(c);
            }
            (_, ')') => flush(&mut cur, &mut parts),
            (_, '$') if chars.peek() == Some(&'(') => {
                chars.next();
                flush(&mut cur, &mut parts);
            }
            (Some(_), _) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, ';' | '&' | '|' | '\n' | '\r' | '(' | '{' | '}') => flush(&mut cur, &mut parts),
            (None, _) => cur.push(c),
        }
    }
    flush(&mut cur, &mut parts);
    parts
}

/// The wrappers Claude Code looks through before it matches a rule (and
/// `exec`, `sudo`), with the options that take a value.
const WRAPPERS: [(&str, &[&str]); 11] = [
    ("time", &[]),
    ("nohup", &[]),
    ("timeout", &["-s", "-k", "--signal", "--kill-after"]),
    ("nice", &["-n", "--adjustment"]),
    ("stdbuf", &["-i", "-o", "-e"]),
    ("env", &["-u", "-C", "-S", "--unset", "--chdir"]),
    ("command", &[]),
    ("builtin", &[]),
    ("noglob", &[]),
    ("exec", &["-a"]),
    ("sudo", &["-u", "-g", "-h", "-p", "-C"]),
];

/// `part` without its leading variable assignments and wrappers.
fn peel(shell: Shell, part: &str) -> String {
    if shell == Shell::PowerShell {
        // `$x = git push` assigns what `git push` prints.
        let re = regex::Regex::new(r"^\$[\w:]+\s*=\s*(.+)$").expect("compiles");
        return re
            .captures(part)
            .map_or_else(|| part.to_string(), |c| c[1].to_string());
    }
    let words: Vec<&str> = part.split_whitespace().collect();
    let assign = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").expect("compiles");
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if assign.is_match(w) {
            i += 1;
            continue;
        }
        let Some((name, takes)) = WRAPPERS.iter().find(|(n, _)| *n == w) else {
            break;
        };
        i += 1;
        while i < words.len() && words[i].starts_with('-') {
            let takes_value = takes.contains(&words[i]) && !words[i].contains('=');
            i += if takes_value { 2 } else { 1 };
        }
        // `timeout 5 cmd`: the duration.
        if *name == "timeout" && i < words.len() {
            i += 1;
        }
    }
    words[i.min(words.len())..].join(" ")
}

/// `cmd` with a program named by its path (`/usr/bin/git`, `C:\Git\git.exe`)
/// named by its file name instead, when it is.
fn by_name(cmd: &str) -> Option<String> {
    let (first, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let first = first.trim_matches(['\'', '"']);
    if !first.contains(['/', '\\']) {
        return None;
    }
    let name = first.rsplit(['/', '\\']).next()?;
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    Some(format!("{name} {rest}").trim().to_string())
}

/// The deny and ask rules for the shell tools in the settings Claude Code
/// merges, and whether it lets a sandboxed command run without a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub deny: Vec<Rule>,
    pub ask: Vec<Rule>,
    /// `sandbox.autoAllowBashIfSandboxed` (on unless a file turns it off).
    pub auto_allow: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            deny: Vec::new(),
            ask: Vec::new(),
            auto_allow: true,
        }
    }
}

impl Rules {
    /// The rules in `files` (each with the name to report it by), least
    /// specific first: the lists add up, the most specific value wins.
    pub fn from_settings(files: &[(String, Value)]) -> Self {
        let mut r = Rules::default();
        for (from, v) in files {
            for (key, list) in [("deny", &mut r.deny), ("ask", &mut r.ask)] {
                let entries = v
                    .pointer(&format!("/permissions/{key}"))
                    .and_then(Value::as_array);
                for e in entries.into_iter().flatten().filter_map(Value::as_str) {
                    if let Some(rule) = Rule::parse(e, from) {
                        list.push(rule);
                    }
                }
            }
            if let Some(b) = v
                .pointer("/sandbox/autoAllowBashIfSandboxed")
                .and_then(Value::as_bool)
            {
                r.auto_allow = b;
            }
        }
        r
    }

    /// The rules for `project`: the user's settings, the project's, its local
    /// ones and the managed policy (`--settings` and command-line rules are
    /// out of a hook's sight).
    pub fn load(project: &Path) -> Self {
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        if let Some(user) = crate::verify::user_settings() {
            files.push(user);
        }
        for rel in [".claude/settings.json", ".claude/settings.local.json"] {
            files.push((project.join(rel), rel.to_string()));
        }
        let managed = crate::verify::managed_settings();
        let label = format!("the managed policy ({})", managed.display());
        files.push((managed, label));
        let read: Vec<(String, Value)> = files
            .into_iter()
            .filter_map(|(p, label)| {
                let v = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
                Some((label, v))
            })
            .collect();
        Rules::from_settings(&read)
    }
}

/// What the hook decides for a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// A deny rule matches.
    Deny(Rule),
    /// An ask rule matches.
    Ask(Rule),
    /// No rule matches: it runs sandboxed without a prompt.
    Allow,
    /// No rule matches, and Claude Code decides (plan or auto mode, a
    /// project that turns `autoAllowBashIfSandboxed` off).
    Pass,
}

/// The modes in which Claude Code lets a sandboxed command run unasked.
const AUTO_ALLOW_MODES: [&str; 4] = ["", "default", "acceptEdits", "dontAsk"];

/// The decision for `cmd` of `shell` under `rules` in permission `mode`.
pub fn decide(shell: Shell, cmd: &str, rules: &Rules, mode: &str) -> Decision {
    let candidates = subcommands(shell, cmd);
    let fold = shell == Shell::PowerShell;
    let hit = |list: &[Rule]| {
        list.iter()
            .find(|r| {
                r.applies_to(shell) && r.matches(&candidates, fold || r.shell == Shell::PowerShell)
            })
            .cloned()
    };
    if let Some(r) = hit(&rules.deny) {
        return Decision::Deny(r);
    }
    if let Some(r) = hit(&rules.ask) {
        return Decision::Ask(r);
    }
    if rules.auto_allow && AUTO_ALLOW_MODES.contains(&mode) {
        Decision::Allow
    } else {
        Decision::Pass
    }
}

// ----------------------------------------------------------------- hook --

/// What the `shell-sandbox` hook works from, for one PreToolUse event.
pub struct Request<'a> {
    /// The event.
    pub event: &'a Value,
    /// The project's folder (`CLAUDE_PROJECT_DIR`).
    pub project: &'a Path,
    /// The ocgen binary the wrapper runs.
    pub exe: &'a Path,
    pub rules: &'a Rules,
    /// Whether the project's container may run commands yet, and if not, why.
    pub ready: Result<(), String>,
}

/// The hook's answer to `r.event` as JSON — `None` to say nothing (a tool
/// without a shell command).
pub fn pre_tool_use(r: &Request) -> Option<Value> {
    let shell = Shell::of_tool(r.event.get("tool_name")?.as_str()?)?;
    let input = r.event.get("tool_input").filter(|v| v.is_object())?;
    let cmd = input.get("command").and_then(Value::as_str)?;
    if cmd.trim().is_empty() {
        return None;
    }
    let cwd = r
        .event
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map_or_else(|| r.project.to_path_buf(), PathBuf::from);
    let (here, project) = (
        super::policy::real_path(&cwd),
        super::policy::real_path(r.project),
    );
    if !super::policy::is_within(&here, &project) {
        return Some(deny(format!(
            "ocgen runs the commands Claude runs on Windows in the project's sandbox, which covers only the project's folder ({}), and this one would run in {}: `cd` back into the project first (in its own command).",
            project.display(),
            here.display()
        )));
    }
    if let Err(why) = &r.ready {
        return Some(deny(why.clone()));
    }
    let (decision, reason) = match decide(shell, cmd, r.rules, mode(r.event)) {
        Decision::Deny(rule) => {
            return Some(deny(format!(
                "denied by the permission rule {} in {} (ocgen checks the rules against the command itself, since it runs the command in the project's sandbox)",
                rule.text, rule.from
            )))
        }
        Decision::Ask(rule) => (
            Some("ask"),
            format!(
                "{} in {} asks first; ocgen runs the command below in the project's sandbox",
                rule.text, rule.from
            ),
        ),
        Decision::Allow => (
            Some("allow"),
            "runs in the project's sandbox (no permission rule denies it or asks)".to_string(),
        ),
        Decision::Pass => (None, String::new()),
    };
    let mut up = input.clone();
    up["command"] = Value::String(wrap(shell, r.exe, r.project, cmd));
    let mut out = serde_json::json!({ "hookEventName": "PreToolUse", "updatedInput": up });
    if let Some(d) = decision {
        out["permissionDecision"] = d.into();
        out["permissionDecisionReason"] = reason.into();
    }
    Some(serde_json::json!({ "hookSpecificOutput": out }))
}

/// The event's permission mode.
fn mode(event: &Value) -> &str {
    event
        .get("permission_mode")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A PreToolUse denial saying `why`.
fn deny(why: String) -> Value {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": why,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(deny: &[&str], ask: &[&str]) -> Rules {
        Rules::from_settings(&[(
            ".claude/settings.json".into(),
            serde_json::json!({ "permissions": { "deny": deny, "ask": ask } }),
        )])
    }

    #[test]
    fn a_bash_wrapper_keeps_the_command_as_written() {
        let exe = Path::new("/opt/ocgen's/ocgen");
        let cmd = "git status && echo \"$HOME\" `id`\ncat <<EOF\nhi\nEOF";
        let w = wrap(Shell::Bash, exe, Path::new("/w/app"), cmd);
        assert_eq!(
            w,
            format!("'/opt/ocgen'\\''s/ocgen' __sandbox --bash --project '/w/app' <<'OCGEN_CMD'\n{cmd}\nOCGEN_CMD")
        );
        // A line that would end the heredoc early picks another delimiter.
        let w = wrap(
            Shell::Bash,
            exe,
            Path::new("/w"),
            "echo a\nOCGEN_CMD\nOCGEN_CMD_1",
        );
        assert!(w.ends_with("\nOCGEN_CMD_2"), "{w}");
        assert!(w.contains("<<'OCGEN_CMD_2'\n"), "{w}");
    }

    /// The helper gets the command byte for byte, through a real bash, the way
    /// Claude Code runs one (`eval`).
    #[cfg(unix)]
    #[test]
    fn bash_hands_the_helper_the_command_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let fake = t.path().join("ocgen helper");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s|' \"$@\"\nprintf '\\n---\\n'\ncat\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        for cmd in [
            "echo \"$HOME\" '$PATH' `id -u` $(pwd) \\\\n",
            "cat <<EOF\nhi $USER\nEOF\nOCGEN_CMD\ngit status   ",
            "printf 'ünï\\t%s\\n' x; exit 3",
        ] {
            let w = wrap(Shell::Bash, &fake, Path::new("/w/it's"), cmd);
            let out = std::process::Command::new("bash")
                .args(["-c", "eval \"$WRAP\" < /dev/null"])
                .env("WRAP", &w)
                .output()
                .unwrap();
            let out = String::from_utf8(out.stdout).unwrap();
            assert_eq!(
                out,
                format!("__sandbox|--bash|--project|/w/it's|\n---\n{cmd}\n"),
                "{w}"
            );
        }
    }

    #[test]
    fn a_powershell_wrapper_keeps_the_command_as_written() {
        let cmd = "Get-ChildItem | Select-Object Name\nWrite-Output 'it''s'";
        let w = wrap(
            Shell::PowerShell,
            Path::new("/x/ocgen"),
            Path::new("/w/it's"),
            cmd,
        );
        assert_eq!(
            w,
            format!("$env:OCGEN_SANDBOX_COMMAND = @'\n{cmd}\n'@\n& '/x/ocgen' __sandbox --powershell --project '/w/it''s'")
        );
        // A line starting `'@` would end the here-string: base64 instead.
        let odd = "Write-Output @'\nx\n'@";
        let w = wrap(
            Shell::PowerShell,
            Path::new("/x/ocgen"),
            Path::new("/w"),
            odd,
        );
        assert!(w.contains("--base64"), "{w}");
        let b64 = w.split('\'').nth(1).unwrap();
        assert_eq!(unbase64(b64).unwrap(), odd.as_bytes());
    }

    #[test]
    fn base64_round_trips() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "ünï\n'@"] {
            let e = base64(s.as_bytes());
            assert_eq!(unbase64(&e).unwrap(), s.as_bytes(), "{s:?} → {e}");
        }
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(unbase64("Zm9v!"), None);
    }

    #[test]
    fn the_powershell_script_exits_as_claude_code_reads_it() {
        let s = powershell_script("git status", Path::new("/p"), Path::new("/p"));
        assert!(s.contains("\ngit status\n$ocgenOk = $?\n"), "{s}");
        assert!(s.ends_with("elseif ($ocgenOk) { 0 } else { 1 })"), "{s}");
        assert!(s.starts_with("[Console]::OutputEncoding"), "{s}");
    }

    /// It starts in the command's folder through a drive rooted at the
    /// project (a quote in a path doubled), and never runs the command
    /// elsewhere.
    #[test]
    fn the_powershell_script_starts_in_the_commands_folder() {
        let s = powershell_script("ls", Path::new("/w/it's"), Path::new("/w/it's/src/a"));
        assert!(
            s.contains("New-PSDrive -Name Project -PSProvider FileSystem -Root '/w/it''s'"),
            "{s}"
        );
        let at = Path::new("Project:\\").join("src").join("a");
        assert!(
            s.contains(&format!("Set-Location -LiteralPath '{}'", at.display())),
            "{s}"
        );
        assert!(s.contains("exit 125 }\nls\n"), "{s}");
    }

    #[test]
    fn subcommands_cover_every_part_a_rule_could_match() {
        let c = subcommands(
            Shell::Bash,
            "cd x && FOO=1 timeout 5 git \"push\" origin; echo $(npm publish) | tee y\n(env -i /usr/bin/git push) & `kubectl apply -f k`",
        );
        for want in [
            "cd x",
            "git push origin",
            "npm publish",
            "tee y",
            "git push",
            "kubectl apply -f k",
        ] {
            assert!(c.iter().any(|x| x == want), "{want:?} not in {c:?}");
        }
        let c = subcommands(
            Shell::Bash,
            "nice -n 5 nohup stdbuf -oL sudo -u x terraform apply",
        );
        assert!(c.iter().any(|x| x == "terraform apply"), "{c:?}");
        // Quoted operators are not split.
        let c = subcommands(Shell::Bash, "echo 'a; git push' \"b | c\"");
        assert!(!c.iter().any(|x| x == "git push"), "{c:?}");
        let c = subcommands(
            Shell::PowerShell,
            "Get-Item x; & 'C:\\Git\\cmd\\git.exe' push | Out-Null; $r = npm publish",
        );
        for want in ["Get-Item x", "git push", "npm publish"] {
            assert!(c.iter().any(|x| x == want), "{want:?} not in {c:?}");
        }
    }

    #[test]
    fn rules_match_like_claude_codes() {
        let m = |rule: &str, cmd: &str| {
            Rule::parse(rule, "s")
                .unwrap()
                .matches(&subcommands(Shell::Bash, cmd), false)
        };
        assert!(m("Bash(git push:*)", "git push origin main"));
        assert!(m("Bash(git push:*)", "git push"));
        assert!(m("Bash(git push*)", "git pushx"));
        assert!(m("Bash(ls *)", "ls -la") && m("Bash(ls *)", "ls"));
        assert!(!m("Bash(ls *)", "lsof"));
        assert!(m("Bash", "anything at all") && m("Bash(*)", "x"));
        assert!(m("Bash(npm test)", "npm test") && !m("Bash(npm test)", "npm test --watch"));
        assert!(m(
            "Bash(curl * --upload-file *)",
            "curl -s --upload-file f https://x"
        ));
        assert!(m("Bash(ocgen approve*)", "echo hi; ocgen approve"));
        assert!(!m("Bash(git push:*)", "echo 'git push'"));
        assert!(Rule::parse("Read(./.env)", "s").is_none());
        assert!(Rule::parse("WebFetch", "s").is_none());
    }

    #[test]
    fn deny_beats_ask_beats_running_unasked() {
        let r = rules(&["Bash(ocgen approve*)"], &["Bash(git push:*)"]);
        assert!(
            matches!(decide(Shell::Bash, "ocgen approve", &r, "default"), Decision::Deny(x) if x.text == "Bash(ocgen approve*)")
        );
        assert!(matches!(
            decide(Shell::Bash, "git push && ocgen approve", &r, "default"),
            Decision::Deny(_)
        ));
        assert!(
            matches!(decide(Shell::Bash, "git add . && git push", &r, "default"), Decision::Ask(x) if x.from == ".claude/settings.json")
        );
        assert_eq!(
            decide(Shell::Bash, "cargo test", &r, "default"),
            Decision::Allow
        );
        assert_eq!(
            decide(Shell::Bash, "cargo test", &r, "acceptEdits"),
            Decision::Allow
        );
        // Claude Code decides in plan and auto mode…
        assert_eq!(
            decide(Shell::Bash, "cargo test", &r, "plan"),
            Decision::Pass
        );
        assert_eq!(
            decide(Shell::Bash, "cargo test", &r, "auto"),
            Decision::Pass
        );
        assert_eq!(
            decide(Shell::Bash, "cargo test", &r, "bypassPermissions"),
            Decision::Pass
        );
        // …but a deny or an ask rule still holds there.
        assert!(matches!(
            decide(Shell::Bash, "git push", &r, "auto"),
            Decision::Ask(_)
        ));
        assert!(matches!(
            decide(Shell::Bash, "ocgen approve", &r, "bypassPermissions"),
            Decision::Deny(_)
        ));
        // A project that turns the sandbox's auto-allow off is asked as usual.
        let off = Rules::from_settings(&[(
            "s".into(),
            serde_json::json!({ "sandbox": { "autoAllowBashIfSandboxed": false } }),
        )]);
        assert_eq!(
            decide(Shell::Bash, "cargo test", &off, "default"),
            Decision::Pass
        );
    }

    #[test]
    fn powershell_commands_answer_to_both_kinds_of_rule() {
        let r = rules(&["PowerShell(Remove-Item*)"], &["Bash(git push:*)"]);
        assert!(matches!(
            decide(Shell::PowerShell, "remove-item -Recurse x", &r, "default"),
            Decision::Deny(_)
        ));
        assert!(matches!(
            decide(Shell::PowerShell, "Get-Date; GIT push", &r, "default"),
            Decision::Ask(_)
        ));
        // A PowerShell rule never applies to a Bash command.
        assert_eq!(
            decide(Shell::Bash, "Remove-Item x", &r, "default"),
            Decision::Allow
        );
    }

    #[test]
    fn rule_lists_add_up_and_the_most_specific_setting_wins() {
        let r = Rules::from_settings(&[
            (
                "~/.claude/settings.json".into(),
                serde_json::json!({ "permissions": { "ask": ["Bash(rm:*)"] }, "sandbox": { "autoAllowBashIfSandboxed": false } }),
            ),
            (
                ".claude/settings.json".into(),
                serde_json::json!({ "permissions": { "ask": ["Bash(git push:*)", "Edit(x)"], "deny": ["Bash(ocgen approve*)"] } }),
            ),
            (
                ".claude/settings.local.json".into(),
                serde_json::json!({ "sandbox": { "autoAllowBashIfSandboxed": true } }),
            ),
        ]);
        let ask: Vec<&str> = r.ask.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(ask, ["Bash(rm:*)", "Bash(git push:*)"]);
        assert_eq!(r.deny[0].from, ".claude/settings.json");
        assert!(r.auto_allow);
    }

    fn event(tool: &str, cmd: &str, cwd: &str, mode: &str) -> Value {
        serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": tool, "cwd": cwd,
            "permission_mode": mode,
            "tool_input": { "command": cmd, "description": "d", "timeout": 5000 }
        })
    }

    #[test]
    fn the_hook_wraps_asks_or_denies() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("app");
        std::fs::create_dir_all(p.join("src")).unwrap();
        let ps = crate::paths::for_shell(&p);
        let exe = Path::new("/opt/ocgen");
        let r = rules(&["Bash(ocgen approve*)"], &["Bash(git push:*)"]);
        let run = |ev: Value, ready: Result<(), String>| {
            pre_tool_use(&Request {
                event: &ev,
                project: &p,
                exe,
                rules: &r,
                ready,
            })
        };
        // Runs unasked, sandboxed, every other field kept.
        let o = run(
            event("Bash", "cargo test", &format!("{ps}/src"), "default"),
            Ok(()),
        )
        .unwrap();
        let h = &o["hookSpecificOutput"];
        assert_eq!(h["permissionDecision"], "allow");
        assert_eq!(h["updatedInput"]["description"], "d");
        assert_eq!(h["updatedInput"]["timeout"], 5000);
        let w = h["updatedInput"]["command"].as_str().unwrap();
        assert!(
            w.contains("__sandbox --bash") && w.contains("\ncargo test\n"),
            "{w}"
        );
        // An ask rule: asked, with the readable wrapper shown.
        let o = run(event("Bash", "git push", &ps, "default"), Ok(())).unwrap();
        assert_eq!(o["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(o["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("Bash(git push:*)"));
        // A deny rule: denied, nothing to run.
        let o = run(event("Bash", "ocgen approve", &ps, "default"), Ok(())).unwrap();
        assert_eq!(o["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(o["hookSpecificOutput"].get("updatedInput").is_none());
        // Plan mode: wrapped, Claude Code decides.
        let o = run(event("Bash", "ls", &ps, "plan"), Ok(())).unwrap();
        assert!(o["hookSpecificOutput"].get("permissionDecision").is_none());
        assert!(o["hookSpecificOutput"]["updatedInput"]["command"].is_string());
        // PowerShell too.
        let o = run(event("PowerShell", "Get-Date", &ps, "default"), Ok(())).unwrap();
        let w = o["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
        assert!(
            w.contains("__sandbox --powershell") && w.contains("\nGet-Date\n'@"),
            "{w}"
        );
        // Fail closed: outside the project, or a container not ready yet.
        let o = run(event("Bash", "ls", "/", "default"), Ok(())).unwrap();
        assert_eq!(o["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(o["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("cd"));
        let o = run(
            event("Bash", "ls", &ps, "default"),
            Err("run `ocgen sandbox grant`".into()),
        )
        .unwrap();
        assert_eq!(o["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(o["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("ocgen sandbox grant"));
        // Other tools, or no command: nothing.
        assert_eq!(run(event("Monitor", "ls", &ps, "default"), Ok(())), None);
        assert_eq!(run(event("Bash", "  ", &ps, "default"), Ok(())), None);
    }
}
