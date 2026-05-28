## MODIFIED Requirements

### Requirement: Per-repo busy marker prevents concurrent work
autocoder SHALL acquire a per-repo busy marker file at the start of each polling iteration and hold it through every stage of the pass (executor invocation, commit, review, push, PR creation). The marker lives at `<runtime_dir>/busy/<workspace-basename>.json` (resolved per the daemon's path resolver) and is created atomically via POSIX `O_EXCL`. Its presence prevents any other autocoder pass — same daemon or different — from concurrently working on the same repo. Crashes that bypass normal release (SIGKILL, segfault, host power loss, daemon restart mid-iteration) leave the marker behind for the next pass to detect and recover from. Stuck-state recovery SHALL prefer the subprocess-sidecar PGID (set by the executor after spawning Claude) over the marker's own `pgid` field when sending kill signals.

The stale-threshold SHALL be a dedicated `executor.busy_marker_stale_threshold_secs` config field (default `600` seconds, max `7200` with WARN-and-clamp), NOT a derived value from `executor.timeout_secs`. Raising the executor timeout for legitimately long work SHALL NOT proportionally delay stale-marker recovery on unrelated iterations.

Dead-pid recovery (the `Stuck threshold exceeded, PID dead` scenario below) SHALL fire IMMEDIATELY when the marker's recorded `pid` no longer exists in `/proc`, without waiting for the stale-threshold to elapse. A pid that no longer exists cannot be doing legitimate work; the marker is unambiguously stale the moment that's true.

The "busy marker present; skipping iteration" INFO log line SHALL include the marker's age, the resolved `busy_marker_stale_threshold_secs`, the PID-alive state, AND a `recovery_eligible` boolean computed as `!pid_alive || age >= threshold`. Operators reading `journalctl` can see the marker's recovery state inline without reading the marker file separately.

At daemon startup, after resolving both `executor.timeout_secs` AND `executor.busy_marker_stale_threshold_secs`, the daemon SHALL log one INFO line naming both resolved values. If the new threshold field was NOT explicitly set in config AND the pre-spec implicit formula (`timeout_secs + 600`) would have produced a longer threshold, an additional INFO line SHALL name the gap so operators migrating from the pre-spec behavior see the change.

#### Scenario: Acquire on a clean repo
- **WHEN** a polling iteration begins AND no marker file exists at the resolved `<runtime_dir>/busy/<workspace-basename>.json`
- **THEN** the daemon creates the marker via `OpenOptions::new().write(true).create_new(true).open(path)` (atomic against concurrent daemons)
- **AND** the marker contains a JSON document with fields `repo_url`, `pid` (this process's PID), `pgid` (this process's process group ID), `comm` (the value of `/proc/<pid>/comm` at acquire time, on Linux; empty string on other platforms), `started_at` (RFC 3339 UTC timestamp), AND `stage` (initially `"executor"`)
- **AND** the iteration proceeds normally

#### Scenario: Atomic stage transitions
- **WHEN** the iteration moves from one stage to the next (`executor → commit → review → push → pr`)
- **THEN** the daemon updates the marker's `stage` field via a write-to-temp-then-rename sequence so concurrent readers see either the prior stage or the new one, never a partial write
- **AND** stage names are exactly: `executor`, `commit`, `review`, `push`, `pr`

#### Scenario: Release on normal iteration end
- **WHEN** `execute_one_pass` returns (success or any error)
- **THEN** the RAII guard holding the marker drops, and the file is removed
- **AND** the next iteration finds no marker and proceeds normally

#### Scenario: Marker exists, age below stuck threshold
- **WHEN** acquire detects an existing marker AND its `started_at` is less than `executor.busy_marker_stale_threshold_secs` old AND the recorded `pid` is alive in `/proc`
- **THEN** the daemon logs INFO with the enhanced log line including `age`, `threshold`, `pid_alive=true`, `recovery_eligible=false` AND skips this iteration without modifying the marker
- **AND** the polling task continues with its normal sleep + next-iteration cycle

#### Scenario: Stuck threshold exceeded, PID dead
- **WHEN** acquire detects a marker whose recorded `pid` does NOT correspond to a running process (verified via `/proc/<pid>` stat returning `ENOENT`)
- **THEN** the daemon deletes the marker AND the subprocess sidecar file (if present), logs WARN naming the marker's prior contents (so operators see what crashed), AND proceeds to acquire a fresh marker and run the iteration
- **AND** the recovery fires IMMEDIATELY regardless of the marker's age — no age-threshold check applies to this branch
- **AND** this differs from pre-spec behavior, which gated recovery on `age > executor.timeout_secs + 600`, causing repos to remain stuck for up to 100 minutes after daemon restart

#### Scenario: Stuck threshold exceeded, PID alive, comm matches
- **WHEN** acquire detects a marker older than `executor.busy_marker_stale_threshold_secs` AND the recorded `pid` is alive in `/proc` AND the value of `/proc/<pid>/comm` matches the recorded `comm` field (Linux; the comm-check is skipped on non-Linux platforms and the PID liveness check is trusted alone)
- **THEN** the daemon reads the subprocess sidecar file at `<runtime_dir>/busy/<workspace-basename>.subprocess` (if present). If present, the recorded subprocess PID is used as the kill target (its PGID equals its PID because the executor spawns with `process_group(0)`); if absent, the marker's `pgid` field is used as the fallback
- **AND** the daemon sends `SIGTERM` to that process group via `killpg(target_pgid, SIGTERM)`, waits up to 5 seconds for the group to exit, sends `SIGKILL` via `killpg(target_pgid, SIGKILL)` if still alive
- **AND** the daemon deletes the marker AND the subprocess sidecar file, logs WARN with the action taken, attempts to post a chatops alert "repo recovered from stuck state" (best-effort), AND proceeds to acquire a fresh marker and run
- **AND** the iteration proceeds even when no chatops backend is configured

#### Scenario: Stuck threshold exceeded, PID alive, comm differs
- **WHEN** acquire detects a marker older than `executor.busy_marker_stale_threshold_secs` AND the recorded `pid` is alive in `/proc` AND the recorded `comm` field is non-empty AND differs from the live `/proc/<pid>/comm` value
- **THEN** the daemon logs ERROR naming the discrepancy, attempts to post a chatops alert "repo stuck — please investigate" (best-effort), AND SKIPS this iteration without modifying the marker or the subprocess sidecar
- **AND** the marker stays in place for human investigation; the next polling iteration will re-evaluate
- **AND** the iteration is skipped even when no chatops backend is configured (the ERROR log is the operator's only signal in that case)

#### Scenario: Malformed marker JSON
- **WHEN** acquire detects a marker file that cannot be parsed as the expected JSON shape
- **THEN** the daemon logs WARN naming the parse failure, deletes the marker AND the subprocess sidecar (if present), AND proceeds to acquire a fresh one

#### Scenario: Threshold change is independent of `executor.timeout_secs`
- **WHEN** an operator sets `executor.timeout_secs: 5400` AND does NOT explicitly set `executor.busy_marker_stale_threshold_secs`
- **THEN** the resolved threshold is `600` (the default), NOT `6000` (the pre-spec coupled formula)
- **AND** a startup INFO log notes the gap so operators migrating from pre-spec behavior see the change
- **AND** dead-pid markers continue to recover immediately regardless of either value

#### Scenario: Out-of-bounds threshold values are clamped
- **WHEN** an operator sets `executor.busy_marker_stale_threshold_secs: 10000`
- **THEN** the resolved value is `7200` (the max)
- **AND** a WARN log at startup names both the requested and clamped values

#### Scenario: PID-alive check uses `/proc/<pid>` stat
- **WHEN** the classification logic checks whether a pid is alive
- **THEN** the implementation stats `/proc/<pid>` (not signal-0 or other approaches)
- **AND** returns `false` on `ENOENT` (pid does not exist)
- **AND** returns `true` on successful stat
- **AND** on any other error (permission, transient) the implementation treats the pid as "unknown alive" — falling through to the age-based branches rather than incorrectly clearing a possibly-live marker

#### Scenario: Enhanced log line includes age, threshold, pid_alive, recovery_eligible
- **WHEN** any iteration's busy-marker classification produces a "busy marker present; skipping" log line
- **THEN** the line contains `age=<duration>`, `threshold=<duration>`, `pid_alive=<bool>`, AND `recovery_eligible=<bool>` fields
- **AND** the operator can determine from a single log line whether the marker is stale, when recovery will fire, AND whether the pid is alive — without reading the marker file separately
