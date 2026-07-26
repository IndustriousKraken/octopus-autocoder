# orchestrator-cli delta: durable-iteration-record

## ADDED Requirements

### Requirement: Every polling iteration writes a durable iteration record
At the end of EVERY polling iteration — success, idle, skipped, audit-only, or failed — the polling loop SHALL write a per-workspace iteration record at `<state_dir>/iteration-record/<workspace-basename>.json` via atomic tempfile-then-rename, overwriting the previous record. The record SHALL contain: `finished_at` (UTC), an outcome distinguishing at least success-with-work (naming the archived changes and processed issue units), idle (empty queue, nothing to do), skipped (naming the park — open agent-branch PR, queue blocked on a waiting change, push-block resume), audit-only, and failed (with a truncated reason), plus the iteration's wall-clock duration. The write SHALL happen at the single point in the iteration driver where the iteration's result is handled, so both `Ok` and `Err` iterations record exactly once. The write is best-effort observability: a write failure logs WARN and does NOT alter the iteration's outcome.

Status surfaces SHALL source their last-iteration data exclusively from this record. Failure-state entries (the per-change perma-stuck counters) are NOT a proxy for iteration recency and SHALL NOT populate any last-iteration surface. Because idle iterations also stamp the record, a `finished_at` older than the poll interval (plus reasonable in-flight time) is a TRUE signal that the repository's polling task has not completed an iteration since then — the record makes daemon-liveness diagnosable from chatops.

#### Scenario: An idle iteration records
- **WHEN** a polling iteration finds an empty queue, runs no executor, and completes
- **THEN** the iteration record is overwritten with an idle outcome and a fresh `finished_at`

#### Scenario: A failed iteration records
- **WHEN** a polling iteration returns `Err`
- **THEN** the iteration record is overwritten with a failed outcome carrying a truncated reason
- **AND** the record write happens even though the iteration errored

#### Scenario: A working iteration names its work
- **WHEN** an iteration archives changes and/or processes issue units
- **THEN** the record's outcome names them

#### Scenario: A skipped iteration names the park
- **WHEN** an iteration is skipped by the open-PR gate (or blocked on a waiting change, or resumes a push-block hold)
- **THEN** the record's outcome names which park applied

#### Scenario: Record write failure never breaks the iteration
- **WHEN** writing the iteration record fails (permissions, disk)
- **THEN** a WARN log names the failure
- **AND** the iteration's own outcome and subsequent scheduling are unchanged

### Requirement: Orphaned failure-state entries are pruned
A failure-state entry naming a change that no longer exists in the workspace SHALL be pruned: at the start of each pass, after branch sync, the polling loop SHALL remove each `<state_dir>/failure-state/<workspace-basename>/<change>.json` whose change directory is absent from the workspace's active changes, logging one INFO line per removal. Entries for changes whose directories still exist — including marker-excluded ones (perma-stuck, needs-revision) — are retained.

Rationale: a change can complete outside the server's own queue walk (implemented on another machine and pushed, or merged by another host), in which case the server's clear-on-archive never runs for it. The orphaned counter then lingers indefinitely; pruning is safe because the counter's only consumer is perma-stuck detection, which is meaningless for a change that no longer exists.

#### Scenario: An entry for a vanished change is pruned
- **WHEN** a pass starts AND a failure-state file names a change with no directory in the workspace's active changes
- **THEN** the file is removed
- **AND** an INFO line names the pruned change

#### Scenario: Entries for present changes survive, including marker-excluded ones
- **WHEN** a pass starts AND a failure-state file names a change whose directory exists (pending, waiting, perma-stuck, or revision-marked)
- **THEN** the file is retained and the counter semantics are unchanged

#### Scenario: Pruning follows branch sync
- **WHEN** the just-pulled base state contains the archive entry for a change the server never archived itself
- **THEN** that change's directory is absent at prune time and its orphaned failure-state entry is removed on this pass
