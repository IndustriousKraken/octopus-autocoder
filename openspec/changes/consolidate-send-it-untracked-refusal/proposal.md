# Consolidate the `send it` thread-context dispatch + untracked-thread refusal

## Why

The `send it` thread-context dispatch — the four-set lookup order (audit,
brownfield-survey, issue-candidate, spec-revision) AND the untracked-thread
refusal posted when a reply matches none of them — is currently described in
FOUR separate requirements, each re-deriving the same negative fallback:

- `orchestrator-cli` / `send it verb in an audit thread schedules a triage
  executor run` — owns the VERBATIM refusal text in its "Send-it in untracked
  thread is politely refused" scenario.
- `chatops-manager` / `Inbound listener routes send it to BrownfieldBatchAction
  when posted in a brownfield-survey thread` — restates the full four-set lookup
  prose AND an untracked-thread scenario.
- `chatops-manager` / `Inbound listener routes send it to the spec-revision
  executor when posted in a revision thread` — restates an untracked-thread
  scenario ("names four contexts").
- `chatops-manager` / `Inbound listener routes send it to issue-candidate
  promotion when posted in an issue-candidate thread` — restates the four-set
  lookup prose AND an untracked-thread scenario.

This duplication is exactly why an unrelated UX change
(`send-it-explains-manual-fix-markers`) kept tripping the verifier's `[canon]`
gate: a one-line tweak to the refusal would have had to be applied consistently
across a four-requirement contract. Many of these specs were written before the
verifier existed, so the same fallback got re-described each time a new `send it`
context was added — and a fifth or sixth context would duplicate it again. One
behavior should be specified once.

## What Changes

- ADD one canonical `chatops-manager` requirement — `Inbound listener dispatches
  send it by thread context AND refuses untracked threads` — that is the SINGLE
  owner of (1) the four-set lookup ORDER (audit → brownfield-survey →
  issue-candidate → revision, at most one match), (2) the untracked-thread refusal
  with its VERBATIM text, and (3) the top-level `send it` → `?` fallback.
- SLIM the four per-context routing requirements to their OWN positive branch
  only, each CITING the new dispatcher requirement for the lookup order AND the
  refusal:
  - The `orchestrator-cli` audit requirement drops its "Send-it in untracked
    thread is politely refused" scenario (the verbatim text MOVES to the
    dispatcher requirement); it keeps every audit-specific scenario (tracked-open
    schedules triage, stale, already-acted, TriageFailed re-attempts).
  - The three `chatops-manager` routing requirements drop their restated four-set
    prose AND their duplicate untracked-thread scenarios; each keeps its own
    positive-branch scenarios.
- NET: the untracked-thread refusal text + four-set lookup are specified ONCE.
  No behavior changes — this is a spec-only consolidation; the code already
  implements a single dispatch path.

## Impact

- Affected specs:
  - `chatops-manager` — ONE ADDED requirement (the dispatcher) AND THREE MODIFIED
    requirements (brownfield-survey, spec-revision, issue-candidate routing,
    each slimmed to its positive branch).
  - `orchestrator-cli` — ONE MODIFIED requirement (the audit `send it`
    requirement, slimmed; the verbatim refusal text relocates to the dispatcher).
- No code change is required: the consolidation reflects the single dispatch path
  the listener already runs. The `[out]` gate's coverage is preserved — every
  removed scenario's behavior is now owned by the dispatcher requirement or the
  relevant context's positive branch.
- Independent of `send-it-explains-manual-fix-markers` (which touches the alert
  body for manual-fix markers, not the `send it` routing). The two do not modify
  the same requirement.
