# ocgen

An interactive wizard that scaffolds [OpenCode](https://opencode.ai) multi-agent
projects. Run it, answer a few questions, and it writes a ready-to-use
`opencode.json` plus a `.opencode/` tree of agents, prompts and commands — with
your own agent names, roles and models.

It can also target **[Claude Code](https://claude.com/claude-code)**: the same wizard
writes a `.claude/` project (subagents, commands, skills, `CLAUDE.md`, `settings.json`)
and/or a distributable **plugin**. See [Claude Code target](#claude-code-target).

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

The wizard asks for:

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
ocgen new ./my-project              # scaffold into a specific directory
ocgen new ./my-project --base-url http://192.168.1.10:8080/v1   # override the seeded provider's base URL
ocgen new ./my-project --target claude   # scaffold a Claude Code project (see "Claude Code target")
ocgen add agent ./my-project        # add one more agent to an existing project
ocgen add skill ./my-project        # author a Claude Code skill (Claude projects)
ocgen edit agent [name] -p ./dir    # tweak any field of an existing agent
ocgen add provider ./my-project     # add another provider to an existing project
ocgen edit provider [key] -p ./dir  # edit a provider and its models
ocgen show agent [name] -p ./dir    # print one agent's full configuration
ocgen landscape ./my-project        # overview of all agents + providers (alias: horizon)
ocgen doctor ./my-project           # repair a project's config and rewrite its files
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

### Backward compatibility

State files (`.opencode/.ocgen-state.json`) written by older versions of ocgen are
migrated automatically on load: an old single-provider project is converted to the
providers list, and agent fields added in later versions are backfilled from the
agent's original role/archetype. So older projects keep working with `landscape`,
`add`, `edit`, and `doctor` without manual edits.

## Claude Code target

ocgen also scaffolds [Claude Code](https://claude.com/claude-code) projects. Because
Claude Code has no per-agent providers or base URLs, an agent's model is a Claude
**alias** (`opus`/`sonnet`/`haiku`/`inherit`) rather than a `provider/model` id — so the
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
.claude/agents/<name>.md       # each subagent: name, description, tools, model alias, color + body
.claude/commands/multi.md      # fan the whole team out on one task
.claude/commands/intake.md     # a structured requirements interview (defaulted questions)
.claude/commands/refine.md     # present a proposal, take reasoned push-back, iterate before approval
.claude/commands/deliver.md    # end-to-end pipeline: route → sharpen → requirements → plan → research → gated execution
.claude/commands/inquire.md    # understand a codebase: sharpened questions, file:line evidence, next-question nudges
.claude/skills/<name>/SKILL.md # reusable skills (optional)
.claude/settings.json          # model, a permissions allow-list, hooks, output style, statusline
.claude/output-styles/*.md
CLAUDE.md                      # project instructions + the roster; the coordinator lives here
```

The **coordinator** (an agent whose mode is `primary`) is not written as an agent file —
it becomes `CLAUDE.md` plus the `/multi` command, because Claude Code's main session *is*
the coordinator. Every other agent becomes a subagent file. The wizard opens `CLAUDE.md`
in your `$EDITOR` so you can shape the project instructions before anything is written.

**Plugin output** (`--output plugin` or `both`) additionally writes, under
`plugin/<name>/`, a complete Claude Code plugin: `.claude-plugin/plugin.json` +
`marketplace.json`, the `agents/`, `commands/`, `skills/` and `output-styles/` trees, a
`hooks/hooks.json`, a `README.md`, and a `.github/workflows/release.yml` that zips the
plugin and attaches it to a GitHub Release on a version tag. Users install it with:

```bash
claude plugin marketplace add <owner>/<repo>
claude plugin install <name>@<name>
```

**Power-user defaults** (toggled in the wizard) put a permissions allow-list, a
`SessionStart` hook that nudges `/intake`, an output style and a statusline into
`settings.json`.

**Skills** can be authored on their own:

```bash
ocgen add skill ./my-team        # preset → tools picker → frontmatter → body ($EDITOR)
ocgen edit skill [name] -p ./my-team
```

`add skill` starts from a **preset** (`command` — a user-run task with `git` tools and a
numbered-step body; `knowledge` — background reference; `forked-research` — runs in a
forked `Explore` subagent), or blank. It then **multi-selects the built-in tools**
(`Read`/`Grep`/`Write`/`Bash`/`Edit`/`Glob`/`WebFetch`/`WebSearch`/`TodoWrite`/`Task`/
`NotebookEdit`/`Skill`) and lets you add `Bash(cmd *)` / `mcp__server__tool` patterns —
warning (not rejecting) on unrecognized names. The full `SKILL.md` frontmatter is exposed:
`description`, `when_to_use`, `argument-hint`, `allowed-tools`, `disable-model-invocation`,
`user-invocable`, `context: fork` + `agent`, and `model`. Presets are editable templates
(`claude/skill-presets.toml`).

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
4. **Nudge** — 3 suggested next questions, each tagged with a lens (structure, flow, contract,
   rationale, change impact, failure) and why it's worth asking now. Reply with a number to
   take one.
5. **Checkpoint** — every ~5 questions (or say "checkpoint") it summarizes your mental model,
   lists the biggest unknowns, and can ask you to explain a piece back to catch gaps.

Say "stop" to finish; it finalizes the ledger and lists the open questions. The ledger lives in
`.claude/notes/<topic>.md` and is **git-ignored** (the command adds `.claude/notes/` to
`.gitignore` if it's missing), so it's personal to this checkout. Once you know what to change,
run `/deliver <change>` — the ledger becomes its starting context.

**Resuming later.** The ledger is saved after every answer, so you can simply close the session.
To come back hours or days later, start a new session and run `/inquire` with **no arguments**: it
lists your ledgers, newest first, and you pick one. `/inquire <topic>` resumes that topic directly.
You get a one-screen **refresher** before continuing:

1. where you left off: the last question and the suggested next ones;
2. your mental model so far, in up to 7 bullets;
3. what changed in the code since then. Answers whose cited files have new commits are marked
   **Stale — re-verify**, and it offers to re-check those first;
4. an optional 2-question recall warm-up.

It then asks whether to continue with a suggested question, re-verify stale answers, or ask
something new. For a short break in the same session, `claude --continue` (or `claude --resume`
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
.claude/commands/team.md       # /team <task> — spawn a parallel team from your agent roles
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
| `ocgen edit skill [name] -p <dir>` | Edit an existing skill; renaming cleans up the old skill directory. Omit `[name]` to pick from a list. |

**Review, repair, reference**

| Command | What it does |
|---|---|
| `ocgen landscape [dir]` (alias `horizon`) | Read-only overview: agents (alias/tools/colour), skills, workflow/output/team setup, delegation topology, and a **Checks** section. |
| `ocgen doctor [dir]` | Repair the project and rewrite files: fix invalid model aliases, non-Claude colours, empty roles, a bad `teammateMode`; also upgrades an older state file. |
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
claude/commands/*.md.j2         # multi / intake / refine / deliver / inquire / team commands
claude/skill/SKILL.md.j2        # one generic skill
claude/skill-presets.toml       # add-skill presets (command / knowledge / forked-research)
claude/hooks/team-*.sh          # Agent Teams quality-gate hook stubs
claude/output-styles/concise.md.j2
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

The scaffolding logic lives in the library (`src/render.rs`); the wizard
(`src/wizard.rs`) is a thin [dialoguer](https://docs.rs/dialoguer) layer on top,
which is why the tests drive the library directly.

### Coverage

```bash
cargo cov --summary-only   # coverage of the testable code (~94% lines)
```

`cargo cov` (see `.cargo/config.toml`) excludes `src/wizard.rs` — the interactive
dialoguer prompt layer needs a real terminal and can't be unit-tested. All of its
non-interactive logic is extracted into the library: input validation lives in
`src/validate.rs` (100% covered) and the consistency checks in `Project::issues`.
Everything else — rendering, migration, `doctor`, the `landscape`/`fields`/`doctor`
CLI output, and template management — is covered by unit tests plus `assert_cmd`
end-to-end tests in `tests/cli.rs`. Run `cargo cov-all` to include the wizard.
