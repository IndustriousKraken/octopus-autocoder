# Tasks

## 1. Add the rotated file sink

- [x] 1.1 Add a `tracing` file-appender sink to the daemon's subscriber that writes
  the structured event stream to `<logs>/journal.log`, in ADDITION to the existing
  stderr/journal sink (do not remove or alter the existing sink).
- [x] 1.2 The file sink SHALL honor the active log level (the same `RUST_LOG` /
  default filtering as the existing sink), so predictable-failure events
  (`WorkspaceInitFailure`, `BranchPushFailure`, `PrCreationFailure`) and their
  underlying error chains land in the file.

## 2. Rotation + retention

- [x] 2.1 Rotate the file by size and/or age, retaining a bounded number of rotated
  segments so the log cannot fill the disk. Thresholds operator-configurable with
  sane defaults.
- [x] 2.2 Place the file under the logs directory alongside `runs/` so it is found
  with the per-session logs.

## 3. Tests

- [x] 3.1 A simulated `BranchPushFailure` (or any predictable-failure event) is
  present in `<logs>/journal.log` after it is logged.
- [x] 3.2 Rotation triggers at the configured threshold and old segments are pruned
  to the retention bound.

## Constraints

- Additive only — the existing journalctl/stderr sink and "Iteration lifecycle
  logging" behavior are unchanged.
- Match the surrounding hand-formatting; do NOT run `cargo fmt`.
