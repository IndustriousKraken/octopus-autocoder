# Decompose revisions::process_one_pr

## Problem

`autocoder/src/revisions.rs` (~6,500 lines) — `process_one_pr` is a ~900-line
function whose six executor-outcome match arms repeat the same post-processing
shape. This is a maintainability signal (a long function with duplicated arms),
not a defect.

## Desired end state

The repeated post-processing shared across the executor-outcome arms is factored
into a helper so each arm carries only its unique logic; the remaining body is
split along its internal phases into smaller functions, so the orchestration is no
longer one ~900-line function. Outcome semantics and PR outcomes are identical.

## Tasks

- [ ] In `process_one_pr`, factor the repeated post-processing shared across the
  executor-outcome arms (`Completed`, `AskUser`, `Failed`, `PreconditionUnmet`,
  `SpecNeedsRevision`, `IterationRequested`, `Aborted`) into a helper so each arm
  carries only its unique logic. Re-locate via the function NAME — line numbers
  have drifted.
- [ ] Split the remaining body along its internal phases into smaller functions so
  the orchestration is no longer one ~900-line function, keeping the outcome
  semantics identical.
- [ ] Verify: `cargo build` and the existing suite pass; PR outcomes for every
  executor outcome are unchanged.

## Constraints (behavior-preserving refactor)

- No observable contract change — PR outcomes and outcome semantics stay
  identical. This is reorganization, not a feature change. No spec delta.
- Keep public call sites compiling by re-exporting moved items (`pub(crate) use`)
  from their original module path.
- Moved unit tests go to a sibling test module, not a fresh inline
  `#[cfg(test)] mod tests` in the new file.
- Match the surrounding hand-formatting; do NOT run `cargo fmt` (this crate is
  intentionally not rustfmt-clean).
- Do not author or restate any size threshold as a spec requirement — the line
  counts are audit selectors, not contracts.
- Verify against a reliably-green test suite — a behavior-preserving refactor
  checked by a flaky suite proves nothing.
