## 1. Capture openspec output unconditionally

- [x] 1.1 Change `run_openspec_archive` in `autocoder/src/cli/sync_specs.rs` to return a struct rather than `Result<(), String>`:
  ```rust
  pub struct ArchiveRunOutput {
      pub status: std::process::ExitStatus,
      pub stdout: String,
      pub stderr: String,
  }
  pub fn run_openspec_archive(workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String>;
  ```
  The outer `Err` covers spawn failure only (openspec binary not on PATH, kernel spawn error). Non-zero exit is reported via `status` in the `Ok` variant so the caller can apply post-condition logic uniformly.
- [x] 1.2 Update every existing caller of `run_openspec_archive` to consume the new shape. There is currently only one production caller (the rebuild loop) plus tests.
- [x] 1.3 Helper: a small `format_archive_output_for_report(out: &ArchiveRunOutput) -> String` that produces a one-line-or-paragraph summary suitable for the `ChangeOutcome.failure_reason` field. Includes exit status, the stderr trimmed and truncated via the existing `truncate_for_report` cap, and the stdout trimmed/truncated if stderr was empty (matches the existing prioritize-stderr-over-stdout pattern, but always includes one of them).

## 2. Post-condition verification

- [x] 2.1 Define `fn verify_archive_post_condition(workspace: &Path, slug: &str) -> Result<PathBuf, PostConditionFailure>` returning either the actual archive directory path (the success case) or a structured failure value naming exactly what was wrong:
  ```rust
  pub enum PostConditionFailure {
      ActivePathStillPresent { path: PathBuf },
      NoArchiveEntryFound,
      MultipleArchiveEntriesFound { matches: Vec<PathBuf> },
  }
  ```
  - `ActivePathStillPresent` — `openspec/changes/<slug>/` still exists after the archive call (the silent-skip case observed in production).
  - `NoArchiveEntryFound` — `openspec/changes/<slug>/` is gone (good) but no `openspec/changes/archive/*-<slug>/` exists (data-loss-shaped — should be impossible if openspec is well-behaved, but worth detecting explicitly rather than crashing later).
  - `MultipleArchiveEntriesFound` — more than one directory matches the slug suffix. Means a stale archive from a prior rebuild was not cleaned up. Treat as failure so the operator manually resolves which one is canonical.
- [x] 2.2 The glob match for `archive/*-<slug>/`: read the archive directory entries, filter to those that end with `-<slug>` and whose remaining prefix matches the `^\d{4}-\d{2}-\d{2}-` date pattern. (Other entries that happen to end in `-<slug>` for unrelated reasons — nested sidecars, operator-placed files — are excluded.)
- [x] 2.3 Tests for `verify_archive_post_condition`:
  - Happy path: only `archive/2026-05-25-foo/` exists and `changes/foo/` does not → returns `Ok(PathBuf)` pointing at the matched archive entry.
  - Silent skip: `changes/foo/` exists, no `archive/*-foo/` → returns `Err(ActivePathStillPresent)`.
  - Data-loss: `changes/foo/` does not exist, no `archive/*-foo/` → returns `Err(NoArchiveEntryFound)`.
  - Collision: `archive/2026-05-24-foo/` AND `archive/2026-05-25-foo/` both exist → returns `Err(MultipleArchiveEntriesFound)` with both paths.
  - Date-prefix filter: `archive/foo-foo/` (no date prefix) is ignored; `archive/2026-05-25-foo/` is the only match returned.

## 3. Rollback on failure

- [x] 3.1 Add `fn rollback_to_archive(workspace: &Path, slug: &str, original_name: &str) -> Result<(), std::io::Error>`: moves `openspec/changes/<slug>/` back to `openspec/changes/archive/<original_name>/`. Idempotent against the case where the source doesn't exist (treat-as-success: nothing to roll back). Errors only on actual rename failure or destination-already-exists.
- [x] 3.2 In the rebuild loop, on any failure of step (b) — non-zero exit code OR post-condition failure — call `rollback_to_archive` before recording the `ChangeOutcome` and continuing.
- [x] 3.3 If rollback ITSELF fails (rare: filesystem permission denied, destination already exists somehow), the rebuild SHALL log a CRITICAL with both the original failure and the rollback failure, mark the change as failed with both errors concatenated, and continue. This is a last-resort defensive path; in practice it should not fire, but it must not crash the rebuild.
- [x] 3.4 Tests:
  - Rollback after silent skip: `changes/foo/` exists, post-condition fails, rollback runs → assert `changes/foo/` does NOT exist after rollback, `archive/<original>-foo/` DOES exist.
  - Rollback after non-zero exit: same shape.
  - Rollback against a no-op source (nothing to move): returns Ok, no fs change.
  - Rollback collision (target already exists): returns Err, the rebuild logs CRITICAL and records the combined failure reason.

