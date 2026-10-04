<!--
GitHub issue template for /intent. The title goes outside this file: imperative and
actionable, at most 72 characters. Keep the description within the configured word
limit (`ocgen edit intent --max-words`). The sections are a guide, not a form: keep
their order and what each is for, adapt the wording to the topic, and leave these
comments out of the draft. GitHub shows every newline in an issue as a line break, so
write each paragraph and each list item on one line; never wrap it by hand. Edit this
structure freely — it's yours.
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
## Intent
<!-- 2-3 sentences, in one paragraph. What should be true afterwards, and why it matters. No solution yet.
Link the intent file when there is one. A flaw behind it (Medium or higher) goes here or in
an option's "what can go wrong": when [trigger], [impact] — severity, and its evidence (F#)
or a link to it in the intent file. -->

**Blocks prod?** Yes / No
**Depends on the platform view** (what we need to protect)? Yes / No

## Options and trade-offs
<!-- The main contribution. 2-3 options, one line each: what it costs, what it protects, what can go wrong. Include "do nothing". -->
- **A.** …
- **B.** …
- **Do nothing.** …

## Proposed call
<!-- Which option and why, in 1-2 lines. -->

## Needs from <approvers>
<!-- A decision, an IT ask, or nothing. Name the approvers in the heading; one pending
sign-off line each — their @mentions are how GitHub notifies them. -->
- [ ] @approver — pending

<details><summary>Plan (fill in once the intent is agreed)</summary>

- **Changes:** templates, code, and tests touched
- **How we know it worked:** the check or command
- **Revert:** how to undo it
</details>
