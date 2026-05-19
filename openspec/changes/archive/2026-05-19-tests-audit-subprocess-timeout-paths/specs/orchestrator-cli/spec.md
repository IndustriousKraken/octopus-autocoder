## ADDED Requirements

### Requirement: Periodic audits enforce their per-audit subprocess timeout
Every audit that spawns the wrapped agent CLI as a child process (`drift_audit`, `architecture_consultative_audit`, `missing_tests_audit`, `security_bug_audit`) SHALL kill the child and return `Err(_)` once the elapsed wall-clock time exceeds `executor.timeout_secs`. The error message SHALL name both the audit type and the timeout condition so the operator can tell from a single log line which audit hung and why. The audit log file SHALL record the timeout outcome before the error returns so post-mortem inspection of `/tmp/autocoder/logs/<basename>/audits/<audit_type>-<ts>.log` is conclusive.

#### Scenario: drift_audit subprocess exceeds timeout
- **WHEN** `DriftAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured `executor.command` is a script that sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose `format!("{err:#}")` contains the substring `drift_audit` AND the substring `timeout`
- **AND** the audit log file written via the audit's `AuditLogWriter` contains a `kind: Err` section together with the substring `reason: timeout`
- **AND** the spawned child process does not survive past the call's return (no orphaned `sleep` left behind)

#### Scenario: architecture_consultative_audit subprocess exceeds timeout
- **WHEN** `ArchitectureConsultativeAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured command sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose message contains `architecture_consultative` AND `timeout`
- **AND** the audit log file contains a `kind: Err` / `reason: timeout` section

#### Scenario: specs-writing audit (via missing_tests) subprocess exceeds timeout
- **WHEN** `MissingTestsAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured command sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose message contains `missing_tests_audit` AND `timeout`
- **AND** no new directory is created under `<workspace>/openspec/changes/` as a side-effect of the timed-out run (defense-in-depth against the spec-writing audit's commit step running on a child that never finished)
