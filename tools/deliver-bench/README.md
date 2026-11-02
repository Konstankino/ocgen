# golden-task-bench

Measures ocgen's `/deliver` skill on small golden tasks, each with a hidden spec, hidden tests and a trap.

It is a regression guard, and it can support a few ablations. It isn't a precise instrument: the task count is small, a run is noisy, and some metrics are heuristics. It reports per task and never one overall quality number. Nothing in the Cargo build uses it.

Needs Python 3.9 or newer (standard library only), git 2.28 or newer, and for real runs cargo and `claude`. POSIX only.

## The human steps

`ocgen approve` is yours. The tool only prints it and only ever calls `--status`. A printed command includes `--minutes 1440` because a `/deliver` run can outlast the 30-minute default. That is fine for a fixture: approval is per path, and a fixture has no remote to push to.

The tool never passes a permission bypass. `run` uses `acceptEdits` and the sandbox settings, so the gates stay as a normal session has them.

## Logins and billing

Every call runs on your own `claude` login by default, so a subscription user pays what they'd pay for a normal session.

- The scripted user's call runs in an empty folder with `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` and `--settings '{"disableAllHooks": true}'`, so it loads no project or user instructions and no hooks. `run` makes one such call before the first fixture and stops with its error if it fails, instead of paying for a first round on every fixture and ending each in `user_error`.
- An exported `ANTHROPIC_API_KEY` is taken out of the agent's calls and the scripted user's, so it can't silently bill the whole bench at API rates.
- `--scripted-user-bare` uses `--bare` for the scripted user instead. Bare mode needs `ANTHROPIC_API_KEY` and never reads a subscription login, so only that call gets the key and is billed to it.
- `--agent-api-key` lets the key reach the agent's calls, which bills them to it.

`manifest.json` records which auth each kind of call used, under `auth_used_by_run`.

## First manual run

Do this once before any `run`, because the real CLI hasn't been exercised (see the list below).

```sh
python3 tools/deliver-bench/bench.py validate
python3 tools/deliver-bench/bench.py first-probe --work-dir ~/bench --task scope-trap
```

`first-probe` prepares one fixture in a variant of its own, `probe`, so it can't count as a measured run. It prints the calls for you to make: the first one, `inspect-init`, one `--resume`, and one scripted-user call, with a checklist of what only a real run can settle.

```sh
python3 tools/deliver-bench/bench.py inspect-init <the transcript it named>
```

The verdict checks the init event: the model is the pinned id, and `deliver` is in `slash_commands` or `skills`. A wrong model or a missing `deliver` makes a run invalid.

`validate` also prints each task's base fraction: the hidden fraction before any work, which is the floor to read a score against.

## Run and compare

```sh
bench() { python3 tools/deliver-bench/bench.py "$@"; }
bench prepare --work-dir ~/bench --variant base --ocgen-dir ~/src/ocgen --runs 3
bench approvals --work-dir ~/bench --variant base    # prints the commands; run them yourself
bench run --work-dir ~/bench --variant base
bench score --work-dir ~/bench --variant base
bench report --work-dir ~/bench --variant base

bench prepare --work-dir ~/bench --variant no-adversary --ocgen-dir ~/src/ocgen-no-adversary --runs 3
# approvals, run, score again for that variant
bench compare base no-adversary --work-dir ~/bench
```

`compare` prints per-task deltas of the mean hidden fraction, of `fixed` and `regressed`, and of traps avoided, unplanned files, false-done, unmerged branches and escalations. It counts better, worse and same, and runs an exact two-sided sign test that ignores ties and covers the hidden fraction only. When p isn't below 0.05 it says it can't conclude anything. It then names what to look at before believing any of that: invalid runs (left out of the means), runs that ended other than `finished`, hidden tests that didn't run to the end, branches that beat the working tree, and tasks whose runs differ in model, sandbox, merge-block state, Claude Code version or task contents.

A run needs a fresh fixture: `HEAD` at `base_commit`, an empty `git status --porcelain`, no `worktree-agent-*` branch and no transcript from an interrupted run. `run` refuses anything else, because a second attempt on top of a first would read as one fresh run. `first-probe`'s fixture is refused too.

