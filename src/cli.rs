//! Command-line surface. The whole tool is interactive, so flags are minimal:
//! subcommand routing plus an optional target directory for `new`.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "ocgen",
    version = ocgen::VERSION,
    about = "Interactive bootstrapper for LLM multi-agent projects (OpenCode, Claude Code)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the scaffolding wizard (default when no subcommand is given).
    New {
        /// Directory to create the project in (otherwise the wizard asks).
        path: Option<String>,
        /// Which platform to generate for (the wizard asks when omitted).
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// Base URL for the seeded provider(s) (OpenCode target only).
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        /// What to emit for the Claude target.
        #[arg(long, value_enum, default_value_t = OutputArg::Project)]
        output: OutputArg,
        /// GitHub owner/repo for the Claude plugin distribution.
        #[arg(long, value_name = "OWNER/REPO")]
        repo: Option<String>,
        /// Enable Claude Code Agent Teams (experimental) in the generated project.
        #[arg(long)]
        team: bool,
        /// Default minimum teammate confidence (0–100, 0 = off) for the governance gate.
        #[arg(long, value_name = "N")]
        team_confidence: Option<u8>,
        /// Default the plan-approval gate (/team-plan) to off in the wizard.
        #[arg(long)]
        no_plan_gate: bool,
        /// Default risk-mitigation rounds to off in the wizard.
        #[arg(long)]
        no_risk_rounds: bool,
        /// Default the execution approval gate (high-impact external actions) to off.
        #[arg(long)]
        no_approval_gate: bool,
    },
    /// Add something to an existing project.
    Add {
        #[command(subcommand)]
        what: AddWhat,
    },
    /// Edit something in an existing project.
    #[command(after_help = EDIT_EXAMPLES)]
    Edit {
        #[command(subcommand)]
        what: EditWhat,
    },
    /// Show an overview of all agents and providers and how they fit together.
    #[command(visible_alias = "horizon")]
    Landscape {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Repair a project's config (and upgrade an old state file), then rewrite files.
    ///
    /// Shows every change with a diff before writing. Permission rules you added
    /// to settings.json by hand are found and can be kept as your rules (like
    /// `ocgen edit permissions`), so they survive this and later regenerations.
    /// For files you changed, choose: apply all, keep all, decide file by file, or
    /// cancel. Whatever is overwritten is backed up to .ocgen-backup/ first.
    #[command(after_help = DOCTOR_EXAMPLES)]
    Doctor {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// Show what would change (with diffs) and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Apply everything without asking (hand-added permission rules are kept as yours).
        #[arg(long, short)]
        yes: bool,
    },
    /// Show the full configuration of one agent.
    Show {
        #[command(subcommand)]
        what: ShowWhat,
    },
    /// Run a generated project's hook (called by Claude Code, not by hand).
    ///
    /// Reads the event JSON on stdin and the gate settings from the environment,
    /// exactly like the `.claude/hooks/*.sh` scripts it replaces.
    Hook {
        /// Hook name, e.g. team-approval-gate.
        name: Option<String>,
        /// Print the hook protocol version and exit (hook commands probe this
        /// before using the binary, falling back to the scripts otherwise).
        #[arg(long)]
        check: bool,
    },
    /// Approve high-impact actions (pushes, deploys…) for a limited time.
    ///
    /// For a human, from their own terminal: it refuses to run under Claude Code
    /// or without a terminal. The approval lives outside the project and expires
    /// by itself.
    Approve {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// How long the approval lasts, in minutes (1 to 1440, i.e. 24 hours).
        #[arg(long, default_value_t = 30)]
        minutes: u64,
        /// Re-lock now.
        #[arg(long)]
        revoke: bool,
        /// Show whether execution is approved, and for how long.
        #[arg(long)]
        status: bool,
    },
    /// Check that a generated project works: up to date, valid settings, hook
    /// commands run, the approval gate blocks, the statusline renders, a
    /// compatible ocgen is on PATH, and Claude Code's validator passes.
    /// Exits 1 if any check fails (usable in CI).
    Verify {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
        /// Skip `claude plugin validate` (no Claude Code CLI needed).
        #[arg(long)]
        no_claude: bool,
        /// Also run the project's check command (usually its tests; may be slow).
        #[arg(long)]
        run_check: bool,
    },
    /// Print a recommended organisation policy (managed-settings.json) that
    /// projects can't override.
    ManagedSettings,
    /// Explain every configurable agent/provider field in detail.
    #[command(visible_alias = "reference")]
    Fields,
    /// Manage the editable template set.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
    /// Show /inquire ledgers (.claude/notes/*.md) as HTML pages.
    ///
    /// Each ledger has a sibling .html that ocgen renders from the Markdown. A
    /// hook re-renders it after every write and refreshes the browser tab that
    /// shows it (opening one when none does); these commands do it by hand.
    #[command(after_help = NOTES_EXAMPLES)]
    Notes {
        #[command(subcommand)]
        action: NotesAction,
    },
    /// Open an /intent issue draft in your browser: edit it as formatted text or as
    /// Markdown and save it back to the file, preview it the way GitHub shows it,
    /// and copy it.
    ///
    /// /intent writes the GitHub issue description to .claude/intent/drafts/<name>.md.
    /// With several drafts and no name, it opens their list to pick from, 10 a page,
    /// newest first. Typing `draft` on its own in Claude Code does the same, with the
    /// draft that session wrote last selected.
    #[command(after_help = DRAFT_EXAMPLES)]
    Draft {
        /// The draft's name, or its start (e.g. ADR-0007); default: the only draft, or
        /// the list of them.
        name: Option<String>,
        /// Directory inside the project (default: current dir).
        #[arg(long, short, default_value = ".")]
        path: String,
    },
}

