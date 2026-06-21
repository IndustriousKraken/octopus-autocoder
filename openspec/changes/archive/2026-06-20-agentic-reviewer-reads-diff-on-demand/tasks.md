# Tasks

The runtime behavior this change specifies is already implemented and merged: the
agentic reviewer provides the unified diff to the session as a readable artifact
rather than inlining it. The remaining work is to fold the corrected delta into
canon (via archive) and confirm the behavior is locked by tests. Each item below
is VERIFIED against the shipped code — do NOT re-implement it (re-deriving the
plumbing would churn the code and trip the `[out]`/stub gates).

## 1. Stop inlining the diff in the agentic prompt

- [x] 1.1 `render_agentic_review_prompt` references the diff-artifact path instead of inlining `ctx.diff` — `code_reviewer.rs:1168-1290` (artifact-reference text at `:1257-1268`); path helper `review_diff_artifact_rel` at `:1146-1153`.
- [x] 1.2 The oneshot prompt path is unchanged (keeps `prompt_budget_chars`-bounded inlining); only the agentic render path references the artifact.

## 2. Write + clean up the diff artifact

- [x] 2.1 The unified diff is written to the sandbox-readable artifact before the session spawns — `CliReviewSessionRunner::run_session`, `code_reviewer.rs:1346-1354`.
- [x] 2.2 The artifact is removed after the session regardless of outcome (success, error, OR timeout): the remove at `:1413` precedes the `?` on the spawn result and the timeout-return at `:1422-1428` — `code_reviewer.rs:1410-1420`.
- [x] 2.3 Per-change mode references a change-scoped artifact (`review_diff_artifact_rel(slug)`); bundled and on-demand paths use one artifact for the diff — per-change at `:2060-2064`, on-demand at `:1920`.

## 3. Tests

- [x] 3.1 `agentic_prompt_lists_paths_and_references_diff_artifact` — the prompt lists the changed-file paths AND references the diff artifact, NOT the inlined diff body (`code_reviewer.rs:4654`).
- [x] 3.2 `agentic_prompt_is_bounded_regardless_of_diff_size` — prompt size is bounded independent of diff size (`code_reviewer.rs:4690`).
- [x] 3.3 Cleanup is verified structurally: the write (`:1349`) precedes the spawn and the remove (`:1413`) runs before any early return, so no diff artifact survives a run on any path. Not covered by an isolated unit test — the write/remove lifecycle lives in the real CLI session runner (`CliReviewSessionRunner::run_session`), which the trait-level tests bypass via a fake `ReviewSessionRunner`. Isolating it would require extracting the lifecycle into a drop-guard (a separate refactor, out of scope for this canon-sync).
- [x] 3.4 Regression: the agentic `ReviewResult` shape and `reviewer.mode` dispatch are unchanged, and the no-submission discard path still fails closed (no `Approve`) — `agentic_no_submission_discards_review` (`:4889`) plus the existing `reviewer.mode` per-change/bundled tests.
