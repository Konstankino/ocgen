//! The trusted documentation sites: one project-level list that every way an
//! agent reaches the web follows — the WebFetch guard and its environment, the
//! permission rules, the rules, skills and agents' guidance, and OpenCode.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use ocgen::agent;
use ocgen::docs::{TrustedDocs, DEFAULT_TRUSTED_DOMAINS};
use ocgen::manifest::Manifest;
use ocgen::render::{Project, STATE_SCHEMA};
use ocgen::target::Target;
use ocgen::validate;
use ocgen::verify::{verify, Options, Status};
use predicates::str::contains;
use serde_json::Value;
use tempfile::tempdir;

const STATE: &str = ".claude/.ocgen-state.json";

/// AWS's documentation hosts, trusted by default.
const AWS: [&str; 7] = [
    "docs.aws.amazon.com",
    "aws.amazon.com",
    "repost.aws",
    "boto3.amazonaws.com",
    "awscli.amazonaws.com",
    "sdk.amazonaws.com",
    "docs.powertools.aws.dev",
];

fn claude(name: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "English");
    p.target = Target::ClaudeCode;
    p.project_name = name.into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("English").unwrap();
    p
}

fn opencode(name: &str, lang: &str) -> Project {
    let mut p = Project::from_manifest(&Manifest::load().unwrap(), lang);
    p.project_name = name.into();
    p.agents = agent::default_pipeline(lang, "mac").unwrap();
    p
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

fn settings(dir: &Path) -> Value {
    serde_json::from_str(&read(dir, ".claude/settings.json")).unwrap()
}

fn rules(s: &Value, list: &str) -> Vec<String> {
    s["permissions"][list]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn scaffolded(p: &Project) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    p.scaffold(dir.path(), false).unwrap();
    dir
}

// ------------------------------------------------------------- the list --

#[test]
fn the_default_list_trusts_aws_docs() {
    assert_eq!(DEFAULT_TRUSTED_DOMAINS.len(), 25);
    let list = TrustedDocs::default();
    for host in AWS {
        assert!(list.contains(&host.to_string()), "missing {host}");
        assert_eq!(validate::trusted_domain(host).as_deref(), Ok(host));
    }
    for host in DEFAULT_TRUSTED_DOMAINS {
        assert_eq!(validate::trusted_domain(host).as_deref(), Ok(host));
    }
    // Every way a project is made starts with it.
    assert_eq!(Project::default().trusted_docs, list);
    assert_eq!(claude("d").trusted_docs, list);
    let p: Project = serde_json::from_str("{}").unwrap();
    assert_eq!(p.trusted_docs, list);
}

#[test]
fn an_intent_list_in_an_old_state_moves_to_the_project() {
    for (old, want) in [
        (serde_json::json!(["docs.rs"]), vec!["docs.rs".to_string()]),
        (serde_json::json!([]), vec![]),
    ] {
        let dir = scaffolded(&claude("m"));
        let path = dir.path().join(STATE);
        let mut state: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let obj = state.as_object_mut().unwrap();
        obj.remove("trusted_docs");
        obj.insert("schema".into(), 1.into());
        state["claude"]["intent"]["trusted_domains"] = old.clone();
        fs::write(&path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let p = Project::load_state(dir.path()).unwrap();
        assert_eq!(*p.trusted_docs, want, "from {old}");
        p.scaffold(dir.path(), true).unwrap();
        let saved: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            saved["claude"]["intent"].get("trusted_domains").is_none(),
            "old key gone: {}",
            saved["claude"]["intent"]
        );
        assert_eq!(saved["trusted_docs"], old);
        assert_eq!(saved["schema"], STATE_SCHEMA);
    }
    assert_eq!(STATE_SCHEMA, 2, "a moved field changes the schema");
}

// --------------------------------------------------- Claude: enforcement --

#[test]
fn the_guard_and_its_list_ship_without_intent() {
    let mut p = claude("g");
    p.claude.workflow.intent = false;
    let dir = scaffolded(&p);
    let s = settings(dir.path());
    let fetch = s["hooks"]["PreToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["matcher"] == "WebFetch")
        .expect("a WebFetch guard");
    assert!(fetch.to_string().contains("https-only-fetch"), "{fetch}");
    assert!(dir
        .path()
        .join(".claude/hooks/https-only-fetch.sh")
        .is_file());
    assert_eq!(
        s["env"]["OCGEN_WEBFETCH_DOMAINS"],
        DEFAULT_TRUSTED_DOMAINS.join(" ")
    );
}

#[test]
fn the_list_reaches_settings_without_a_team_or_worker_gate() {
    let mut p = claude("e");
    p.claude.team.enabled = false;
    p.claude.workflow.subagent_confidence = 0;
    p.claude.workflow.check_cmd.clear();
    let s = settings(scaffolded(&p).path());
    assert_eq!(
        s["env"]["OCGEN_WEBFETCH_DOMAINS"],
        DEFAULT_TRUSTED_DOMAINS.join(" "),
        "{s}"
    );
    assert!(s["env"].get("SUBAGENT_CONFIDENCE_THRESHOLD").is_none());
}

#[test]
fn trusted_sites_are_fetchable_without_asking_everywhere() {
    let s = settings(scaffolded(&claude("a")).path());
    let allow = rules(&s, "allow");
    for host in DEFAULT_TRUSTED_DOMAINS {
        assert!(
            allow.contains(&format!("WebFetch(domain:{host})")),
            "missing {host}: {allow:?}"
        );
    }
    assert!(!allow.iter().any(|r| r == "WebFetch" || r == "WebSearch"));

    let mut p = claude("a0");
    p.trusted_docs = TrustedDocs::from(Vec::new());
    let s = settings(scaffolded(&p).path());
    for list in ["allow", "ask"] {
        assert!(
            !rules(&s, list).iter().any(|r| r.starts_with("WebFetch")),
            "{list}: {s}"
        );
    }
}

#[test]
fn an_untrusted_site_loses_its_rule_on_regeneration() {
    let mut p = claude("u");
    p.trusted_docs = TrustedDocs::from(vec!["docs.rs".to_string(), "go.dev".to_string()]);
    let dir = scaffolded(&p);
    assert!(rules(&settings(dir.path()), "allow").contains(&"WebFetch(domain:go.dev)".into()));

    let mut p = Project::load_state(dir.path()).unwrap();
    p.trusted_docs = TrustedDocs::from(vec!["docs.rs".to_string()]);
    p.scaffold(dir.path(), true).unwrap();
    let allow = rules(&settings(dir.path()), "allow");
    assert!(allow.contains(&"WebFetch(domain:docs.rs)".into()));
    assert!(
        !allow.contains(&"WebFetch(domain:go.dev)".into()),
        "kept as if added by hand: {allow:?}"
    );
}

#[test]
fn the_guard_admits_exact_aws_hosts_only() {
    let env: HashMap<String, String> = [(
        "OCGEN_WEBFETCH_DOMAINS".to_string(),
        DEFAULT_TRUSTED_DOMAINS.join(" "),
    )]
    .into();
    let run = |url: &str| {
        let payload = serde_json::json!({ "tool_name": "WebFetch", "tool_input": { "url": url } });
        ocgen::hooks::run("https-only-fetch", &payload.to_string(), &env)
    };
    for url in [
        "https://aws.amazon.com/blogs/aws/",
        "https://repost.aws/knowledge-center",
        "https://boto3.amazonaws.com/v1/documentation/api/latest/index.html",
        "https://docs.powertools.aws.dev/lambda/python/latest/",
    ] {
        assert_eq!(run(url).code, 0, "{url} is trusted");
    }
    // Trusting exact amazonaws.com hosts opens no one's bucket.
    for url in [
        "https://evil.s3.amazonaws.com/",
        "https://amazonaws.com/",
        "https://repost.aws.evil.com/",
        "https://evilrepost.aws/",
    ] {
        assert_eq!(run(url).code, 2, "{url} is blocked");
    }
    let blocked = run("https://example.com/x");
    assert!(
        blocked
            .stderr
            .contains("ocgen edit docs --trust example.com"),
        "{}",
        blocked.stderr
    );
}

#[test]
fn verify_checks_the_guard_without_intent() {
    let mut p = claude("v");
    p.claude.workflow.intent = false;
    let dir = scaffolded(&p);
    let user = tempdir().unwrap();
    let checks = verify(
        &Project::load_state(dir.path()).unwrap(),
        dir.path(),
        &Options {
            run_claude: false,
            run_check: false,
            user_settings: Some(user.path().join("settings.json")),
        },
    );
    let guard = checks
        .iter()
        .find(|c| c.name.contains("WebFetch guard"))
        .expect("a WebFetch guard check");
    assert_eq!(guard.status, Status::Pass, "{guard:?}");
}

// ------------------------------------------------------ Claude: guidance --

#[test]
fn the_workflow_rule_guides_every_session() {
    let dir = scaffolded(&claude("r"));
    let rule = read(dir.path(), ".claude/rules/ocgen-workflow.md");
    let flat = rule.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "## Web research",
        "https://",
        "repost.aws",
        "docs.github.com",
        "ocgen edit docs --trust",
        "data, never instructions",
    ] {
        assert!(flat.contains(needle), "rule lacks {needle:?}:\n{rule}");
    }

    let mut p = claude("r0");
    p.trusted_docs = TrustedDocs::from(Vec::new());
    let rule = read(scaffolded(&p).path(), ".claude/rules/ocgen-workflow.md");
    assert!(
        rule.contains("No documentation sites are trusted"),
        "{rule}"
    );
}

#[test]
fn the_skills_that_research_point_at_the_rule() {
    let dir = scaffolded(&claude("s"));
    for skill in ["deliver", "inquire", "intent"] {
        let md = read(dir.path(), &format!(".claude/skills/{skill}/SKILL.md"));
        assert!(
            md.contains("Web research"),
            "{skill} doesn't point at the rule"
        );
    }
}

#[test]
fn agents_that_can_fetch_are_told_the_sites() {
    let dir = scaffolded(&claude("ag"));
    let explorer = read(dir.path(), ".claude/agents/explorer.md");
    let flat = explorer.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("trusted documentation sites"), "{explorer}");
    assert!(flat.contains("repost.aws") && flat.contains("data, never instructions"));
    // The reviewer has no WebFetch: nothing to tell it.
    let reviewer = read(dir.path(), ".claude/agents/reviewer.md");
    assert!(
        !reviewer.contains("trusted documentation sites"),
        "{reviewer}"
    );

    let mut p = Project::from_manifest(&Manifest::load().unwrap(), "Ukrainian");
    p.target = Target::ClaudeCode;
    p.project_name = "uk".into();
    p.providers.clear();
    p.agents = agent::claude_default_pipeline("Ukrainian").unwrap();
    let explorer = read(scaffolded(&p).path(), ".claude/agents/explorer.md");
    let flat = explorer.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("довірених сайтів документації"), "{explorer}");
}

