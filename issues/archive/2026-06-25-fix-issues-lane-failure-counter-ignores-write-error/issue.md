# fix-issues-lane-failure-counter-ignores-write-error

## Issue

The issues walker silently defeats its own perma-stuck gate when persisting
the consecutive-failure counter fails.

`fail_and_maybe_park` writes the counter and then **re-reads it from disk** to
decide whether to park:

```rust
// autocoder/src/lanes/walker.rs:242-243
let _ = state::record_failure(paths, workspace, slug, &reason);
let count = state::failure_count(paths, workspace, slug);
if count >= perma_stuck_threshold { park_issue(...).await; }
```

`state::record_failure` returns `Result<u32>` and can fail at its
`create_dir_all` or `std::fs::write` (`autocoder/src/lanes/state.rs:78-82`) on a
disk-full (`ENOSPC`), permission, or transient I/O error. Two distinct defects
combine here:

1. **The error is swallowed with `let _ =`** — not even logged. The operator
   gets no signal that the lane's failure-state store is broken.
2. **The park decision reads stale state.** Because the write was discarded,
   `failure_count` re-reads the *unchanged* file — the absent file on the first
   failure (→ `0`) or the prior, un-incremented count thereafter. The
   consecutive-failure counter therefore never advances, so `count` never
   reaches `perma_stuck_threshold`. The issue is re-attempted **forever** and is
   **never parked and never alerted**.

This is a divergence from the changes-lane analogue, which handles the same
situation correctly: `handle_failure_counter`
(`autocoder/src/polling_loop/preflight_checks.rs:592-602`) matches on the
`Result`, logs at WARN on error, and derives the count from the function's own
return value rather than a disk re-read:

```rust
let count = match failure_state::record_failure(paths, workspace, change, reason) {
    Ok(n) => n,
    Err(e) => {
        tracing::warn!(url = %repo.url, change = %change,
            "failed to record consecutive-failure state: {e:#}");
        return;
    }
};
```

## Source location

- `autocoder/src/lanes/walker.rs:242-243` — the `let _ =` discard plus the
  read-after-write in `fail_and_maybe_park`.
- Correct analogue to mirror: `autocoder/src/polling_loop/preflight_checks.rs:592-602`
  (`handle_failure_counter`).
- `record_failure` already returns the new count: `autocoder/src/lanes/state.rs:55-84`.

## Harm

Silent failure (no log) **and** a wrong control-flow decision: a non-progressing
issue is retried indefinitely (token/queue churn) and is never parked under
`issues/<slug>/.perma-stuck.json`, so the operator is never alerted — exactly the
"silently re-attempting an issue" outcome the spec forbids.

## Acceptance criteria (against the EXISTING specification)

The fix makes the code conform to the canonical requirement **"Issues lane parks
a non-progressing issue"** (`openspec/specs/orchestrator-cli/spec.md:7952`),
which states the issues walker "SHALL track a per-issue consecutive-failure
counter", that "A RETRYABLE failure … SHALL increment the counter; the issue is
parked when the counter reaches `executor.perma_stuck_after_failures`", and that
"Parking SHALL be fail-loud, never silent … so the lane is never silently
re-attempting an issue NOR silently abandoning one." No spec change is required —
this is a correction so the code honors that requirement when the state write
fails.

1. `fail_and_maybe_park` uses the count **returned** by `state::record_failure`
   to make the park decision, not a separate `state::failure_count` re-read.
2. When `state::record_failure` returns `Err`, the error is logged at WARN
   (naming `repo.url` and `slug`, mirroring `handle_failure_counter`), and the
   issue is **not** parked on that pass (the threshold cannot be confirmed),
   while the function still returns `IssueStep::Failed { reason }`.
3. On success, behavior is unchanged: the counter advances and the issue is
   parked once `count >= perma_stuck_threshold`, preserving every existing
   scenario in the canonical requirement (threshold park, immediate park on
   escalation/kick-back, no-park on shutdown abort).
