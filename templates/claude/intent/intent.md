<!--
Intent file template for /intent (ADR style). Written by /intent as
<dir>/<PREFIX>-<number>-<slug>.md. Edit this structure freely — it's yours.
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
# <PREFIX>-<number>: <Title>

Status: Proposed
Severity: <Critical | High | Medium | Low>, <Likely | Possible | Rare>
Approvers: <@handle, @org/team>
Date: <YYYY-MM-DD>
Issue: <filled in once the GitHub issue exists>

## Context
The problem and the forces at play: what the design was meant to achieve, and the
constraints and decisions at the time (cite the commits), with evidence from the investigation.

## Evidence
Key findings from the analysis (F1, F2, …), each with `file:line` or a link.

## Decision
What we will do, stated plainly, and the decision requested from the approvers.

## Options considered
1. **Option A** (chosen) — why.
2. **Option B** — why not.

## Consequences
What becomes easier or harder; follow-up work.

## Risks
- Risk — mitigation.

## Acceptance criteria
- [ ] …

## Sign-off
One line per approver: pending, approved, changes requested or rejected — with the date
and a link to where they said so (issue comment, PR review, …).
- @approver — pending
