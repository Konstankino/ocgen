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
to host in a bucket, after which `scoop install ocgen` / `scoop update ocgen` work (its
`autoupdate` takes each release's hash from `SHA256SUMS`).

**macOS / Linux** (x86_64 and arm64) — installs to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Konstankino/ocgen/main/install.sh | sh
```

On Apple silicon it installs the native arm64 build, even from a shell running under Rosetta.
Or download a prebuilt archive for your platform from the releases page and put the
binary on your PATH.

**Checksums.** Both installers check the downloaded archive against the release's
`SHA256SUMS` and refuse a mismatch (`install.sh` needs `sha256sum` or `shasum`). Releases up
to v0.4.6 predate `SHA256SUMS` and install with a warning. To skip the check — say, on a
machine with no hash tool — set `OCGEN_INSTALL_SKIP_VERIFY=1` (either installer) or pass
`-SkipVerify` to the PowerShell one.

**From source** (any OS with [Rust](https://rustup.rs)):

```bash
cargo install --git https://github.com/Konstankino/ocgen ocgen
```

### Updating

New versions ship as GitHub Releases: pushing a version tag (`git tag v0.2.0 && git push
--tags`) triggers [`.github/workflows/release.yml`](.github/workflows/release.yml), which
builds binaries for Windows, macOS (arm64, x86_64) and Linux (x86_64, arm64) and uploads them
with a `SHA256SUMS` file. A release is all or nothing: if one target fails to build, nothing is
published, and a manual run that isn't on a tag builds without publishing. The binaries report
the tag's version (`ocgen --version` → `ocgen 0.2.0`), so a release needs no Cargo.toml edit; a
tag that isn't `vX.Y.Z` fails the release. Local builds report Cargo.toml's version. To update:

- **Windows:** re-run the PowerShell one-liner, or `scoop update ocgen`.
- **macOS / Linux:** re-run the `install.sh` one-liner.
- **From source:** `cargo install --git https://github.com/Konstankino/ocgen ocgen --force`.

Every installer is idempotent — running it again just replaces the binary with the newest
release. The binary is swapped in by a rename, so updating works while ocgen is running (the
notes viewer, a hook); on Windows the running `ocgen.exe` is moved aside to `ocgen.exe.old`,
which a later run deletes.

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

1. **Project basics** — project name, instruction language and answer language (see
   [Languages](#languages)).
2. **Providers** — define one or more providers (key, display name, npm adapter,
   base URL, and its models). Start from the seeded provider or add your own. Pass
   `ocgen new --base-url <url>` to set the seeded provider's base URL from the command
   line (so the server address need not be baked into the template); it becomes the
   editable default at the base-URL prompt.
3. **Utility model** — which provider + model the built-in compaction/title/summary
   agents use.
4. **Agents** — start from the default 6-agent pipeline (coordinator, explorer,
   implementer, scope-guard, reviewer, adversary), then add/rename as many as you like. For each agent you configure **every field OpenCode exposes**:
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
--dry-run` shows later how it differs from the generated version. If the directory already
holds an ocgen project, `ocgen new` first asks **Replace its configuration?** (default *No*,
which writes nothing and points you to `ocgen edit`, `add` or `doctor`); without a terminal it
refuses. Re-running with identical answers changes nothing and doesn't ask.

Input is validated as you type — the wizard re-prompts on bad values rather than
writing them: identifiers (agent name, provider key, model id) must be safe for
filenames and `@mentions` and unique, ignoring case (names that differ only in case collide on
macOS and Windows); temperature must be 0–2 and top_p 0–1; steps
a positive integer; color a theme name or `#hex`; and a provider base URL must be a
real `http(s)://` URL.

Multi-line fields (permissions, prompt body) use a hybrid flow: the seeded default
is shown, and your `$EDITOR` opens pre-filled only if you choose to edit.

**Multiple providers, per agent:** each agent picks which provider hosts its model,
so its model reference becomes `<provider>/<model>`; all providers are written into
`opencode.json`.

**Dynamic roles:** roles are not limited to the built-in presets. Pick `blank (custom
role)`, name the role, and fill in the fields yourself — no archetype file involved.

Cross-references stay consistent automatically: the coordinator's task permissions,
its prompt file, and the `multi` command are all generated from the agent names you
chose.

### Languages

A project has two languages:
- the **instruction language** (`language`) for the agents' prompts and descriptions, and for
  the command and skill texts;
- the **answer language** (`response_language`) for what you read.

New projects default to **English instructions with Ukrainian answers**. Models follow English
instructions most precisely, and English costs the fewest tokens, which matters most for small
local models. Subagents report to the coordinator, so their English costs you nothing.

When the two languages differ:
- The coordinator's prompt ends with an answer line instead of its own: "Answer the user in
  Ukrainian. Never answer in Russian." It also tells the coordinator to brief subagents in the
  instruction language and to keep protocol lines (`Verdict:`, `Confidence: NN%`,
  `Status: APPROVED`, `Owner:`) exactly as they are, since the hooks parse them.
- The line goes in the Claude team rule or the OpenCode prompt file, ahead of the adversary loop.
  It is written in the instruction language.
- A Claude project's `settings.json` also gets Claude Code's own `language` setting
  (`"ukrainian"`), which also sets the language of session titles and voice dictation.
- Projects whose languages match, including every project made before this setting, render
  exactly as before.

**Caveats:**
- The `language` setting lives in the committed project settings, so it overrides each
  teammate's personal choice.
- Claude Code doesn't document whether it reaches subagents. If it does, their reports come back
  in the answer language. The worker gate still asks a subagent that drops `Confidence: NN%` to
  restate it.
- If a coordinator prompt you edited still says "Respond in English." before its last line,
  `ocgen landscape` warns that it contradicts the answer line.

**Switch an existing project** with `ocgen edit language` (interactive), or with flags:
- `--answers Ukrainian` only sets the answer language. The agents keep their text, and the
  coordinator gets the answer line.
- `--prompts English` sets the instruction language. Each preset agent whose text is still what
  its preset gave it is re-seeded in that language. Agents you edited, custom agents and skills
  keep their text, and the command lists them.
- A project made before this setting answers in its old instruction language. Changing only the
  instructions keeps that answer language, so the answers don't flip.

**Local documents are kept in both languages, each with its own page; Markdown that others read
stays English.** The documents ocgen's skills keep on your machine (git-ignored) exist in the answer
language and, when the instruction language differs, in that language too.
- **Naming:** the unsuffixed file is the answer language's (`request-flow.md`); the other is
  `<name>.<language>.md` (`request-flow.english.md`). Each version gets its own page, in its own
  language's words, with a language switch to the other.
- **`/inquire`:** the ledger page shows its own words in the page's language: labels, counts, lens
  names and evidence tags. Claude's background helper writes each update to both versions. The
  ledger's structural keys (`Topic:`, `## Mental model`, `Q:`, `Verified`, `Source:`) stay English in
  both, because ocgen parses them. The `note` list shows each topic once.
- **`/recap`:** the report is saved in both languages, `.claude/notes/recap/<date>.md` and
  `<date>.<language>.md`. Branch names, paths, SHAs and commands stay as they are.
- **`/intent`:** the intent file and the GitHub issue draft are always English: they are the shared
  record and GitHub's. When either language isn't English, `/intent` also writes a translated
  **reading copy** of the intent file to `.claude/intent/view/<name, lowercased>.md`, which is
  git-ignored. The live viewer serves it beside the intent file, with a switch between the two, and
  `adr` (or `ocgen adr`) opens the copy in the answer language when there is one. The issue draft
  stays English only.
- **`/review-intent`:** the same as `/intent`: the issue draft and the ADR are English, and the ADR
  gets a reading copy.
- **Kept in step:** after a write, the notes hook tells Claude which version is missing or behind.
  A turn — or a subagent, such as the ledger helper — that ends with one still owed is sent back
  once. A fix that concerns one language only can stay in that one; the other version's page then
  says it may be out of date. The hook keeps its record in `.versions/` inside each git-ignored
  folder.
- **Changing languages:** `ocgen edit language` renames the versions so the unsuffixed file stays in
  the answer language, and lists what it renamed. A version in a language the project no longer
  keeps stays on disk, unlisted and never asked for.
- **Other languages:** the pages have built-in words for English and Ukrainian. Any other language
  shows English labels.

An override `manifest.toml` from before this setting asks no answer-language question. Its
projects answer in the instruction language until you add the `response_language` variable to
the override.

### The agents' mindset: accidents, not insiders

Every project ocgen sets up protects against accidents, not against insiders. An accident is harm
done in good faith: a user or agent takes an ordinary action and it does more than they meant (a
plain command, a default, a typo, a wrong path, a platform difference, a check that silently lets
everything through). An insider is anyone with legitimate access (you, the maintainers, the other
agents), and insiders are not treated as adversaries: someone who deliberately misuses access
they already have is out of scope. The sandbox, permissions, hooks, gates and withheld
credentials are guard-rails against accidents, not a wall against insiders.

ocgen gives every agent this mindset, in the instruction language: each subagent's file ends with
it (OpenCode agents and prompt files too), and so does the coordinator's text, before its answer
line. A Claude project without a coordinator agent gives it to the main session whenever that
session runs the scope or adversary loop. In short:
- **Agents never get around a safeguard.** One that is blocked stops and says what it needs.
- **One test for every safety question:** could someone acting in good faith cause this through
  ordinary use? Then it is an accident and in scope.
- **Accidents get** a safe default, blocking when a check fails, a confirmation before anything
  destructive, a way back (backup, dry run, undo), a message that says what happened, and a test.
- **Insider scenarios** are not fixed, hardened or tested, never rated above Low, never sent back
  for rework, and noted once in a final `Insider (out of scope):` section of the report.

The mindset overrides any threat model in a role's own text. A project made before it existed
gets it with `ocgen doctor`, which also gives its adversary the current preset's text unless you
edited that agent (see [`doctor`](#other-commands) below). To change the wording, edit `mindset.md.j2` (see
[Customising the templates](#customising-the-templates)).

### Commit messages and AI credit

Every project ocgen sets up tells its agents two things about what they write:
- **Short commit messages.** A subject line of at most about 72 characters that says what
  changed, in the repository's own convention when it has one (`type(scope): …`). A body only
  when the reason isn't obvious from the subject: a few lines, never a file-by-file changelog.
- **No AI credit.** No `Co-Authored-By:` trailer naming an AI, no "Generated with …" line, no
  🤖, no "written with Claude", in commits, PR descriptions, issue drafts, ADRs, reports, code
  comments, changelogs or docs. Naming Claude where it is the subject (`CLAUDE.md`, agent files,
  docs about configuring Claude Code) is fine, and so are ocgen's own "Generated by ocgen" and
  "Rendered by ocgen" marks.

Where it lives:
- **Claude Code:** a "Commits and authorship" section of `.claude/rules/ocgen-workflow.md`, in
  English like the rest of that rule. Every session loads it, and so do subagents and Agent Teams
  teammates; only the read-only built-in Explore and Plan skip project rules. `settings.json` also
  sets `"attribution": {"commit": "", "pr": "", "sessionUrl": false}`, so Claude Code adds no
  trailer, PR line or session link of its own. That setting covers only what Claude Code appends;
  the rule covers what the agents write themselves. ocgen doesn't write `"attribution": false`:
  a Claude Code older than v2.1.281 rejects that value and skips the whole settings file, hooks
  and permissions included.
- **OpenCode:** it has no attribution setting, and its own prompts ask for no AI credit. Every
  agent's text and prompt file carries the same section instead, in the instruction language,
  before the mindset.

A project made before this gets it with `ocgen doctor`, which changes only those files. A
`.claude/settings.local.json` or a managed policy that sets `attribution` wins over the project's.
To change the wording, edit `authorship.md.j2` (see
[Customising the templates](#customising-the-templates)). An override of
`claude/rules/ocgen-workflow.md.j2` from before this has no `{{ authorship }}` line, so its
projects get no section until you add one.

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
ocgen edit language -p ./dir --prompts English --answers Ukrainian   # instruction / answer language
ocgen add provider ./my-project     # add another provider to an existing project
ocgen edit provider [key] -p ./dir  # edit a provider and its models
ocgen show agent [name] -p ./dir    # print one agent's full configuration
ocgen landscape ./my-project        # overview of all agents + providers (alias: horizon)
ocgen doctor ./my-project           # repair a project's config and rewrite its files (shows a diff, asks first)
ocgen doctor --dry-run ./my-project # show exactly what doctor would change; write nothing
ocgen doctor --force ./my-project   # also give agents you edited their current preset's text
ocgen verify ./my-project           # check the project actually works (exit 1 on failure; CI-friendly)
ocgen managed-settings              # print an organisation policy (managed-settings.json)
ocgen approve ./my-project          # you unlock pushes/deploys for 30 min (refuses to run under Claude Code)
ocgen fields                        # explain every configurable field (alias: reference)
ocgen templates init                # copy the editable templates to ~/.config/ocgen/templates (--force overwrites your copies)
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
a `.toml` template are checked for valid syntax, with a warning if they don't parse. Hook
scripts and the gate-protocol templates can't be overridden (see
[Customising the templates](#customising-the-templates)).

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
then regenerates everything.

**Preset agents nobody edited get the current preset's text.** A project keeps each agent's
description, instructions and prompt in its state file, so a regeneration alone renders what the
preset said when the agent was made. ocgen knows every text its presets have shipped with (as
fingerprints, in `src/render/preset_history.txt`). When all of an agent's text is one its role's
preset once had, nobody edited it, so `doctor` replaces it with the current preset's, in the
instruction language, and reports `agent '<name>': text from an older '<role>' preset → the current
one`. Its other fields (model, tools, turns, effort…) are kept. An agent whose text you changed,
and a custom role, keep their text: compare with `ocgen show agent
<name>` and edit it with `ocgen edit agent <name>`.

**`doctor --force` overwrites edited agent text too.** Every preset agent gets its current
preset's text, in the instruction language, even one you edited (`agent '<name>': edited text →
the current '<role>' preset (--force)`). The current preset is the one new projects get, so an
agent made from your own preset in `~/.config/ocgen/templates/archetypes/` gets that preset's
current text. Its other settings are still kept. A custom role, or a preset ocgen no longer ships,
has nothing to restore: `doctor` names it and keeps its text. Run `ocgen doctor --dry-run --force`
first to see which agents it would overwrite. The overwritten agent files are backed up to
`.ocgen-backup/` like any other, so an edit you lose is still there. Running it on a project made by an **older version of
ocgen** also upgrades it — the state file and generated files are rewritten in the
current format.

**`doctor` shows its plan before it writes.** Every file it would add (`+`), change (`~`) or
delete as stale (`-`) is listed with a short diff.
- **Your hand edits are flagged by name.** ocgen fingerprints every file it writes, so it can
  tell your edits apart from its own updates. For a hand-edited `settings.json` it points you
  to `.claude/settings.local.json`, which ocgen never touches. For permission rules the whole
  project should share, use `ocgen edit permissions` instead.
- **What you added by hand is kept.** Permission rules in `settings.json` and MCP servers in
  `.mcp.json` that ocgen didn't generate (say, from `claude mcp add --scope project`) are listed,
  and one prompt, **Keep them as yours?**, adopts them into the state (the same store as
  `ocgen edit permissions` / `ocgen add mcp`), so every later regeneration keeps them. Fields
  ocgen doesn't model, such as a server's `timeout`, are kept too. `--yes` and non-interactive
  runs keep them without asking. A rule ocgen generated last time, or one you removed, is never
  re-adopted.
- **You choose per file.** When files you changed would be overwritten, pick **Apply all**,
  **Keep all changed files** (only add new ones), **Decide file by file** (Overwrite / Keep mine /
  Show full diff) or **Cancel**. Files ocgen no longer generates are part of the review: Remove /
  Keep mine / Show it, and a kept one becomes yours. A kept file stays as it is; the next `doctor`
  or `verify` reports it again. `--yes` applies everything, and non-interactive runs such as CI
  apply directly.
- **Backups:** before overwriting or deleting anything it copies the previous versions to
  `.ocgen-backup/<UTC timestamp>/`, which git-ignores itself and keeps the newest 5. Copy a
  file back to restore it.
- **Preview:** `--dry-run` shows the plan and writes nothing.

**`add` and `edit` regenerate just as safely**, without the review:
- hand-added permission rules and MCP servers are adopted as yours (a server you removed with
  ocgen stays removed);
- files ocgen no longer generates are removed — an agent turned primary or renamed, a renamed
  skill, the Agent Teams or `/intent` files once you turn them off, `.mcp.json` once its last
  server goes;
- a file edited by hand (or of unknown origin) is backed up to `.ocgen-backup/` before it is
  overwritten or removed, and a short notice lists what was kept, removed and backed up.

**Guard rails for every write:**
- **Version:** the state records the ocgen that wrote it. An older ocgen refuses to change
  (`add`, `edit`, `doctor`) a project a newer one wrote, and names the version you need;
  `doctor --dry-run` only warns, and read-only commands still work. Keys it doesn't know are kept.
- **Names:** agent, skill, MCP server and provider names in the state must be plain identifiers,
  or loading fails and says which.
- **Links:** ocgen never writes through a symbolic-link file. A linked folder you made yourself is
  yours, and ocgen writes through it; one a repository tracks is refused if it leads out of that
  repository.
- **Line endings:** Claude projects get an ocgen-marked block in `.gitattributes` pinning LF for
  `.claude/**`, `.mcp.json` and `.worktreeinclude` (appended once to an existing file; your lines
  are never touched), so a Windows checkout can't break the `sh` hooks. A CRLF checkout doesn't
  count as a hand edit.
- **Odd files:** a file that isn't UTF-8 (a UTF-16 `.mcp.json` from PowerShell's `>`) counts as
  changed and is backed up byte for byte, never silently replaced; one ocgen can't read at all
  stops the write.

**`ocgen verify [dir]`** checks that a generated project *works*, not just that it exists:
- files are up to date (the same plan as `doctor`), and there are no consistency issues;
- `settings.json` is valid JSON with known keys;
- **effective settings:** it merges `~/.claude/settings.json`, the project's `settings.json` and
  `settings.local.json` and any managed policy the way Claude Code does, and fails, naming the
  file, when one turns the hooks off (`disableAllHooks`, a managed `allowManagedHooksOnly`) or
  weakens a gate: a switch that isn't `1`, a lower confidence bar or looser loop budget, an
  emptied check command, an added read-only role or trusted docs site, a shortened sandbox list.
  Other changes to ocgen's settings warn;
- it runs scripts and hook commands with the shell Claude Code uses: Git Bash on Windows (found
  even when only `Git\cmd` is on PATH), `sh` elsewhere. If no shell starts at all, it says so once
  and skips the checks that need one;
- every hook script exists and parses, and the side-effect-free hook commands ocgen generates
  actually run;
- every hook pins `"shell": "bash"`, and on Windows Git Bash is installed. The hook commands
  are POSIX sh; without Git Bash Claude Code runs them in PowerShell, where they fail to
  parse and **the gates let everything through** — install Git for Windows (or set
  `CLAUDE_CODE_GIT_BASH_PATH`);
- the approval gate blocks `git push`, deploys and self-approval, and allows `git status`;
- every gate (approval, WebFetch, plan, task, worker, risk) is the one ocgen generates: a
  hand-edited script or `settings.json` hook fails. So does one an older ocgen wrote — after
  upgrading, run `ocgen doctor` before `verify` in CI;
- the WebFetch guard (every Claude project) allows only `https://` fetches to the trusted docs
  sites and blocks `http://`, other hosts and look-alikes;
- the no-op `cd` hook drops `cd <project> &&` and leaves a `cd` into another folder alone;
- **git credentials:** with credentials withheld, warns while git can ask an OS keychain
  (osxkeychain, Git Credential Manager, wincred, libsecret, `gh auth git-credential`) — it reads
  the helpers git uses in the repository and never runs one. A settings file that takes back
  the empty-helper reset fails, like any other weakened setting;
- **sandbox:** warns when the approval gate is on and the sandbox off (the gate is then advisory),
  on native Windows (no sandbox there), and on Linux when `bwrap` is missing;
- **git pre-push hook:** it runs the installed hook as git would — it must refuse a push made under
  Claude Code and let yours through, and it fails if it isn't executable;
- **line endings:** a generated script with CRLF endings fails, with the fix;
- **CODEOWNERS:** the file `/intent`'s approvers are linked to is still there under its exact
  name, no CODEOWNERS that GitHub reads first hides it, it's under GitHub's 3 MB limit, the intent
  folder matches its case on disk, no rule of yours after ocgen's block takes precedence over it,
  and something owns the CODEOWNERS file itself (with a reminder that GitHub enforces it only with
  "Require review from Code Owners" and only for owners with write access);
- the statusline renders;
- a compatible `ocgen` is on your `PATH`; a stale binary is flagged, and so are ignored template
  overrides;
- `.claude/` is tracked by git;
- `claude plugin validate` accepts the agents, skills and plugin (skip with `--no-claude`).

**What verify runs.** Only what ocgen generates: hook commands, hook scripts and the statusline
are re-rendered from the state, and one that differs on disk is reported (`… differs from what
ocgen generates — not running a hand-edited command`), never executed. Probes get only ocgen's
gate settings (`OCGEN_*`, `TEAM_*`, `LOOP_GUARD_*`, `SUBAGENT_*`), never `PATH`, `BASH_ENV` or the
check and formatter commands. It still runs `git` in the repository, the installed
`.git/hooks/pre-push` when it is ocgen's, and `claude plugin validate` on the repository's
agents, skills and plugin. `--run-check` also runs the project's committed check command, as
you and unsandboxed — use it only on a repository you trust.

Hooks run with the loop guard off and record nothing, so verification leaves no state behind,
and hooks with side effects (notifications, the formatter, the audit log) aren't executed. It
exits 1 on any failure, so it can gate CI.

### Backward compatibility

State files (`.opencode/.ocgen-state.json`) written by older versions of ocgen are
migrated automatically on load: an old single-provider project is converted to the
providers list, and agent fields added in later versions are backfilled from the
agent's original role/archetype. So older projects keep working with `landscape`,
`add`, `edit`, and `doctor` without manual edits. The reverse doesn't hold: an older ocgen
refuses to change a project a newer one wrote, so it can't drop settings it doesn't know.

## Claude Code target

> **Scope.** The two targets share the wizard, agents, templates and the `landscape`/`doctor`
> tooling. Everything built on Claude Code's own mechanisms is **Claude-only**, and the OpenCode
> target keeps its current feature set:
> - governance gates, the loop guard and the approval gate (hooks);
> - skills and the workflow skills (`/deliver`, `/inquire`, `/intent`, `/review-intent`, `/recap`, …);
> - MCP, the statusline, subagent controls, and plugin output.
>
> OpenCode has no hook system, so the gates can't be ported.

ocgen also scaffolds [Claude Code](https://claude.com/claude-code) projects. Because
Claude Code has no per-agent providers or base URLs, an agent's model is a Claude
**alias** (`opus`/`sonnet`/`haiku`/`fable`/`inherit`) or a full model ID such as `claude-opus-5-5`
(an alias follows the newest model, an ID pins one) rather than a `provider/model` id — so the
provider and utility-model questions are replaced by an alias plus a **tools** allow-list
per agent.

The default team gives each role a model that fits its job:

| Role | Model | Effort | Why |
|---|---|---|---|
| coordinator (the session's `model` in `settings.json`) | `opus` | session's | Planning errors cascade |
| explorer | `sonnet` | `medium` | Reads the most and decides the least; others check its findings |
| implementer | `opus` | inherit | Writes the code |
| scope-guard | `sonnet` | `high` | Traces plan items to hunks: careful reading, and another model than the implementer's |
| reviewer | `fable` | `high` | Another model than the implementer's, so it doesn't share its blind spots |
| adversary | `opus` | `high` | Covers what the reviewer's model misses |
| verifier (preset, not in the default team) | `sonnet` | inherit | Runs checks and reports pass/fail |

These only seed new agents: a project keeps the models it recorded, and `ocgen doctor` doesn't
change them. Change one with `ocgen edit agent <name>`.

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
.claude/skills/inquire/SKILL.md      # /inquire — understand a codebase with evidence and one hint per answer; resumable ledger with a live HTML view
.claude/skills/intent/SKILL.md       # /intent — prompt → investigate → approved plan → numbered intent file (optional) → issue draft (you run it)
.claude/skills/review-intent/SKILL.md # /review-intent — a change reviewed by your agents in parallel, each finding checked by another, recorded as an issue draft; an ADR on request (you run it)
.claude/skills/recap/SKILL.md        # /recap — fast-forward your branches, then a per-branch report of what changed since the last recap, plus new GitHub comments and reviews (you run it)
.claude/intent/issue-template.md     # /intent's GitHub issue structure — yours, written once
.claude/intent/drafts/<name>.md      # /intent's issue draft — type `draft` to edit, preview and copy it in your browser (git-ignored)
.claude/intent/viewer/               # the intent files' viewer and session records — type `adr` to read them (git-ignored)
.claude/intent/view/<name>.md        # an intent file's reading copy in your language, with its page (git-ignored)
.claude/intent/intent-template.md    # /intent's intent-file (ADR) structure — yours, written once
.claude/skills/fanout/SKILL.md       # /fanout — worktree-isolated parallel writers (you run it)
.claude/skills/improve-prompt/SKILL.md
.claude/skills/<name>/SKILL.md       # your own skills (`ocgen add skill`)
.claude/settings.json                # $schema, model, attribution off, permissions (+ secret read-denies), sandbox, hooks, output style, statusline
.claude/hooks/*.sh                   # gate scripts, loop guard, optional notify/format/audit hooks
.claude/output-styles/ocgen-concise.md
.mcp.json                            # project MCP servers (`ocgen add mcp`)
.claude/rules/*.md                   # the workflow (and team) rules every session loads
.claude/statusline.sh                # the status line (optional)
.claude/.ocgen-state.json            # ocgen's record of the project: `apply`, `doctor` and the hooks read it
.gitattributes                       # an ocgen block pinning LF line endings for the files above
.worktreeinclude                     # what Claude Code copies into each new worktree: `.env` + the config when it isn't committed
.git/info/exclude                    # an ocgen block keeping the config out of git in this clone, while nothing under .claude/ is committed
CLAUDE.md                      # project instructions + the roster; the coordinator lives here
```

The **coordinator** (an agent whose mode is `primary`) is not written as an agent file —
it becomes `CLAUDE.md` plus the `/multi` command, because Claude Code's main session *is*
the coordinator. Every other agent becomes a subagent file. The wizard opens `CLAUDE.md`
in your `$EDITOR` so you can shape the project instructions before anything is written. The
coordinator's body is rendered as a template (it may loop over `{{ subagents }}`); text that
isn't a valid template, or names a value ocgen doesn't know (`${{ secrets.X }}`,
`{{ .Values }}`), is written exactly as typed, with a warning.

**The scope check.** The default team has a `scope-guard` after the implementer. It checks the
finished change against the plan in both directions, and states its blast radius:
- **Nothing planned missing:** every plan item and acceptance criterion gets a line: Done (with
  the hunks that implement it), Partial (what is missing), Missing, or Differs (built another way
  than planned; for you to accept, never reworked).
- **Nothing unplanned added:** every hunk belongs to a plan item, is **Also needed** (reverting
  it alone would break the build, a test or a criterion), is a **Cut** (a drive-by refactor,
  rename, hand reformatting, speculative abstraction, unrequested feature, dependency bump or
  debug leftover), or is **Deferred** with a reason. It never cuts an added check, error handling,
  test or doc fix — smaller must not mean less safe — and never what the approved plan names.
- **Blast radius:** files and lines, public surface, dependencies, stable files (none of the last
  100 commits touched them), what could break, and how to roll back.

It checks that planned behavior is *present*; whether it is *correct* stays with the reviewer and
the adversary. It has `Read, Grep, Glob, Bash` with the file tools denied, and runs only
`git diff`, `git log` and `git status`. Its report starts with `Verdict: CLEAN`, `TRIM` (cuts) or
`INCOMPLETE` (anything Partial or Missing).

Whenever the team has an enabled subagent whose role (or name) is `scope-guard`, ocgen adds its
loop to the coordinator's instructions, ahead of the adversary loop:
- Note the base (`git status --porcelain=v2 --branch`) before the first change; files already
  modified then are yours and aren't judged. Tell whoever makes a change to change only what
  the task needs.
- When every change is done, and before any review, call the scope guard with the goal, the
  plan and its criteria verbatim, and the decisions you confirmed.
- **Cuts:** the coordinator reverts them itself, exactly, and writes nothing new — no subagent
  round trip, no extra merge. Restoring a whole file (`git checkout <base> -- <path>`) asks your
  permission. A revert that breaks a test is undone and reported as kept.
- **Gaps:** the implementer gets the Partial and Missing items for one completion round, then
  the scope guard checks once more. Anything still missing is reported as not done.
- The final report gives the plan coverage, the blast radius, every cut, every Differs and the
  Defer list.

The commands use it too. In **`/deliver`**, the plan numbers its success criteria and gets a
**Scope** line (what changes, what is out of scope, which refactors the goal needs), and the scope
check closes phase 5, before the adversary check. In **`/intent`**, the scope guard checks the
plan before you see it: a criterion or risk with no task is a gap, a task that traces to nothing
is shown as **Deferred by the scope check**, and the predicted blast radius goes into the intent
file's Consequences. Without a scope guard, `/intent` reads exactly as before, and `/deliver` runs
its own **final check** in its place: when the plan's tasks are done, it checks the change against
every success criterion and runs the tests (after merging a worktree implementer's branch), goes back
to the task for each miss, at most 2 rounds, and reports what is still unmet as not done.

**The adversary loop.** The default team ends with `adversary`, a skeptic that hunts accidents:
the ways ordinary, good-faith use of the change does harm. It checks what the explorer found, the implementer built and the reviewer approved,
and accepts no claim ("tests pass", "Confidence: 97%") without evidence. It has
`Read, Grep, Glob, Bash`, so it can run the tests, the built binary and PoC inputs, with
`Edit, Write, NotebookEdit` denied. Each finding carries `file:line`, a scenario, the impact and a
fix, marked Verified or Inferred. Its report starts with `Verdict: PASS` or `Verdict: REWORK`;
any Critical or High finding means REWORK.

A subagent can't call another, so the coordinator runs the loop. Whenever the team has an
enabled subagent whose role (or name) is `adversary`, ocgen adds the loop to the coordinator's
instructions (`.claude/rules/ocgen-team.md`, or the OpenCode prompt file):
- Call the adversary last, before the final report. If the implementer works in a worktree,
  merge its branch first.
- On REWORK, send the implementer exactly the Critical and High findings, then call the
  adversary again to check the fixes.
- After 2 rework rounds (at most 3 checks), report what is left as **UNRESOLVED**.

The two commands that produce work use it too, naming the adversary by its own name:
- **`/deliver`:** after phase 5 (Execute) and before Synthesize, the adversary checks the whole
  change. The implementer's worktree branch is merged first, and the same REWORK rounds apply.
  Unresolved findings are reported in the synthesis, never as done.
- **`/intent`:** the adversary challenges the findings in Pass 4, in place of the explorer.
  It then checks the plan for accidents before you are asked to approve it, revising it for up
  to 2 rounds.
  Any Critical or High finding still open is listed as **UNRESOLVED** in the intent file's Risks
  and in the issue draft, and the tone check makes sure both have it.

Teams without an adversary render exactly as before. To add one to an existing project, run
`ocgen add agent`. The preset picker suggests the first default-team role the project lacks, so a
team without either gets `scope-guard` preselected, then `adversary` the next time; in OpenCode,
name the agent `scope-guard` or `adversary` and its preset is picked.

**Workflow commands are skills.** Claude Code merged commands into skills, so ocgen renders
its workflows as `.claude/skills/<name>/SKILL.md`. You still type `/deliver`, `/inquire` and so on.
- **You start the side-effecting ones:** `/multi`, `/fanout`, `/deliver`, `/intent`,
  `/review-intent`, `/recap`, `/team` and `/team-plan` carry `disable-model-invocation: true`, so Claude can't start them on
  its own.
- **Claude may use the rest when relevant:** `/inquire`, `/intake`, `/refine` and
  `/improve-prompt`.
- **Upgrading:** `ocgen doctor` removes the old generated `.claude/commands/*.md`. Files you
  wrote yourself are kept.
- **Reserved names:** a skill of your own can't take a workflow name. A skill made before its
  name was reserved (say, your own `recap`) keeps its file: ocgen skips the generated one and
  `ocgen landscape` warns you to rename yours.

**Extra hooks** (offered with the power-user defaults; apart from the config audit's guard, none
of them ever block Claude):
- **Notes pages** (on with `/inquire`, `/recap`, or reading copies): after each write to a ledger in
  `.claude/notes/`, a `/recap` report or a reading copy, ocgen renders its HTML page and refreshes the
  browser tab that shows it (see [The visual ledger](#the-visual-ledger)). When the project's two
  languages differ, it also tells Claude which language version is missing, and holds the end of a
  turn once while one is (see [Languages](#languages)).
- **Re-inject context after compaction** (on by default): when a conversation is compacted,
  Claude is re-pointed at `.claude/rules/` and any `/inquire` notes in `.claude/notes/`.
- **Drop a no-op `cd`** (on by default): `cd <the folder Claude is in> && …` becomes `…`
  (see **Fewer read-block prompts** below). It never allows, asks or blocks; the
  shorter command is permission-checked as usual. The sh fallback needs `jq`.
- **Desktop notifications:** when Claude needs you, or a turn fails (`osascript` on macOS,
  `notify-send` on Linux, otherwise a terminal bell).
- **Formatter after edits:** e.g. `cargo fmt` or `terraform fmt -recursive`. It runs after
  every `Edit`/`Write`, in the project (or, for a `/fanout` worker's file, the same folder in
  that worktree; never in an unrelated repository), and a failure is reported but never blocks.
  With the sandbox on it runs sandboxed, like the check command.
- **Config audit:** settings/skills changes made during a session are logged to the
  git-ignored `.claude/audit/config-changes.log`. While the approval gate or the WebFetch guard
  is on, a change that would weaken it is **blocked**: `disableAllHooks`, the gate switched off,
  a WebFetch site added, or — in ocgen's own `settings.json` — the gate's env or hooks removed or
  the file deleted. Claude Code keeps the settings it loaded; if you meant the change (an
  `ocgen edit docs --trust` mid-session, say), restart Claude Code. It reads the file's
  text, so it stops the plain ways, not a determined rewrite. Policy settings and skills are only
  logged.

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
  reviewer, verifier, adversary and scope-guard ship with `Edit, Write, NotebookEdit` denied, so
  they have no file-editing tools. The explorer and reviewer are **hard read-only**; the verifier
  and adversary keep `Bash` to run tests, and the scope-guard to read git, so their read-only
  status rests on their prompts.
- `effort`: the reviewer, adversary and scope-guard use `high`, the explorer `medium`.
- `permissionMode`.
- `memory`: `project` is committed under `.claude/agent-memory/`, `local` is git-ignored,
  `user` spans every project.
- `background`.
- preloaded `skills`.
- the **MCP servers** it may use.

**Fewer read-block prompts.** With `permissions.blockReadsOutsideWorkingDirectories` on, Claude
Code asks about every shell command it can't prove only reads inside the project: a `cd …;`
before relative paths, `find -exec`, `for` loops and `$(…)`, sed programs, PowerShell script
blocks. "Don't ask again" can't save them, so they come back every time. Three defaults keep
them rare:
- **Research goes to `explorer`.** Claude Code's built-in `Explore` agent ignores CLAUDE.md and
  writes exactly those one-liners, so `Agent(Explore)` is denied and the workflow rule sends
  research to the generated `explorer`, which has Read, Grep and Glob but no shell. Answer *no*
  to the wizard's explorer question to keep the built-in one (for example, to let research run
  `git`/`gh`); nothing is denied when the project has no `explorer`.
- **A "Shell commands" rule.** `.claude/rules/ocgen-workflow.md` tells the main session and
  every agent to read with Read, Grep and Glob, to run one plain command per Bash call (no
  `cd`, loops, `$(…)`, `find -exec`, sed programs or script blocks), and to pass that on when it
  delegates to a built-in agent.
- **A no-op `cd` is dropped.** Claude Code only ignores a `cd` spelled exactly like its working
  directory; `cd C:/…` for `C:\…`, a trailing `/` or a symlinked path still asks. A
  `PreToolUse` hook removes a leading `cd` into the current folder (by text, then by resolved
  path) and hands back the rest, which Claude Code checks as usual. A `cd` anywhere else is
  left alone.

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
  Its steps are strict (no skipping or improvising), and it ends by checking the result and going back
  on a miss, at most twice.
- `knowledge`: conventions or domain rules Claude applies automatically when relevant: principles
  with their reasons, applied with judgment.
- `forked-research`: heavy reading in an isolated `Explore` subagent; you get a summary back. It
  checks each claim against the file it cites before summarizing.
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

Three more rules cover the files beside `SKILL.md`. They're yours, so ocgen reports them (in
`landscape`, `verify`, and after `add skill` / `edit skill`) and never changes them:
- **A long reference file starts with a contents list.** Claude often reads only about the first
  100 lines of a file to decide whether to read the rest. A reference file over 100 lines without a
  `## Contents` (or `Table of contents`, `Зміст`) heading in its first 30 lines is flagged.
- **References stay one level deep.** Name every reference file in `SKILL.md` itself. A file reached
  only through another reference file may be read only in part, and a file nothing names is never
  read; both are flagged.
- **Never assume a tool is installed.** When `scripts/` holds helpers, `SKILL.md` says what they need
  and how to install it, next to the step that runs them ("needs jq: `brew install jq`"). Flagged
  when it never does.

Two more aren't checked, but the presets show them:
- **Match the strictness to the risk.** Exact steps where a slip does damage (side effects,
  deletions, migrations); plain principles with their reasons for judgment calls. One skill can mix
  both, step by step.
- **End multi-step work with a check that loops back:** compare the result with what was asked, go
  back on a miss, and stop after a few rounds with what's still wrong, so a step is done because it
  passed, not because it was attempted.

ocgen's own workflow skills follow the same rules, and a test keeps them under 500 lines.

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
Claude Code. Most skills that "don't work" never trigger. Test it on each model you'll run it with:
smaller models need explicit numbered steps, while the strongest do worse when over-prescribed. If
it only works well on one, pin it with `model`. Change it later with `ocgen edit skill`,
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
your approval; checks the result against the plan's success criteria before reporting (the scope
guard does it when the team has one, otherwise a **final check** runs the tests and loops back);
and summarizes. Small, clear goals collapse the interview and plan into a one-paragraph brief.

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

##### The visual ledger

Every ledger has an HTML page next to it, `.claude/notes/<topic>.html`, and the two always
match: **ocgen renders the page from the Markdown** — the model only ever writes the `.md`. The
page is a **report in tabs**:

- **The header:** topic, summary, commit, date, ledger path and status (e.g. "Phase 3 in progress").
- **Overview:** where you left off (last question + hint), your mental model, a **lens coverage**
  bar (a gap is a good place to look next), the map with a Mermaid diagram, open questions and the
  glossary.
- **One tab per study phase** — a run of questions on one theme, such as "Phase 1 · CI dbt-build".
  After each answer, the model lays its evidence out as cards there, choosing the form that fits.
  The page opens on the latest phase.
- **Q&A log:** one compact card per answer with its lens, a Verified / Inferred NN% / Stale badge,
  `file:line` citations and the hint.

The phase cards are written in plain Markdown plus a few fenced **visual blocks**, one row per line
with cells separated by ` | `, so the `.md` stays readable on its own:

| Block | A row | Draws |
|---|---|---|
| `stats` | `10 \| Steps, strictly serial` | big-number tiles |
| `claims` | `Inferred 70% \| ADaM reads a study schema \| _sources.yml:4-11` | findings with Verified / Inferred NN% / Corrected badges |
| `steps` | `5 #orange \| dbt seed \| 109 CSVs \| :68-73` | a numbered, colour-coded sequence |
| `bars` | `pytest test files \| 240 \| hover text` | a horizontal bar chart |
| `compare` | `Silver #orange \| genomic_snv`, then `**usubjid** \| …` | one record across stages, with arrows |
| `stack` | `Redshift #orange \| built by dbt`, then `7 · Gold marts ! \| CTAS \| table` | layered stores side by side |
| `cards` | `✓ Bronze: right #blue \| It checks…` | short verdicts side by side |

Markdown tables, `> **Hint (Failure):** …` callouts, `Source: …` footnotes and `mermaid` flowcharts
(`node:::blue` colours a node) complete the set. `### Title [half]` puts two cards side by side, and
`legend: #blue Setup · #orange Build` adds a legend. Everything follows your light or dark theme.

**It opens and refreshes by itself.** A `PostToolUse` hook (`inquire-notes`) runs after every
write to a ledger: it re-renders the page and shows it. The first update opens the page in your
default browser; later updates **refresh the tab that already shows it** instead of opening
another. Close the tab, and the next update opens a fresh one. This works the same on macOS,
Linux and Windows: ocgen runs a small viewer on `127.0.0.1` that each open tab stays connected
to, so it knows which tabs are open without scripting a particular browser. The viewer stops by
itself after 30 idle minutes with no tab open.

To keep that working, the ledger's layout is fixed by the skill (headings, `### Q<n> · <Lens> ·
<Verified|Inferred NN%>` entries with `Q:`/`A:`/`Cites:`/`Hint:` lines), and the helper writes it
with the Write or Edit tool, so the hook sees every write. Older ledgers still render, and are
converted on their next update.

| Command / variable | What it does |
|---|---|
| `ocgen notes open [topic] [-p dir]` | Render a ledger and show it: refresh its tab, or open one. Picks by file name, then by `Topic:` line; with no topic, the latest ledger. `/inquire` runs this when it resumes. |
| `ocgen notes render <file.md>…` | Render pages without showing them. Only `/inquire` ledgers (`.claude/notes/<topic-slug>.md`), `/recap` reports (`.claude/notes/recap/<date>.md`) and `/intent` reading copies (`.claude/intent/view/<name>.md`) — and their other language versions (`<name>.<language>.md`) — by any path, are rendered; any other file is refused, and every argument is checked before anything is written. |
| `OCGEN_NOTES_OPEN=0` | Never open a browser (pages are still rendered). `=1` always does. |
| `OCGEN_NOTES_BROWSER` | Open pages with this command instead of the system default (`open`, `xdg-open`, `start`). Like those, it runs detached: ocgen doesn't wait for it or check how it exits, so a plain `firefox` is fine. |

**Open them from the prompt.** Type **`note`** or **`notes`** (the word alone, as your whole
message, any case) and the ledger opens in your browser. With several, their list opens first: 10 a
page, newest first, each with its topic, status and date, and the ledger this session wrote last
already selected; ↑/↓ and Enter open one, ←/→ turn the page. With none, nothing opens and Claude
says there are no `/inquire` notes yet. An open tab is reused: sent again, the word refreshes the
ledger's tab or moves the list's. Any other prompt (`note this`) reaches Claude untouched. The same
`inquire-notes` hook catches the word (`UserPromptSubmit`) and remembers the ledger each write
makes the session's, one small file per session in `.claude/notes/.sessions/`. Projects created
before get the word with `ocgen doctor [dir]`.

**When nothing opens.** In CI (`CI` is set) and on Linux without a display, the hook only renders
the pages; `ocgen notes open` still opens one when you ask. The page and the live view need the
ocgen binary on `PATH` (the bundled script fallback does nothing), and `ocgen verify` tells you
when it is missing. If the viewer can't start (a sandbox, say), the page is opened once as a
plain file and won't refresh by itself.

**Privacy.** The viewer listens on loopback only, every URL carries a random token, and requests
for any other host are refused, so other sites can't read your notes (and only requests that
pass those checks keep its idle timer alive). The diagrams use Mermaid
from `cdn.jsdelivr.net` (a pinned version with an integrity hash); no note content is sent
anywhere, and without a network the diagram's source is shown instead. Browsers allow about six
live connections per host, so keep fewer than six ledger pages open at once.

To add these commands to an existing project, re-render it with `ocgen doctor [dir]` (projects
created by older versions get `/inquire` switched on automatically).

#### From findings to a GitHub issue (`/intent`)

**`/intent <the problem or idea>`** turns an investigation into an agreed change you can file as a
GitHub issue (wizard: "Include the /intent command…"; on by default). You start it; Claude can't.

1. **Improve the prompt** — your request is rewritten with an explicit role, task, success criteria
   and constraints; you confirm it, and everything after works from it.
2. **Investigate — exhaustively.** It runs at `effort: xhigh` and works in passes:
   - **Map:** entry points, data flow and a list of search terms (symbols, errors, config keys,
     flags).
   - **Deep dive:** several research subagents at once — the project's `explorer` when it's
     preferred (the default; it has no shell, so `/intent` runs the git and `gh` reads itself and
     hands them over), else the built-in `Explore` — covering every call site, tests and gaps,
     config/build/CI, history (`git log -S`, `git blame`, `git show`: *why* the code is this way,
     never *who* — no author names in the drafts), prior work (existing intents, open and closed
     GitHub issues and PRs) and docs.
   - **Close the gaps:** again in parallel, one research subagent per open gap, until a pass
     finds nothing new; then a **challenge** pass that hunts for evidence against the findings.

   It shows an analysis report — current behaviour, root cause, impact, constraints, related work,
   risks, open questions, a coverage checklist (entry points to platforms, security and
   performance) and what would make it wrong — with numbered findings (F1, F2…), each Verified
   (`file:line`) or Inferred (`Confidence: NN%`).
   The read-only git and `gh` commands it needs, and `WebFetch` for the project's **trusted
   documentation sites**, are pre-approved while `/intent` runs (the skill's `allowed-tools`);
   other tools still ask. If `gh` isn't installed (https://cli.github.com), isn't logged in or
   can't reach GitHub, the analysis says so and continues without it. Docs come only from those sites — see
   [Trusted documentation sites](#trusted-documentation-sites-ocgen-edit-docs).
3. **Plan and approve** — options with a recommendation, scope, risks with mitigations, acceptance
   criteria. Every statement cites its findings (F#) or is labelled an Assumption; it iterates on
   your push-back until you approve. Your approval makes the plan **approved for drafting**, not
   accepted: the **approvers** decide (below).
4. **Intent file** (optional: `/intent` asks once you approve the plan) — e.g.
   `docs/adr/ADR-0007-cache-invalidation.md`, with
   `Status: Proposed`, a `Severity:` line, an `Approvers:` line and every approver pending in its
   Sign-off section. The number is one more than the highest found in the local directory **and**
   on the remote main branch (`git fetch` + `git ls-tree`), matching both `ADR-0007-…` and `0007-…`
   names, so it never collides with an existing ADR.
5. **Issue draft** — an actionable title (≤ 72 characters) and a description of at most 250 words,
   written to `.claude/intent/drafts/<name>.md` (git-ignored). It follows the project's issue
   template **as a guide, not a form**: the sections keep their order and purpose, the wording fits
   the topic. The default template is the team convention below. You also get the
   `gh issue create --title "…" --body-file .claude/intent/drafts/<name>.md` command **for you to
   run**: it files the draft as it stands, with your edits. Claude never files it: the skill says
   so, and `Bash(gh issue create*)` is in the generated `deny` list. Approvers aren't made assignees
   (assignees do the work). Before you see either draft, a **tone check** runs (below).
6. **Sign-off, link and continue** — after you file it, `/intent #123` (or the URL) adds `Issue:`
   to the intent file. Claude never marks or infers an approval: it records one only when you
   report it with evidence (`@alice approved: <link>`) — **approved** (date and link), **changes
   requested** (their feedback, quoted and linked; back to step 3) or **rejected**
   (`Status: Rejected`). `Status: Accepted` needs every approver's approval and your confirmation.
   The intent is then agreed: Claude fills in the draft's Plan and gives you the
   `gh issue edit <number> --body-file …` command to update the issue.

   **Without an intent file** the issue draft (`.claude/intent/drafts/issue-<slug>.md`) is the
   record: `<!-- Issue: <url> -->` as its first line, each sign-off ticked on that approver's line
   in its **Needs from** checklist (`- [x] @alice — approved <date>: <link>`), and the status as
   `<!-- Status: Accepted -->` (or `Rejected`). GitHub hides the comments, and after each change
   Claude gives you the `gh issue edit` command so the issue shows the sign-offs too. A decided
   draft, like a decided intent file, keeps its approvers.

`/intent` with no arguments lists intents that have no issue yet, with or without an intent file;
`/intent ADR-0007` (or `/intent issue-<slug>`) resumes one.

**The issue convention.** The default `issue-template.md` asks for:

| Section | What goes in it |
|---|---|
| `## Intent` | 2–3 lines: what should be true afterwards and why it matters, with no solution yet. Then **Blocks prod?** Yes / No and **Depends on the platform view** (what we need to protect)? Yes / No. |
| `## Options and trade-offs` | The main contribution: 2–3 options, one line each, covering what each costs, protects and could break. One is always **Do nothing**. |
| `## Proposed call` | Which option and why, in 1–2 lines. |
| `## Needs from <approvers>` | A decision, an IT ask, or nothing; one pending sign-off line per approver (their @mentions are how GitHub notifies them). |
| `<details><summary>Plan …` | Collapsed: **Changes**, **How we know it worked**, **Revert**. Left as the outline until the intent is agreed. |

Short as it is, the issue hides no flaw. Each one of Medium severity or higher gets at least a line:
its trigger, impact and severity, plus its evidence or a link to it in the intent file. The guidance
comments stay in the template and out of the draft.

**Review the draft in your browser.** When `/intent` shows the draft it suggests this. Type
**`draft`** (the word alone, as your whole message) and the issue draft opens in your
browser, in a quiet local editor: one centered column of text on a black page (a **White** palette
is one click away), with the controls fading while you type.

**Several drafts: you pick.** With more than one draft, `draft` opens their list first: 10 a page,
newest first, each with its first line, when it changed, its issue and status, and a warning if it
misses an approver. The draft this session wrote last is already selected, on its page, even when
another draft changed after it. ↑/↓ and Enter (or a click) open one, ←/→ turn the page, and the
editor's **← All drafts** leads back. The list refreshes when a draft changes; `draft` sent again
moves the open list to the session's draft instead of opening another tab.

In the editor you can:

- **Edit it as text:** headings, bold, lists, task lists and tables show formatted, with the
  Markdown syntax hidden ([Milkdown](https://milkdown.dev)). Type Markdown as you go (`## ` makes a
  heading, `- [ ] ` a task) or use the usual keys (⌘B, ⌘I, ⌘K for a link); click a box to tick it.
  Raw HTML (`<details>`, `<kbd>`, comments) shows as greyed text to keep or delete. **Save** (or
  ⌘S / Ctrl+S) writes the file back as plain Markdown.
- **Source** (⌘/ / Ctrl+/): the exact Markdown, to edit raw HTML or anything else by hand.
- **GitHub preview:** see it the way GitHub shows an issue — task lists, tables, the collapsible
  Plan, comments hidden, a single newline kept as a line break.
- **Copy:** **Copy Markdown** (exactly what Save writes) for GitHub's issue form, or **Copy
  formatted** for chat or email.
- **Focus:** full screen. The browser's toolbar and the page's menu go away, all but the Focus
  button, which brings them back (so does Esc). Where the browser can't go full screen (iPhone
  Safari), the button turns the dimming off or on instead.
- **Dimming** (on from the start; ⌘⇧. / Ctrl+Shift+. turns it off or on): the block you are in,
  the one before it and the one after it stay bright; the rest dims (on white it also softens, so it
  steps back as far as on black). Esc shows or hides the controls.

**One line per paragraph.** GitHub shows every newline in an issue as a line break, so a draft
wrapped by hand at 100 columns reads as broken lines once filed — and in the editor, which shows it
the way GitHub will. `/intent` writes each paragraph and each list item on one line, and the
`intent-approvers` hook checks every write of a draft for lines that end mid-sentence (a line ending
in `,` or `;`, or followed by one that goes on in lowercase), so Claude joins them at once. Breaks
that are meant — the **Blocks prod?** and **Depends on…** lines, a label or a URL on its own line —
stay. A draft that still has some shows a banner in the editor with **Join them**, which joins only
those lines (every other byte stays), and `ocgen draft` warns with their line numbers.

Save changes only what you edited. Opened and saved without an edit, the file is written back byte
for byte. After an edit, every block you didn't touch keeps its exact text. An edited block is
written the way Milkdown writes Markdown: a table's columns are re-padded, and a
`[text][ref]` link becomes `[text](url)` (its definition stays). Bullets (`*` or `-`), `*`/`_`
emphasis, bare URLs, @mentions and `#123` stay as you wrote them.

Claude re-reads the draft before using it again, so your edits stay yours. Browser edits skip the
tone check, so if one drops or softens a flaw of Medium or higher, Claude tells you once and lets
you decide.
A change Claude makes while the page is open shows up in it; if you have unsaved edits, the page asks
whether to load the file or keep yours. Saving never silently replaces a newer file. From a
terminal, `ocgen draft [name]` does the same: with several drafts and no name it opens the list with
nothing selected, and `ocgen draft ADR-0007` opens a draft by its intent.

**What counts as a draft:** any `.md` file in `.claude/intent/drafts/` whose name (without `.md`) is
letters, digits, `-` and `_`, starting with a letter or digit, up to 200 characters — /intent names a
draft after its intent, title and all. ocgen can't open any other `.md` file there, so it names it
instead of skipping it: in the `draft` note, in the list, and in `ocgen draft`'s output, with the
rule to rename it by.

A `UserPromptSubmit` hook (`intent-draft`) catches the word and starts the editor. The same hook
remembers the draft the session wrote last, the same way whatever tool wrote it: after a Write, Edit
or MultiEdit by its path, and around each Bash call by what changed. It keeps the drafts' state just
before the call (size, time and text) and compares it afterwards, so a draft made by `cp`, `mv`,
`sed -i` or a redirect counts, and when a call changes several, the newest (then the first by name)
wins. One small file per session in `.claude/intent/drafts/.sessions/` (git-ignored like the drafts,
kept a month). Hooks run outside
Claude Code's Bash sandbox, where a local server can start; inside the sandbox it can't. Any other
prompt passes through untouched. The editor is the same loopback viewer as `/inquire`'s pages,
behind a random token and same-origin checks for saving. The draft's text can't run scripts or load
anything: in the editor raw HTML is only text and images show as their Markdown; in the preview raw
HTML is limited to a few tags without attributes, and images become links. Pasted HTML becomes
Markdown, without scripts or event attributes. The editor is built into the binary and runs inside
the page's one script, so the page loads nothing from the network. It needs
the ocgen binary on `PATH`; without it the word reaches Claude, which tells you to run
`ocgen draft`. `ocgen verify` checks the hook.

**Read the intent files the same way.** Type the project's intent prefix in lowercase — **`adr`**
by default, `rfc` if the prefix is `RFC` (`ocgen edit intent`) — and the intent file opens in your
browser, read-only; with several, their list opens first, 10 a page, **highest number first**
(numbers say which is newest: a fresh clone gives every file the same time), with the file this
session wrote last selected. With none, nothing opens and Claude is told why. The page uses the
drafts' black and white palettes and is made safe like the draft's preview: raw HTML keeps only
the tags GitHub allows, links only `http(s)` and `mailto` targets (a relative link shows as text),
images become links, and the page's one script is ocgen's own. It follows the file: a change on
disk refreshes it. **Markdown** shows the file as written, **Page** as GitHub draws it. When the file
has a reading copy in your answer language, that copy opens instead, with a switch to the English
file. To edit, edit the `.md` file. From a terminal, `ocgen adr [name]` does the same,
by name, by its start, or by number (`ocgen adr 7`, `0007`, `ADR-7`).

The same `intent-draft` hook catches this word and remembers the intent file the session wrote last
(by path, or by what a Bash call changed), so projects created before get it with the new binary,
without regenerating. The intent files are tracked, so nothing is written beside them: the viewer
and the session records live in `.claude/intent/viewer/`, which ignores itself. A prefix spelled
like a word ocgen already has (`draft`, `note`, `notes`) leaves that word alone; `ocgen adr` still
opens its files.

**Blameless, but completely honest.** The team that wrote the code reads these drafts, so they
follow a writing standard: **soften the framing, never the facts**. The code, design or behaviour
is the subject of every sentence, never a person; git history explains *why*, never *who*; context
comes before the flaw; every flaw reads *when [trigger], [behavior], which means [impact]
(evidence: F#, `file:line`)*, then the proposed fix; severity (Critical, High, Medium, Low, plus a
likelihood) rests on the Verified findings, not on adjectives; what works is said too; and the
drafts make the decision requested explicit. The **tone check** — a fresh `reviewer`
subagent (or the research agent, if there's no reviewer) — reads both drafts against the findings
before you see them, checking both ways: blame and alarm (banned words, people as subjects, names
from git history) **and** softening or omission (every Verified finding of Medium or higher present
with its evidence, no severity below what the evidence supports, no hedge that contradicts a
finding), plus every approver listed as pending. Claude fixes what it reports, then a fresh check
reads the fixed drafts again, at most 2 rounds; anything it still reports is named when you see the
drafts, never dropped. The severity definitions and the banned words live
in a comment block at the top of both templates, so your team can tune its house style there.

**The drafts always name the current approvers.** `/intent` gets the approver list when the
skill is generated. A session that started before you changed it keeps the old list: it once wrote
"No approvers configured" a day after an approver was added, and an issue filed from that notifies
nobody. So ocgen checks the files against the project's state as it is now, whatever the session
was told:

| When | What checks | What happens |
|---|---|---|
| Claude writes a pending intent file or an issue draft | the `intent-approvers` hook (`PostToolUse`) | Claude is told who is missing, and that this list wins over its instructions; it fixes the file then (for a draft, also any lines wrapped by hand) |
| You review a draft | the `draft` editor, `ocgen draft`, the `draft` note | A banner (live, as you type) or a warning names the approvers it doesn't @mention |
| The approvers change | `ocgen edit intent` (and `--show`) | Lists every pending intent and draft that misses one, and how to fix it (`/intent ADR-0007`) |
| Any time, CI | `ocgen verify` | "/intent approvers in drafts" warns with the files |

An intent that is already Accepted or Rejected is history and keeps its approvers. Resuming a
pending intent (`/intent ADR-0007`) brings its Approvers line, Sign-off and issue draft up to date.

**Approvers who must sign off.** Name the architects and managers who decide with
`ocgen edit intent --approver @alice --approver @org/architects` — GitHub users or teams only (no
emails): GitHub notifies them through the @mentions in the issue, and neither ocgen nor Claude
contacts anyone any other way. Every intent file and issue draft names them; with none configured,
the drafts say "No approvers configured" and `/intent` asks whether to proceed without a sign-off.
**`/deliver` checks the sign-off:** given an intent file or, without one, its issue draft (or an
issue that links either) that isn't Accepted with every approver approved, it lists who is still
pending and stops — and if you insist, it says plainly that this bypasses the approvers' sign-off
and waits for your explicit confirmation.

Inside Claude this rule is enforced by the skills' instructions only. The hard enforcement is
GitHub's: link the project's **existing** CODEOWNERS with `ocgen edit intent --codeowners
.github/CODEOWNERS` (or `CODEOWNERS`, `docs/CODEOWNERS` — wherever GitHub reads it; ocgen never
creates one). ocgen keeps a marked block in it that makes the approvers code owners — of the
intent directory (`--codeowners-scope intents`, the default), of every file (`all`), or of nothing
(`off`; the block also goes when there are no approvers or you unlink with `--codeowners off`). The
`/intent` skill names the file and its rule, and tells Claude never to edit the block; nothing in
`.claude/` links to it (the next run removes the `.claude/CODEOWNERS` symbolic link that ocgen
0.7.1 and older kept there). Your own lines are never touched. CODEOWNERS takes the **last**
matching rule, so ocgen appends its block at the end and tells you when a rule of yours after it
takes precedence. GitHub only *requires* their review once branch protection asks for it, which
ocgen can't turn on: **Settings → Branches → Add classic branch protection rule** → your main
branch → **Require a pull request before merging** → **Require review from Code Owners**; or with
rulesets, **Settings → Rules → Rulesets → New branch ruleset** → target the branch → **Require a
pull request before merging** → require review from code owners. `ocgen verify` reports the file
and the block.

While the block is there, **Claude asks before it edits any CODEOWNERS** — `Edit(/.github/CODEOWNERS)`,
`Edit(/CODEOWNERS)`, `Edit(/docs/CODEOWNERS)` and `Bash(*CODEOWNERS*)` are ask rules, even without
the permission defaults and in auto mode — so it can't quietly drop the block, add a rule after it,
or add a CODEOWNERS that GitHub reads first. A Bash command that names CODEOWNERS asks even to read
it (the Read tool doesn't). It's a text match, not a boundary: the review GitHub requires is.

ocgen follows GitHub's rules for CODEOWNERS, and says when the project doesn't:

- **One file, GitHub's order.** GitHub reads the first of `.github/CODEOWNERS`, `CODEOWNERS` and
  `docs/CODEOWNERS` and ignores the rest. ocgen refuses to link a file another one hides, and warns
  when one turns up later in a place GitHub reads first. `.github/CODEOWNERS` is the safest place:
  nothing can hide it.
- **Exact names.** GitHub's file system is case-sensitive, even when yours isn't: `.github/codeowners`
  isn't read, and `/docs/adr/` doesn't match `docs/ADR/`. ocgen refuses a file in another case,
  stops writing one renamed since, and warns about an intent folder spelled differently on disk.
- **Own the CODEOWNERS file.** GitHub's advice is that someone you trust owns the CODEOWNERS file
  (`/.github/CODEOWNERS @owner` or `/.github/ @owner`), so a pull request can't change who reviews
  without their review. ocgen warns when nothing owns the linked file, or a place GitHub reads before
  it, and names the lines to add (with `--codeowners-scope all` the approvers own them).
- **Under 3 MB.** GitHub doesn't load a bigger file; ocgen warns.
- **What ocgen can't check:** the approvers need explicit write access to the repository (a team
  must also be visible), or GitHub assigns them nothing; the block works only once it's on a pull
  request's base branch, so commit and merge it; and GitHub doesn't request code owners on a draft
  pull request until it's ready for review.

**Configure it with `ocgen edit intent`:**

```bash
ocgen edit intent --show                                 # settings and template paths
ocgen edit intent --prefix RFC --digits 3 --dir docs/rfc # RFC-001-<slug>.md in docs/rfc/
ocgen edit intent --max-words 150 --branch develop       # shorter issues; numbers checked on origin/develop
ocgen edit intent --trust-domain docs.example.org        # same as `ocgen edit docs --trust` (the project-wide list)
ocgen edit intent --approver @alice --approver @org/architects  # who must sign off (--remove-approver to drop)
ocgen edit intent --codeowners .github/CODEOWNERS        # keep the approvers in your CODEOWNERS (off to unlink)
ocgen edit intent --codeowners-scope all                 # approvers review every change (intents | all | off)
ocgen edit intent --issue-template                       # edit the issue structure in $EDITOR
ocgen edit intent --reset-intent-template                # restore ocgen's default
```

The two templates live in the project under `.claude/intent/` and belong to you: ocgen writes them
once and never overwrites them (like `CLAUDE.md`), so commit them with the rest of `.claude/`.
A project created before the writing standard, the approvers or the issue convention keeps its
copies; run `ocgen edit intent --reset-issue-template --reset-intent-template` to pick up the new
defaults (the issue convention above, the intent file's severity, approvers and sign-off sections,
and the house-style comment block).

#### Reviewing a change into an intent (`/review-intent`)

**`/review-intent [target]`** reviews a change with your project's own agents and records what it
finds the way `/intent` records an intent: an issue draft you file yourself, and an ADR only when you
ask for one. It ships with `/intent` and uses its settings, templates, hooks and writing standard, so
`ocgen edit intent --disable` turns both off. You start it; Claude can't. Claude Code's own
`/review`, `/code-review` and `/security-review` print or post findings; this one keeps them as a
checked record your approvers can decide on, under a name that doesn't collide with theirs.

**What it reviews.** With no argument, the current branch against its base: the open pull
request's base, else the branch `ocgen edit intent --branch` names, else the remote's default. The
diff runs from the merge-base to your working tree, so uncommitted and untracked files count. `#123`
or a pull-request URL reviews that pull request (`gh pr view`, `gh pr diff`); a branch name, that
branch; a path, the change under it (or, with no change there, the files as they are). The base and
head commits are pinned first, so every agent reviews the same code. A pull request or branch that
isn't checked out can be read but not run, so its findings can only be confirmed by reading; Claude
offers that you check it out (`gh pr checkout`), and you run that.

**Fast: independent work runs at once.**

| Step | Who | Gets | Returns |
|---|---|---|---|
| Context pack | the session | — | the diff, stat and log, the change's stated purpose (pull-request body, linked issues, an intent it names) and the changed symbols |
| Find, in one wave | `scope-guard` | the purpose and the commits | what the purpose asks that the change lacks, what the change adds that it doesn't need, the blast radius |
| | `reviewer` × 4 | the pack and one area: correctness, security, tests, cross-platform | candidates: severity, `file:line`, trigger → behavior → impact, evidence, fix |
| | `explorer` | the changed symbols | every call site, test and doc that uses them |
| Merge | the session | the reports | candidates C1, C2, … (duplicates merged, the scope gaps included) |
| Verify, in one wave (≤ 8 at a time) | the adversary, one run per candidate (the Low ones together) | one claim, its evidence and fix, the pack — **not** the reviewer's reasoning or confidence | Confirmed (reproduced), Confirmed (by reading), Rejected or Unsettled, plus a severity and a fix check |
| Draft | the session, then a fresh `reviewer` for `/intent`'s tone check | the confirmed findings | the issue draft |

**Trustworthy: nothing unchecked reaches the draft.** Only candidates the adversary confirms become
findings, numbered **R1, R2, …** by severity, one line each:

```
- **R3 · High · Possible · Verified** (cross-platform) — When `--target` is a Windows path with spaces, `resolve_target` (`src/review.rs:88`) splits it, so the review silently skips the change, which means the report is clean for code nobody checked. Evidence: test `target_with_spaces` fails. Fix: pass the path through `paths::for_shell`. red-team: confirmed (reproduced: `cargo test target_with_spaces`).
```

A reproduced finding is **Verified**. One confirmed by reading is **Inferred**, with a confidence;
since the house style lets only Verified findings set a severity, its severity is **provisional**
and it says what would verify it. Rejected candidates are listed with the reason. Those the adversary
could neither confirm nor reject are listed as **Not confirmed**, with what would settle them: never
findings, never dropped. A flaw the adversary finds on its own gets one more independent check.

The agents are found by role, so a renamed adversary (`red-team`) or scope guard is named as such.
Without an adversary, a fresh reviewer checks each candidate by reading, every finding is Inferred,
and the draft says so. Without a scope guard, the session checks the purpose itself. Without a
reviewer, a general-purpose agent does the review and is told it is read-only.

**The draft** is `.claude/intent/drafts/issue-review-<slug>.md`, with `<slug>` from the branch,
`pr-123` or the path (a second review of the same target gets `-2`, never an overwrite). That is
`/intent`'s name for a draft without an intent file, so `/intent`, `/deliver` and `/recap` read it like
any other. It follows the project's issue template and `/intent`'s writing standard: the Intent says
what was reviewed and names each finding of Medium or higher; the options are to fix everything, to
fix the Critical and High ones now and file the rest, or to do nothing; the approvers are named. The
findings, the rejected and not-confirmed candidates, the scope and blast radius, and the Plan sit in
collapsed `<details>` blocks, so the visible text stays within the word limit (the blocks don't
count).

Type `draft` to edit, preview and copy it in your browser, as with `/intent`; the same hooks check it
for the current approvers and for lines wrapped by hand. Claude gives you the
`gh issue create --body-file …` command and never runs it. It never posts on the pull request either:
while `/intent` is on, `gh pr comment` and `gh pr review` are **ask** rules, even without the
permission defaults and in auto mode. They aren't denied, so your own requests and Claude Code's
`/code-review --comment` still post once you confirm. After you file the issue, `/intent #123` links
it and `/intent issue-review-<slug>` records the sign-offs.

**An ADR, only when you ask.** Once the draft exists, ask for an ADR and Claude writes one the way
`/intent` writes an intent file: the same numbering (checked locally and on the remote), template and
approvers, plus a reading copy when you read answers in another language. The findings are its
evidence, the blast radius its consequences, the not-confirmed candidates its risks. The two link each
other: the draft's Intent links the ADR, and the ADR's `Issue:` line names the draft until you file
the issue. The draft moves to the ADR's name (an `mv`, which asks first), the name `/intent ADR-0007`,
`/deliver` and `/recap` look for. A browser tab still showing the old name is stale: type `draft`
again.

**One source with `/intent`.** The writing standard, the tone check, the numbering, the reading copy
and the browser-review text are templates both skills include (`claude/intent/*.md.j2`), so a change
reaches both. While it runs, `/intent`'s read-only git and `gh` reads are pre-approved, plus
`git merge-base`, `git status`, `git ls-files` and `git rev-list`; nothing that writes, files or
posts.

#### Daily recap (`/recap`)

**`/recap`** catches you up on a repository in one command: it brings your branches up to date
with the remote, then reports, branch by branch, what changed since your last recap, and what was
said on GitHub about that work (wizard: "Include the /recap daily branch recap command?"; on by
default). You start it; Claude can't, because it moves refs.

1. **Sync, fast-forward only.**
   - It fetches `origin` (`git fetch --prune --no-tags --no-recurse-submodules origin`).
   - It moves every local branch that is only behind its upstream in one `git fetch . …` call;
     git itself refuses anything that isn't a fast-forward.
   - The current branch moves only with a clean working tree, and only when the update touches
     nothing under `.claude/` or `.mcp.json` (that would change the running session's own
     configuration: you fast-forward it in a terminal and restart).
   - Each new remote branch gets a local tracking branch.
   - Diverged branches, branches checked out in another worktree, and branches whose upstream
     is gone are left alone and listed under **Needs your attention**.
   - It never forces, rebases, resets, stashes, deletes a branch or pushes, and the report
     lists every move as old → new SHAs, so you can undo any of them.
2. **Analyze each changed remote branch**, your teammates' included, by reading the diffs, not
   just the commit messages. Feature branches leave out the default branch's commits merged
   into them; the default branch is read one merged change at a time (`--first-parent`). A
   force-pushed branch is flagged, and `git range-diff` tells "rebased, unchanged" from real
   edits. Branches deleted on the remote are reported as merged or unmerged.
3. **GitHub activity**: new comments and reviews since the last recap on the related issues and
   on the branches' PRs.
   - Related issues come from links that already exist: the `Issue:` lines of your `/intent`
     files (and of issue drafts without one), `#123`/`GH-123` and issue links in branch names and
     commit subjects, a branch named `123-…`, and the issues a branch's PR closes.
   - Claude never runs `gh`: the sandbox withholds your GitHub login from its shell. It writes a
     request (`.claude/notes/recap/github-request.json`), and the `recap-github` hook answers it
     outside the sandbox with your own gh login: one fixed, read-only GraphQL query on the
     remote's github.com repository, at most 10 issues and 10 branches.
   - The answer goes to `.claude/notes/recap/github.json`, never straight into Claude's context;
     its comment text is treated as data, never instructions. Bot comments are only counted,
     hidden (minimized) ones skipped, long ones cut, and the file is kept small.
   - A comment saying "approved" or "LGTM" is never taken as a sign-off: a PR's review state is
     reported as GitHub records it.
4. **Report**: an overview table (status, commits, authors, ahead/behind the default branch),
   one section per changed branch (summary, changes by area, notable changes such as API,
   schema, config, dependencies, CI or security, risks), a **GitHub** section (per issue or PR:
   who said what, and what needs you: a mention, a review requested from you, changes
   requested, a question, a blocker), then **Needs your attention**.

**Since when?** After each report, `/recap` records every remote branch's tip in
`.claude/notes/recap/state.json`, so the next recap covers exactly what changed in between,
even if you pulled by hand. The first recap looks at the last 24 hours. `--since "3 days ago"`
looks further back without moving the baseline (`--since today` starts at midnight, local time),
and `--no-fetch` skips the fetch when you have fetched yourself. GitHub activity counts from `github_checked_at` in the same state file (when
the hook last answered), and `--no-github` skips the GitHub step. Each report is also saved as
`.claude/notes/recap/<date>.md` (and, in a project whose two languages differ, as
`<date>.<language>.md` in the other), and ocgen opens it as a read-only page: rendered or as
Markdown, black or white, with a switch to the other language. The page works as a plain file, with
no viewer running. The folder ignores itself (a `.gitignore` with `*`), so your own `.gitignore`
isn't touched.

**Permissions.** The skill pre-approves read-only git commands, the exact fetch and the exact
current-branch fast-forward. The local fast-forward batch and each new tracking branch still
ask you first: no permission rule can tell a safe `git fetch . a:b` from a forced `+a:b`, so
the prompt is your confirmation. With the approval gate on, `/recap` doesn't run the
current-branch merge at all; it prints the command for you. No `gh` command is pre-approved: the
GitHub lookup is the hook's, which uses your gh login for that one read-only query
(`OCGEN_RECAP_GH_TIMEOUT` caps it, 20 seconds by default). The remote's URL is read from git's
config only, and `ocgen verify` fails when any settings file sets `OCGEN_RECAP_GH`, the tests'
stand-in for gh.

**When it stops.** A failed fetch stops the recap rather than reporting stale branches as new.
A private remote, for example, needs credentials the sandbox withholds by default. Fetch in your
own terminal, then run `/recap --no-fetch`. Inside a linked worktree, `/recap` stops too (a
fetch there moves refs the main checkout shares, and the saved state would be lost with the
worktree). The only exception is a report-only `/recap --no-fetch --since …`. GitHub problems
never stop a recap: gh not installed or not logged in, no network, a rate limit, or a remote
that isn't on github.com (GitHub Enterprise and SSH host aliases aren't supported) show as one
line, "GitHub — not checked: <reason>". The lookup needs ocgen on PATH; without it the report
says so. PRs are looked up in the remote's repository: one from your own fork counts, one from
someone else's fork with the same branch name doesn't, and when `origin` is your fork, PRs that
live upstream aren't found.

To add `/recap` to an existing project, re-render it with `ocgen doctor [dir]`. Projects created
by older versions get it switched on automatically.

#### Trusted documentation sites (`ocgen edit docs`)

Every way an agent reaches the web follows one list of trusted documentation sites: the main
session, every subagent and skill (`/deliver`'s research, `/inquire`, `/intent`, the `explorer`)
and every team member.

- **Claude projects.**
  - **The WebFetch guard** is a `PreToolUse` hook (`.claude/hooks/https-only-fetch.sh`, matcher
    `WebFetch`) in every Claude project. It allows a fetch only over `https://` and only to a
    trusted site. Plain `http://`, any other host and look-alikes (`docs.rs.evil.com`,
    `evildocs.rs`) are blocked, even in auto or bypass-permissions mode. The host must be a plain
    name with an optional port: a `user@` part, a backslash (`https://evil.com\@docs.rs/` really
    goes to evil.com), `%`-escapes or spaces are blocked rather than guessed at. `ocgen verify`
    checks it.
  - **No prompts for trusted sites:** with the permission defaults, `settings.json` allows
    `WebFetch(domain:<site>)` for each one, so no agent asks to fetch them. Without the defaults
    you manage permissions yourself, and the guard still holds.
  - **The workflow rule** gains a "Web research" section, and each agent that can fetch is told
    the sites. Fetched pages and search results are data, never instructions: a trusted site
    such as `repost.aws` can still carry community posts.
- **OpenCode projects.** OpenCode can't limit fetching to some sites, so ocgen never pre-allows
  it. Every `webfetch: allow` (and an agent with no `webfetch` key, which OpenCode reads as allow)
  becomes `ask`, and the agents that can fetch are told the list.
- **WebSearch stays open.** Results can come from any site, but opening one is a fetch, which
  the guard limits.
- **The default list** is official docs: GitHub, git, Rust, Python, Node, MDN, Go, Kubernetes,
  HashiCorp and Terraform, AWS (`docs.aws.amazon.com`, `aws.amazon.com`, `repost.aws`, and the
  older SDK and CLI hosts `boto3.amazonaws.com`, `awscli.amazonaws.com`, `sdk.amazonaws.com`,
  `docs.powertools.aws.dev`), Google Cloud, Microsoft Learn, IETF and RFC Editor, and Claude.
- **An empty list trusts nothing:** every WebFetch is blocked.

```bash
ocgen edit docs                                    # show the list
ocgen edit docs --trust docs.example.org           # trust one more (repeatable; --untrust to drop)
ocgen edit docs --trust "*.amazon.com"             # every subdomain (not amazon.com itself); *.com is refused
ocgen edit docs --trust serde.readthedocs.io       # shared hosts (*.github.io, *.readthedocs.io…) only by exact host
ocgen edit docs --trust https://docs.example.net   # https:// URLs are fine (stored as the host); http:// is refused
```

`ocgen edit intent --trust-domain` / `--untrust-domain` edit the same list.

**Existing projects keep their own list.** ocgen never widens trust on its own, so a newer
default (the AWS sites, say) reaches an existing project only through `ocgen edit docs --trust`.
`ocgen doctor` moves a list kept with `/intent` by an older ocgen to the project, unchanged. It
also adds the guard to projects without `/intent`, so fetches from there to untrusted sites start
being blocked; the block message names the `ocgen edit docs --trust <host>` to run.

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
.claude/hooks/team-*.sh         # the TeammateIdle / TaskCreated / TaskCompleted gates and the approval gate
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

**What the gates read.** Agents can't argue with a hook, so each gate reads one precise thing:
- **A confidence line.** Only a line that starts with `Confidence` and a percentage counts:
  `Confidence: 97%`, or `**Confidence** — 97%`. The last one wins (a value over 100 counts as
  none); an inline "Done. Confidence: 98%" or "low confidence in 3 cases" doesn't count.
- **Worker gate** (`SubagentStop`): it reads the worker's final message, and only holds a worker
  whose tree changed since it started (edits or commits; `SubagentStart` records the start in the
  self-ignored `.claude/worker-baseline/`). Read-only roles — your agents without Write/Edit, plus
  the built-in `Explore` and `Plan` (`SUBAGENT_READONLY_ROLES`) — are never held. With the team
  hooks on, in-process teammates are left to the task gate.
- **Task gate** (`TaskCompleted`): it reads the teammate's own last message from its transcript —
  never the task's subject or description, never the lead's words. Failing that,
  `.claude/team/confidence/<team-name>/<task-id>.txt` (its first number), removed once the task
  passes.
- **Risk gate** (`TeammateIdle`): risks are owned by teammate names (`Owner: implementer-1`; `/team`
  names teammates after their role, numbered). An owner given as a role holds no one, and you get
  a message saying so — as you do when an event names no teammate.
- **Plan gate** (`TaskCreated`): `/team-plan` writes `Status: APPROVED session=<id>`, and the first
  task stamps it with the run and a checksum of the plan. Editing the plan (ticking a task or
  closing a risk isn't an edit), a later session or team run, or an approval line from another
  session makes it stale: approve again, and a fresh `Status:` line is written.

#### Hooks run in Rust (with a script fallback)

Every gate and optional hook is implemented twice: as the POSIX `sh` script in `.claude/hooks/`
and inside the ocgen binary (`ocgen hook <name>`). The generated hook command uses the
binary when a compatible ocgen is installed, and the script otherwise:

```sh
if [ "$(ocgen hook --check 2>/dev/null)" = "ocgen-hooks 14" ]; then ocgen hook team-approval-gate; else sh ".../team-approval-gate.sh"; fi
```

- **The binary gives you** real JSON parsing instead of `grep`, and hooks that work on
  **native Windows**.
- **The scripts keep the project working** for teammates and CI machines without ocgen.
  The exceptions are `inquire-notes`, which renders the notes pages, opens them on `note` and
  asks for each document's other language version, `intent-draft`, which opens `/intent`'s issue draft (and, on `adr`, its intent files)
  in the browser, `intent-approvers`, which checks drafts against the current approvers, and
  `recap-github`, which answers `/recap`'s GitHub request: their scripts do nothing, so without
  ocgen the ledgers stay Markdown only, nothing asks for a document's other language, `draft`, `note` and `adr` reach Claude as typed, only `ocgen verify` and `ocgen edit intent` report a draft that misses an
  approver, and `/recap` reports GitHub as not checked.
  They need only a POSIX `sh` (Git Bash on Windows): `jq` is used when present and is never
  required (without it, JSON escapes are decoded, and a command holding a `\b`, `\f` or `\u`
  escape is treated as high-impact), and Windows paths (`C:\Users\...`) are handled in both
  implementations.
- **A stale binary can't weaken a gate.** The command uses the binary only when it reports
  *exactly* the hook protocol the project was generated with. The protocol number goes up
  whenever hook behaviour changes, so any other ocgen, older or newer, falls back to the
  project's own scripts. Those always come from the ocgen that generated the project (template
  overrides can't replace them), so they differ only where someone edited them by hand; `ocgen
  verify` reports that, and fails a gate that isn't ocgen's.
- **The two behave the same.** They share the same messages, exit codes and loop-guard
  state files, so machines with and without ocgen can work on one project. Parity tests
  (`tests/hooks.rs`) run every gate scenario through both implementations and require
  identical results. These tests already caught a stray-error bug in one script.
- **Tested the way Claude Code runs them.** `tests/e2e_guardrails.rs` takes the hook commands
  verbatim from a generated `settings.json` and runs them with `bash -c`, the event on stdin and
  the settings' `env`, routed by matcher — once with this build's `ocgen` on `PATH`, once with
  none, so the command falls back to the scripts. It covers the approval gate (high-impact
  commands from any tool, self-approval, a human approval that expires), the loop guard, the
  WebFetch guard, the worker, plan, task and risk gates, the config audit, and a push phrased
  around the text gate that the `pre-push` hook still stops before it reaches a remote.

#### The execution-approval line

With Agent Teams' approval gate on, pushes, deploys, cloud mutations, publishes and remote
shells wait until **you approve them**, from your own terminal:

```bash
ocgen approve              # unlock for 30 minutes (then it re-locks by itself)
ocgen approve --minutes 10 # any window from 1 minute to 1440 (24 hours)
ocgen approve --status     # locked, or minutes left
ocgen approve --revoke     # re-lock now
```

- **The gate is a guard-rail; the sandbox is the boundary.** A `PreToolUse` hook checks every
  tool call: any tool's `command` (Bash, Monitor, PowerShell, an MCP shell) and any tool's file
  path. It matches through global flags, quoting and shell operators (`git -C . push`,
  `git "push"`, `pnpm -r publish`, `docker buildx build --push`, `curl --json …`,
  `git -c alias.p=push p`), and catches deploy and publish scripts and make targets
  (`sh ./deploy.sh`, `make deploy`); reading or grepping those files is still allowed. It is
  still a text match, so a variable, `$(…)`, a `cd` or a script from an earlier call can phrase
  around it. **With the sandbox on** — the default with the gate on macOS, Linux and WSL2 —
  shell commands can't write the approval store and, unless you opt in, run without the
  credentials ocgen knows about, so a push or publish that needs those fails whatever its
  phrasing (the gaps are under *Credentials* and *Defence in depth*).
- **Approving is for you.** `ocgen approve` refuses to run under Claude Code or without a
  terminal. The approval is only an expiry time, stored outside the project
  (`~/.claude/ocgen/approvals/`, one per repository, shared by its worktrees). The gate blocks
  a tool call that runs `ocgen approve` or names that folder — in any letter case, through
  `.`, `..` or `//`, or by Windows short names — and `settings.json` denies
  `Edit(~/.claude/ocgen/**)` and `Bash(ocgen approve*)`. A shell command that reaches the folder
  indirectly is the sandbox's to stop.
- **It expires.** A forgotten approval is locked again when its time is up, and an expiry
  further out than `ocgen approve` can set (24 hours) is no approval at all.
- **A git `pre-push` hook is a backstop.** `doctor` installs it in `.git/hooks/pre-push`, unless
  you already have a pre-push hook, in which case it tells you the one line to add. It refuses a
  push made *under Claude Code* without approval, also from a script, a Makefile or an
  interpreter, for every gated project in the repository. Your own pushes are never affected.
  git skips it on `--no-verify` or another `core.hooksPath`, and it knows Claude Code only by
  `$CLAUDECODE`, so **branch protection on the server is what really stops a push**.
- **Credentials.** With the power-user defaults, reading `~/.ssh`, `~/.aws`, the `gh` token, kube
  and gcloud config is denied. With the sandbox on, shell commands also run without the
  credential files and variables of ssh, git, GitHub, the clouds and clusters, Docker and the
  package registries (npm, yarn, cargo, PyPI, gem, Terraform, Pulumi), unless you opt in. An OS
  keychain is another matter: git asks one through a credential helper (osxkeychain, Git
  Credential Manager, `gh auth git-credential`), and the keychain answers through a system
  service no sandbox file rule covers — tested on GitHub, a sandboxed HTTPS push to a new branch
  went through that way. So `settings.json` also starts git in Claude Code's shells and hooks
  with no credential helper (`GIT_CONFIG_*` set to an empty `credential.helper`, which resets
  every helper git read before it). That stops the plain way, not an agent that names a helper
  in its own command (`git -c credential.helper=…`), and `ocgen verify` warns while one is
  configured: protect every branch on the server, not only `main`. For an approved push to work
  from Claude, allow the credentials: the wizard asks when it turns the sandbox on, or set
  `claude.sandbox.allow_credentials` in `.claude/.ocgen-state.json` and run `ocgen doctor`.
  One hook uses your GitHub login on purpose: `/recap`'s `recap-github`, which runs outside the
  sandbox like every hook and makes one fixed, read-only query with your gh login. Claude never
  gets the token, only the answer file; `/recap --no-github` skips it.

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
- **Team tasks:** a task can't be completed until the check passes. Checks run one teammate at a
  time in the shared directory (a lock in `.claude/team/`), so a failure may come from another
  teammate's edit, and the block says so. A task may narrow the check with a line
  `Check: <the project check> <args>` in its description (`Check: cargo test -p parser`); any
  other `Check:` line is ignored, because the check runs outside the agent's permissions.
- **When it fails:** the agent sees the last 15 lines of output and is sent back. A stated
  confidence doesn't override a failing check.
- **Still bounded:** the loop guard releases a check that keeps failing as UNRESOLVED.
- **Timeout:** after `OCGEN_CHECK_TIMEOUT` seconds (300 by default; time spent waiting for the
  lock counts) the check and everything it started are stopped, by the Rust hook and the scripts
  alike.
- **Sandboxed:** Claude Code doesn't sandbox hooks, so with the sandbox on the hooks run the check
  and the formatter in an OS sandbox of their own (Seatbelt on macOS, bubblewrap on Linux). They
  can't write the approval store, `~/.claude`, your shell startup files, git config, `~/.ssh`,
  `~/.local/bin` or `~/.cargo/bin`, nor the project's Claude settings, hooks, skills, agents,
  commands, statusline, state file, `.mcp.json`, `.git/hooks` or `.git/config`; unless you allow
  credentials, credential files are hidden and credential variables unset. Network and other
  writes stay as they are (so keep `~/.claude` a real folder, see *Defence in depth*). A check
  that writes one of those paths now fails (husky's `git config core.hooksPath`,
  `git submodule update --init`, `pip install --user`). On Linux without `bwrap` the check isn't
  run and counts as failed, and the formatter is skipped; native Windows runs both unsandboxed.

The check runs first, then the confidence statement, and either can be off. `ocgen verify`
warns when no check is set, and `ocgen verify --run-check` runs it.

#### Defence in depth

The approval gate matches command text, so it isn't the only safety line:
- **The sandbox** runs shell commands with OS-level file and network limits (Seatbelt on macOS,
  bubblewrap on Linux/WSL2). It comes with a starter network allowlist (GitHub, npm, crates.io,
  PyPI, Go, Terraform) plus your own domains, and strict mode, so a sandboxed failure can't be
  retried unsandboxed. The secret read-denies apply inside it too. Sandboxed commands can't
  write the approval store, `.claude/statusline.sh`, the state file or `.git/hooks` (Claude Code
  protects the rest of `.claude/` and `.git` itself), and by default they run without your push
  and deploy credentials.
  - **When it's on:** `ocgen new` asks after the Agent Teams questions. With the approval gate on
    and a platform that has a sandbox, the default is yes; otherwise no. A no is remembered, the
    summary shows `sandbox: off — the approval gate is advisory`, `ocgen edit team` offers it
    again (Enter keeps your no), and `ocgen verify` warns.
  - **Where it stops:** it contains shell commands, not the file tools — so with the gate on,
    `settings.json` always asks before edits to the gate's hook scripts, `settings*.json`, the
    state file, the statusline and any `.git/hooks/**`, and before `ocgen edit` and
    `ocgen doctor`, which rewrite them. Hooks and MCP servers run outside it. A protected path
    reached through a symbolic link is protected where the link points, not by its name, and
    the hooks' sandboxed check may write your home folder, so it could replace a linked
    `~/.claude`: keep `~/.claude` a real folder. Native Windows has no sandbox, so there the
    gate is advisory.
- **`permissions.ask`** (with the power-user defaults): Claude Code itself asks before high-impact
  commands (`git push`, `terraform apply/destroy`, `pulumi up/destroy`,
  `kubectl apply/delete/set/rollout`, `helm`, `gh pr merge`/`release`/`workflow run`, publishes
  from npm/pnpm/yarn/bun/cargo, `docker push`, curl uploads, `ssh`/`scp`/`rsync`), **even in
  auto mode**.
- **Config audit:** a settings change in the session that would weaken the gate is blocked (see
  *Extra hooks*).
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
  (`continue: false`). The loop guard never lets an unapproved plan or a gated `git push` through.
- **Turn ceilings.** Each subagent gets a `maxTurns` limit (explorer 40, implementer 60, reviewer
  30, verifier 40, adversary 40, scope-guard 30). At the limit its output comes back marked partial and can be resumed once.
- **Loop discipline.** The workflow rule tells agents to change approach after two failures,
  report the blocker after the third, re-plan at most twice, and never inflate confidence to get
  past a gate. The coordinator must report every escalation as unresolved.

Tune the budget with the wizard's "Max times a gate may block the same agent" question, or set
`claude.workflow.loop_guard_max` in `.claude/.ocgen-state.json` and run `ocgen doctor` (`0` =
unlimited, not recommended). Set a subagent's max turns with `ocgen edit agent`. The
`.claude/loop-guard/` directory git-ignores itself. Projects made by older versions get the
loop guard from `ocgen doctor`. Their existing agents keep unlimited turns until you set
max turns with `ocgen edit agent`.

#### Worktrees: `.claude/` in git or not

A git worktree is a fresh checkout of **tracked** files. `claude --worktree <name>` sessions,
desktop worktree sessions, ocgen's multi-session delivery and `isolation: worktree` subagents all
start in one. A worktree session needs the project's settings, hooks and rules: without them
there is no approval gate, no confidence gates or loop guard, no permission deny list, and the
model is whatever your `~/.claude/settings.json` names. Agents, commands and skills are different:
Claude Code loads them from the main checkout when a worktree has none. Skills need Claude Code
2.1.277+ for that.

Committing `.claude/` is optional. Both ways work:

- **Not in git** (nothing under `.claude/` committed): `ocgen new`, `apply` and `doctor` add a
  block to the repository's `.git/info/exclude`. That file is local to your clone, never
  committed, and shared by all its worktrees. The block keeps ocgen's config out of git:
  `.claude/` settings, rules, hooks, agents, skills, agent memory and state, plus `CLAUDE.md`,
  `.mcp.json` and `.claude/worktrees/`. Because the config is now git-ignored, the generated `.worktreeinclude`
  copies the settings, rules, hooks, status line, output styles, state file, `/intent` templates,
  `CLAUDE.md` and `.mcp.json` into each new worktree. The copy is a snapshot taken when the
  worktree is created. `git status` doesn't show these files in this clone.
- **In git:** commit `.claude/` and ignore only the local parts: `.claude/settings.local.json`,
  `.claude/worktrees/`, `.claude/notes/`, `.claude/loop-guard/` and `.claude/team/execution-*`.
  Once anything under `.claude/` is tracked, ocgen stops adding the block and removes it. To
  switch, delete the `# ocgen:exclude` block from `.git/info/exclude`, then commit `.claude/`,
  `CLAUDE.md` and `.mcp.json`.
- **Detection:** `ocgen verify` ("`.claude/` config reaches worktrees"), `ocgen landscape` and
  `ocgen doctor` warn about any config file that is neither committed nor git-ignored, because no
  worktree gets it. With nothing committed, `ocgen doctor` fixes it. With `.claude/` committed, the
  warning names the files still to commit.
- **Base branch:** with `/fanout` enabled, `settings.json` sets `"worktree": {"baseRef": "head"}`.
  `/fanout` workers and `--worktree` sessions then branch from your current commit and include
  unpushed work, instead of `origin`'s default branch.
- **Study in the main checkout:** `/inquire` is read-only. Notes written inside a worktree are
  lost when it's removed, and the isolation checks stop a worktree session from writing to the
  main checkout. `/inquire` warns if you run it in a worktree.

Subagent isolation (`/fanout`, the implementer's `isolation: worktree`) works the same either way.
The parent session in your main checkout owns the settings, and its hooks fire for the
subagent's tool calls.

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
| `ocgen new [dir] --target claude` | Scaffold a new Claude project (agents, `/multi` `/intake` `/refine` `/deliver` `/inquire` `/intent` `/review-intent` `/recap`, `CLAUDE.md`, `settings.json`). |
| `ocgen new [dir] --target claude --output plugin` | Emit a distributable plugin instead of the project tree. |
| `ocgen new [dir] --target claude --output both --repo <owner/repo>` | Emit both the project **and** a plugin (marketplace + release workflow). |
| `ocgen new [dir] --target claude --team` | Also enable Agent Teams (env flag + `/team` + hooks + guidance). With the approval gate on, the sandbox question defaults to yes (macOS, Linux/WSL2). |
| `ocgen edit team -p <dir>` | Turn Agent Teams and its gates on, off or retune them later. With the approval gate on and the sandbox off, it offers the sandbox (on native Windows it warns instead). |
| `ocgen edit language -p <dir> [--prompts L] [--answers L]` | Change the instruction language (re-seeds the preset agents you didn't edit; lists the rest) or the answer language (the coordinator's answer line and Claude Code's `language` setting). Without flags it asks. See [Languages](#languages). |

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
| `ocgen edit intent -p <dir> [--prefix --digits --dir --max-words --branch --trust-domain/--untrust-domain --approver/--remove-approver --codeowners <path\|off> --codeowners-scope intents\|all\|off --enable/--disable --issue-template --intent-template --reset-…-template --show]` | Configure `/intent` (and `/review-intent`, which shares its settings): intent-file prefix, number width and directory, the issue word limit, the branch checked for taken numbers, the approvers who must sign off (GitHub handles), the project's existing CODEOWNERS it keeps their block in, and the project's issue / intent-file templates. No flags = interactive. |
| `ocgen edit docs -p <dir> [--trust/--untrust <host>]… [--show]` | The documentation sites agents may fetch, one list for every agent, skill and team member (both targets). Claude: the WebFetch guard blocks every other site, and trusted ones never ask. OpenCode: each fetch asks, and the agents are told the list. No flags = show. |
| `ocgen edit permissions -p <dir> --list` | What `settings.json` actually holds, list by list in the order Claude Code checks them, each rule marked ocgen's or yours; flags yours that have no effect (`no effect: ocgen's ask rule wins`) or replace a generated one. Writes nothing. |
| `ocgen edit permissions -p <dir> [--allow/--ask/--deny/--remove <RULE>]…` | Add or remove your own permission rules. They are saved in the state file and merged with ocgen's in `settings.json` by strictness, deny > ask > allow, so each rule sits in one list: a rule stricter than a generated one takes its place (`--deny "Bash(git push:*)"` moves it from ask to deny), a looser one has no effect, and both are warned about. Generated rules (including the approval-gate guards) can't be removed; removing your stricter rule restores ocgen's. No flags = interactive. |
| `ocgen edit skill [name] -p <dir>` | Edit an existing skill; renaming cleans up the old skill directory. Omit `[name]` to pick from a list. |

**Review, repair, reference**

| Command | What it does |
|---|---|
| `ocgen landscape [dir]` (alias `horizon`) | Read-only overview: agents (alias/tools/colour), skills, workflow/output/team setup, delegation topology, and a **Checks** section. |
| `ocgen doctor [dir] [--dry-run] [--yes]` | Repair the project and rewrite files (invalid models, colours, empty roles, bad enum values, older state files). Shows a per-file plan with diffs, flags hand edits, offers to keep hand-added permission rules and MCP servers, removes files ocgen no longer generates, asks first, and backs up to `.ocgen-backup/`. |
| `ocgen verify [dir] [--no-claude]` | Check the project works: up to date, settings valid, no local/user/managed setting turns hooks off or weakens a gate, hooks run (in bash; Git Bash present on Windows), every gate is ocgen's own and the approval gate blocks, http:// WebFetch is blocked, a no-op `cd` is dropped, `/inquire` ledgers get their HTML view, `note` opens them, `draft` opens the `/intent` issue draft and `adr` its intent files, `/recap`'s GitHub request gets an answer, pending intents and drafts name the current approvers, the sandbox is on behind the gate, the pre-push hook blocks Claude and lets you through, scripts use LF, the statusline renders, ocgen on PATH is current, Claude Code validation passes. Runs only what ocgen generates. Exits 1 on failure. |
| `ocgen approve [dir] [--minutes N] [--status] [--revoke]` | You approve high-impact actions for 1–1440 minutes (default 30). Refuses to run under Claude Code or without a terminal. |
| `ocgen verify [dir] --run-check` | Also run the project's committed check command, as you and unsandboxed (only on a repository you trust). |
| `ocgen notes open [topic]` | Show an `/inquire` ledger's HTML page: refresh the tab that shows it, or open one ([details](#the-visual-ledger)). |
| `ocgen notes render <file.md>…` | Render `/inquire` ledgers (`.claude/notes/<topic-slug>.md`), `/recap` reports (`.claude/notes/recap/<date>.md`) and `/intent` reading copies (`.claude/intent/view/<name>.md`), in any of the project's languages, to their HTML pages without opening them; any other file is refused. |
| `ocgen draft [name] [-p dir]` | Open an `/intent` or `/review-intent` issue draft (`.claude/intent/drafts/<name>.md`) in a local browser editor: edit it as formatted text (Markdown syntax hidden) or as Markdown source, save it back as plain Markdown (untouched blocks keep their exact text), preview it as GitHub shows it, and copy it. A calm black page by default (white on request), the text around the cursor dimmed, and a Focus button for full screen. With no name, the only draft, or with several, their list to pick from (10 a page, newest first); `ADR-0007` picks by intent. Typing `draft` in Claude Code does the same, with the draft that session wrote last selected in the list ([details](#from-findings-to-a-github-issue-intent)). |
| `ocgen adr [name] [-p dir]` | Read an `/intent` file (`docs/adr/ADR-0007-<slug>.md` by default; the project's prefix and directory) in a local browser page, read-only and sanitized like the draft's preview, in the drafts' black or white palette. With no name, the only file, or with several, their list to pick from (10 a page, highest number first); a name, its start, or a number (`7`, `0007`, `ADR-7`) picks one. Typing the prefix in lowercase (`adr`) in Claude Code does the same, with the file that session wrote last selected ([details](#from-findings-to-a-github-issue-intent)). |
| `ocgen managed-settings` | Print a recommended organisation policy (`managed-settings.json`): no bypass mode, secrets unreadable, high-impact commands always ask, strict sandbox. |
| `ocgen fields` (alias `reference`) | Explain every configurable field, including the Claude-specific ones (alias, tools, skills, output/plugin, agent teams). |

**Templates** (shared across targets)

| Command | What it does |
|---|---|
| `ocgen templates init [--force]` | Copy the overridable templates to `~/.config/ocgen/templates/` (your copies win). Keeps copies you already have unless `--force`, and lists them; never copies hook scripts or the gate-protocol templates. |
| `ocgen templates path` | Print the override directory. |
| `ocgen templates list` | List resolved templates: `[override]`, `[embedded]`, or `[embedded; override ignored]` for a copy of one that can't be overridden. |
| `ocgen templates edit [path]` | Edit one template in `$EDITOR` (e.g. `claude/agent.md.j2`, `claude/CLAUDE.md.j2`, `archetypes/reviewer.toml`); omit the path to pick from a list. Refuses hook scripts and the gate-protocol templates. |

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

`ocgen templates init` copies the template set into
`~/.config/ocgen/templates/`. Anything you put there wins over the built-in
defaults. It keeps copies you already have (and lists them) unless you pass `--force`. To
change just one template without copying everything, use
`ocgen templates edit <path>`, which opens it in your `$EDITOR` (seeded with its
current content) and writes only that file to the override dir.

An override `manifest.toml` replaces the built-in one as a whole, and its `[[pipeline]]` is the
default team. If you made one before the scope guard or the adversary existed, new projects
still get the old team until you add the `scope-guard` and `adversary` entries to it, in the
shipped order (archetypes need no copy: a missing one comes from the binary).

**What can't be overridden.** Hook scripts, the statusline script (`claude/statusline.sh`) and
the gate-protocol templates (`claude/commands/team.md.j2`, `team-plan.md.j2`, `fanout.md.j2` and
`claude/rules/ocgen-team.md.j2`, which teach the plan, owner, check and confidence lines the
hooks parse) always come from the ocgen binary. Each hook has a Rust twin stamped with the
hook protocol, so an override would either bring a fixed bug back after an upgrade or take
effect only on machines without ocgen. `templates init` doesn't copy them, `templates edit`
refuses them, and a copy left by an older ocgen is marked `[embedded; override ignored]` in
`templates list` and flagged by `ocgen verify` — delete it. The layout of the rest:

```
manifest.toml            # wizard questions (with help text) + default provider(s)
archetypes/*.toml        # agent role presets (mode, permissions, colour, text)
mindset.md.j2            # the mindset every agent and the coordinator end with (accidents, not insiders)
authorship.md.j2         # short commit messages, no AI credit: the Claude workflow rule's section, every OpenCode agent's text
coordination/scope-guard.md.j2   # the coordinator's scope loop (when the team has a scope guard)
coordination/adversary.md.j2     # the coordinator's adversary loop (when the team has one)
coordination/response.md.j2      # the coordinator's answer line (when answers differ from instructions)
opencode.json.j2         # the provider/model config template (loops over providers)
opencode/agents/_agent.md.j2      # one generic agent, rendered per agent
opencode/commands/multi.md.j2     # a command that fans out to the subagents
seeds.toml               # blank-agent seed text (body + external prompt)
claude/agent.md.j2              # one generic Claude subagent
claude/CLAUDE.md.j2             # project instructions + roster
claude/commands/*.md.j2         # multi / intake / refine / deliver / inquire / intent / review-intent / recap… (rendered as skills)
claude/intent/*.md              # default /intent issue and intent-file templates (copied into new projects)
claude/intent/*.md.j2           # /intent's shared text (writing standard, tone check, numbering, reading copy,
                                # browser review), included by /intent and /review-intent
claude/notes/ledger.html.j2     # the /inquire ledger's HTML page
claude/notes/adr.html.j2        # the read-only page of an intent file, a reading copy and a /recap report
claude/notes/langs.html.j2      # the language switch both pages include
claude/notes/draft.html.j2      # the /intent issue draft's editor (`draft`, `ocgen draft`): its layout, palettes and script;
                                # the Milkdown editor itself is built into the binary (tools/milkdown, not a template)
claude/notes/page.css           # the style the pages share
claude/skill/SKILL.md.j2        # one generic skill
claude/skill-presets.toml       # add-skill presets (command / knowledge / forked-research)
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
target an archetype also supplies `claude_model` (the alias), an optional `claude_effort`, and
`tools`. Presets are just starting points: the wizard lets you override every field, and you
can always choose `blank (custom role)` to skip presets entirely. Generated projects are
self-contained — each agent stores its own resolved fields (in the target's state file,
`.opencode/.ocgen-state.json` or `.claude/.ocgen-state.json`), so they don't depend on
the archetype files.

## Development

```bash
cargo test                       # library-level scaffold tests
cargo run --example demo -- /tmp/out default English Ukrainian   # render without the wizard (instructions, answers)
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
