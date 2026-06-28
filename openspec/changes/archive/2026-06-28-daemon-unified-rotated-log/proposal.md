# Daemon emits a unified, rotated log file

## Why

The daemon's most expensive failure modes are the hardest to diagnose because
their detail lives ONLY in the process journal. A branch-push rejection, for
example, is emitted via `tracing` (→ journald) and a throttled chatops alert, but
is never written to a discoverable log file under the logs directory — so an
operator grepping `~/.local/state/autocoder/logs/runs/` finds nothing and must
fall back to `journalctl -u autocoder`, which is awkward to search and easy to
miss. Per-session logs (executor, gates, reviewer, audits) live under the logs
directory, but the daemon's own structured event stream does not. Logs should be
in one place.

## What Changes

The daemon SHALL ALSO write its structured `tracing` output to a rotated log file
under the logs directory, in addition to its existing stderr/journal destination
(the journal output is unchanged). The file:

- lives alongside `runs/` in the logs directory (e.g. `<logs>/journal.log`),
- captures the daemon's structured events at the active level — including the
  predictable-failure details (workspace init, branch push, PR creation) that are
  currently journal-only — so a push rejection is greppable from disk,
- rotates by size and/or age to bound disk usage, retaining a bounded number of
  rotated segments, with operator-configurable thresholds and sane defaults.

This is purely additive: journalctl visibility (per "Iteration lifecycle
logging") is unchanged; the rotated file is a second sink.

## Impact

- One greppable place for daemon-level diagnostics, alongside the per-session logs.
- Adds one `orchestrator-cli` requirement ("Daemon emits a unified rotated log
  file"); no existing requirement changes.
- Makes the push-failure reason discoverable on disk, complementing the
  push-failure-preserves-completed-work change.
