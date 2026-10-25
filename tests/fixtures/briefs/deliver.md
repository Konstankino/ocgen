# Brief: billing-retry
Target: deliver
Status: ready
Updated: 2026-10-24 · Commit: 6e43cdc
Hand-off: /deliver .claude/briefs/billing-retry.md

## Goal
Add retries with backoff to the billing client so a transient timeout at the payment
provider no longer fails the whole checkout. Today one timeout returns an error to the
user, who then pays twice when they try again.

## Context
- `src/billing/client.rs:41` — `charge()` makes one HTTP call and maps any error to
  `ChargeError::Provider`.
- `src/billing/client.rs:88` — the idempotency key is already sent on every call, so a
  retry cannot double-charge.
- `tests/billing.rs:12` — the fake provider answers once; there is no timeout case yet.
- `cargo test --test billing` is the check the project runs on this module.

## Scope
Changes `charge()` and its tests only: a retry loop with exponential backoff and a cap,
read from the existing `BillingConfig`. No new dependency; no change to the HTTP client
or the provider's API types.

## Success criteria
- retries-on-timeout: a timeout is retried up to three times with backoff; a test with a
  fake provider that times out twice passes.
- no-double-charge: every retry sends the same idempotency key; the fake provider asserts
  one charge for one key.
- gives-up-cleanly: after the last retry the caller gets `ChargeError::Provider` with the
  attempt count in its message.

## Constraints
Must keep `charge()`'s signature; must not retry a 4xx answer (the request is wrong, not
the network); must stay under the checkout's 10-second budget in total.

## Out of scope
The refund path, the webhook handler and the provider's rate limits — a later brief.

## Risks
- budget-overrun: three retries with backoff can exceed the checkout budget; the test
  with a slow fake provider shows it.
- hidden-outage: retries hide a provider outage from the dashboards; the retry counter
  metric shows it.

## Open

## Inferred
- backoff-shape: exponential with jitter, from the `retry` helper the fetcher already
  uses at `src/net/retry.rs:9`.
