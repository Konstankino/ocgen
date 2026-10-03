//! `ocgen fields` — a detailed reference for every configurable field, so users
//! know exactly what each agent/provider setting means when filling in the wizard.

use console::style;

use crate::ui;

struct Field {
    label: &'static str,
    detail: &'static str,
    /// Optional example value; may contain newlines (printed as a block).
    example: &'static str,
}

const AGENT_FIELDS: &[Field] = &[
    Field {
        label: "name",
        detail: "Identifier for the agent. Becomes the file .opencode/agents/<name>.md and the \
                 @<name> other agents (and you) use to invoke or delegate to it. Use a short, \
                 lowercase word with no spaces. It must be a plain identifier, unique ignoring \
                 case (names that differ only in case collide on macOS and Windows); a state \
                 file holding any other name fails to load.",
        example: "editor",
    },
    Field {
        label: "role",
        detail: "A free-text label describing the agent's job. It is shown in `ocgen landscape` \
                 but is not written into the generated agent file. One value matters: an enabled \
                 subagent with role (or name) `adversary` makes the coordinator run its check \
                 last, send \
                 Critical/High findings back to the implementer for up to 2 rework rounds, and \
                 report what is left as UNRESOLVED.",
        example: "reviewer",
    },
    Field {
        label: "mode",
        detail: "One of primary, subagent, or all. A primary agent is one you invoke directly; \
                 it can delegate to subagents (ocgen fills its task permissions with them) and it \
                 gets the /multi command. A subagent is only ever called by another agent. 'all' \
                 can be used both ways. Most projects have exactly one primary — the coordinator.",
        example: "subagent",
    },
    Field {
        label: "provider",
        detail: "Which of the configured providers hosts this agent's model. The model reference \
                 written into the file becomes <provider>/<model>, so the same model id can live \
                 under different providers.",
        example: "mac",
    },
    Field {
        label: "model",
        detail: "The model id to run this agent on, chosen from the selected provider's models. \
                 Written into the file as <provider>/<model>.",
        example: "qwen3-35b-a3b",
    },
    Field {
        label: "variant",
        detail: "Selects a model variant for this agent when the provider exposes variants (for \
                 example a thinking / non-thinking split). Applies only to the agent's own model. \
                 Leave blank for the provider default.",
        example: "thinking",
    },
    Field {
        label: "temperature",
        detail: "Controls sampling randomness, typically 0.0–2.0 (many local models cap at 1.0). \
                 Lower values make output more focused, deterministic and repeatable; higher \
                 values make it more varied and creative. For editing, review or coding agents \
                 use roughly 0.1–0.3; for brainstorming try 0.7–1.0.",
        example: "0.2",
    },
    Field {
        label: "top_p",
        detail: "Nucleus sampling, 0.0–1.0 — an alternative to temperature that restricts \
                 sampling to the most probable tokens. Generally set either temperature or top_p, \
                 not both. Leave blank to use the model default.",
        example: "0.9",
    },
    Field {
        label: "steps",
        detail: "The maximum number of agentic steps (model → tool → model cycles) the agent may \
                 run before it is forced to stop — a safety cap against runaway loops. Leave it \
                 blank for no limit. Only meaningful for agents that iterate with tools.",
        example: "30",
    },
    Field {
        label: "color",
        detail: "A cosmetic accent colour OpenCode uses for the agent in its UI. Either a named \
                 colour (accent, warning, success, …) or a hex code. It has no effect on \
                 behaviour.",
        example: "#4ec9b0",
    },
    Field {
        label: "disable",
        detail: "Turns the agent off without deleting its file — a disabled agent is not loaded. \
                 Useful for temporarily parking an agent.",
        example: "true",
    },
    Field {
        label: "hidden",
        detail: "Hides a subagent from the @ autocomplete menu (only applies to subagents). It \
                 can still be called by other agents; it is just kept out of the picker.",
        example: "true",
    },
    Field {
        label: "description",
        detail: "A one-line summary of what the agent does. It appears in agent listings and is \
                 given to the coordinator so it can decide when to hand work to this agent — so \
                 make it specific and task-focused.",
        example: "Fixes grammar and spelling",
    },
    Field {
        label: "permissions",
        detail: "What the agent is allowed to do, written as a YAML block. Every value is allow, \
                 ask, or deny (or, for the rule keys, a map of pattern → value with \"*\" as the \
                 default). Rule keys (support per-pattern rules): read, edit, glob, grep, list, \
                 bash, task, external_directory, lsp, skill. Action keys (a single value): \
                 todowrite, question, webfetch, websearch, doom_loop. For a primary agent ocgen \
                 additionally manages the task: block listing which subagents it may call. (The \
                 older tools: map is deprecated — use permission.)",
        example:
            "edit: allow\nbash:\n  \"*\": ask\n  \"git *\": allow\nwebfetch: deny\nwebsearch: allow",
    },
    Field {
        label: "system prompt (body)",
        detail: "The agent's system prompt — its persona and standing instructions, in Markdown. \
                 The token $ARGUMENTS is replaced with the user's task when the agent is invoked. \
                 A coordinator's body is rendered as a template, so it may use {{ subagents }}; \
                 text that isn't a valid template, or names a value ocgen doesn't know (a GitHub \
                 Actions ${{ secrets.X }}, a Helm {{ .Values }}), is written exactly as typed, \
                 with a warning.",
        example: "You are a proofreader. Fix errors and list the changes you made.",
    },
    Field {
        label: "external prompt file",
        detail: "If enabled, the system prompt is stored in a separate file \
                 .opencode/prompts/<name>.txt and referenced from the agent, instead of being \
                 inlined. Useful for long coordinator prompts, and the external prompt can \
                 reference {{ subagents }} to list the team dynamically. Like a coordinator's \
                 body, a prompt that isn't a valid template is written as typed, with a warning.",
        example: "enabled for coordinators",
    },
    Field {
        label: "options",
        detail: "Extra provider-specific model options passed for this agent, as a nested YAML \
                 map. Use it to tune things the model exposes beyond the common fields — for \
                 example the reasoning effort on a reasoning model. Leave unset for none.",
        example: "reasoningEffort: high",
    },
];

