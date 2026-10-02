<!--
GitHub issue template for /intent. The title goes outside this file: imperative and
actionable, at most 72 characters. Keep the description within the configured word
limit (`ocgen edit intent --max-words`). Edit this structure freely — it's yours.
-->
<!--
House style — /intent reads this block and follows it in every draft. Tune it for your team.

Soften the framing, never the facts. Every flaw, in this order: when [trigger],
[behavior], which means [impact] (evidence: F#, file:line); then the proposed fix.

Severity — each label cites the Verified findings it rests on; an Inferred finding
alone can't set one (say what evidence would confirm it):
- Critical: a security boundary is bypassed, data is lost or corrupted, or normal use
  is blocked, with no workaround.
- High: wrong behaviour with security, correctness or data impact that realistic use
  triggers; a workaround exists, or the trigger needs a specific but plausible condition.
- Medium: wrong behaviour with limited impact — one feature, recoverable, or an
  unusual but realistic trigger.
- Low: no security, correctness or data impact — wording, docs, ergonomics; or an
  improbable trigger with small impact.
Likelihood: Likely (normal use triggers it; the evidence shows it) · Possible (a
specific, plausible condition) · Rare (unusual conditions; the evidence shows they're rare).

Never write (alarm): careless, sloppy, broken (write "fails when…"), obviously, simply,
should have, bad, terrible, fundamentally flawed — nor "critical" unless the severity
is Critical.
Never write (hiding): minor quirk; edge case (unless the evidence shows it's rare);
could potentially / in theory (when the evidence shows it happens); nice-to-have or
cosmetic for a security, correctness or data-loss problem.
-->
## Summary
One or two sentences: what should change, why it matters, and the recommendation.

## Current behaviour
When [trigger], [behavior] (evidence: F#, file:line, logs, links). What works correctly.

## Impact
Severity: <Critical | High | Medium | Low>, <Likely | Possible | Rare>
Which means [impact]: who is affected, and how.

## Background
Why the code is this way today: the constraint or decision at the time (cite the commits).

## Proposal
The approved approach, in a few bullets.

## Decision requested
What the approvers are asked to decide: approve, choose an option, or redirect.

## Approvers / sign-off
- [ ] @approver — pending

## Acceptance criteria
- [ ] Observable, testable outcome
- [ ] …

## Out of scope
What this issue deliberately does not cover.

## Links
Intent file, related issues, docs.
