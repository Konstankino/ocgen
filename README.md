# ocgen

An interactive bootstrapper for LLM multi-agent projects. Run it, pick the tool
the project is for, answer a few questions, and it writes a ready-to-use agent
setup — with your own agent names, roles and models.

- **[OpenCode](https://opencode.ai)**: an `opencode.json` plus a `.opencode/` tree of
  agents, prompts and commands.
- **[Claude Code](https://claude.com/claude-code)**: a `.claude/` project (subagents,
  skills, `CLAUDE.md`, `settings.json`, `.mcp.json`) and/or a distributable **plugin**.
  See [Claude Code target](#claude-code-target).

It's a single self-contained binary for **Windows, macOS and Linux**. The default
templates are baked in but fully editable.

## Install

`ocgen` runs on **Windows, macOS and Linux**. Pick your platform — every method
produces the same binary, and re-running any of them **updates** to the latest release
(see [Updating](#updating)).

> The commands below use `Konstankino/ocgen` as a placeholder — replace it with the GitHub
> repository that hosts the releases.

**Windows** (PowerShell) — downloads the latest `ocgen.exe`, installs it to
`%LOCALAPPDATA%\ocgen\bin`, and adds it to your PATH:

```powershell
irm https://raw.githubusercontent.com/Konstankino/ocgen/main/install.ps1 | iex
```

Prefer [Scoop](https://scoop.sh)? A manifest is in [`scoop/ocgen.json`](scoop/ocgen.json)
to host in a bucket, after which `scoop install ocgen` / `scoop update ocgen` work.

**macOS / Linux** — installs to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Konstankino/ocgen/main/install.sh | sh
```

Or download a prebuilt archive for your platform from the releases page and put the
binary on your PATH.

**From source** (any OS with [Rust](https://rustup.rs)):

```bash
cargo install --git https://github.com/Konstankino/ocgen ocgen
```

### Updating

New versions ship as GitHub Releases: pushing a version tag (`git tag v0.2.0 && git push
--tags`) triggers [`.github/workflows/release.yml`](.github/workflows/release.yml), which
builds and uploads binaries for Windows, macOS and Linux. To update:

- **Windows:** re-run the PowerShell one-liner, or `scoop update ocgen`.
- **macOS / Linux:** re-run the `install.sh` one-liner.
- **From source:** `cargo install --git https://github.com/Konstankino/ocgen ocgen --force`.

Every installer is idempotent — running it again just replaces the binary with the newest
release.

## Use

Just run it and answer the prompts (press Enter to accept each default):

```bash
ocgen
```

Every prompt shows a short dimmed help line reminding you what the field means.
For the full story, run `ocgen fields` — a detailed reference explaining every
agent and provider setting (temperature, steps, permissions, …), with valid
values and examples.

The wizard first asks **which tool the project is for** (Claude Code or OpenCode);
pass `--target claude` or `--target opencode` to skip that question. For an OpenCode
project it then asks for:

1. **Project basics** — project name and prompt language.
2. **Providers** — define one or more providers (key, display name, npm adapter,
   base URL, and its models). Start from the seeded provider or add your own. Pass
   `ocgen new --base-url <url>` to set the seeded provider's base URL from the command
   line (so the server address need not be baked into the template); it becomes the
   editable default at the base-URL prompt.
3. **Utility model** — which provider + model the built-in compaction/title/summary
   agents use.
4. **Agents** — start from the default 4-agent writing pipeline, then add/rename as
   many as you like. For each agent you configure **every field OpenCode exposes**:
   name, a starting **role/preset** (or `blank (custom role)`), **mode**
   (primary/subagent/all), **provider** and **model**, **variant**, **temperature**,
   **top_p**, **max steps**, **color**, **disable**, **hidden**, **description**, the
   full **permission block**, provider-specific **options** (e.g. `reasoningEffort`),
   the **system-prompt body**, and an optional **external prompt file**.
5. **Review & confirm** before anything is written.

**Existing files are never overwritten blindly.** If the directory already holds files
that differ from what would be generated, the wizard lists each one with a short diff
and asks what to do: **Overwrite all**, **Keep all existing** (write only the new
files), **Decide file by file** (Overwrite / Keep mine / Show full diff for each), or
**Cancel**. Anything overwritten is first copied to `.ocgen-backup/<timestamp>/` (the
folder git-ignores itself; the newest five are kept). Identical files and your own
`CLAUDE.md` are not conflicts. A file you keep is left untouched; `ocgen doctor
--dry-run` shows later how it differs from the generated version.

Input is validated as you type — the wizard re-prompts on bad values rather than
writing them: identifiers (agent name, provider key, model id) must be safe for
filenames and `@mentions` and unique; temperature must be 0–2 and top_p 0–1; steps
a positive integer; color a theme name or `#hex`; and a provider base URL must be a
real `http(s)://` URL.

Multi-line fields (permissions, prompt body) use a hybrid flow: the seeded default
is shown, and your `$EDITOR` opens pre-filled only if you choose to edit.

**Multiple providers, per agent:** each agent picks which provider hosts its model,
so its model reference becomes `<provider>/<model>`; all providers are written into
`opencode.json`.

**Dynamic roles:** roles are not limited to the built-in four. Pick `blank (custom
role)`, name the role, and fill in the fields yourself — no archetype file involved.

Cross-references stay consistent automatically: the coordinator's task permissions,
its prompt file, and the `multi` command are all generated from the agent names you
chose.

### Other commands

```bash
ocgen new ./my-project              # scaffold into a specific directory (asks which tool)
ocgen new ./my-project --target opencode   # scaffold an OpenCode project
ocgen new ./my-project --base-url http://192.168.1.10:8080/v1   # override the seeded provider's base URL
ocgen new ./my-project --target claude   # scaffold a Claude Code project (see "Claude Code target")
ocgen add agent ./my-project        # add one more agent to an existing project
ocgen add skill ./my-project        # author a Claude Code skill (Claude projects)
ocgen add mcp ./my-project          # add an MCP server to .mcp.json (Claude projects)
ocgen edit mcp [name] -p ./dir      # change or remove an MCP server
ocgen edit permissions -p ./dir --allow "Bash(gh run view:*)"   # your own settings.json permission rules (Claude projects)
ocgen edit permissions -p ./dir --list                          # every permission rule at a glance
ocgen edit agent [name] -p ./dir    # tweak any field of an existing agent
ocgen add provider ./my-project     # add another provider to an existing project
ocgen edit provider [key] -p ./dir  # edit a provider and its models
ocgen show agent [name] -p ./dir    # print one agent's full configuration
ocgen landscape ./my-project        # overview of all agents + providers (alias: horizon)
ocgen doctor ./my-project           # repair a project's config and rewrite its files (shows a diff, asks first)
ocgen doctor --dry-run ./my-project # show exactly what doctor would change; write nothing
ocgen verify ./my-project           # check the project actually works (exit 1 on failure; CI-friendly)
ocgen managed-settings              # print an organisation policy (managed-settings.json)
ocgen approve ./my-project          # a human unlocks pushes/deploys for 30 min (human-only)
ocgen fields                        # explain every configurable field (alias: reference)
ocgen templates init                # copy the editable templates to ~/.config/ocgen/templates
ocgen templates path                # print that directory
ocgen templates list                # show which templates are overridden vs built-in
ocgen templates edit [path]         # edit a template in $EDITOR; saves to the override dir
```

`ocgen templates edit` opens a template in your `$EDITOR` pre-filled with its current
content (your override if you have one, otherwise the embedded default) and saves the
result into `~/.config/ocgen/templates/` — so you can tweak an existing template (or a
one you added yourself, like a new archetype) without running `templates init` or hunting
for files. Pass a path (e.g. `ocgen templates edit seeds.toml` or
`ocgen templates edit archetypes/reviewer.toml`) or omit it to pick from a list. Edits to
a `.toml` template are checked for valid syntax, with a warning if they don't parse.

`ocgen edit agent` reloads the saved project, lets you pick an agent (or names one
directly), and walks every field seeded with its current value — press Enter to
keep, or type a new value. It then re-renders the whole project, so renaming an
agent updates the coordinator's task permissions, its prompt file and the `multi`
command, and removes the old files. This is how you adjust an agent that has already
been in use for a while.

`ocgen edit provider` works the same way for providers: pick one, edit its key,
name, npm adapter, base URL, and its model list (keep/edit/remove each model, then
add new ones). Renaming a provider's key automatically updates every agent that
referenced it and the utility provider.

The `add`, `edit` and `landscape` commands find the project automatically: they
look for it in the given directory (default: the current one) and walk **up**
through parent directories, so you can run them from anywhere inside a project —
not just its root. If no project is found, they say so and point you at `ocgen new`.

`ocgen landscape` (alias `ocgen horizon`) prints a read-only overview so you can
review the whole setup at a glance: a providers table (with the utility provider
marked), an agents table (mode, model, temperature, steps, colour, prompt, and a
one-line description), the delegation topology (which primary delegates to which
subagents, and the `/multi` command), and a **Checks** section that flags problems
worth fixing — an agent pointing at an unknown provider or a model its provider
doesn't offer, missing or multiple coordinators, or an undefined utility provider.
Use it to spot what to change, then adjust with the `edit` commands above.

`ocgen show agent [name]` zooms into a single agent and prints its **full**
configuration: every field (with the colour shown as a swatch), the effective
permission block (including the auto-generated `task:` list for a coordinator), any
provider-specific options, the system prompt (inline or the external file), and how
it fits with the team (what it delegates to, or which coordinator calls it). Omit the
name to pick one from a list.

`ocgen doctor` repairs a project and rewrites its files: it reassigns agents that
point at an unknown provider or a model their provider doesn't offer, fills empty
required fields (permissions, temperature, colour, role), fixes the utility
provider/model, and disables a prompt file that has no body. It reports each change,
then regenerates everything. Running it on a project made by an **older version of
ocgen** also upgrades it — the state file and generated files are rewritten in the
current format.

**`doctor` shows its plan before it writes.** Every file it would add (`+`), change (`~`) or
delete as stale (`-`) is listed with a short diff.
- **Your hand edits are flagged by name.** ocgen fingerprints every file it writes, so it can
  tell your edits apart from its own updates. For a hand-edited `settings.json` it points you
  to `.claude/settings.local.json`, which ocgen never touches. For permission rules the whole
  project should share, use `ocgen edit permissions` instead.
- **Confirmation:** in a terminal it asks before applying. `--yes` skips the question, and
  non-interactive runs such as CI apply directly.
- **Backups:** before overwriting or deleting anything it copies the previous versions to
  `.ocgen-backup/<UTC timestamp>/`, which git-ignores itself and keeps the newest 5. Copy a
  file back to restore it.
- **Preview:** `--dry-run` shows the plan and writes nothing.

**`ocgen verify [dir]`** checks that a generated project *works*, not just that it exists:
- files are up to date (the same plan as `doctor`), and there are no consistency issues;
- `settings.json` is valid JSON with known keys;
- every hook script exists and parses, and the side-effect-free hook commands actually run;
- every hook pins `"shell": "bash"`, and on Windows Git Bash is installed. The hook commands
  are POSIX sh; without Git Bash Claude Code runs them in PowerShell, where they fail to
  parse and **the gates let everything through** — install Git for Windows (or set
  `CLAUDE_CODE_GIT_BASH_PATH`);
- the approval gate blocks `git push` and allows `git status`, using the real generated
  command;
- the statusline renders;
- a compatible `ocgen` is on your `PATH`; a stale binary is flagged;
- `.claude/` is tracked by git;
- `claude plugin validate` accepts the agents, skills and plugin (skip with `--no-claude`).

Hooks run with the loop guard off, so verification leaves no state behind, and hooks with
side effects (notifications, the formatter, the audit log) aren't executed. It exits 1 on
any failure, so it can gate CI.

### Backward compatibility

State files (`.opencode/.ocgen-state.json`) written by older versions of ocgen are
migrated automatically on load: an old single-provider project is converted to the
providers list, and agent fields added in later versions are backfilled from the
agent's original role/archetype. So older projects keep working with `landscape`,
`add`, `edit`, and `doctor` without manual edits.

## Claude Code target

> **Scope.** The two targets share the wizard, agents, templates and the `landscape`/`doctor`
> tooling. Everything built on Claude Code's own mechanisms is **Claude-only**, and the OpenCode
> target keeps its current feature set:
> - governance gates, the loop guard and the approval gate (hooks);
> - skills and the workflow skills (`/deliver`, `/inquire`, …);
> - MCP, the statusline, subagent controls, and plugin output.
>
> OpenCode has no hook system, so the gates can't be ported.

ocgen also scaffolds [Claude Code](https://claude.com/claude-code) projects. Because
Claude Code has no per-agent providers or base URLs, an agent's model is a Claude
**alias** (`opus`/`sonnet`/`haiku`/`fable`/`inherit`) or a full model ID such as `claude-opus-5-5`
(an alias follows the newest model, an ID pins one) rather than a `provider/model` id — so the
provider and utility-model questions are replaced by an alias plus a **tools** allow-list
per agent.

```bash
ocgen new ./my-team --target claude
ocgen new ./my-team --target claude --output both --repo me/my-team
```

`--target claude` switches the wizard to the Claude flow; `--output` chooses what to emit
(`project`, `plugin`, or `both`; default `project`); `--repo <owner/name>` sets the GitHub
repo used for a plugin's marketplace and release workflow; `--team` enables
[Agent Teams](#agent-teams).

**Project output** writes:

```
.claude/agents/<name>.md             # each subagent: model, tools, disallowedTools, effort, memory, MCP, … + body
.claude/skills/multi/SKILL.md        # /multi — fan the whole team out on one task (you run it)
.claude/skills/intake/SKILL.md       # /intake — a structured requirements interview
.claude/skills/refine/SKILL.md       # /refine — propose, take reasoned push-back, iterate
.claude/skills/deliver/SKILL.md      # /deliver — route → sharpen → requirements → plan → research → gated execution (you run it)
.claude/skills/inquire/SKILL.md      # /inquire — understand a codebase with evidence and next-question nudges
.claude/skills/fanout/SKILL.md       # /fanout — worktree-isolated parallel writers (you run it)
.claude/skills/improve-prompt/SKILL.md
.claude/skills/<name>/SKILL.md       # your own skills (`ocgen add skill`)
.claude/settings.json                # $schema, model, permissions (+ secret read-denies), hooks, output style, statusline
.claude/hooks/*.sh                   # gate scripts, loop guard, optional notify/format/audit hooks
.claude/output-styles/ocgen-concise.md
.mcp.json                            # project MCP servers (`ocgen add mcp`)
CLAUDE.md                      # project instructions + the roster; the coordinator lives here
```

The **coordinator** (an agent whose mode is `primary`) is not written as an agent file —
it becomes `CLAUDE.md` plus the `/multi` command, because Claude Code's main session *is*
the coordinator. Every other agent becomes a subagent file. The wizard opens `CLAUDE.md`
in your `$EDITOR` so you can shape the project instructions before anything is written.

**Workflow commands are skills.** Claude Code merged commands into skills, so ocgen renders
its workflows as `.claude/skills/<name>/SKILL.md`. You still type `/deliver`, `/inquire` and so on.
- **You start the side-effecting ones:** `/multi`, `/fanout`, `/deliver`, `/team` and
  `/team-plan` carry `disable-model-invocation: true`, so Claude can't start them on its own.
- **Claude may use the rest when relevant:** `/inquire`, `/intake`, `/refine` and
  `/improve-prompt`.
- **Upgrading:** `ocgen doctor` removes the old generated `.claude/commands/*.md`. Files you
  wrote yourself are kept.
- **Reserved names:** a skill of your own can't take a workflow name.

**Extra hooks** (offered with the power-user defaults; none of them ever block Claude):
- **Re-inject context after compaction** (on by default): when a conversation is compacted,
  Claude is re-pointed at `.claude/rules/` and any `/inquire` notes in `.claude/notes/`.
- **Desktop notifications:** when Claude needs you, or a turn fails (`osascript` on macOS,
  `notify-send` on Linux, otherwise a terminal bell).
- **Formatter after edits:** e.g. `cargo fmt` or `terraform fmt -recursive`. It runs after
  every `Edit`/`Write`, and a failure is reported but never blocks.
- **Config audit:** settings/skills changes made during a session are logged to the
  git-ignored `.claude/audit/config-changes.log`.

The plugin output carries the same hooks.

**Plugin output** (`--output plugin` or `both`) additionally writes, under
`plugin/<name>/`, a complete Claude Code plugin: `.claude-plugin/plugin.json` +
`marketplace.json`, the `agents/`, `skills/` and `output-styles/` trees, `.mcp.json`, a
`hooks/hooks.json`, a `README.md`, and a `.github/workflows/release.yml` that zips the
plugin and attaches it to a GitHub Release on a version tag. The plugin carries the **same
gates** as the project. Its `hooks.json` registers the confidence gates, approval gate, team
gates and loop guard from `${CLAUDE_PLUGIN_ROOT}/hooks/`. Plugins can't set `env`, so each
hook command carries its thresholds as a prefix. Both manifests pass `claude plugin validate`.
Users install it with:

```bash
claude plugin marketplace add <owner>/<repo>
claude plugin install <name>@<name>
```

**Power-user defaults** (toggled in the wizard) put these into `settings.json`:
- a permissions list: safe reads and git commands allowed; `git commit`/`git push` ask first;
  `rm -rf` denied;
- **read-denies for secrets**: `.env`, `.env.local`, `*.pem`, `*.key`, `*.tfstate`. Example
  files such as `.env.example` and `example.tfvars` stay readable;
- a `SessionStart` hook that nudges `/intake`;
- the `ocgen-concise` output style. It sets `keep-coding-instructions: true` so Claude keeps
  its software-engineering instructions, and has its own name so it doesn't shadow the
  built-in `Concise`. `doctor` removes the old generated `concise.md`;
- a **statusline** (`.claude/statusline.sh`), in the dim style of a typical personal
  statusline:

  ```
  Opus 5.5  my-project (main)  58% left  $0.42  wt:feature-a  ⚠ 2 unresolved
  ```

  - model, folder and branch;
  - context left, turning yellow under 30% and red under 15%;
  - the session's cost so far;
  - the worktree, when you're in one;
  - the number of results the loop guard released as UNRESOLVED (from
    `.claude/loop-guard/escalations.md`; delete that file once you've reviewed them).

  It uses `jq` when available, and plain `sh` otherwise. A project `statusLine` overrides your
  personal one; to keep yours, set `statusLine` in `.claude/settings.local.json`. Plugins can't
  set a statusline, so the plugin output doesn't include it.

Every generated `settings.json` declares
`"$schema": "https://json.schemastore.org/claude-code-settings.json"`, so editors validate
and autocomplete it.

**Subagent controls.** Beyond model, tools and color, each subagent can set:
- `disallowedTools`: removes tools even if its tool list would allow them. The explorer,
  reviewer and verifier ship with `Edit, Write, NotebookEdit` denied, so they are **hard
  read-only**.
- `effort`: the reviewer uses `high`.
- `permissionMode`.
- `memory`: `project` is committed under `.claude/agent-memory/`, `local` is git-ignored,
  `user` spans every project.
- `background`.
- preloaded `skills`.
- the **MCP servers** it may use.

All are optional and set in `ocgen add/edit agent`. `landscape` and `doctor` check their values
and warn about `permissionMode: bypassPermissions`.

**MCP servers** connect Claude to tools like GitHub, Terraform or a database:

```bash
ocgen add mcp ./my-team                 # stdio command or http/sse URL → .mcp.json
ocgen edit mcp [name] -p ./my-team      # change or remove
```

Servers go in `.mcp.json` at the repo root (and the plugin, if you build one).
- **Secrets:** reference them as `${VAR}`. `landscape` flags literal-looking tokens, so they
  never land in git.
- **Pre-approving** a server writes `enabledMcpjsonServers`, so teammates aren't prompted once
  they trust the folder.
- **Per-agent access:** pick which subagents may use each server. Any of them with a tool
  allow-list get `mcp__<server>` added automatically.
- **Status:** `claude mcp list` inside Claude Code shows each server's status.

**Skills** can be authored on their own:

```bash
ocgen add skill ./my-team        # is it a skill? → preset → fields → checks → optional stubs
ocgen edit skill [name] -p ./my-team
```

**First, is a skill the right tool?** `add skill` asks before anything else:

| You need… | Use | Why |
|---|---|---|
| A capability Claude uses when relevant, or a procedure you run by name | **a skill** | Only its description sits in context until it's needed, so it costs almost nothing |
| Guidance Claude follows in every session | `CLAUDE.md` or `.claude/rules/*.md` | Always loaded; nothing needs to trigger it |
| A worker with its own context and tools | a subagent (`ocgen add agent`) | Its file reads don't fill your main context |

**Presets are listed by when to choose them:**
- `command`: a procedure you run by name that has side effects. It's user-run only and never starts on its own.
- `knowledge`: conventions or domain rules Claude applies automatically when relevant.
- `forked-research`: heavy reading in an isolated `Explore` subagent; you get a summary back.
- `improve-prompt`: rewrites a prompt on request.
- blank.

Presets are editable templates (`claude/skill-presets.toml`, including their `choose_when` text).

**What makes a skill good.** Every prompt explains its why, and after you finish, the skill is
checked against these rules. If any fail, you're offered **Revise now?**. `ocgen landscape`
shows the same checks for existing skills.

- **The description is the trigger.** Claude loads a skill from its description alone, so say
  what it does *and when* ("Use when …"), in the words you'd type. It's flagged if empty, under
  40 characters, over the 1024-character limit, or missing a "when" (user-run skills are exempt).
- **A valid name:** lowercase letters, digits and hyphens, up to 64 characters (`tf-plan-review`).
- **Keep `SKILL.md` short.** Over 500 lines is flagged. Put long reference in `reference.md`
  and deterministic logic in `scripts/`. `add skill` can create stubs for both, and ocgen never
  rewrites them. A rename carries them to the new folder.
- **Grant the fewest tools.** `allowed-tools` run without asking, so an unscoped `Bash` is
  flagged; scope it as `Bash(git status:*)`. A skill that can start on its own **and**
  pre-approves `Write`/`Edit`/`Bash` is flagged too: make it user-run only, or drop those tools.
- **Fork heavy reading** (`context: fork`), so the research doesn't flood your conversation.
- **No contradictions:** user-run only *and* hidden from the `/` menu means nobody can invoke it.

Newer skill controls are prompted too, each optional:
- **`paths`:** globs such as `**/*.tf`, so the skill only activates for matching files.
- **`arguments`:** named arguments (`plan_file` → `$plan_file`).
- **`disallowed-tools`:** tools removed while the skill runs.
- **`effort`:** how hard Claude thinks during the skill.
- **`background`:** for forked skills.

The tool picker multi-selects the built-in tools and accepts `Bash(cmd:*)` / `mcp__server__tool`
patterns, warning (not rejecting) on unrecognized names. The full `SKILL.md` frontmatter is
exposed: `description`, `when_to_use`, `argument-hint`, `allowed-tools`,
`disable-model-invocation`, `user-invocable`, `context: fork` + `agent`, and `model`.

**After creating it:** test that it triggers by running the **skill-creator** skill on it in
Claude Code. Most skills that "don't work" never trigger. Change it later with `ocgen edit skill`,
not by hand: `ocgen doctor` rewrites `SKILL.md` from saved state. `reference.md` and `scripts/`
are yours.

#### Delivering and understanding code (`/deliver`, `/inquire`)

Two generated commands cover the two kinds of work you bring to a codebase. Both are on by
default; the wizard asks about each ("Include the /deliver pipeline command?", "Include the
/inquire codebase Q&A command?").

**`/deliver <goal>`** takes a change end-to-end in one session. It first **routes** the goal —
*build/change* runs the pipeline, *understand* hands off to `/inquire`, *mixed* runs a short
`/inquire` orientation and then continues — and tells you the classification so you can
override it. The pipeline then: sharpens the goal; skims the relevant code and interviews you
one question at a time (never asking what the code already answers); drafts a plan + risk
register for your approval; researches in parallel (re-planning if findings contradict the
plan); executes with a stated `Confidence: NN%` per decision and no deploys/pushes without
your approval; and summarizes. Small, clear goals collapse the interview and plan into a
one-paragraph brief.

**`/inquire <topic or question>`** is a read-only loop for learning a codebase by asking
questions — and for getting better at asking them:

```
/inquire how does a request get from the CLI to the renderer?
```

1. **Orient** — on first use it maps the codebase (entry points, modules, data flow,
   build/test commands, git hotspots) into a ledger; later runs resume from the ledger.
2. **Sharpen** — a vague, too-broad or assumption-laden question gets 2–3 sharper variants
   to choose from (or keep your own).
3. **Answer with evidence** — every claim cites `file:line` and is tagged **Verified** (read
   or ran it) or **Inferred** (with a confidence), plus what wasn't checked.
4. **Nudge** — one **hint**, not a question: a single line pointing at where your understanding
   is thinnest, tagged with a lens (structure, flow, contract, rationale, change impact,
   failure). For example: `Hint: (Failure) the retry path in src/queue.rs is only Inferred.`
   It never lists ready-made next questions, because you learn more by forming the question
   yourself. Say "hint" for another one.
5. **Checkpoint** — every ~5 questions (or say "checkpoint") it summarizes your mental model,
   lists the biggest unknowns, and can ask you to explain a piece back to catch gaps.

Say "stop" to finish; it finalizes the ledger and lists the open questions. The ledger lives in
`.claude/notes/<topic>.md` and is **git-ignored** (the command adds `.claude/notes/` to
`.gitignore` if it's missing), so it's personal to this checkout. Once you know what to change,
run `/deliver <change>` — the ledger becomes its starting context.

**Resuming later.** The ledger is saved after every answer, so you can simply close the session.
It's written quietly by a background subagent (on `haiku`), so you don't get a diff after every
answer that repeats what you just read. Each Q&A entry is capped at 4 lines.
To come back hours or days later, start a new session and run `/inquire` with **no arguments**: it
lists your ledgers, newest first, and you pick one. `/inquire <topic>` resumes that topic directly.
You get a one-screen **refresher** before continuing:

1. where you left off: the last question and the hint that followed it;
2. your mental model so far, in up to 7 bullets;
3. what changed in the code since then. Answers whose cited files have new commits are marked
   **Stale — re-verify**, and it offers to re-check those first;
4. an optional 2-question recall warm-up.

It then asks what you want to ask next: follow the hint, re-verify stale answers, or something
new. For a short break in the same session, `claude --continue` (or `claude --resume`
to pick a conversation) restores the full chat instead. The ledger is the better choice after a
longer gap, because it gives a compact recap and catches code changes.

To add these commands to an existing project, re-render it with `ocgen doctor [dir]` (projects
created by older versions get `/inquire` switched on automatically).

#### Agent Teams

[Agent Teams](https://code.claude.com/docs/en/agent-teams) coordinate several parallel
Claude Code sessions — a lead spawns teammates that work independently, share a task list,
and message each other. It's experimental and **off by default**; enable it with `--team`
(or the wizard's "Enable Agent Teams?" prompt):

```bash
ocgen new ./my-team --target claude --team
```

When enabled, ocgen adds to the generated project:

```
.claude/settings.json          # env.CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS="1" + teammateMode
.claude/skills/team/SKILL.md   # /team <task> — spawn a parallel team from your agent roles
.claude/hooks/team-*.sh         # TeammateIdle / TaskCreated / TaskCompleted gate stubs (no-op)
CLAUDE.md                      # an "Agent Teams" section explaining how to spawn one
```

Your **subagents double as teammate roles** — Claude reads a named agent's `tools`/`model`/
body when spawning it as a teammate (and auto-adds `SendMessage` + the Task tools), so no
extra wiring is needed. The default display mode is `in-process`; edit `teammateMode`
(`in-process`/`auto`/`tmux`/`iterm2`) in `settings.json`, or move it to
`~/.claude/settings.json` if a project-scope value isn't honored. ocgen never pre-authors
runtime team state (`~/.claude/teams/…` is generated by Claude Code at session start). Use
teams for parallel research/review or independent modules; for sequential work, the
`/multi` subagent flow (still generated) is cheaper.

#### Hooks run in Rust (with a script fallback)

Every gate and optional hook is implemented twice: as the POSIX `sh` script in `.claude/hooks/`
and inside the ocgen binary (`ocgen hook <name>`). The generated hook command uses the
binary when a compatible ocgen is installed, and the script otherwise:

```sh
if [ "$(ocgen hook --check 2>/dev/null)" = "ocgen-hooks 3" ]; then ocgen hook team-approval-gate; else sh ".../team-approval-gate.sh"; fi
```

- **The binary gives you** real JSON parsing instead of `grep`, and hooks that work on
  **native Windows**.
- **The scripts keep the project working** for teammates and CI machines without ocgen.
  They need only a POSIX `sh` (Git Bash on Windows): `jq` is used when present and is never
  required, and Windows paths (`C:\Users\...`) are handled in both implementations.
- **A stale binary can't weaken a gate.** The command uses the binary only when it reports
  *exactly* the hook protocol the project was generated with. The protocol number goes up
  whenever hook behaviour changes, so any other ocgen, older or newer, falls back to the
  project's own scripts, which always match the project.
- **The two behave the same.** They share the same messages, exit codes and loop-guard
  state files, so machines with and without ocgen can work on one project. Parity tests
  (`tests/hooks.rs`) run every gate scenario through both implementations and require
  identical results. These tests already caught a stray-error bug in one script.

#### The execution-approval line

With Agent Teams' approval gate on, no agent can push, deploy, mutate cloud resources, publish
or open a remote shell until **a human approves**, from their own terminal:

```bash
ocgen approve              # unlock for 30 minutes (then it re-locks by itself)
ocgen approve --minutes 10 # a shorter window
ocgen approve --status     # locked, or minutes left
ocgen approve --revoke     # re-lock now
```

- **Only a human can approve.** `ocgen approve` refuses to run under Claude Code or without a
  terminal. The approval is stored outside the project (`~/.claude/ocgen/approvals/`, one per
  repository, shared by its worktrees), so an agent can't create it as part of normal work.
  The hook also blocks any tool call that runs `ocgen approve` or touches that folder, and
  `permissions.deny` covers it too.
- **It expires.** A forgotten approval is locked again when its time is up.
- **Detection survives phrasing.** The gate matches through global flags and quoting
  (`git -C . push`, `git "push"`, `terraform -chdir=infra apply`, `kubectl -n prod delete`),
  and also catches deploy and publish scripts and make targets (`sh ./deploy.sh`,
  `make deploy`). Reading or grepping those files is still allowed.
- **A git `pre-push` hook stops what text matching can't see.** `doctor` installs it in
  `.git/hooks/pre-push`, unless you already have a pre-push hook, in which case it tells you
  the one line to add. It blocks any push made *under Claude Code* without approval, however
  it's launched: a script, a Makefile, an interpreter. Your own pushes are never affected.
- **Credentials are off limits.** Reading `~/.ssh`, `~/.aws`, the `gh` token, kube and gcloud
  config is denied. With the sandbox on, they're withheld from shell commands too (unless you
  opt in), so an agent can't push or deploy at all, and you do those steps yourself.

The old `touch .claude/team/execution-approved` file is no longer honoured, because an agent
could create it. `ocgen verify` flags a leftover one.

If you use `/fanout` with the gate on: its final `git merge` is gated too, so approve before
the merge step.

#### The check command: an objective gate

Stated confidence is self-reported. Set a **check command** in the wizard ("Check command that
must pass before a worker finishes", e.g. `cargo test`, `npm test`, `terraform validate`) and
the gates stop taking the agent's word alone:

- **Workers:** a subagent that changed files can't finish until the check passes. It runs in
  the worker's own directory, which is its worktree for `/fanout`.
- **Team tasks:** a task can't be completed until the check passes.
- **When it fails:** the agent sees the last 15 lines of output and is sent back. A stated
  confidence doesn't override a failing check.
- **Still bounded:** the loop guard releases a check that keeps failing as UNRESOLVED.
- **Timeout:** the Rust hook kills a check after 5 minutes (`OCGEN_CHECK_TIMEOUT` seconds).

The check runs first, then the confidence statement, and either can be off. `ocgen verify`
warns when no check is set, and `ocgen verify --run-check` runs it.

#### Defence in depth

The approval gate matches command text, so it isn't the only safety line:
- **`permissions.ask`** (with the power-user defaults): Claude Code itself asks before high-impact
  commands (`git push`, `terraform apply/destroy`, `kubectl apply/delete`, `helm`,
  `gh pr merge`/`release`, npm/cargo publish, `docker push`, `ssh`/`scp`/`rsync`), **even in
  auto mode**.
- **Sandbox (opt-in):** the wizard's "Enable the sandbox?" runs shell commands with OS-level
  file and network limits (Seatbelt on macOS, bubblewrap on Linux/WSL2). It comes with a
  starter network allowlist (GitHub, npm, crates.io, PyPI, Go, Terraform) plus your own
  domains, and strict mode, so a sandboxed failure can't be retried unsandboxed. The secret
  read-denies apply inside the sandbox too. By default it also withholds your push and deploy
  credentials, and makes the approval store unwritable.
- **Organisation policy:** `ocgen managed-settings > managed-settings.json` gives an admin a
  policy that projects can't override: no bypass mode, secrets unreadable, the ask rules,
  and a strict sandbox. It prints where to deploy it on each OS.

#### Loop guards

Gate hooks block by exiting 2, and Claude Code puts no limit on how often a hook may block — so
an agent that can't satisfy a gate would be sent back forever. ocgen bounds every blocking gate
with a shared `.claude/hooks/loop-guard.sh`:

- **Budget.** A gate may block the same agent on the same thing (a task, a role, a worker) at
  most **3** times by default. It trips **early** when the agent's stated `Confidence: NN%` stops
  rising between blocks, since that means it's stuck rather than converging. Each block tells the
  agent how much budget is left, and the gate resets once it passes.
- **Quality gates release.** The subagent-confidence, teammate-confidence and risk-round gates let
  the agent stop, complete or idle once the budget is spent, **marked UNRESOLVED**: you get a
  warning, and an entry is written to `.claude/loop-guard/escalations.md`.
- **Approval gates never release.** The plan gate keeps blocking but tells the agent to stop
  retrying and go idle. The execution-approval gate denies the command **and halts the agent**
  (`continue: false`). An unapproved plan or a `git push` never gets through.
- **Turn ceilings.** Each subagent gets a `maxTurns` limit (explorer 40, implementer 60, reviewer
  30, verifier 40). At the limit its output comes back marked partial and can be resumed once.
- **Loop discipline.** The workflow rule tells agents to change approach after two failures,
  report the blocker after the third, re-plan at most twice, and never inflate confidence to get
  past a gate. The coordinator must report every escalation as unresolved.

Tune the budget with the wizard's "Max times a gate may block the same agent" question, or set
`claude.workflow.loop_guard_max` in `.claude/.ocgen-state.json` and run `ocgen doctor` (`0` =
unlimited, not recommended). Set a subagent's max turns with `ocgen edit agent`. The
`.claude/loop-guard/` directory git-ignores itself. Projects made by older versions get the
loop guard from `ocgen doctor`. Their existing agents keep unlimited turns until you set
max turns with `ocgen edit agent`.

#### Worktrees: commit `.claude/`

A git worktree is a fresh checkout of **tracked** files. `claude --worktree <name>` sessions,
desktop worktree sessions and ocgen's multi-session delivery all start in one. If your project
git-ignores `.claude/`, these sessions start **without** its settings, hooks and rules. That
means no approval gate, confidence gates or loop guard, no permission deny list, and whatever
model your `~/.claude/settings.json` names. Agents and commands still load, because Claude
Code reads them from the main checkout. Skills need Claude Code 2.1.277+ for that.

- **Commit `.claude/`** (recommended) and ignore only the local parts: `.claude/settings.local.json`,
  `.claude/worktrees/`, `.claude/notes/`, `.claude/loop-guard/` and `.claude/team/execution-*`.
- **Detection:** `ocgen landscape` and `ocgen doctor` warn when `.claude/` is git-ignored.
- **Safety net:** the generated `.worktreeinclude` copies `.claude/settings.json`, the rules, the
  hook scripts and the output styles into each new worktree. It only copies files that are
  git-ignored, so it does nothing once `.claude/` is committed. The copy is a snapshot taken
  when the worktree is created.
- **Base branch:** with `/fanout` enabled, `settings.json` sets `"worktree": {"baseRef": "head"}`.
  `/fanout` workers and `--worktree` sessions then branch from your current commit and include
  unpushed work, instead of `origin`'s default branch.
- **Study in the main checkout:** `/inquire` is read-only. Notes written inside a worktree are
  lost when it's removed, and the isolation checks stop a worktree session from writing to the
  main checkout. `/inquire` warns if you run it in a worktree.

Subagent isolation (`/fanout`, the implementer's `isolation: worktree`) is not affected by an
ignored `.claude/`. The parent session in your main checkout owns the settings, and its hooks
fire for the subagent's tool calls.

Every other command is target-aware. On a Claude project, `ocgen landscape` shows the
agents' aliases and tools, the skills, the workflow/output setup, the delegation topology
and whether Agent Teams are enabled; `ocgen show agent` shows an agent's alias, tools and
role (or, for the coordinator, that it lives in `CLAUDE.md`); and `ocgen doctor` repairs
invalid model aliases, non-Claude colours, empty roles and a bad `teammateMode`. Run
`ocgen fields` for the full Claude field
reference. Claude projects are found by their `.claude/.ocgen-state.json`, so `add`,
`edit`, `landscape` and `doctor` work from anywhere inside the project.

### Command reference (Claude Code target)

Every command below is interactive (it prompts, with a dimmed help line and defaults) and
target-aware — run it inside a Claude project (or pass its path) and it uses the Claude
flow. Commands that take a project location find it by walking **up** to the nearest
`.claude/.ocgen-state.json`; `[dir]` defaults to the current directory and `-p <dir>`
defaults to `.`.

**Create a project**

| Command | What it does |
|---|---|
| `ocgen new [dir] --target claude` | Scaffold a new Claude project (agents, `/multi` `/intake` `/refine` `/deliver` `/inquire`, `CLAUDE.md`, `settings.json`). |
| `ocgen new [dir] --target claude --output plugin` | Emit a distributable plugin instead of the project tree. |
| `ocgen new [dir] --target claude --output both --repo <owner/repo>` | Emit both the project **and** a plugin (marketplace + release workflow). |
| `ocgen new [dir] --target claude --team` | Also enable Agent Teams (env flag + `/team` + hooks + guidance). |

`--output` is `project` (default) / `plugin` / `both`; `--repo` is the GitHub `owner/repo`
for a plugin; `--team` is off by default. (The `--base-url` flag is OpenCode-only.)

**Add / update agents**

| Command | What it does |
|---|---|
| `ocgen add agent [dir]` | Add one subagent: name, preset (or blank), mode, **model alias**, **tools**, description, body (`$EDITOR`). |
| `ocgen edit agent [name] -p <dir>` | Walk every field of an existing agent, seeded with its current value (Enter keeps). Renaming updates cross-references and cleans up the old file. Omit `[name]` to pick from a list. |
| `ocgen show agent [name] -p <dir>` | Print one agent's full config (alias, tools, role, body; the coordinator shows it lives in `CLAUDE.md`). Omit `[name]` to pick from a list. |

**Add / update skills** (Claude only)

| Command | What it does |
|---|---|
| `ocgen add skill [dir]` | Author a new skill → `.claude/skills/<name>/SKILL.md` (name, description, allowed-tools, body). |
| `ocgen edit permissions -p <dir> --list` | Every permission rule at a glance — ocgen's and yours, list by list in the order Claude Code checks them; flags your rules that have no effect. Writes nothing. |
| `ocgen edit permissions -p <dir> [--allow/--ask/--deny/--remove <RULE>]…` | Add or remove your own permission rules. They are saved in the state file and appended to ocgen's generated `allow`/`ask`/`deny` lists in `settings.json`, so regeneration keeps them. Generated rules (including the approval-gate guards) can't be removed. Warns when a generated `deny`/`ask` rule overrides yours. No flags = interactive. |
| `ocgen edit skill [name] -p <dir>` | Edit an existing skill; renaming cleans up the old skill directory. Omit `[name]` to pick from a list. |

**Review, repair, reference**

| Command | What it does |
|---|---|
| `ocgen landscape [dir]` (alias `horizon`) | Read-only overview: agents (alias/tools/colour), skills, workflow/output/team setup, delegation topology, and a **Checks** section. |
| `ocgen doctor [dir] [--dry-run] [--yes]` | Repair the project and rewrite files (invalid models, colours, empty roles, bad enum values, older state files). Shows a per-file plan with diffs, flags hand edits, asks first, and backs up to `.ocgen-backup/`. |
| `ocgen verify [dir] [--no-claude]` | Check the project works: up to date, settings valid, hooks run (in bash; Git Bash present on Windows), the approval gate blocks, the statusline renders, ocgen on PATH is current, Claude Code validation passes. Exits 1 on failure. |
| `ocgen approve [dir] [--minutes N] [--status] [--revoke]` | A human approves high-impact actions for a limited time. Refuses to run under Claude Code or without a terminal. |
| `ocgen verify [dir] --run-check` | Also run the project's check command. |
| `ocgen managed-settings` | Print a recommended organisation policy (`managed-settings.json`): no bypass mode, secrets unreadable, high-impact commands always ask, strict sandbox. |
| `ocgen fields` (alias `reference`) | Explain every configurable field, including the Claude-specific ones (alias, tools, skills, output/plugin, agent teams). |

**Templates** (shared across targets)

| Command | What it does |
|---|---|
| `ocgen templates init` | Copy the editable templates to `~/.config/ocgen/templates/` (your copies win). |
| `ocgen templates path` | Print the override directory. |
| `ocgen templates list` | List resolved templates, marking overridden vs embedded. |
| `ocgen templates edit [path]` | Edit one template in `$EDITOR` (e.g. `claude/agent.md.j2`, `claude/CLAUDE.md.j2`, `archetypes/reviewer.toml`); omit the path to pick from a list. |

> `ocgen add provider` / `ocgen edit provider` apply to the **OpenCode** target only —
> Claude Code has no per-agent providers, so they aren't used for a Claude project.

### Not yet generated (roadmap)

A gap analysis against Claude Code's full configuration surface left these for later. Until
they're generated, set them by hand. Settings go in `.claude/settings.local.json` so ocgen
never overwrites them:
- **CI:** `anthropics/claude-code-action` workflows, and a devcontainer with the firewall.
- **Plugin evals** (`claude plugin eval`), and `claude plugin validate` in the release workflow.
- **Path-scoped rules** (`paths:` in `.claude/rules/*.md`), `CLAUDE.local.md`, and monorepo
  options (`worktree.sparsePaths`, `claudeMdExcludes`).
- **Workflow templates** (`.claude/workflows/*.js`), a `subagentStatusLine`, and `doctor`
  warnings for settings that only apply after workspace trust.

## Customising the templates

`ocgen templates init` copies the whole template set into
`~/.config/ocgen/templates/`. Anything you put there wins over the built-in
defaults. To change just one template without copying everything, use
`ocgen templates edit <path>`, which opens it in your `$EDITOR` (seeded with its
current content) and writes only that file to the override dir. The layout:

```
manifest.toml            # wizard questions (with help text) + default provider(s)
archetypes/*.toml        # agent role presets (mode, permissions, colour, text)
opencode.json.j2         # the provider/model config template (loops over providers)
opencode/agents/_agent.md.j2      # one generic agent, rendered per agent
opencode/commands/multi.md.j2     # a command that fans out to the subagents
seeds.toml               # blank-agent seed text (body + external prompt)
claude/agent.md.j2              # one generic Claude subagent
claude/CLAUDE.md.j2             # project instructions + roster
claude/commands/*.md.j2         # multi / intake / refine / deliver / inquire / team… (rendered as skills)
claude/skill/SKILL.md.j2        # one generic skill
claude/skill-presets.toml       # add-skill presets (command / knowledge / forked-research)
claude/hooks/team-*.sh          # Agent Teams quality-gate hook stubs
claude/output-styles/ocgen-concise.md.j2
claude/plugin/*.j2              # plugin.json, marketplace.json, README, release.yml
```

Templates use [MiniJinja](https://docs.rs/minijinja) (Jinja2 syntax). Values such
as the model, base URL, provider and language are variables filled in at
generation time.

**Add a new role preset** by dropping an `archetypes/<name>.toml` file — no code
changes needed. Each archetype declares `mode`, `default_model`, `temperature`,
`color`, an optional `steps`, a raw `permissions` YAML block, and language-keyed
`description` / `body` (and, for coordinators, an external `prompt`). For the Claude
target an archetype also supplies `claude_model` (the alias) and `tools`. Presets are
just starting points: the wizard lets you override every field, and you can always
choose `blank (custom role)` to skip presets entirely. Generated projects are
self-contained — each agent stores its own resolved fields (in the target's state file,
`.opencode/.ocgen-state.json` or `.claude/.ocgen-state.json`), so they don't depend on
the archetype files.

## Development

```bash
cargo test                       # library-level scaffold tests
cargo run --example demo -- /tmp/out default Ukrainian   # render without the wizard
```

The scaffolding logic lives in the library (`src/render.rs`). The wizard (`src/wizard/`: `mod.rs`
plus `skill.rs` and `mcp.rs`) is a [dialoguer](https://docs.rs/dialoguer) layer on top, and all
its prompts go through `src/prompt.rs`.

### Coverage

```bash
cargo cov --summary-only   # coverage of the testable code (~94% lines)
```

**The wizard is scriptable.** Every question goes through `src/prompt.rs`. A test calls
`prompt::script(&[...answers])`, and the real flow then consumes those answers instead of a
terminal. Validators still run, so tests can also assert that bad input is rejected.
`src/wizard/tests.rs` drives `add skill` and `add mcp` end to end this way. `cargo cov` (see
`.cargo/config.toml`) still excludes `src/wizard/mod.rs`, the long `new`/agent/provider flows,
until they get scripted tests.

All non-interactive logic lives in the library: input validation in `src/validate.rs` and the
consistency checks in `Project::issues`.
Everything else — rendering, migration, `doctor`, the `landscape`/`fields`/`doctor`
CLI output, and template management — is covered by unit tests plus `assert_cmd`
end-to-end tests in `tests/cli.rs`. Run `cargo cov-all` to include the wizard.
