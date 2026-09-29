//! Pure input validators used by the wizard. They live in the library (not the
//! interactive layer) so they can be unit-tested and are covered by tests.
//!
//! Each returns `Err(message)` for invalid input; the wizard shows the message
//! and re-prompts.

/// A safe identifier: agent name, provider key, model id. No spaces or slashes,
/// so it is valid as a file name and an `@mention`.
pub fn ident(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("cannot be empty".into());
    }
    if !s.chars().next().unwrap().is_ascii_alphanumeric() {
        return Err("must start with a letter or digit".into());
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("use only letters, digits, '-' or '_' (no spaces or '/')".into());
    }
    Ok(())
}

/// A number within `[min, max]`.
pub fn number(s: &str, min: f64, max: f64) -> Result<(), String> {
    match s.parse::<f64>() {
        Ok(v) if v >= min && v <= max => Ok(()),
        Ok(_) => Err(format!("must be between {min} and {max}")),
        Err(_) => Err("must be a number".into()),
    }
}

/// A positive whole number (agent steps).
pub fn steps(s: &str) -> Result<(), String> {
    match s.parse::<u32>() {
        Ok(v) if v > 0 => Ok(()),
        Ok(_) => Err("must be greater than 0".into()),
        Err(_) => Err("must be a whole number".into()),
    }
}

/// A `#RGB` or `#RRGGBB` hex colour.
pub fn hex(s: &str) -> Result<(), String> {
    match s.strip_prefix('#') {
        Some(h) if (h.len() == 3 || h.len() == 6) && h.chars().all(|c| c.is_ascii_hexdigit()) => {
            Ok(())
        }
        _ => Err("hex colour must be #RGB or #RRGGBB (e.g. #4ec9b0)".into()),
    }
}

/// An `http(s)://` URL with a host.
pub fn url(s: &str) -> Result<(), String> {
    let host = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"));
    match host {
        Some(rest) if !rest.is_empty() => Ok(()),
        _ => Err("must start with http:// or https:// and a host".into()),
    }
}

/// A Claude Code model alias.
pub fn claude_model(s: &str) -> Result<(), String> {
    const ALLOWED: [&str; 5] = ["opus", "sonnet", "haiku", "fable", "inherit"];
    let t = s.trim();
    if t.is_empty() {
        return Err("required (e.g. sonnet)".into());
    }
    if ALLOWED.contains(&t) {
        Ok(())
    } else {
        Err(format!("must be one of: {}", ALLOWED.join(", ")))
    }
}

/// A GitHub `owner/repo` slug.
pub fn owner_repo(s: &str) -> Result<(), String> {
    let t = s.trim();
    let parts: Vec<&str> = t.split('/').collect();
    let ok = parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        });
    if ok {
        Ok(())
    } else {
        Err("must be <owner>/<repo>".into())
    }
}

/// A Claude Code Agent Teams display mode.
pub fn teammate_mode(s: &str) -> Result<(), String> {
    const ALLOWED: [&str; 4] = ["in-process", "auto", "tmux", "iterm2"];
    let t = s.trim();
    if ALLOWED.contains(&t) {
        Ok(())
    } else {
        Err(format!("must be one of: {}", ALLOWED.join(", ")))
    }
}

/// A team confidence threshold: an integer 0–100 (0 disables the gate).
pub fn confidence_threshold(s: &str) -> Result<(), String> {
    match s.trim().parse::<u16>() {
        Ok(n) if n <= 100 => Ok(()),
        Ok(_) => Err("must be between 0 and 100".into()),
        Err(_) => Err("must be a whole number between 0 and 100".into()),
    }
}

/// A loop-guard budget: blocks allowed before escalating, 0 (unbounded) to 20.
pub fn loop_budget(s: &str) -> Result<(), String> {
    match s.trim().parse::<u8>() {
        Ok(n) if n <= 20 => Ok(()),
        _ => Err("must be a whole number between 0 and 20".into()),
    }
}

/// A non-empty value that contains no whitespace (e.g. an npm package name).
pub fn nonempty_nospace(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("cannot be empty".into());
    }
    if s.chars().any(char::is_whitespace) {
        return Err("cannot contain spaces".into());
    }
    Ok(())
}