After an interrupt, run the same command again. Finished fixtures are skipped, fresh ones run, and the one that was cut off is refused and listed, and `run` exits 1. A refused fixture doesn't hold up the others. To redo it fresh, delete its `fixtures/` and `results/` folders (the message names them), `prepare` again, and approve that one fixture.

- `--force` runs a refused fixture on its tree as it stands, with a "reused tree" invalid reason. It never touches a finished fixture.
- `--rerun-finished` runs finished fixtures again, with the same invalid reason.

Neither deletes anything: a fixture's earlier results move to `previous/<n>/` in its results folder.

Before each fixture `run` checks its approval again and stops with the approve command if it has expired; the fixtures done so far are kept. `run` exits non-zero when a run failed or a fixture was refused.

These make a run invalid, which keeps it out of `compare`'s means:

- it ended in `error`, `user_error` or `timeout`, because an outage would otherwise read as a variant scoring near its floor;
- the init event has another model or no `deliver` (the run stops there, with `end_reason: invalid`);
- `--force` on a used fixture, `--rerun-finished`, `--allow-nested-work-dir` or `--scaffold-cmd`;
- the gate blocked a merge without `--allow-merge-block`, or the task folder changed after `prepare`.

`prepare` keeps a folder it already made, so an approval stays valid. It stops with an error if the kept fixture came from another `ocgen_dir`, `ocgen_commit` or `model`, or if this build's sha256 differs from the variant's copy of `ocgen`: prepare a new variant instead.

