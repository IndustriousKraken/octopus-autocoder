# fix-issues-lane-failure-state-nonatomic-write

## Issue

The issues-lane consecutive-failure counter is persisted with a **non-atomic**
write, so a crash mid-write silently resets the counter and defeats the
perma-stuck gate.

`state::record_failure` serializes the counter entry and writes it with a direct
open-truncate-then-write:

```rust
// autocoder/src/lanes/state.rs:81-82
let raw = serde_json::to_string_pretty(&entry)?;
std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
```

`std::fs::write` truncates the destination before writing the new contents. If
the daemon is killed during the write — power loss, OOM, or this daemon's own
SIGKILL cascade against a stuck session — the on-disk
`<state>/issues-state/<repo-basename>/<slug>.json` is left **truncated**.

On the next pass, both readers treat a corrupt file as count `0`:

- `failure_count` (`autocoder/src/lanes/state.rs:45-50`):
  `serde_json::from_str::<IssueFailureEntry>(&raw).map(|e| e.count).unwrap_or(0)`.
- `record_failure` (`autocoder/src/lanes/state.rs:62-73`):
  `serde_json::from_str::<IssueFailureEntry>(&raw).unwrap_or(IssueFailureEntry { count: 0, .. })`.

So the consecutive-failure history is lost and the counter restarts from `0`.
A genuinely non-progressing issue then never reaches
`executor.perma_stuck_after_failures` and is silently re-attempted forever,
never parked, never alerted.

This is a divergence from the module's own stated contract. `state.rs:11` says
"Shape mirrors `crate::failure_state`", but `crate::failure_state::save_entry`
(`autocoder/src/failure_state.rs:154-159`) writes **atomically** via a temp file
in the destination directory plus an atomic rename:

```rust
let tmp = tempfile::NamedTempFile::new_in(parent)?;
serde_json::to_writer_pretty(&tmp, entry)?;
tmp.persist(&path).map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
```

The same atomic temp-then-rename pattern is used across the codebase's other
counter/marker stores (`failure_state`, `alert_state`, `busy_marker`,
`iteration_pending`). `state.rs` is the lone outlier doing a direct
`std::fs::write`.

## Source location

- `autocoder/src/lanes/state.rs:81-82` — the non-atomic `std::fs::write` in
  `record_failure`.
- Correct analogue to mirror: `autocoder/src/failure_state.rs:142-161`
  (`save_entry`).

## Harm

State corruption on crash: a torn `<slug>.json` parses as count `0`, resetting
the consecutive-failure counter. The perma-stuck gate never fires for that
issue, which is then silently re-attempted indefinitely — the outcome the spec
forbids.

## Acceptance criteria (against the EXISTING specification)

The fix makes the code conform to the canonical requirement **"Issues lane parks
a non-progressing issue"** (`openspec/specs/orchestrator-cli/spec.md:7952`),
which requires the issues walker to "track a per-issue consecutive-failure
counter" and increment it on retryable failures until the threshold, and that
"the lane is never silently re-attempting an issue." A counter that a torn write
can silently reset violates that requirement. No spec change is required — this
is a durability correction that preserves all observable behavior.

1. `state::record_failure` persists the entry **atomically**: serialize to a
   temp file in the same parent directory, then atomically rename/persist it onto
   `<slug>.json`, matching `crate::failure_state::save_entry`.
2. A crash between the temp write and the rename leaves the previous valid
   `<slug>.json` intact — there is no window in which `<slug>.json` is truncated
   or partially written.
3. Round-trip behavior is unchanged: `record_failure` returns the incremented
   count and `failure_count` reads it back (existing
   `record_increments_and_clear_resets` test continues to pass).
