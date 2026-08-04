---
title: Operator-triggered RAD loop — iterate build/test/fix on a branch until a feature works, then re-enter the gated pipeline
status: proposed
added: 2026-07-31
---

An operator-triggered escape hatch from the one-pass-per-change pipeline: a
bounded loop that builds, runs, tests, and fixes a feature repeatedly on its own
branch until an external oracle says it works, then emerges as a PR that
re-enters the normal gated flow.

**Blocked on `app-under-test-e2e`.** Do not start this until runtime
verification is in production and demonstrably usable by the agents. The reason
is in "Termination oracle" below: RAD without a mechanical oracle is just a
longer churn loop with a bigger bill.

## Why

The structured pipeline's unit of work is one pass, one implementation attempt,
and static verification — every gate reads text and judges whether it looks
right. That is the correct shape for work whose correctness is legible in the
diff. It is the wrong shape for work that can only be established empirically:
UI wiring, complex integrations, anything where "does it actually do the thing"
is not answerable by reading.

Observed failure mode: autocoder implements a feature, nothing executes it, the
human becomes the test harness and sends the same feature back repeatedly. The
human-in-the-loop review is right ~80% of the time; the remaining 20% is the
operator re-requesting the same work because autocoder had no way to verify it.

## Shape

- **Operator-triggered only.** Never cadence-driven, never automatic. A chatops
  verb or CLI subcommand starts a RAD run against a named goal and repository.
- **Its own branch** in the agent's repo, so main-line pipeline work is
  unaffected and an unwanted end state is a `rollback` away.
- **Bounded**: max iterations, max wall clock, max tokens. There is no
  max-iterations concept in config today — only `executor.timeout_secs` per
  session — so this is new machinery, and it is the most important machinery in
  the feature. Given the 2026-07 token-burn history (gate retries ×3, the
  PR-create loop, the audit fail-loop), an unbounded "keep going until it works"
  loop is the highest-risk thing that could be added to this system.
- **Ends in a PR** the operator reviews and accepts, never a direct merge.

## Termination oracle

The loop's exit condition must be **external and mechanical** — a test exits 0,
an assertion script passes, the e2e command succeeds. If the exit condition is
the agent's own judgment that it is finished, the result is a machine that
convinces itself, at many times the token cost, reproducing the original problem
one level up.

This is why `app-under-test-e2e` is a hard prerequisite: it supplies the
non-self-referential oracle. Ship it, watch how much of the churn it removes
inside the existing pipeline, and let that decide whether RAD is still worth the
complexity. A plausible outcome is that most of the problem disappears without
this feature.

## Iteration summary and operator continuation

On exhausting its bound the loop reports, briefly and scannably:

- how many iterations ran,
- what was accomplished,
- what remains unaccomplished,
- whether more iterations would plausibly finish it.

The operator then decides: continue for N more iterations, fail it, or accept as
is. Continuation should be a reply to the chatops completion notification — the
revision-thread machinery (`revision_thread.rs`, `polling/revision_session.rs`)
already implements exactly this "reply in the thread to drive the next round"
pattern and is the obvious thing to reuse.

De-risks the loop considerably: an experimental feature can be tried, and a
failed attempt ends with "well, that didn't work" plus a rollback, rather than
silent churn.

## Spec reconciliation after a RAD run

RAD builds first and specifies after, which inverts the pipeline's normal order
and creates two risks pulling in opposite directions:

1. **Frozen spec** — the app cannot evolve because canon painted it into a
   corner.
2. **Drift** — RAD-built code silently redefines decisions that should be
   deliberate (database, language, framework, wire formats).

Proposed handling, in this order:

- An audit at the end of a RAD run emits a **brief, human-scannable summary** of
  what would have to change in canon — the decisions, not the diff.
- Followed by the actual deltas / archivable changes.

The point of the summary-then-deltas split is that a reviewer must not miss what
is actually changing inside a large mechanical delta. `brownfield` and `scout`
modes plus `prompts/brownfield-draft.md` already get much of the way there; the
gap is the digestibility of the output, not the extraction itself.

Major decisions (database, language, framework) should be called out
distinctly from incremental behavior additions, so the operator's attention
lands on the choices that are expensive to reverse.

**Constraint:** RAD replaces *how the code got written*, not *whether it is
specified*. Code that lands with no canon backing is invisible to `[canon]`, to
the canon-contradiction audit, and to every downstream consistency check.

## Provenance

A RAD-produced PR must be labeled as such: that it came from a RAD run, how many
iterations, which gates ran and which did not. An escape hatch that is marked is
honest. One that produces PRs indistinguishable from gated ones corrodes the
core proposition that every requirement, change, and sign-off is on file.

## Open questions

- **What artifact carries the RAD goal?** For `app-under-test-e2e` no new
  artifact type is needed: canon scenarios are already WHEN/THEN assertions, and
  the environment is operator config. RAD is different — the whole point is
  looping toward a goal not yet stateable as canon. Options: a roadmap item
  promoted to a RAD run, a transient goal file, or a chatops-supplied goal
  string held in state. Deliberately unresolved; do not invent a fourth lane
  without first establishing that the existing three cannot carry it.
- Does a RAD branch get gates at all during the loop, or only on the exit PR?
- How does perma-stuck / failure-state interact with a loop that is *expected*
  to fail repeatedly before succeeding?
- Concurrency: does a RAD run block the repository's normal pass, or run beside
  it? (The per-repo busy marker currently serializes everything.)
