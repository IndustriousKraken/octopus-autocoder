# Executor run log is truncated on every re-invocation (data loss)

## Problem

The executor summary log is written to a per-change path with NO run-id:

```
run_log_path() = <logs>/runs/<workspace-basename>/<change>.log   (claude_cli.rs:1857)
```

and `persist_run_log` (`claude_cli.rs:1875`) writes it with `std::fs::write`, which
TRUNCATES. The JSON-streaming path opens the same `<change>.log` and rewrites it
incrementally. So every time the executor runs for the same change — a doom loop,
an executor retry, a multi-iteration sequence, or a revision — the new run blows
away the previous run's transcript at the moment it starts. An operator who is
`cat`-ing or tailing a run log watches it go to a blank slate; a completed
implementation's transcript (which can represent tens of dollars of tokens) is
lost the instant the next run begins.

This also contradicts canon's own intent: `executor/spec.md:1094` requires a
RECOVERY TURN's output to be **appended** to the per-change run log with a
`=== RECOVERY TURN ===` divider — which is impossible if the writer truncates.
The truncate-on-write is a defect against the append-with-divider contract, not a
deliberate design, so this is a bug to fix in place rather than a canon change
(the path `<change>.log` and the retention pass that targets it stay exactly as
specced in `executor/spec.md:322` and `:381`).

## Desired end state

Re-invoking the executor for a change PRESERVES the prior run(s) in `<change>.log`
instead of erasing them. Each run/turn is appended under a clear divider line
(carrying a timestamp or run counter) so concatenated runs are separable, exactly
mirroring the existing `=== RECOVERY TURN ===` append. The log path
(`<change>.log`) and the retention pass are unchanged, so no spec delta is needed.

## Tasks

- [x] Change `persist_run_log` (`claude_cli.rs:1875`) to APPEND a new run section
  under a divider line (e.g. `=== RUN <utc-timestamp> ===`) rather than
  `std::fs::write` (truncate). Re-locate by function name; line numbers drift.
- [x] Make the JSON-streaming log writer open `<change>.log` in APPEND mode (not
  truncate) and emit the same run-divider at run start, so a fresh streamed run
  does not blank the prior run's content the moment it opens the file.
- [x] Confirm the recovery-turn append (`=== RECOVERY TURN ===`,
  `executor/spec.md:1094`) still lands after the run it belongs to, now that the
  base content is preserved rather than rewritten.
- [x] Confirm the retention pass (`executor/spec.md:381`, `log_retention.rs`) is
  unaffected — it still targets `<change>.log` (and its `.stream.log` sibling) by
  mtime + archived-change check; appended history does not change file identity.
- [x] Test: two consecutive executor runs for the same change leave BOTH
  transcripts present in `<change>.log`, each under its own divider; assert the
  earlier run's marker text survives the second run.

## Constraints (behavior-preserving bug fix, no spec delta)

- Do NOT change the log path or the retention contract — `<change>.log` and the
  per-change retention pass stay exactly as `executor/spec.md:322`/`:381` specify.
  This is an append-vs-truncate fix, nothing more.
- If the operator would rather have per-run FILES (`<change>-<runid>.log`) than one
  appended file, that WOULD touch canon (the `<change>.log` naming + retention) and
  belongs in a separate change — out of scope here.
- Match the surrounding hand-formatting; do NOT run `cargo fmt` (this crate is
  intentionally not rustfmt-clean).
- The unbounded-growth risk from appending across a long doom loop is mitigated by
  the separate push-failure-preserves-work change (which stops the loop); do not
  add a truncation cap here that would re-introduce data loss.
