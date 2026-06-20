# The [out] gate fails closed on a missing delta and verifies the same-pass-archived delta

## Why

The `[out]` gate (code-implements-spec verification) was fail-OPEN in its
normal operating condition. autocoder archives a completed change in the SAME
pass — moving `openspec/changes/<slug>/` to `openspec/changes/archive/` and
folding its delta into canon — BEFORE the post-executor `[out]` gate runs. But
the gate resolved its spec-delta files only from the active path
`openspec/changes/<slug>/specs/`, which is empty by then. So the gate found no
delta, ran the agent with a "nothing to verify against" prompt, the agent could
only return `implemented` (the schema forbids `gaps_found` with no gaps to
cite), and the gate rendered a PASS — recording `[verifier:out] PASS` for a
change it never actually verified. A verifier that always passes is worse than
none: it manufactures false assurance.

This is exactly the non-conformance the `gatekeepers-contain-no-judgment`
standard names: the code synthesized a verdict (pass) from empty input instead
of obtaining a genuine agent evaluation or failing closed.

## What Changes

- The `orchestrator-cli` "Code-implements-spec verification (the [out] gate,
  advisory)" requirement is clarified on two points:
  1. **Resolve the delta wherever it lives.** The gate looks for the spec-delta
     at the active path AND, when absent, at the same-pass archived path
     (`openspec/changes/archive/*-<slug>/specs/`), so it verifies the
     just-archived delta instead of finding nothing.
  2. **Fail closed on a missing delta.** When no delta is found in either
     location, the gate does NOT run the agent against an empty contract and
     does NOT synthesize a pass — it renders `## Spec Verification: FAILED TO
     RUN` (could not verify: no spec-delta contract found). Two scenarios pin
     this.

## Impact

- Affected specs: `orchestrator-cli` (MODIFY the [out] gate requirement — all
  existing clauses and scenarios preserved; two scenarios added).
- Affected code: `code_implements_spec.rs` — `spec_delta_paths` resolves the
  archived path too (mirroring `review_context::locate_archive_dir`); the
  orchestration short-circuits to `FailedToRun` when the resolved delta is empty
  (before running the agent), instead of building a "nothing to verify" prompt.
- No change to the advisory posture (annotate, never block) or the verdict
  schema. This closes the fail-open: in the normal case the gate now actually
  verifies the archived delta, and a genuinely missing contract fails visibly.