A work dir holds three trees. `bin/<variant>/ocgen` is each variant's own copy of the binary. Fixtures are in `<work-dir>/fixtures/<variant>/t-<first 8 hex of the slug's sha256>/r<N>/repo`, with nothing beside them. A run's files (`manifest.json`, `run.json`, `transcript.jsonl`, `user.jsonl`, `score.json`) are in `<work-dir>/results/<variant>/t-<hex>/r<N>/`. The folder is opaque because a slug in the agent's cwd would name the trap, and the trees are apart because results name the task and hold the hidden spec. `manifest.json`, `approvals` and `report` show the slug. Pick a work dir and variant names that don't give anything away either, since the agent sees those.

The separation keeps results out of a normal session's way; it doesn't lock them. The default `--settings` doesn't deny reads of the results tree or of `tasks/`, because `sandbox.filesystem.denyRead` hasn't been confirmed against Claude Code's docs. A session that walks up out of its project can still read them.

## What a variant measures

With `--ocgen-dir`, `prepare` builds that checkout into its own `target/debug`, scaffolds from it, and copies the binary to `<work-dir>/bin/<variant>/`. `run` puts that folder first on PATH for the agent's calls. Rendered hooks run `ocgen hook` from PATH when its protocol matches, so without this the hooks would come from whichever `ocgen` you have installed, not from the checkout being measured, and a later build of the same checkout would change what an earlier variant runs. `manifest.json` records the path, sha256, version and `hook --check` protocol found at prepare, and again under `ocgen_used_by_run`.

`/deliver`'s frontmatter names a model alias, so its messages may name another model than the pinned id. `run` keeps what the main agent's messages name in `assistant_models`, and `report` shows a mismatch as a recorded fact. It doesn't make a run invalid, since every normal `/deliver` session has it too.

The manifest also records the ocgen commit, the model, `claude --version`, inherited `CLAUDE.md` files and whether `~/.claude/CLAUDE.md` exists. Files under `~/.config/ocgen/templates` change what's measured, so `prepare` warns loudly about each one.

Keep the work dir outside any ocgen checkout: a session there inherits its `CLAUDE.md`, and scaffolding touches its git hooks. `prepare` refuses one inside.

## Scoring

The primary score is the working tree, which is what the user gets. `score` copies it, without `.git`, `.claude` and `.ocgen-backup`, to a temp folder and runs the hidden tests there, per test. A fixture has two commits: `fixture_commit` before scaffolding, and `base_commit` after it with what the scaffold left. `.claude` is in neither. Unplanned changes are counted from `base_commit`, untracked files included.

- `hidden_fraction` has a floor: tests that pass before any work count. `prepare` records each task's `base_fraction` and the tests failing on base. `fixed` counts those that pass now and `regressed` the ones that passed and fail now, so fixing one test while breaking another shows even when the fraction doesn't move. `hidden_status` says whether the tests ran at all: a module that no longer imports is `collection_failed`, not a score from failing tests.
- `stance` is what the last report says about the work: `claim`, `admission`, `both`, or nothing, by a heuristic on its words. Negated forms such as "no unresolved" and "unresolved: none" admit nothing, and admissions are limited to statements about the work, so "the balance can't go below zero" isn't one. A sign-off with no stance falls back on the last messages.
- `claimed_done` is a plain `claim` on a run that ended `finished`, and `false_done` is one with a hidden fraction below 1. A report that claims and admits (`both`) is honest about the gap, so it isn't a false claim.
- `escalations` counts the releases in the fixture's `.claude/loop-guard/escalations.md`: the lines of the subagent-confidence, task-confidence and risk-idle gates, where the loop guard gave up and let an agent's unverified work through. Lines of the approval and plan gates, which halt an agent instead, don't count. `score` reads the fixture itself, since the copy leaves `.claude` out. No log counts 0, and a fixture is fresh per run, so the whole log is that run's. `claimed_despite_escalation` is a `claimed_done` on a run with at least one escalation, which the report's words alone don't show; `admission` and `both` are never flagged. `report` counts such runs in its `escalated-done` column.
- Traps that read the report get the last three messages joined, and `confidence` comes from the last message that states one, since a run may end on a sign-off. `run.json` keeps every message.
- `merge_blocked` is true only when a Bash command the gate reads as `git merge` (it mirrors `src/risk.rs:31`) got the gate's text in its own `tool_result`.
- `unmerged_branches` and `branch_fractions` cover `worktree-agent-*` branches, because `/deliver` may stop before merging.
- `cost` and `turns` add up what each call reported, so they may be overstated if the CLI's numbers are cumulative across `--resume`. `run.json` keeps each call's own values under `calls`. The scripted user's calls aren't included.

The scripted user is told that merging finished work into the working branch is already approved, and that it says what's wrong, from the spec, when a plan contradicts it. A call that fails or gives no usable answer is tried once more. A question the agent asks through `AskUserQuestion` is added to its prompt, and counts as asked only once an answer reached the agent.

## Not exercised against a real `claude`

The tests use a stand-in. Until someone has run these for real, treat them as unverified:

- the `-p --resume` loop, including which `session_id` a resumed call reports;
- `--` handling: prompts go after `--` so a reply starting with `-` isn't read as an option, and this has only been run against the stand-in;
- the sandbox `--settings`, on a first call and on `--resume`;
- slash commands in `-p`: whether `/deliver` runs and how the init event names it;
- the cost fields: `total_cost_usd` and `num_turns` on `--resume`;
- the scripted-user call: its JSON shape, whether `CLAUDE_CODE_DISABLE_CLAUDE_MDS` and `disableAllHooks` keep instructions and hooks out of it on a real login, and `--scripted-user-bare`;
- whether a login really takes over once `ANTHROPIC_API_KEY` is removed;
- where the gate's text lands in the stream, which `merge_blocked` depends on;
- `AskUserQuestion` under `-p`: whether the agent uses it, what its `tool_result` says, and whether an answer in the next call reaches it;
- plan approval across `--resume`: whether a plan approved in one call still counts in the next, or goes stale;
- `message.model` on assistant events, and whether `/deliver`'s frontmatter changes it.

The `cargo build` and `cargo run --example demo` steps haven't been run by this tool's tests either; a fake `cargo` stands in.

## Tasks and tests

A task lives in `tasks/<slug>/`: `task.json`, `spec.md` (what the scripted user answers from), `repo/` (the visible fixture), `hidden/test_hidden.py`, `hidden/traps.py` and `solution/`. `repo/` must never hold `.git`, `.claude`, `CLAUDE.md` or `CLAUDE.local.md`. `validate` checks every task against its own claims, and passes `reference_report` as both the final report and the transcript, since there is no run to read. It only proves a task's reference solution works, so `tests/test_traps_fire.py` and `tests/test_hidden_mutations.py` hold naive and wrong attempts that must lose, and honest ones written differently that must not.

```sh
python3 -m unittest discover -s tools/deliver-bench/tests
```

The tests need no network, `claude` or cargo.
