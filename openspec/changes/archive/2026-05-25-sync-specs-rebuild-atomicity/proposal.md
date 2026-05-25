## Why

`autocoder sync-specs --rebuild` walks archived changes chronologically and for each one (a) moves the archived directory back to the active path `openspec/changes/<slug>/`, (b) runs `openspec archive <slug> -y`, then (c) restores the original date prefix on the resulting archive directory. Step (b) is treated as authoritative: if `openspec archive` exits 0, the rebuild assumes the archive happened and proceeds to (c).

That assumption is wrong. In an observed real-world rebuild against a 41-change archive, 7 changes failed in a way the operator had to dig into source to understand:

- `openspec archive <slug> -y` exited 0
- The change directory was **not** moved out of `openspec/changes/<slug>/`
- `archive/<today>-<slug>/` did not appear
- Autocoder's step (c) ENOENT'd looking for the never-created archive directory
- The change was reported as failed with the misleading message "openspec archive succeeded but date-prefix restore failed"
- The change was left in `openspec/changes/<slug>/` — an active-path location it did not occupy before the rebuild started, polluting the operator's workspace and (in daemon mode) queueing 7 unintended changes for the next polling iteration to *implement*

The rebuild PR generated for the operator contained 7 active-change additions alongside the 30 canonical-spec syncs. Merging that PR would have cascaded into the daemon picking up 7 changes that should have stayed archived.

Three failures compounded:

1. **`openspec archive` exited 0 without archiving.** Plausible root causes include a delta-vs-canonical conflict that openspec treats as a skip rather than an error, a stacked-dependency ordering issue where the change references a not-yet-synced earlier requirement, or an upstream openspec bug. The captured operator output had no diagnostic because of (2).
2. **Autocoder swallowed openspec's stdout/stderr on exit-0.** The current implementation only surfaces openspec's output when the exit code is non-zero. The skip-reason that would have explained (1) was discarded.
3. **Autocoder did not verify the post-condition.** No check that `changes/<slug>` was actually moved out or that `archive/*-<slug>` actually appeared. Exit code was treated as ground truth.

The combined effect is a destructive partial: the rebuild moved data out of `archive/` into `changes/` in step (a) and never put it back. The "leave failing changes at the active path for inspection" semantics in the original rebuild spec assumed the failure happened cleanly between iterations — it did not anticipate a silent-skip path that leaves the workspace in a state worse than before the rebuild started.

## What Changes

**Always capture and surface openspec's output.** `run_openspec_archive` returns the captured stdout and stderr alongside the exit status, not just on the non-zero path. The rebuild logs the captured output at INFO when the call succeeds-with-output (typical openspec behavior: it prints a one-line confirmation on success that's useful even when nothing went wrong) and at ERROR when post-condition verification subsequently fails (this is the diagnostic that explains a silent skip).

**Verify the post-condition before declaring success.** After `openspec archive <slug> -y` returns exit 0, the rebuild SHALL assert two things: (i) `openspec/changes/<slug>/` no longer exists, AND (ii) at least one directory matching the glob `openspec/changes/archive/*-<slug>/` does exist. Both checks SHALL hold for the call to count as a successful archive. If either fails, the call is treated as failed regardless of openspec's exit code.

**Roll back on post-condition failure.** If the post-condition check fails, the rebuild SHALL move the directory back from `openspec/changes/<slug>/` to its original location `openspec/changes/archive/<original_name>/`, restoring the workspace to the state it was in before the change was processed. The rebuild SHALL then record the change as failed (with the captured openspec output as the failure reason) and continue to the next change. The operator sees one consolidated report of failed changes; their working tree is not contaminated with active-path artifacts from a partial rebuild.

**Observe instead of guess for the success path.** When the post-condition is satisfied, the rebuild SHALL find the actual archive directory via the glob `openspec/changes/archive/*-<slug>/` (exactly one match per slug after step (a) removed the previous archive entry) and rename that directory to the original date-prefixed name. This replaces the current "guess that openspec used `<today-UTC>-<slug>`" path, which is fragile to local-timezone differences, openspec collision suffixes, and any future change to openspec's naming format.

## Impact

- **Affected specs:** `orchestrator-cli` — one ADDED requirement covering the atomic per-change semantics (post-condition verification + rollback + output capture + glob-based date-prefix restore). Composes cleanly with the existing rebuild semantics whenever the prior "Rebuild canonical specs from archive" requirement (still only in archive, not yet canonical) gets backfilled by a sync-specs run.
- **Affected code:**
  - `autocoder/src/cli/sync_specs.rs` — change `run_openspec_archive` to return `(ExitStatus, stdout, stderr)` rather than `Result<(), String>`. Update the rebuild loop to verify the post-condition, perform rollback on failure, and use a glob match for the success-path rename. Replace the existing `today_dated_name` guess with the glob match. Keep `today_dated_name` only for use in test fixtures that need a synthetic name.
  - Tests:
    - Mock openspec invocations via a controlled spawn helper (or extract `run_openspec_archive` to take an injectable `dyn ArchiveRunner` for testability). Fixtures cover: exit-0-with-move (happy path), exit-0-without-move (silent skip — assert rollback restores the archive entry), exit-non-zero (failure — assert rollback runs), exit-0-with-collision-suffix (archive name is `2026-05-25-<slug>-2` instead of `2026-05-25-<slug>` — assert glob match still finds it).
    - Rollback assertion: after a silent-skip failure, `openspec/changes/<slug>/` does NOT exist, `openspec/changes/archive/<original>/` DOES exist, and a fresh `git diff` against the pre-rebuild snapshot shows no change for this entry.
    - Output capture assertion: openspec stdout/stderr is included verbatim in the `ChangeOutcome.failure_reason` when post-condition fails, truncated only by the existing `truncate_for_report` cap.
- **Operator-visible behavior:** a rebuild that hits silent-skip failures produces a PR containing only the changes that actually archived successfully. Failures are reported with openspec's actual output (so the operator can see WHY openspec skipped each one). The active path is never contaminated with leftover directories.
- **Breaking:** no. The change is purely a tightening of behavior; successful archives continue to land in the same place. The misleading "date-prefix restore failed" error message disappears and is replaced with openspec's actual output.
- **Acceptance:** `cargo test` passes (new + existing). A rebuild against a fixture archive that includes a "silent-skip" change (simulated via a stubbed archive runner that exits 0 but does no fs work) produces a report listing that change as failed AND leaves `openspec/changes/archive/<original>/` intact AND leaves `openspec/changes/` clean of the failed slug.
