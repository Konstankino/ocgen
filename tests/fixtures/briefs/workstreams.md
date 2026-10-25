# Brief: search-rewrite
Target: workstreams
Status: ready
Updated: 2026-10-24 · Commit: 6e43cdc
Hand-off: /workstreams .claude/briefs/search-rewrite.md

## Goal
Replace the search module's hand-rolled tokenizer and ranking with the shared text
pipeline, so results match what the indexer already produces and the two stop drifting.

## Context
- `src/search/tokenize.rs:1` — the old tokenizer, 300 lines, no tests of its own.
- `src/search/rank.rs:14` — the ranking, which calls the old tokenizer directly.
- `src/text/pipeline.rs:30` — the shared pipeline the indexer uses since last month.
- `git log --oneline -- src/search` shows no change in six weeks: nobody else is in it.

## Scope
Three independent pieces: the tokenizer swap, the ranking rewrite on top of it, and the
compatibility tests that pin the results before and after. No change to the indexer or
the pipeline itself.

## Workstreams
- Workstream: tokenizer-swap — Files: src/search/tokenize.rs, tests/search_tokenize.rs — After: none — Worker: session — Check: cargo test --test search_tokenize — Criteria: same-tokens-as-indexer
- Workstream: ranking-rewrite — Files: src/search/rank.rs, tests/search_rank.rs — After: tokenizer-swap — Worker: session — Check: cargo test --test search_rank — Criteria: same-top-ten
- Workstream: compat-pins — Files: tests/search_compat.rs, tests/fixtures/search/ — After: none — Worker: subagent — Check: cargo test --test search_compat — Criteria: results-pinned

## Success criteria
- same-tokens-as-indexer: for the fixture corpus the search tokenizer and the indexer
  produce identical token streams; `tests/search_tokenize.rs` asserts it.
- same-top-ten: the top ten results for the fixture queries do not change; the
  compatibility test pins them.
- results-pinned: the pins exist before the rewrite lands, so a regression is visible.

## Constraints
Must land in three separate pull requests; must not touch `src/text/`; must keep the
public `search()` signature.

## Out of scope
Query suggestions, the admin reindex command and any ranking improvement beyond parity.

## Risks
- pin-before-swap: if the pins land after the swap they pin the new behaviour; the
  board's `After:` order shows it.
- shared-export: both pieces need a new export from `src/search/mod.rs`; the tokenizer
  swap adds it first, and the `After:` line shows the order.

## Open

## Inferred
- subagent-for-pins: the compatibility tests need no commit of their own and no check
  the worker runs, so a subagent does them.
