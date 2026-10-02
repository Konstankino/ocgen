//! Unit tests for the write helpers in `render.rs` (kept out of line so the
//! file's own items stay together).

use super::*;

#[test]
fn versions_compare_by_major_minor_patch() {
    assert_eq!(semver("0.4.6"), Some((0, 4, 6)));
    assert_eq!(semver("v1.2"), Some((1, 2, 0)));
    assert_eq!(semver("1.0.0-rc.1+abc"), Some((1, 0, 0)));
    assert_eq!(semver(""), None);
    assert_eq!(semver("dev"), None);
    assert!(refuse_newer("s", "", 0).is_ok(), "old states have neither");
    assert!(refuse_newer("s", crate::VERSION, STATE_SCHEMA).is_ok());
    assert!(refuse_newer("s", "999.0.0", STATE_SCHEMA).is_err());
    assert!(refuse_newer("s", "", STATE_SCHEMA + 1).is_err());
}

#[test]
fn the_gitattributes_block_is_added_once_and_user_lines_stay() {
    let block = GITATTRIBUTES_PROJECT;
    assert_eq!(with_gitattributes_block(None, block).unwrap(), block);
    let mine = with_gitattributes_block(Some("*.png binary"), block).unwrap();
    assert_eq!(mine, format!("*.png binary\n\n{block}"));
    assert_eq!(with_gitattributes_block(Some(&mine), block), None);
}

#[test]
fn line_endings_and_paths() {
    assert_eq!(fingerprint("a\r\nb\r\n"), fingerprint("a\nb\n"));
    assert_ne!(fingerprint("a\nb\n"), fingerprint("a\nc\n"));
    assert!(plain_rel(Path::new(".claude/agents/x.md")));
    for bad in ["", "../x", "/etc/x", "a/../../x"] {
        assert!(!plain_rel(Path::new(bad)), "{bad}");
    }
}

#[test]
fn an_mcp_server_from_json_writes_back_the_same() {
    for v in [
        json!({ "type": "http", "url": "https://x/mcp", "headers": { "A": "${B}" }, "oauth": { "c": 1 } }),
        json!({ "type": "stdio", "command": "npx", "args": ["-y", "s"], "env": { "K": "${K}" } }),
        json!({ "type": "stdio", "command": "uvx", "args": ["s", 3] }),
    ] {
        assert_eq!(mcp_server_from_json("s", &v).to_json(), v);
    }
    // Without a type: a command means stdio, a URL http.
    assert_eq!(
        mcp_server_from_json("s", &json!({ "command": "x" })).transport,
        "stdio"
    );
    assert_eq!(
        mcp_server_from_json("s", &json!({ "url": "https://x" })).transport,
        "http"
    );
}