const PROVIDER_FIELDS: &[Field] = &[
    Field {
        label: "key",
        detail: "Short identifier for the provider. Used in every model reference as \
                 <key>/<model>, and as the provider.<key> entry in opencode.json.",
        example: "mac",
    },
    Field {
        label: "name",
        detail: "A human-readable display name for the provider.",
        example: "M4 Pro 24GB",
    },
    Field {
        label: "npm",
        detail: "The @ai-sdk adapter package OpenCode uses to talk to this provider's API.",
        example: "@ai-sdk/openai-compatible",
    },
    Field {
        label: "base URL",
        detail: "The provider's OpenAI-compatible API base URL.",
        example: "http://mac.home:8080/v1",
    },
    Field {
        label: "models",
        detail: "The list of models this provider serves. Each has an id (used in references) \
                 and a display name. Agents and the utility model are chosen from these.",
        example: "id: qwen3-35b-a3b   name: Qwen3 35B",
    },
];

const CLAUDE_FIELDS: &[Field] = &[
    Field {
        label: "model (alias or ID)",
        detail: "For the Claude Code target an agent's model is a Claude alias — opus, sonnet, \
                 haiku, fable, or inherit (use the session's model) — or a full model ID such as \
                 claude-opus-5-5 (add [1m] for the 1M-token context). An alias follows the newest \
                 model; a full ID pins one. There are no per-agent providers or base URLs.",
        example: "sonnet  or  claude-opus-5-5",
    },
    Field {
        label: "tools",
        detail: "A comma-separated allow-list of the tools a subagent may use, written to its                  frontmatter. Empty means it inherits every tool. This replaces OpenCode's                  permission block for the Claude target.",
        example: "Read, Grep, Edit, Write",
    },
    Field {
        label: "subagent controls",
        detail: "disallowedTools removes tools even if `tools` allows them (the explorer, reviewer, \
                 verifier and adversary ship with Edit, Write, NotebookEdit denied, so they have no \
                 file-editing tools; the verifier and adversary keep Bash to run tests); effort \
                 (low/medium/high/xhigh/max) trades cost for depth (the reviewer and adversary use \
                 high); \
                 permissionMode (plan/acceptEdits/dontAsk/default); memory (project = committed \
                 .claude/agent-memory/, local = git-ignored, user = every project); background; \
                 skills to preload; and the MCP servers it may use. All optional; set them in \
                 `ocgen add/edit agent`.",
        example: "disallowedTools: Edit, Write   effort: high",
    },
    Field {
        label: "mcp servers",
        detail: "Project MCP servers live in .mcp.json at the repo root: stdio (a local command) or \
                 http/sse (a URL). Reference secrets as ${VAR}, never literal values — `landscape` \
                 flags literal tokens. Pre-approving writes enabledMcpjsonServers so the team isn't \
                 prompted once they trust the folder. Add with `ocgen add mcp`; change or remove \
                 with `ocgen edit mcp`. A server added to .mcp.json by hand (e.g. `claude mcp add \
                 --scope project`) is kept as yours by the next add, edit or doctor, with the \
                 fields ocgen doesn't model (timeout, oauth) under its `extra` key in the state; \
                 one you removed with ocgen stays removed.",
        example: "tf: stdio  npx -y terraform-mcp   env TF_TOKEN=${TF_TOKEN}",
    },
    Field {
        label: "mode (coordinator)",
        detail: "A primary agent becomes the project's CLAUDE.md coordinator and the /multi                  command rather than a .claude/agents/ file; subagents each become an agent file.",
        example: "primary",
    },
    Field {
        label: "max turns",
        detail: "Claude subagent `maxTurns`: a hard ceiling on the agent's agentic turns. At the \
                 limit Claude Code returns its output marked partial, which can be resumed. \
                 Defaults come from the role (explorer 40, implementer 60, reviewer 30, verifier \
                 40, adversary 40); '-' means unlimited.",
        example: "60",
    },
    Field {
        label: "color",
        detail: "A Claude Code named colour: red, blue, green, yellow, purple, orange, pink, or                  cyan.",
        example: "cyan",
    },
    Field {
        label: "skill",
        detail: "A reusable capability written to .claude/skills/<name>/SKILL.md. The description \
                 is the trigger — Claude loads the skill from it alone — so say what it does and \
                 when (\"Use when …\"). Name: lowercase letters, digits, hyphens (≤ 64). Keep the \
                 body short (detail in reference.md, logic in scripts/), grant the fewest \
                 allowed-tools (scope Bash as Bash(cmd:*)), and make anything with side effects \
                 user-run only. `ocgen add skill` checks these and `ocgen landscape` reports them. \
                 Optional: paths (globs that limit activation), arguments ($name), \
                 disallowed-tools, effort, background (forked skills). ocgen's own workflows \
                 (/deliver, /inquire, …) are skills too, so their names are reserved.",
        example: "commit",
    },
    Field {
        label: "output / plugin",
        detail: "The Claude target can emit a project .claude/ tree, a distributable plugin                  (.claude-plugin/ + a GitHub release workflow), or both. A plugin needs a GitHub                  owner/repo so users can install it with `claude plugin marketplace add`.",
        example: "both  (--output both --repo owner/repo)",
    },
    Field {
        label: "agent teams",
        detail: "Opt-in (experimental). Enables Claude Code Agent Teams: sets \
                 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 and teammateMode in settings.json, \
                 emits a /team command, CLAUDE.md guidance, and quality-gate hook \
                 scripts. Your subagent roles double as reusable teammates.",
        example: "--team  (mode: in-process)",
    },
    Field {
        label: "team governance",
        detail: "Optional gates layered on Agent Teams. Plan gate: /team-plan builds the shared \
                 task list + risk register and the TaskCreated hook blocks work until \
                 .claude/team/plan.md says `Status: APPROVED session=<id>`; editing the plan \
                 (ticking a task or closing a risk isn't an edit), or a later session or team \
                 run, makes the approval stale until you approve again. Confidence gate: a \
                 teammate's own last message must hold a line `Confidence: NN%` >= N (the last \
                 such line counts; 0 disables) before its task completes — never the task's \
                 subject or description. Risk rounds: every risk, owned by a teammate name \
                 (`Owner: implementer-1`), gets a mitigation round before that teammate goes idle; \
                 a role as owner holds no one.",
        example: "--team-confidence 96  --no-plan-gate  --no-risk-rounds",
    },
    Field {
        label: "execution approval gate",
        detail: "A human-approval line for high-impact EXTERNAL actions (ssh, cloud mutations, \
                 git push/merge, gh pr merge/release/workflow run, terraform/pulumi/kubectl/helm, \
                 deploys, publishes). A PreToolUse hook checks every tool call's command and file \
                 path (through flags, quoting and shell operators) and blocks a match until you \
                 run `ocgen approve` from your own terminal (1 to 1440 minutes, 30 by default; \
                 the command refuses to run under Claude Code). The approval is stored outside \
                 the project (~/.claude/ocgen/approvals), and tool calls that name it are \
                 blocked. The hook is a text match — a guard-rail, not a boundary: the sandbox \
                 is the boundary (sandboxed commands can't write the approval store and, unless \
                 you allow credentials, run without the credentials ocgen knows about), and it \
                 is on by default with the gate on macOS, Linux and WSL2. A git pre-push hook \
                 is a backstop for pushes made under Claude Code (`--no-verify` skips it); \
                 branch protection on the server is what really stops a push. With the gate on, \
                 settings.json always denies Edit(~/.claude/ocgen/**) and `ocgen approve`, and \
                 asks before edits to the gate's hooks, settings, state file, statusline and git \
                 hooks, and before `ocgen edit` and `ocgen doctor`.",
        example: "on by default with --team;  --no-approval-gate to disable",
    },
    Field {
        label: "worker confidence",
        detail: "The bar (default 96, 0 = off) for a subagent that changed its tree since it \
                 started — edits or commits; SubagentStart records where it began. Its final \
                 message must hold a line `Confidence: NN%` at or above the bar; a number \
                 mentioned mid-sentence doesn't count, and the last such line wins. Read-only \
                 roles (SUBAGENT_READONLY_ROLES: agents without Write/Edit, plus the built-in \
                 Explore and Plan) are never gated.",
        example: "96  (SUBAGENT_CONFIDENCE_THRESHOLD in settings.json)",
    },
    Field {
        label: "sandbox",
        detail: "Runs Bash with OS-level file and network limits (Seatbelt on macOS, bubblewrap \
                 on Linux/WSL2; native Windows has none), a starter network allowlist plus your \
                 extra domains, and no unsandboxed retries. Sandboxed commands can't write the \
                 approval store (keep ~/.claude a real folder: a symbolic link there is protected \
                 where it points, not by its name), the statusline, ocgen's state file or \
                 .git/hooks. Unless you allow credentials, it withholds the credential files and \
                 variables of ssh, git, GitHub, the clouds, Docker and the package registries; \
                 git starts with no credential helper, so it doesn't ask an OS keychain the \
                 sandbox can't hide — though a command can name one itself (`ocgen verify` \
                 warns while one is configured). It contains shell \
                 commands only, not the file tools, hooks or MCP servers (the hooks run the \
                 check and formatter in a sandbox of their own). The wizard asks after the \
                 Agent Teams questions and defaults to yes when the approval gate is on and the \
                 platform has a sandbox; without it the gate is advisory. claude.sandbox.declined \
                 records a no to that default, so `ocgen edit team` keeps your no as its default.",
        example: "sandbox.enabled: true   extra_domains: [registry.example.com]",
    },
    Field {
        label: "permission rules (yours)",
        detail: "Your own allow/ask/deny rules (`ocgen edit permissions`), kept in the state and \
                 merged with ocgen's in settings.json by strictness, deny > ask > allow: a rule \
                 stricter than a generated one takes its place, a looser one has no effect, and \
                 each rule sits in one list. Rules added to settings.json by hand are adopted as \
                 yours by the next add, edit or doctor. `--list` shows what settings.json holds.",
        example: "--deny \"Bash(git push:*)\"   (moves it from ocgen's ask list to deny)",
    },
    Field {
        label: "check command",
        detail: "A command that must exit 0 before a worker that changed files may finish, and \
                 before a team task may complete — e.g. cargo test or terraform validate. An \
                 objective gate next to the self-reported confidence: a failing check sends the \
                 work back with the tail of its output, whatever confidence was stated. It runs \
                 in the worker's own directory; team checks run one at a time, and a task may \
                 narrow it with a `Check: <this command> <args>` line in its description (any \
                 other Check: line is ignored). It is stopped, with all it started, after \
                 OCGEN_CHECK_TIMEOUT seconds (300). With the sandbox on, the hooks run it — and \
                 the formatter — in an OS sandbox that can't write the approval store, ~/.claude, \
                 shell and git config or the project's Claude settings and git hooks, and \
                 (unless you allow credentials) without credentials; on Linux without bubblewrap \
                 the check isn't run and counts as failed. Empty = off.",
        example: "cargo test",
    },
    Field {
        label: "loop guard",
        detail: "Caps how many times one gate may block the same agent on the same task, role or \
                 action (default 3; it trips early when stated confidence stops rising), so no \
                 hook holds an agent forever. Quality gates (subagent/teammate confidence, risk \
                 rounds) then release the agent with the result marked UNRESOLVED; the plan and \
                 execution-approval gates never release — they tell the agent to stop and halt it. \
                 Every trip is logged to .claude/loop-guard/escalations.md. 0 = unlimited.",
        example: "3  (LOOP_GUARD_MAX_BLOCKS in settings.json)",
    },
    Field {
        label: "intent approvers",
        detail: "Who must sign off on an /intent before any work starts: GitHub users (@login) or \
                 teams (@org/team) — no emails; GitHub notifies them through the @mentions in the \
                 issue. Named in every intent file and issue draft, each pending until you report \
                 their sign-off with a link; the intent is Accepted only when every approver has \
                 approved, and /deliver won't start before that unless you explicitly confirm the \
                 bypass. Your own approval of a plan only approves it for drafting. None = the \
                 drafts say \"No approvers configured\" and /intent asks whether to proceed.",
        example: "--approver @alice --approver @org/architects",
    },
    Field {
        label: "CODEOWNERS",
        detail: "The project's existing CODEOWNERS (.github/CODEOWNERS, CODEOWNERS or \
                 docs/CODEOWNERS — ocgen never creates one), linked at .claude/CODEOWNERS. ocgen \
                 keeps a marked block there that makes the approvers code owners of the intent \
                 directory (scope intents), every file (all), or nothing (off); your own lines are \
                 never touched, and a later rule of yours that takes precedence is reported. \
                 GitHub enforces it only when branch protection requires a review from code \
                 owners — ocgen can't turn that on.",
        example: "--codeowners .github/CODEOWNERS --codeowners-scope intents",
    },
];

