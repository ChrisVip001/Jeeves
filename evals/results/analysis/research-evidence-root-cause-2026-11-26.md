# Research replay: causal diagnosis

Baseline: `e3cd3ef668c21bc1f06dcee134039638a5bbc311`.
Input: the [four-arm DeepSeek replay](../deepseek-flash-research-ranking-replay-2026-11-26.json).
This note is diagnosis, not a production change or a live-web benchmark.

## What a research answer must establish

A search hit is a lead, a fetch is an observation, a supported claim is an
interpretation of an identified observation, and a deliverable is a set of
such claims plus explicit gaps. None of these implies the next. The user
needs an answer with a stated scope and the ability to see which evidence
earned each conclusion. A failed task can retain work without calling it an
accepted deliverable.

## Acquisition: information was available but not acquired

With a fixed budget of four web calls there was a viable plan: one search
and one read for each of the three requested measures. In both complete
arms DeepSeek received the same list twice and then read two measures. It
saw but did not read the vessel record before the cap; swapping the top
result did not change that outcome in these two runs.

This isolates a failure to allocate calls, not an indexing or DNS failure.
The fixture returns its fixed pool regardless of query, so it does not
prove that a second, targeted query on the real web would be wasteful or
that different queries should be collapsed. Production currently caches
equivalent search arguments for the turn; it does not track marginal
evidence gained or reserve time for remaining questions. The model sees
a transcript of actions, not an authoritative worklist of what remains
unproven. The no-TEU arm is additionally confounded by the model identifying
the abbreviated fixture pages as placeholders.

## Acceptance: three different checks were conflated

1. The draft was written. The current file tool writes the workspace file
   before task settlement; file presence is not final delivery.
2. The provenance check inspects the separate claim file. A temporary
   negative control exercised the real verifier with an unrelated
   `report.md`, a genuine snapshot quote saying revenue grew 12.5%, a
   claim saying it fell 12.5%, and an intentionally incorrect digest.
   Both configured checks, `fileExists` and `sourceSupports`, returned
   true. The generic verifier checks quote occurrence and numeric
   substrings; it neither binds the delivered report to those claims nor
   checks entailment or snapshot digest. This is not a claim about the
   separate named-source recurring-summary verifier.
3. Acceptance still matters: in the earlier resumed run the report existed
   but the task was Failed. The historical terminal diagnostic for that
   first run was not captured; a separately labeled diagnostic repeat is
   attached below. Do not infer the original failure reason from the repeat.

## Design constraint and verification

Use one research state for quick answers and deep tasks. The minimum state
is the original scope, required or optional subquestions, unique
observations (with origin, event/publication date, fetch time, and evidence
identity where known), claim-to-observation links, and current validation
results. Mark missing and conflicting evidence explicitly. The model can
propose questions, sources, and interpretations. The runtime owns budgets,
candidate reuse, checkpoint transitions and the rule that a final report
cannot reference unsupported material as established fact.

The quick path uses the same state for a bounded turn and can return
verified partial findings. The deep path persists it on the existing task
runtime and resumes from evidence gaps rather than depending on the
model to reconstruct progress. Render the accepted report from validated
claims or check those exact report claims before recording a delivery
receipt; a draft file or a separate evidence file alone is insufficient.

Start with fixed facts that admit deterministic checks for date, scope and
units. Free-form interpretation requires a distinct, measured semantic
review, not a substring test dressed up as entailment. Then evaluate at
unchanged budgets on held-out cases: a repeated pool, a refinement that
really adds evidence (must still be allowed), two distinct observations
from one source, a syndicated duplicate, missing/conflicting data, and
restart before publication. Compare coverage, correct abstention,
unsupported published claims, repeat work and total cost against the same
baseline. No port- or flight-specific logic is proposed.

## Diagnostic repeat: restart arm only

The same restart fixture was rerun on official DeepSeek Flash, with the
three unaffected chat arms skipped. It fetched the missing terminal
record, read the vessel mirror, and read the channel record twice.
The draft and claim file both existed. The runtime recorded:

- File verdict: passed, `report.md` existed (2.2 KB).
- Evidence verdict: failed, claim `c1-channel-restored` included `2024`
  but the terse channel quote did not.
- Terminal state: `Failed`; last stop reason was
  `Model call failed: Fixture model cap reached`.
- Usage: eight requests and responses, 51,503 input tokens and 6,663
  output tokens.

In this repeated run the numeric check caught an unsupported qualifier
*in the captured quote*, then recovery exhausted the model-call budget.
It does not show that the year is false. The fixture answer lacked the
section/date needed to prove it. The right repair is to capture dated
evidence or narrow the claim, not to exempt dates from verification.
The diagnostic allows precise future comparisons: stop at a named
evidence gap, retain the draft, and use a separate receipt to mark the
exact report as accepted only when its scoped claims pass.
