## ADDED Requirements

### Requirement: Chatops `audit` verb queues an on-demand audit run for the next polling iteration
The chatops listener SHALL recognize `@<bot> audit <audit-substring> <repo-substring>` as the `AuditNow` command. The audit-substring SHALL be matched case-insensitively against the registered audit-type names by substring (same rule the repo-substring uses against configured repository URLs). The repo-substring SHALL be matched per the existing repo-substring rules. On a unique match in both, the dispatcher SHALL submit a `queue_audit` control-socket action AND post a one-line ack naming the resolved audit-type and repo URL. On ambiguous or no-match, the dispatcher SHALL reply with the candidate list (mirroring the existing `match_repo` reply shapes).

#### Scenario: Unique substring matches queue the audit
- **WHEN** an operator posts `@<bot> audit sec myrepo` AND `sec` uniquely matches `security_bug_audit` AND `myrepo` uniquely matches a configured repo URL
- **THEN** the dispatcher submits a `queue_audit` action with both resolved names
- **AND** the bot posts a threaded reply whose first line is `✓ Queued security_bug_audit for <repo_url>. Will run on the next polling iteration (~Nm).` (where `~Nm` is the per-repo poll interval rounded to minutes, OR `imminently` when the next iteration is <30 seconds away)

#### Scenario: Ambiguous audit substring lists candidates
- **WHEN** an operator posts `@<bot> audit arch myrepo` AND `arch` matches both `architecture_brightline` and `architecture_consultative`
- **THEN** the bot replies `✗ audit substring \`arch\` matches multiple: architecture_brightline, architecture_consultative. Be more specific.`
- **AND** no audit is queued

#### Scenario: Unknown audit substring lists all registered names
- **WHEN** an operator posts `@<bot> audit gibberish myrepo`
- **THEN** the bot replies `✗ no audit matched \`gibberish\`; registered: architecture_brightline, architecture_consultative, drift_audit, missing_tests_audit, security_bug_audit.`
- **AND** no audit is queued

### Requirement: Queued audit runs bypass cadence on the next iteration
The audit scheduler SHALL, at the start of each iteration's audit-scheduling phase, drain the `pending_audit_runs` queue for the repo AND run each queued audit-type unconditionally (regardless of cadence or `last_run` timestamp). After running, the audit's `last_run` timestamp SHALL be updated as if it were a cadence-driven run. Cadence-driven scheduling continues to fire for audit types NOT already run via the queue in this iteration.

#### Scenario: Queued audit runs even when cadence says not due
- **WHEN** a repo's `pending_audit_runs` contains `security_bug_audit` AND `security_bug_audit`'s cadence says "not due for 28 more days"
- **THEN** the audit runs in this iteration
- **AND** its `last_run` timestamp is updated to the current time
- **AND** the cadence-based "next scheduled fire" effectively moves forward by the cadence interval from the new `last_run` (no double-run within the cadence window)

#### Scenario: De-duplicated queue entries produce one run
- **WHEN** the same audit-type appears in `pending_audit_runs` more than once for a single iteration
- **THEN** the audit runs exactly once in that iteration
- **AND** subsequent appearances of the same audit-type in the queue are no-ops

#### Scenario: Queue is drained after the iteration
- **WHEN** an iteration runs queued audits AND completes
- **THEN** the repo's `pending_audit_runs` is empty
- **AND** a subsequent iteration without new queue entries does NOT re-run those audits (cadence resumes)

#### Scenario: Cadence-driven audits coexist with queued audits in the same iteration
- **WHEN** an iteration has queued `security_bug_audit` AND cadence-due `drift_audit`
- **THEN** both audits run in the iteration
- **AND** the queue-drained audits run first, then the cadence-due audits

### Requirement: CLI `audit run` subcommand triggers on-demand from the command line
The `autocoder` CLI SHALL expose `audit run --workspace <path> --audit <name>` as a subcommand. The subcommand SHALL probe for the control socket at the resolved runtime path; when the socket is reachable, the subcommand sends the same `queue_audit` action a chatops `audit` verb would submit. When the socket is NOT reachable, the subcommand runs the audit standalone against the named workspace path AND prints the audit's findings to stdout.

#### Scenario: CLI talks to the running daemon when the socket is present
- **WHEN** the autocoder daemon is running on the host AND `autocoder audit run --workspace <path> --audit security_bug_audit` is invoked AND the workspace matches a repo the daemon is managing
- **THEN** the CLI connects to the control socket
- **AND** submits `queue_audit` with the resolved audit-type and repo URL
- **AND** prints the daemon's ack response to stdout
- **AND** exits 0

#### Scenario: CLI runs standalone when no daemon is present
- **WHEN** no autocoder daemon is running on the host AND `autocoder audit run --workspace <path> --audit security_bug_audit` is invoked
- **THEN** the CLI invokes the audit module directly against the workspace path
- **AND** prints the audit's findings to stdout
- **AND** exits 0 on successful audit, non-zero on audit failure

#### Scenario: CLI errors when daemon is running but workspace is not managed
- **WHEN** the daemon is running AND the named workspace is NOT in the daemon's configured repo list
- **THEN** the CLI prints a clear error naming the workspace path and the daemon's known repos
- **AND** exits non-zero
- **AND** does NOT fall back to standalone mode (the daemon is the owner of the workspace lifecycle when present; falling back would race the daemon)
