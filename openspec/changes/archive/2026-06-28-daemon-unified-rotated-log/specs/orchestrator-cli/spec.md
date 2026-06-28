## ADDED Requirements

### Requirement: Daemon emits a unified rotated log file
The daemon SHALL write its structured `tracing` event stream to a rotated log file under the logs directory, IN ADDITION to its existing stderr/journal destination. The existing journal output (per "Iteration lifecycle logging") is unchanged; the file is a second sink, so operators have one greppable on-disk place for daemon-level diagnostics alongside the per-session logs under `runs/`.

The file SHALL live in the logs directory (e.g. `<logs>/journal.log`, a sibling of `runs/`). It SHALL honor the same active level filtering as the existing sink (the `RUST_LOG` / default filter), so daemon-level events — INCLUDING the predictable-failure categories (`WorkspaceInitFailure`, `BranchPushFailure`, `PrCreationFailure`) and their underlying error chains — are recorded on disk rather than only in the process journal.

The file SHALL be rotated by size AND/OR age to bound disk usage, retaining a bounded number of rotated segments; the rotation thresholds SHALL be operator-configurable with sane defaults.

#### Scenario: A predictable failure is greppable on disk
- **WHEN** a polling iteration emits a predictable-failure event (e.g. a `BranchPushFailure` carrying the git rejection text)
- **THEN** that event AND its error chain appear in `<logs>/journal.log`
- **AND** the same event still appears in the process journal (the existing sink is unchanged)

#### Scenario: The log rotates and is retained within a bound
- **WHEN** the journal log reaches the configured size or age threshold
- **THEN** it is rotated and a bounded number of prior segments is retained
- **AND** older segments beyond the retention bound are removed so the log cannot grow without limit

#### Scenario: The unified log lives with the per-session logs
- **WHEN** an operator looks for daemon diagnostics
- **THEN** the unified log file is in the logs directory alongside the `runs/` per-session logs, not only in `journalctl`
