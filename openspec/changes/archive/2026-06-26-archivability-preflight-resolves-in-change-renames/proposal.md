# Archivability pre-flight resolves in-change renames before checking MODIFIED/REMOVED

## Why

The spec-delta archivability pre-flight (`autocoder/src/preflight/spec_archivability.rs`)
checks a change's `## MODIFIED`/`## REMOVED` headers against CURRENT canon. But
`openspec archive` applies a change's `## RENAMED Requirements` BEFORE its MODIFIED
and REMOVED blocks (rename-then-modify), so a change that renames `from: "A"`
`to: "B"` AND modifies `"B"` in the same change is perfectly archivable — the
rename creates `B`, then the modify lands on it (verified: `openspec archive`
succeeds and canon ends up titled `B`). Meanwhile `openspec validate --strict`
REJECTS a MODIFIED block that still uses the old name `A` with "MODIFIED
references old name from RENAMED. Use new header" — so the MODIFIED block MUST use
the new name `B`.

These two openspec rules form a deadlock against the current pre-flight: the
pre-flight checks MODIFIED `"B"` against raw canon (which still has `A`, not `B`),
finds no `B`, and flags `kind=Modified reason="header not found in canonical ..."`.
There is NO delta structure that satisfies both `openspec validate --strict` (must
say `B`) and the current pre-flight (would only accept `A`, which validate
rejects). The pre-flight is therefore wrong: it permanently false-flags EVERY
rename+modify change. Such changes are held by `.needs-spec-revision.json`; clearing
the marker just re-flags them on the next iteration. This currently blocks the
real-world `human-revise-cap-opt-in` change, which is correct but false-flagged.

## What Changes

- MODIFY the `Spec-delta archivability pre-flight check` requirement so the MODIFIED
  and REMOVED header preconditions are checked against a rename-adjusted **effective
  header set** = canonical headers, MINUS every in-change RENAMED `from:` title,
  PLUS every in-change RENAMED `to:` title. A MODIFIED/REMOVED title that equals an
  in-change RENAMED `to:` (whose `from:` is canonical) is treated as present and is
  NOT flagged.
- The ADDED and RENAMED preconditions are UNCHANGED: ADDED is checked against raw
  canon; RENAMED `from:` must be in raw canon, RENAMED `to:` must not be in raw
  canon (the rename creates it).
- The a07 protection is retained: a MODIFIED/REMOVED title that is neither in raw
  canon NOR an in-change rename `to:` target is still flagged.

## Impact

- Affected specs: `orchestrator-cli` (the modified pre-flight requirement).
- Affected code: `spec_archivability::check_spec_deltas_archivable` — it builds the
  per-capability effective header set from the change's own `## RENAMED` blocks
  before evaluating MODIFIED/REMOVED preconditions against it. RENAMED and ADDED
  evaluation is unchanged.
- Unblocks the real-world `human-revise-cap-opt-in` change (correct but currently
  false-flagged) and every future rename+modify change.
- Aligns the pre-flight with `openspec archive`'s actual rename-then-modify behavior;
  no other requirement is touched.
