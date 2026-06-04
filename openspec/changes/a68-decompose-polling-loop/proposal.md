## Why

`autocoder/src/polling_loop.rs` is **17,943 lines** — the project's worst structural-bloat offender and the file that motivated the a67 size budget. An audit established the defect is *reachable bloat*, not a graveyard: `cargo check` reports zero unused items and there are no `#[allow(dead_code)]` escapes. The file is thirteen distinct responsibilities in one module (alert posting, queue walking, waiting-change handling, pre-flight checks, review-context assembly, PR construction, rebuild iteration, audit-triage, proposals, outcome handling), its largest function (`run_with_hooks`) is **571 lines**, and a family of ~12 near-identical `maybe_post_*` alert helpers collapses to one parameterized helper. Its `#[cfg(test)] mod tests` block runs from line ~7488 to EOF — **~10,456 lines, 58% of the file** — and ~90 of those tests assert literal alert / notification / PR-body / marker **message wording**, which the existing `Tests assert behavior or derivation, never message wording` requirement (a48) forbids.

a67 makes the auditors flag this file and sets the budget; a68 remediates it. The wording-test purge folds in because the decomposition relocates *every* test anyway — pruning them in the same pass is strictly cheaper than chasing them across the new module layout afterward.

## What Changes

**Decompose the production half into single-responsibility submodules.** `polling_loop.rs` becomes a directory module (`polling_loop/`) whose submodules each own one responsibility — alerts, queue walking, waiting-change handling, pre-flight, review-context assembly, PR construction (open + body), rebuild, audit-triage, proposals, outcome handling — leaving a ~1,000-line orchestration core (`run` / `run_with_hooks` / `execute_one_pass` / `run_pass_through_commits`). The seams already exist as free functions, so this is relocation, not redesign. **Behavior is identical** — no production logic changes, and the module's public surface is unchanged.

**Collapse the near-identical alert families.** The ~12 `maybe_post_*` helpers are two copy-paste families (a per-comment-dedup family and a throttle family) that collapse into two parameterized helpers, removing the duplicated 9-step skeletons, the two byte-identical `35_000` thread-cap constants, and the duplicated truncation strings — the exact duplication the a67 duplicate-body detector flags. Behavior-preserving.

**Split oversized functions.** A function over the function budget (e.g. the 571-line `run_with_hooks`) is split along its internal phases rather than left as one body.

**Relocate tests to a sibling module and prune the wording-assertion tests.** The inline `#[cfg(test)] mod tests` moves to a sibling `#[path]` test module (safe: the suite uses `super::*`, crate-private items, and the `test_hooks` override, all of which a `#[path]` sibling preserves). During the move, the ~90 tests that assert hand-authored message substrings are deleted or rewritten to assert behavior / derivation per a48; the genuine behavioral and boundary tests are kept.

**Codify the decomposition.** A new `project-documentation` requirement states that the polling-loop orchestration is decomposed by responsibility, within budget, with tests not inline — so the architecture-brightline / drift / consultative audits keep it that way rather than letting it re-accrete.

## Impact

- **Affected specs:** `project-documentation` — ADD `The orchestrator polling loop is decomposed by responsibility`.
- **Affected code:** `autocoder/src/polling_loop.rs` → `autocoder/src/polling_loop/` (orchestration core + responsibility submodules); a sibling `#[path]` test module; the `maybe_post_*` alert families collapse into parameterized helpers. No public-API change; no behavior change.
- **Behavior:** none — this is a pure refactor plus the removal of wording-assertion tests. All surviving tests pass unchanged.
- **Dependencies:** processes **after a67** — references a67's `Source files and functions stay within a size budget` standard and the existing a48 `Tests assert behavior or derivation, never message wording` rule. No symbol dependency (validates standalone); the ordering is logical, and the same-repo queue is strict-ordered so a67 lands first.
- **Acceptance:** `cargo test` passes (surviving suite); `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a68-decompose-polling-loop --strict` passes; AND a behavior-preservation check — the diff changes no production control flow (only relocates code and collapses provably-equivalent duplicates), and the module's externally-called functions keep their signatures.
