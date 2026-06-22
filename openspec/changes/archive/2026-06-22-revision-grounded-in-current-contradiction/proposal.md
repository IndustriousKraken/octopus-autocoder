# The spec-revision executor is grounded in the current contradiction and never revises blind

## Why

When a `[in]` / `[canon]` gate flags a change, the operator discusses it in a
chat thread and triggers `@<bot> send it` to revise. Today that executor can loop
indefinitely without converging, for three compounding reasons:

1. **It can revise blind.** On `send it` the executor fetches the thread
   transcript to recover the operator's direction; on any fetch error it logs a
   WARN ("transcript fetch failed (degrading to transcript-less)") and proceeds
   with an EMPTY discussion. The operator still sees "✓ Revising along the
   discussed direction," but there is no direction — the session runs blind.

2. **The marker is frozen at the first finding.** The `.needs-spec-revision.json`
   marker is written once at pre-flight and never updated. Each re-gate's
   newer, more-specific finding is posted ONLY to the chat thread, never written
   back. So the durable record points at the original contradiction, while the
   real, current contradiction lives only in the ephemeral thread — exactly the
   thing that fails to load in (1).

3. **It fixes one contradiction at a time.** The executor resolves the
   contradiction it was told about, re-gates once, and stops; a second
   contradiction the same change always had (often against a different canonical
   requirement) only surfaces on the next round. The operator becomes the loop
   controller for a loop that re-derives from the original deltas each round
   (every failed `send it` discards its work and recreates the branch from base).

Combine these and a transcript-less `send it` re-reads the stale marker,
re-attempts the original fix, and re-fails — forever, silently. The fix is to
make the durable marker — not the ephemeral chat — the source of truth for what
currently contradicts, to refuse to revise when the discussion cannot be read,
and to converge within a single `send it`.

## What Changes

- **The marker carries the current contradiction set.** When a `send it` re-gate
  still finds contradictions, the daemon refreshes `.needs-spec-revision.json`
  with those CURRENT findings (replacing the prior set), recorded with enough
  structure to enumerate each distinct contradiction. The next revision attempt —
  even one that cannot read the thread — is grounded in the current contradiction,
  not the original.

- **Never revise blind.** The executor reads the thread transcript with a bounded
  retry. If it still cannot be read, the executor opens NO PR and reports that it
  could not read the discussion, rather than silently revising against an empty
  thread. (The read-only advisor still answers when degraded — it is low-stakes —
  but surfaces that it could not load the full thread.)

- **Resolve every recorded contradiction.** The executor is grounded in the
  marker's full current finding set and addresses every contradiction it records,
  not only one.

- **Converge within one `send it`.** The executor may re-edit and re-gate up to a
  small bounded number of attempts within a single `send it`, accumulating fixes,
  so a multi-contradiction change is resolved in one trigger. When the same
  conflicting requirement survives the bounded attempts, the report names that
  specific requirement and that the revision is not clearing it, so a persistent
  non-convergence is legible rather than an opaque repeating failure.

## Impact

- Affected capability: `orchestrator-cli` — modifies "Send it in a revision
  thread runs the spec-revision executor" (grounding, fail-closed-on-unreadable-
  thread, resolve-all, bounded converge, escalation) and adds "The spec-revision
  marker carries the current contradiction set."
- Affected code: `autocoder/src/polling/revision_session.rs` (the `send it`
  executor: bounded transcript fetch + fail-closed abort; re-gate returns
  structured findings; marker refresh on re-gate failure; bounded converge loop;
  escalation report) and `autocoder/src/spec_revision.rs` (marker carries
  structured findings). New config under `executor.` for the converge-attempt and
  transcript-retry bounds.
- Complementary to (not overlapping with) `revision-clears-needs-spec-revision-
  marker`, which clears the marker on a successful PR-comment `@<bot> revise`
  (a different flow). And complementary to `contradiction-gates-report-all-
  findings`, which raises how many contradictions the gate surfaces up front —
  with exhaustive gates, the converge loop rarely runs more than once.