pub fn run() {
    ui::banner("field reference");

    ui::section("Agent fields");
    for f in AGENT_FIELDS {
        print_field(f);
    }

    ui::section("Provider fields");
    for f in PROVIDER_FIELDS {
        print_field(f);
    }

    ui::section("Utility model (compaction / title / summary)");
    for line in wrap(
        "OpenCode runs three built-in helper agents — compaction, title and summary — for \
         housekeeping. They share one provider + model, chosen separately from your agents. A \
         small, fast model is usually the right pick.",
        74,
    ) {
        println!("  {line}");
    }

    ui::section("Claude Code target fields");
    for f in CLAUDE_FIELDS {
        print_field(f);
    }

    ui::tip("set these with `ocgen new`, `ocgen add/edit agent`, `ocgen add/edit provider`, or `ocgen add skill`");
}

fn print_field(f: &Field) {
    println!("\n  {}", style(f.label).bold().cyan());
    for line in wrap(f.detail, 74) {
        println!("    {line}");
    }
    if !f.example.is_empty() {
        println!("    {}", ui::muted("example:"));
        for line in f.example.lines() {
            println!("      {}", ui::muted(line));
        }
    }
}

/// Greedy word-wrap to `width` display columns.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_respects_width_and_keeps_words() {
        let lines = wrap("the quick brown fox jumps over the lazy dog", 15);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.chars().count() <= 15));
        assert_eq!(
            lines.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );
    }

    #[test]
    fn wrap_short_text_is_one_line() {
        assert_eq!(wrap("short", 40), vec!["short".to_string()]);
        assert!(wrap("", 40).is_empty());
    }

    #[test]
    fn reference_runs() {
        // Smoke: ensure the reference renders without panicking.
        run();
    }
}