/// Build a validator that enforces [`ident`] and rejects anything already in `taken`.
pub fn unique_ident(taken: Vec<String>, noun: &'static str) -> impl Fn(&str) -> Result<(), String> {
    move |s: &str| {
        ident(s)?;
        if taken.iter().any(|t| t == s) {
            Err(format!("{noun} '{s}' is already used"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers() {
        assert!(ident("editor").is_ok());
        assert!(ident("gpt-4o_mini").is_ok());
        assert!(ident("9x").is_ok());
        assert!(ident("my agent").is_err()); // space
        assert!(ident("a/b").is_err()); // slash
        assert!(ident("-x").is_err()); // leading dash
        assert!(ident("a.b").is_err()); // dot
        assert!(ident("").is_err()); // empty
    }

    #[test]
    fn numbers() {
        assert!(number("0", 0.0, 2.0).is_ok());
        assert!(number("2", 0.0, 2.0).is_ok());
        assert!(number("0.2", 0.0, 2.0).is_ok());
        assert!(number("-0.1", 0.0, 2.0).is_err());
        assert!(number("5", 0.0, 2.0).is_err());
        assert!(number("1.5", 0.0, 1.0).is_err()); // top_p range
        assert!(number("abc", 0.0, 1.0).is_err());
    }

    #[test]
    fn positive_steps() {
        assert!(steps("30").is_ok());
        assert!(steps("1").is_ok());
        assert!(steps("0").is_err());
        assert!(steps("-1").is_err());
        assert!(steps("1.5").is_err());
        assert!(steps("x").is_err());
    }

    #[test]
    fn hex_colours() {
        assert!(hex("#4ec9b0").is_ok());
        assert!(hex("#abc").is_ok());
        assert!(hex("#ABC123").is_ok());
        assert!(hex("#zzz").is_err());
        assert!(hex("#12").is_err()); // wrong length
        assert!(hex("accent").is_err()); // no leading #
        assert!(hex("").is_err());
    }

    #[test]
    fn urls() {
        assert!(url("http://mac.home:8080/v1").is_ok());
        assert!(url("https://api.openai.com/v1").is_ok());
        assert!(url("http://").is_err()); // no host
        assert!(url("ftp://x").is_err());
        assert!(url("not-a-url").is_err());
    }

    #[test]
    fn nonempty_nospace_values() {
        assert!(nonempty_nospace("@ai-sdk/openai-compatible").is_ok());
        assert!(nonempty_nospace("has space").is_err());
        assert!(nonempty_nospace("").is_err());
    }

    #[test]
    fn claude_models() {
        for ok in ["opus", "sonnet", "haiku", "fable", "inherit"] {
            assert!(claude_model(ok).is_ok(), "{ok}");
        }
        assert!(claude_model(" sonnet ").is_ok()); // trimmed
        assert!(claude_model("").is_err());
        assert!(claude_model("gpt-4").is_err());
    }

    #[test]
    fn owner_repos() {
        assert!(owner_repo("me/team").is_ok());
        assert!(owner_repo("My-Org/my.repo_1").is_ok());
        assert!(owner_repo("noslash").is_err());
        assert!(owner_repo("a/b/c").is_err());
        assert!(owner_repo("/repo").is_err());
        assert!(owner_repo("owner/").is_err());
        assert!(owner_repo("own er/repo").is_err());
    }

    #[test]
    fn teammate_modes() {
        for ok in ["in-process", "auto", "tmux", "iterm2"] {
            assert!(teammate_mode(ok).is_ok(), "{ok}");
        }
        assert!(teammate_mode(" tmux ").is_ok()); // trimmed
        assert!(teammate_mode("").is_err());
        assert!(teammate_mode("split").is_err());
    }

    #[test]
    fn confidence_thresholds() {
        for ok in ["0", "96", "100", " 96 "] {
            assert!(confidence_threshold(ok).is_ok(), "{ok}");
        }
        assert!(confidence_threshold("101").is_err());
        assert!(confidence_threshold("-1").is_err());
        assert!(confidence_threshold("").is_err());
        assert!(confidence_threshold("abc").is_err());
    }

    #[test]
    fn loop_budgets() {
        for ok in ["0", "3", "20", " 5 "] {
            assert!(loop_budget(ok).is_ok(), "{ok}");
        }
        for bad in ["21", "-1", "", "x"] {
            assert!(loop_budget(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn uniqueness() {
        let v = unique_ident(vec!["mac".into(), "openai".into()], "provider key");
        assert!(v("cloud").is_ok());
        assert!(v("mac").is_err()); // duplicate
        assert!(v("bad key").is_err()); // also fails ident rules
    }
}
