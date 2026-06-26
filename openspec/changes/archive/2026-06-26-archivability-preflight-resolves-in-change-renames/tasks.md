# Tasks

## 1. Build the rename-adjusted effective header set

- [x] 1.1 In `spec_archivability::check_spec_deltas_archivable`, for each capability parse the change's own `## RENAMED Requirements` blocks into `(from, to)` pairs.
- [x] 1.2 Compute the per-capability **effective header set** = canonical headers, MINUS every in-change RENAMED `from:` title, PLUS every in-change RENAMED `to:` title. Build it once per capability before evaluating MODIFIED/REMOVED blocks.

## 2. Check MODIFIED/REMOVED against the effective set

- [x] 2.1 Change the MODIFIED precondition: a MODIFIED title is "present" iff it is in the effective header set (so a MODIFIED equal to an in-change RENAMED `to:` whose `from:` is canonical is treated as present and NOT flagged). Keep the exact character-for-character match.
- [x] 2.2 Change the REMOVED precondition identically: a REMOVED title is "present" iff it is in the effective header set.
- [x] 2.3 Leave ADDED unchanged (checked against raw canonical) AND leave RENAMED unchanged (`from:` must be in raw canonical, `to:` must not be in raw canonical).

## 3. Tests

- [x] 3.1 rename+modify passes: a change with RENAMED `from: "A"` `to: "B"` (A canonical, B not) AND MODIFIED `"B"` in the same capability is NOT flagged; the pre-flight returns empty AND the executor is invoked.
- [x] 3.2 rename+remove passes: RENAMED `from: "A"` `to: "B"` AND REMOVED `"B"` is NOT flagged.
- [x] 3.3 plain-missing-still-flagged: a MODIFIED `"C"` with `C` absent from canon AND not any in-change rename `to:` target is still flagged with `kind=Modified` (a07 regression guard).
- [x] 3.4 rename-from-missing-still-flagged: a RENAMED whose `from:` is absent from raw canonical is still flagged with `kind=Renamed` (effective-set logic does not alter the RENAMED `from:` check).
