# Brief: thin
Target: deliver
Status: draft
Updated: 2026-10-24
Hand-off: /deliver .claude/briefs/thin.md

## Goal
Retry billing

## Context
- `src/billing/client.rs:41` — `charge()` makes one HTTP call and maps any error to
  `ChargeError::Provider`.

## Scope
Changes `charge()` and its tests only: a retry loop with backoff and a cap, nothing else.

## Success criteria
- A timeout is retried up to three times.
- The same idempotency key goes on every retry.

## Constraints
Must keep the signature of `charge()` and must not retry a 4xx answer from the provider.

## Out of scope
The refund path and the webhook handler stay as they are for now; a later brief covers them.

## Open
- How many retries, and with what cap.
- Whether a 5xx answer counts as a timeout.

## Inferred
