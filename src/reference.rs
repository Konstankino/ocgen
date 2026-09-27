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
                 lowercase word with no spaces.",
        example: "editor",
    },
    Field {
        label: "role",
        detail: "A free-text label describing the agent's job. Informational only — it is shown \
                 in `ocgen landscape` but is not written into the generated agent file.",
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
        example: "edit: allow\nbash:\n  \"*\": ask\n  \"git *\": allow\nwebfetch: deny\nwebsearch: allow",
    },
    Field {
        label: "system prompt (body)",
        detail: "The agent's system prompt — its persona and standing instructions, in Markdown. \
                 The token $ARGUMENTS is replaced with the user's task when the agent is invoked.",
        example: "You are a proofreader. Fix errors and list the changes you made.",
    },
    Field {
        label: "external prompt file",
        detail: "If enabled, the system prompt is stored in a separate file \
                 .opencode/prompts/<name>.txt and referenced from the agent, instead of being \
                 inlined. Useful for long coordinator prompts, and the external prompt can \
                 reference {{ subagents }} to list the team dynamically.",
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
        label: "model (alias)",
        detail: "For the Claude Code target an agent's model is a Claude alias, not a                  provider/model id. One of opus, sonnet, haiku, fable, or inherit (use the                  session's model). There are no per-agent providers or base URLs in Claude Code.",
        example: "sonnet",
    },
    Field {
        label: "tools",
        detail: "A comma-separated allow-list of the tools a subagent may use, written to its                  frontmatter. Empty means it inherits every tool. This replaces OpenCode's                  permission block for the Claude target.",
        example: "Read, Grep, Edit, Write",
    },
    Field {
        label: "mode (coordinator)",
        detail: "A primary agent becomes the project's CLAUDE.md coordinator and the /multi                  command rather than a .claude/agents/ file; subagents each become an agent file.",
        example: "primary",
    },
    Field {
        label: "color",
        detail: "A Claude Code named colour: red, blue, green, yellow, purple, orange, pink, or                  cyan.",
        example: "cyan",
    },
    Field {
        label: "skill",
        detail: "A reusable capability written to .claude/skills/<name>/SKILL.md, with a name, a                  description that tells Claude when to use it, an optional allowed-tools list, and                  a body. Author one with `ocgen add skill`.",
        example: "commit",
    },
    Field {
        label: "output / plugin",
        detail: "The Claude target can emit a project .claude/ tree, a distributable plugin                  (.claude-plugin/ + a GitHub release workflow), or both. A plugin needs a GitHub                  owner/repo so users can install it with `claude plugin marketplace add`.",
        example: "both  (--output both --repo owner/repo)",
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
        assert_eq!(lines.join(" "), "the quick brown fox jumps over the lazy dog");
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
