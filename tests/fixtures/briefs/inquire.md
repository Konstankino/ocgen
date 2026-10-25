# Brief: request-flow
Target: inquire
Status: draft
Updated: 2026-10-24 · Commit: 6e43cdc
Hand-off: /inquire .claude/briefs/request-flow.md

## Goal
Understand how a request travels from the HTTP handler to the database and back, and
where a slow query would show up, before I touch the caching layer next month.

## Context
- `src/http/mod.rs:20` — the router; every route ends in a `Handler` closure.
- `src/db/pool.rs:55` — one pool, borrowed per request through `Conn`.
- `.claude/notes/request-flow.md` does not exist yet: no ledger to resume.

## What I know
- assumed: every handler borrows a connection for its whole life, even when it only
  reads a cache.
- verified: the router is built once at start (`src/http/mod.rs:20`).
- assumed: there is no per-request timeout on the database side.

## First question
Where does a request hold a database connection longer than it needs one, and what
decides when it is given back?

## Success criteria
- explain-the-path: I can draw the path of one request through the router, the handler
  and the pool without reading the code again.
- name-the-slow-spot: I can say which layer a slow query shows up in first, and why.

## Constraints
Stay inside the request path: the router, the handlers and the pool. Favour the flow and
failure lenses; the contract lens only where a type decides the flow.

## Out of scope
The caching layer itself, the migrations and the admin routes.

## Open
- Which handler to start from: the one with the most traffic, or the simplest one.

## Inferred
- start-with-reads: the read handlers are the ones that borrow for too long, from the
  `Conn` lifetimes in `src/db/pool.rs:55`.
