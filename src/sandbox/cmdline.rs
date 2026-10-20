//! A command line without a shell: the strict subset of `sh` syntax a sandbox
//! can run one program from (native Windows, where Git Bash may not start in an
//! AppContainer, and WASI modules), and the quoting Windows programs parse
//! their command line with.

/// One program to run: leading `NAME=value` assignments, then its arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Argv {
    pub assign: Vec<(String, String)>,
    pub args: Vec<String>,
}

/// Why a command needs a shell, for a message that ends "…: use a script or a
/// simpler command".
pub const NEEDS_SHELL: &str = "it needs a shell, and the sandbox here runs one program without one";

/// `cmd` as one program's arguments, the way `sh` would split it — or why it
/// can't be split without a shell: an unquoted `| & ; < > ( ) $ \` * ? [ ] { }
/// # !`, a leading `~`, a newline, or `$`/`` ` `` in double quotes. Single
/// quotes are literal; in double quotes `\"`, `\\`, `\$` and `` \` `` escape;
/// outside quotes a backslash escapes the next character.
pub fn split(cmd: &str) -> Result<Argv, String> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = cmd.chars().peekable();
    let bad = |c: char| Err(format!("`{c}` in `{cmd}`: {NEEDS_SHELL}"));
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err(format!("unclosed quote in `{cmd}`")),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                            Some('\n') => return bad('\n'),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err(format!("unclosed quote in `{cmd}`")),
                        },
                        Some(x @ ('$' | '`')) => return bad(x),
                        Some(x) => cur.push(x),
                        None => return Err(format!("unclosed quote in `{cmd}`")),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | None => return bad('\\'),
                Some(x) => {
                    in_word = true;
                    cur.push(x);
                }
            },
            '~' if !in_word => return bad('~'),
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '*' | '?' | '[' | ']' | '{'
            | '}' | '!' | '\n' | '\r' => return bad(c),
            '#' if !in_word => return bad('#'),
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    let mut out = Argv::default();
    let mut rest = words.into_iter().peekable();
    while let Some(w) = rest.peek() {
        match w.split_once('=') {
            Some((k, _)) if super::policy::env_name(k) && out.args.is_empty() => {
                let w = rest.next().unwrap_or_default();
                let (k, v) = w.split_once('=').unwrap_or_default();
                out.assign.push((k.to_string(), v.to_string()));
            }
            _ => break,
        }
    }
    out.args.extend(rest);
    if out.args.is_empty() {
        return Err(format!("no program in `{cmd}`"));
    }
    Ok(out)
}

/// One argument quoted the way Microsoft's C runtime (and `CommandLineToArgvW`)
/// reads it back: as is when it has no space, tab or quote; else in double
/// quotes, with the backslashes before a quote (or the closing one) doubled.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"', '\n', '\x0b']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(c);
                continue;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// A Windows command line for `args`.
pub fn command_line(args: &[String]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Whether `arg` can go to a batch file through `cmd.exe` unchanged: `cmd`
/// expands `%` and `!` and splits on `^ & | < >` and quotes even inside quotes
/// (the "BatBadBut" problem), so any of those is refused.
pub fn batch_safe(arg: &str) -> Result<(), String> {
    match arg
        .chars()
        .find(|c| matches!(c, '%' | '!' | '^' | '&' | '|' | '<' | '>' | '"' | '\r' | '\n'))
    {
        Some(c) => Err(format!(
            "`{c}` in the argument `{arg}` would be read by cmd.exe, which runs a .cmd or .bat program"
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splits_words_quotes_and_assignments() {
        let a =
            split("RUST_LOG=debug  X= cargo test --locked -p 'my crate' \"a \\\"b\\\" \\x\" c\\ d")
                .unwrap();
        assert_eq!(
            a.assign,
            [
                ("RUST_LOG".into(), "debug".into()),
                ("X".into(), String::new())
            ]
        );
        assert_eq!(
            a.args,
            args(&[
                "cargo",
                "test",
                "--locked",
                "-p",
                "my crate",
                "a \"b\" \\x",
                "c d"
            ])
        );
        // `=` after the program is an argument.
        let a = split("make CC=gcc 'A=b'").unwrap();
        assert_eq!(a.assign, []);
        assert_eq!(a.args, args(&["make", "CC=gcc", "A=b"]));
        assert_eq!(split("a#b ''").unwrap().args, args(&["a#b", ""]));
        assert_eq!(
            split("x a~y").unwrap().args,
            args(&["x", "a~y"]),
            "~ only at a word's start"
        );
    }

    #[test]
    fn refuses_what_needs_a_shell() {
        for cmd in [
            "cargo test && echo ok",
            "a | b",
            "a; b",
            "a > out",
            "echo $HOME",
            "echo \"$HOME\"",
            "echo `id`",
            "ls *.rs",
            "ls ~/x",
            "a # comment",
            "a\nb",
            "(a)",
            "a {b}",
            "echo hi!",
            "unclosed 'quote",
            "trailing \\",
            "",
            "X=1",
        ] {
            assert!(split(cmd).is_err(), "{cmd:?}");
        }
        assert!(split("a | b").unwrap_err().contains(NEEDS_SHELL));
    }

    #[test]
    fn quotes_like_the_c_runtime() {
        for (arg, want) in [
            ("plain", "plain"),
            ("", "\"\""),
            ("a b", "\"a b\""),
            ("C:\\x\\", "C:\\x\\"),
            ("C:\\a b\\", "\"C:\\a b\\\\\""),
            ("say \"hi\"", "\"say \\\"hi\\\"\""),
            ("a\\\"b", "\"a\\\\\\\"b\""),
            ("tab\there", "\"tab\there\""),
        ] {
            assert_eq!(quote(arg), want, "{arg:?}");
        }
        assert_eq!(
            command_line(&args(&["C:\\Git\\usr\\bin\\sh.exe", "-c", "cargo test"])),
            "C:\\Git\\usr\\bin\\sh.exe -c \"cargo test\""
        );
    }

    /// What Windows itself reads back from our command lines.
    #[cfg(windows)]
    #[test]
    fn command_lines_round_trip_through_windows() {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let cases = args(&[
            "prog",
            "",
            "a b",
            "say \"hi\"",
            "a\\\"b",
            "C:\\a b\\",
            "x\\\\",
        ]);
        let line: Vec<u16> = command_line(&cases).encode_utf16().chain([0]).collect();
        let mut n = 0;
        // SAFETY: a NUL-terminated buffer; the array is freed below.
        let got = unsafe {
            let argv = CommandLineToArgvW(line.as_ptr(), &mut n);
            assert!(!argv.is_null());
            let out: Vec<String> = (0..n as usize)
                .map(|i| {
                    let p = *argv.add(i);
                    let len = (0..).take_while(|&j| *p.add(j) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
                })
                .collect();
            LocalFree(argv as _);
            out
        };
        assert_eq!(got, cases);
    }

    #[test]
    fn batch_arguments_refuse_what_cmd_reads() {
        assert!(batch_safe("--release").is_ok());
        assert!(batch_safe("a b").is_ok());
        for a in ["%PATH%", "a&b", "x\"y", "!v!", "^", "a|b", "<x"] {
            assert!(batch_safe(a).is_err(), "{a}");
        }
    }
}