const DRAFT_EXAMPLES: &str = "Examples:
  ocgen draft                            the only draft, or the list of them to pick from
  ocgen draft ADR-0007                   by the intent it belongs to
  ocgen draft issue-retry-budget         by file name
  ocgen draft issue-review-feat-retry    a /review-intent draft, by file name

The editor is a local page (127.0.0.1, a secret token in its address). Save writes the
.md file only if it hasn't changed since you opened it; the file is what
`gh issue create --body-file` files.

Environment:
  OCGEN_NOTES_OPEN=0     never open a browser
  OCGEN_NOTES_BROWSER    a command to open pages with instead of the system default";

const NOTES_EXAMPLES: &str = "Examples:
  ocgen notes open                       the most recently updated ledger
  ocgen notes open request-flow          by file name (slug)
  ocgen notes open \"request flow\"        by its Topic: line
  ocgen notes render .claude/notes/request-flow.md

Environment:
  OCGEN_NOTES_OPEN=0     never open a browser (=1: always, even in CI or without a display)
  OCGEN_NOTES_BROWSER    a command to open pages with instead of the system default";

#[derive(Subcommand)]
pub enum NotesAction {
    /// Render a ledger and show it: refresh the tab that shows it, or open one.
    Open {
        /// Ledger slug or topic (default: the most recently updated ledger).
        topic: Option<String>,
        /// Directory inside the project (default: current dir).
        #[arg(long, short, default_value = ".")]
        path: String,
    },
    /// Render ledgers (and /intent reading copies) to their HTML pages without
    /// showing them.
    Render {
        /// Ledger files (.claude/notes/<topic>.md) or /intent reading copies
        /// (.claude/intent/view/<name>.md).
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Run the live viewer for a notes directory (started by the hook).
    #[command(hide = true)]
    Serve {
        /// The .claude/notes directory.
        dir: String,
    },
}

/// Team governance defaults gathered from CLI flags, used to seed the wizard's
/// Agent Teams prompts (the wizard stays interactive; these set the defaults).
pub struct TeamCli {
    pub enabled: bool,
    pub confidence: Option<u8>,
    pub plan_gate: bool,
    pub risk_rounds: bool,
    pub approval_gate: bool,
}

/// Which platform `ocgen new` generates for.
#[derive(Clone, Copy, ValueEnum)]
pub enum TargetArg {
    Opencode,
    Claude,
}

/// What the Claude target emits.
#[derive(Clone, Copy, ValueEnum)]
pub enum OutputArg {
    Project,
    Plugin,
    Both,
}

#[derive(Subcommand)]
pub enum AddWhat {
    /// Interactively add one agent to an existing project.
    Agent {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Interactively add one provider to an existing project.
    Provider {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Author a new Claude Code skill (Claude projects only).
    Skill {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
    /// Add an MCP server to `.mcp.json` (Claude projects only).
    Mcp {
        /// Directory of the existing project (default: current dir).
        path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ShowWhat {
    /// Show one agent's configuration (you pick from a list if no name is given).
    Agent {
        /// Name of the agent to show.
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

#[derive(Subcommand)]
pub enum EditWhat {
    /// Interactively edit the fields of an existing agent.
    Agent {
        /// Name of the agent to edit (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Interactively edit an existing provider (and its models).
    Provider {
        /// Key of the provider to edit (otherwise you pick from a list).
        key: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Edit an existing Claude Code skill.
    Skill {
        /// Name of the skill to edit (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Edit or remove an MCP server (Claude projects only).
    Mcp {
        /// Name of the server (otherwise you pick from a list).
        name: Option<String>,
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Enable or adjust Claude Code Agent Teams (and its governance gates).
    Team {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Change the instruction language (the agents' prompts) or the answer
    /// language (what you read).
    ///
    /// A new instruction language re-seeds each preset agent whose text is still
    /// what its preset gave it; agents you edited keep theirs and are listed. When
    /// the answers are in another language than the instructions, the coordinator
    /// is told to answer in it (and a Claude project's settings.json gets Claude
    /// Code's `language`). With flags the change applies directly; without flags
    /// the command asks.
    Language {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
        /// The instruction language, e.g. English.
        #[arg(long, value_name = "LANGUAGE")]
        prompts: Option<String>,
        /// The answer language, e.g. Ukrainian.
        #[arg(long, value_name = "LANGUAGE")]
        answers: Option<String>,
    },
    /// Add or remove your own permission rules in settings.json (Claude projects only).
    ///
    /// Your rules are saved in the project state and appended to the rules ocgen
    /// generates, so `doctor`, `add` and `edit` keep them. The generated rules,
    /// including the approval-gate guards, always stay; only your own can be removed.
    ///
    /// Claude Code checks deny, then ask, then allow: an allow rule that a generated
    /// ask or deny rule also matches has no effect, and ocgen warns about it. A rule
    /// stricter than ocgen's (a deny of a generated ask or allow) takes its place.
    ///
    /// With flags the changes apply directly (usable from scripts and from Claude
    /// itself); without flags the command is interactive.
    #[command(after_help = PERMISSIONS_EXAMPLES)]
    Permissions {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
        #[command(flatten)]
        changes: PermissionsCli,
    },
    /// Configure /intent: intent-file numbering, the issue word limit and the
    /// project's issue / intent-file templates (Claude projects only).
    ///
    /// /intent improves your prompt, investigates, agrees a plan with you, writes
    /// a numbered intent file if you want one (e.g. docs/adr/ADR-0007-slug.md, the
    /// number checked against the remote main branch so it is never reused) and
    /// drafts an actionable GitHub issue for you to file. Claude never files it:
    /// `gh issue create` is denied.
    ///
    /// The two templates live in the project (.claude/intent/) and are yours:
    /// ocgen writes them once and never overwrites them. With flags the changes
    /// apply directly; without flags the command is interactive.
    #[command(after_help = INTENT_EXAMPLES)]
    Intent {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
        #[command(flatten)]
        changes: IntentCli,
    },
    /// The documentation sites agents may fetch: one list for every agent, skill
    /// and team member (both targets).
    ///
    /// Claude projects: a PreToolUse guard blocks every WebFetch to any other site
    /// or over plain http://, and fetching a trusted site never asks. OpenCode
    /// can't limit fetching to some sites, so there each fetch asks and the agents
    /// are told the list. WebSearch stays open; opening a result is a fetch.
    /// Without flags it shows the list.
    #[command(after_help = DOCS_EXAMPLES)]
    Docs {
        /// Directory of the existing project.
        #[arg(short, long, default_value = ".")]
        path: String,
        #[command(flatten)]
        changes: DocsCli,
    },
}

const DOCS_EXAMPLES: &str = "\
Examples:
  ocgen edit docs                                   # show the trusted sites
  ocgen edit docs --trust docs.example.com          # trust one more site (repeatable)
  ocgen edit docs --trust \"*.amazon.com\"            # every subdomain (not shared hosts like *.github.io)
  ocgen edit docs --untrust go.dev                  # stop trusting a site";

/// Trusted documentation sites given on the command line (`ocgen edit docs`).
#[derive(clap::Args, Default)]
pub struct DocsCli {
    /// Trust a documentation site (repeatable). A leading `*.` trusts every
    /// subdomain, e.g. "*.amazon.com" — never on a shared host such as
    /// *.github.io. An https:// URL is accepted (its host is stored); http:// is
    /// refused: docs are fetched over HTTPS only.
    #[arg(long, value_name = "DOMAIN")]
    pub trust: Vec<String>,
    /// Stop trusting a documentation site (repeatable).
    #[arg(long, value_name = "DOMAIN")]
    pub untrust: Vec<String>,
    /// Show the trusted sites and change nothing.
    #[arg(long, conflicts_with_all = ["trust", "untrust"])]
    pub show: bool,
}

const INTENT_EXAMPLES: &str = "\
Examples:
  ocgen edit intent --show                                 # current settings and templates
  ocgen edit intent                                        # interactive
  ocgen edit intent --prefix RFC --digits 3 --dir docs/rfc # RFC-001-<slug>.md in docs/rfc/
  ocgen edit intent --max-words 150                        # shorter issue descriptions
  ocgen edit intent --branch develop                       # check numbers against origin/develop
  ocgen edit intent --trust-domain docs.example.org        # let /intent read these docs without asking
  ocgen edit intent --trust-domain \"*.amazon.com\"          # every subdomain (not amazon.com itself)
  ocgen edit intent --approver @alice                      # who must sign off (repeatable)
  ocgen edit intent --codeowners .github/CODEOWNERS        # keep the approvers in your CODEOWNERS
  ocgen edit intent --codeowners-scope all                 # approvers review every change, not just intents
  ocgen edit intent --issue-template                       # edit the issue structure in $EDITOR
  ocgen edit intent --reset-intent-template                # back to ocgen's default
  ocgen edit intent --disable                              # remove /intent from the project";

/// `/intent` changes given on the command line (`ocgen edit intent`).
#[derive(clap::Args, Default)]
pub struct IntentCli {
    /// Intent-file prefix, e.g. ADR or RFC.
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,
    /// Width of the zero-padded number (1–6).
    #[arg(long, value_name = "N")]
    pub digits: Option<u8>,
    /// Directory for intent files, relative to the project root.
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,
    /// Word limit for the GitHub issue description (50–1000).
    #[arg(long, value_name = "N")]
    pub max_words: Option<u16>,
    /// Remote branch to check for numbers already taken ("" = the remote's default).
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,
    /// Trust a documentation site (repeatable) — the same project-wide list as
    /// `ocgen edit docs --trust`: every agent and skill may fetch it without asking.
    /// A leading `*.` trusts every subdomain, e.g. "*.amazon.com". An https:// URL
    /// is accepted (its host is stored); http:// is refused — docs are HTTPS only.
    #[arg(long, value_name = "DOMAIN")]
    pub trust_domain: Vec<String>,
    /// Stop trusting a documentation site (repeatable; same as `ocgen edit docs --untrust`).
    #[arg(long, value_name = "DOMAIN")]
    pub untrust_domain: Vec<String>,
    /// Add an approver who must sign off before work starts (repeatable): a GitHub
    /// user (@login) or team (@org/team). GitHub notifies them through the
    /// @mentions in the issue.
    #[arg(long, value_name = "HANDLE")]
    pub approver: Vec<String>,
    /// Remove an approver (repeatable).
    #[arg(long, value_name = "HANDLE")]
    pub remove_approver: Vec<String>,
    /// Link the project's existing CODEOWNERS file (.github/CODEOWNERS, CODEOWNERS
    /// or docs/CODEOWNERS): ocgen keeps its approvers block in it and a link to it
    /// at .claude/CODEOWNERS. "off" unlinks it and removes the block.
    #[arg(long, value_name = "PATH")]
    pub codeowners: Option<String>,
    /// What the CODEOWNERS block covers: the intent directory, every file, or nothing.
    #[arg(long, value_name = "SCOPE")]
    pub codeowners_scope: Option<ocgen::claude::CodeownersScope>,
    /// Add /intent to the project.
    #[arg(long, conflicts_with = "disable")]
    pub enable: bool,
    /// Remove /intent from the project (your templates are kept).
    #[arg(long)]
    pub disable: bool,
    /// Edit the project's issue template in $EDITOR.
    #[arg(long)]
    pub issue_template: bool,
    /// Edit the project's intent-file template in $EDITOR.
    #[arg(long)]
    pub intent_template: bool,
    /// Restore ocgen's default issue template.
    #[arg(long)]
    pub reset_issue_template: bool,
    /// Restore ocgen's default intent-file template.
    #[arg(long)]
    pub reset_intent_template: bool,
    /// Show the settings and template paths, and write nothing.
    #[arg(long, conflicts_with_all = [
        "prefix", "digits", "dir", "max_words", "branch", "trust_domain", "untrust_domain",
        "approver", "remove_approver", "codeowners", "codeowners_scope",
        "enable", "disable",
        "issue_template", "intent_template", "reset_issue_template", "reset_intent_template",
    ])]
    pub show: bool,
}

impl IntentCli {
    pub fn is_empty(&self) -> bool {
        self.prefix.is_none()
            && self.digits.is_none()
            && self.dir.is_none()
            && self.max_words.is_none()
            && self.branch.is_none()
            && self.trust_domain.is_empty()
            && self.untrust_domain.is_empty()
            && self.approver.is_empty()
            && self.remove_approver.is_empty()
            && self.codeowners.is_none()
            && self.codeowners_scope.is_none()
            && !self.enable
            && !self.disable
            && !self.issue_template
            && !self.intent_template
            && !self.reset_issue_template
            && !self.reset_intent_template
            && !self.show
    }
}

const DOCTOR_EXAMPLES: &str = "\
Examples:
  ocgen doctor --dry-run          # what would change, with diffs; writes nothing
  ocgen doctor                    # review: keep hand-added permission rules, then apply all / per file
  ocgen doctor -y ./my-project    # apply everything (hand-added permission rules are kept as yours)";

const EDIT_EXAMPLES: &str = "\
Examples:
  ocgen edit agent reviewer                       # walk every field of one agent
  ocgen edit skill -p ./my-project                # pick a skill from a list
  ocgen edit team                                 # turn Agent Teams and its gates on/off
  ocgen edit permissions --allow \"Bash(gh run view:*)\"   # stop a command from asking
  ocgen edit intent --prefix RFC --dir docs/rfc            # how /intent numbers intent files

Run `ocgen edit <COMMAND> --help` for a command's options.";

const PERMISSIONS_EXAMPLES: &str = "\
Rules:
  Tool or Tool(specifier), e.g. Read, Bash(gh run view:*), Read(./docs/**),
  WebFetch(domain:example.com), mcp__github__get_issue. Quote them in the shell.

Examples:
  ocgen edit permissions --list                            # every rule at a glance
  ocgen edit permissions                                   # interactive
  ocgen edit permissions --allow \"Bash(gh run view:*)\" --allow \"Bash(gh run list:*)\"
  ocgen edit permissions --ask \"Bash(docker push:*)\" --deny \"Read(./secrets/**)\"
  ocgen edit permissions --remove \"Bash(gh run list:*)\"
  ocgen edit permissions -p ./my-project --allow WebSearch";

/// Permission rule changes given on the command line (`ocgen edit permissions`).
#[derive(clap::Args, Default)]
pub struct PermissionsCli {
    /// Rule to allow without asking, e.g. "Bash(gh run view:*)" (repeatable).
    #[arg(long, value_name = "RULE")]
    pub allow: Vec<String>,
    /// Rule to always confirm, even in auto mode (repeatable).
    #[arg(long, value_name = "RULE")]
    pub ask: Vec<String>,
    /// Rule to never allow (repeatable).
    #[arg(long, value_name = "RULE")]
    pub deny: Vec<String>,
    /// One of your rules to remove, from whichever list holds it (repeatable).
    #[arg(long, value_name = "RULE")]
    pub remove: Vec<String>,
    /// Show every rule at a glance — ocgen's and yours, list by list — and write nothing.
    #[arg(long, conflicts_with_all = ["allow", "ask", "deny", "remove"])]
    pub list: bool,
}

impl PermissionsCli {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty()
            && self.ask.is_empty()
            && self.deny.is_empty()
            && self.remove.is_empty()
    }
}

#[derive(Subcommand)]
pub enum TemplatesAction {
    /// Copy the embedded templates into ~/.config/ocgen/templates for editing.
    ///
    /// Existing copies are kept unless --force. Hook scripts and the
    /// gate-protocol templates (team, team-plan, fanout, the team rule) are
    /// never copied: they always come from the ocgen binary.
    Init {
        /// Overwrite existing copies with the built-in defaults.
        #[arg(long)]
        force: bool,
    },
    /// Print the template override directory.
    Path,
    /// List resolved templates, marking overridden ones.
    List,
    /// Edit a template in $EDITOR, saving your changes to the override dir.
    Edit {
        /// Template path to edit, e.g. seeds.toml or archetypes/reviewer.toml
        /// (otherwise you pick one from a list).
        path: Option<String>,
    },
}
