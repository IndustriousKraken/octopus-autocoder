# Tasks

## 1. Resolve the delta at the archived path too

- [x] 1.1 In `code_implements_spec.rs::spec_delta_paths`, for each change slug resolve the `specs/<cap>/spec.md` delta files from the active path `openspec/changes/<slug>/specs/` AND, when that directory is absent, from the same-pass archived path `openspec/changes/archive/*-<slug>/specs/` (mirror the archive-locating used by `review_context::locate_archive_dir` / `lanes::issues::locate_archive_dir`). Prefer the active path when present; fall back to the lexically-highest archive match. The returned paths are what the prompt lists for the agent to `Read`.

## 2. Fail closed when no delta is found

- [x] 2.1 In the gate orchestration (`run_code_implements_spec_check_with_runner`), compute the resolved delta paths BEFORE building the prompt / running the session. When the set is EMPTY, do NOT run the agent and do NOT build a "nothing to verify against" prompt — return `SpecVerificationOutcome::FailedToRun { cause }` naming the cause (no spec-delta contract found to verify against). Remove (or make unreachable) the prior empty-delta "nothing to verify against" prompt branch that fed the agent a contract-less prompt.
- [x] 2.2 Confirm the caller renders the `FailedToRun` outcome as the `## Spec Verification: FAILED TO RUN` section (existing behavior) AND records the ledger verdict as failed-to-run (not pass), so a missing delta is visible and non-passing.

## 3. Tests

- [x] 3.1 A processed change whose delta exists only at the archived path (`openspec/changes/archive/<dated>-<slug>/specs/`) resolves its delta files and is verified (not treated as empty).
- [x] 3.2 A processed change with NO delta in either location yields `FailedToRun` WITHOUT invoking the session runner (assert the runner is not called, e.g. via a runner that panics/records if called), and renders FAILED TO RUN — never a synthesized `implemented`.
- [x] 3.3 The existing diff-based verification tests (delta present at the active path) still pass unchanged.