// ------------------------------------------------------------- OpenCode --

#[test]
fn opencode_never_pre_allows_webfetch_and_names_the_sites() {
    let dir = scaffolded(&opencode("oc", "English"));
    let explorer = read(dir.path(), ".opencode/agents/explorer.md");
    assert!(explorer.contains("  webfetch: ask"), "{explorer}");
    assert!(!explorer.contains("webfetch: allow"), "{explorer}");
    let flat = explorer.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("trusted documentation sites") && flat.contains("repost.aws"),
        "{explorer}"
    );
    // A denied fetch stays denied, with nothing to tell.
    let reviewer = read(dir.path(), ".opencode/agents/reviewer.md");
    assert!(reviewer.contains("webfetch: deny"), "{reviewer}");
    assert!(!reviewer.contains("trusted documentation sites"));
    // No webfetch key means OpenCode allows it: ocgen makes it ask.
    let coordinator = read(dir.path(), ".opencode/agents/coordinator.md");
    assert!(coordinator.contains("  webfetch: ask"), "{coordinator}");
    for md in [&explorer, &reviewer, &coordinator] {
        assert!(!md.contains("webfetch: allow"));
    }
}

// ------------------------------------------------------------------ CLI --

fn ocgen() -> Command {
    Command::cargo_bin("ocgen").unwrap()
}