## 4. Glob-based success path

- [x] 4.1 Replace the existing `today_dated_name` + `today_path` block in the success path with a call to `verify_archive_post_condition`. On `Ok(actual_path)`, rename `actual_path` to `archive_root.join(&original_name)` if `actual_path.file_name() != Some(&original_name)`. (When they already match — happens for changes archived today — skip the rename.)
- [x] 4.2 Keep `today_dated_name` only for test fixtures (mark `#[cfg(test)]` if no production caller remains, or leave public if it has external utility).
- [x] 4.3 Tests:
  - Glob match returns `archive/2026-05-25-foo/`, original name was `2026-05-15-foo` → rename succeeds, final state has `archive/2026-05-15-foo/`.
  - Glob match returns `archive/2026-05-25-foo-2/` (collision suffix) → rename to `2026-05-15-foo` succeeds.
  - Glob match returns `archive/2026-05-15-foo/` (no rename needed) → no fs rename call; outcome marked successful.

## 5. Rebuild-loop integration + report shape

- [x] 5.1 Update the per-change block in the rebuild loop to the new contract:
  1. Move `archive/<original>/` to `changes/<slug>/` (existing step (a), unchanged).
  2. Call `run_openspec_archive` and capture `ArchiveRunOutput`.
  3. If exit non-zero: call `rollback_to_archive`, record `ChangeOutcome { success: false, failure_reason: format_archive_output_for_report(&out) }`, continue.
  4. If exit zero: call `verify_archive_post_condition`.
     - On `Err(_)`: call `rollback_to_archive`, record `ChangeOutcome { success: false, failure_reason: format!("openspec archive exited 0 but post-condition failed: {reason}; openspec output: {output}") }`, continue.
     - On `Ok(actual_path)`: if needed, rename to original_name. Record success.
- [x] 5.2 The `RebuildReport` and its printed summary continue to list per-change outcomes; the `failure_reason` strings now contain openspec's actual output rather than the misleading "date-prefix restore failed" message.
- [x] 5.3 The summary line printed at the end of the rebuild gains a clause when rollbacks happened: `"<N> change(s) rolled back to archive due to silent-skip or post-condition failure — see per-change reasons above"`. Operators reading just the summary know whether to dig in.

## 6. Integration test against a stubbed archive runner

- [x] 6.1 Refactor the rebuild's openspec invocation to go through a trait `trait ArchiveRunner { fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String>; }` so tests can inject a stub without spawning real subprocesses. Production uses a `RealArchiveRunner` that calls `Command::new("openspec")` as today.
- [x] 6.2 `SilentSkipArchiveRunner` test stub: exits 0, prints `"would archive <slug>"` to stdout, performs no fs work. Used to reproduce the silent-skip scenario deterministically in tests.
- [x] 6.3 End-to-end test using `SilentSkipArchiveRunner`: build a fixture workspace with 3 archived changes, run the rebuild with the stub, assert:
  - The rebuild's report lists all 3 as failed with `failure_reason` containing `"would archive"`.
  - All 3 archive directories are back in `openspec/changes/archive/` with their original date prefixes.
  - `openspec/changes/` is empty (no active-path leakage).
- [x] 6.4 End-to-end test using a mixed stub (succeeds for some slugs, silent-skips for others): assert per-change outcomes match the stub's behavior and that rollback fires exactly for the silent-skipped ones.

## 7. Spec delta

- [x] 7.1 The ADDED requirement in `openspec/changes/sync-specs-rebuild-atomicity/specs/orchestrator-cli/spec.md` codifies: post-condition verification (the two assertions), rollback contract (failed-archive restores the source), output-capture contract (openspec's stdout/stderr always included in failure reasons), and the glob-based success path. Scenarios cover the four post-condition outcomes (happy / silent-skip / data-loss / collision), the rollback path, and the collision-suffix glob match.

## 8. Verification

- [x] 8.1 `cargo test` passes (new + existing).
- [x] 8.2 `openspec validate sync-specs-rebuild-atomicity --strict` passes.
- [x] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