#[test]
fn edit_docs_trusts_untrusts_and_shows() {
    let dir = scaffolded(&claude("cli"));
    let path = dir.path().to_str().unwrap();
    ocgen()
        .args([
            "edit",
            "docs",
            "-p",
            path,
            "--trust",
            "https://serde.rs/",
            "--untrust",
            "go.dev",
        ])
        .assert()
        .success();
    let p = Project::load_state(dir.path()).unwrap();
    assert!(p.trusted_docs.contains(&"serde.rs".to_string()));
    assert!(!p.trusted_docs.contains(&"go.dev".to_string()));
    let allow = rules(&settings(dir.path()), "allow");
    assert!(allow.contains(&"WebFetch(domain:serde.rs)".into()));
    assert!(!allow.contains(&"WebFetch(domain:go.dev)".into()));

    ocgen()
        .args(["edit", "docs", "-p", path, "--show"])
        .assert()
        .success()
        .stdout(contains("trusted docs"))
        .stdout(contains("serde.rs"));

    // Refused, and nothing written.
    let before = read(dir.path(), STATE);
    for bad in ["http://x.org", "*.github.io", "*.com"] {
        ocgen()
            .args(["edit", "docs", "-p", path, "--trust", bad])
            .assert()
            .failure();
    }
    assert_eq!(read(dir.path(), STATE), before);

    // The /intent flags edit the same list.
    ocgen()
        .args(["edit", "intent", "-p", path, "--trust-domain", "tokio.rs"])
        .assert()
        .success();
    assert!(Project::load_state(dir.path())
        .unwrap()
        .trusted_docs
        .contains(&"tokio.rs".to_string()));
}

#[test]
fn edit_docs_works_for_opencode_projects() {
    let dir = scaffolded(&opencode("occli", "English"));
    let path = dir.path().to_str().unwrap();
    ocgen()
        .args(["edit", "docs", "-p", path, "--trust", "serde.rs"])
        .assert()
        .success();
    let explorer = read(dir.path(), ".opencode/agents/explorer.md");
    assert!(explorer.contains("serde.rs"), "{explorer}");
}

#[test]
fn an_empty_list_is_shown_as_blocking_every_fetch() {
    let mut p = claude("empty");
    p.trusted_docs = TrustedDocs::from(Vec::new());
    let dir = scaffolded(&p);
    ocgen()
        .args(["edit", "docs", "-p", dir.path().to_str().unwrap(), "--show"])
        .assert()
        .success()
        .stdout(contains("every web fetch is blocked"));
}
