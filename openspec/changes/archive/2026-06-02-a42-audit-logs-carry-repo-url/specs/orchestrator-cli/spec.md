# orchestrator-cli — delta for a42-audit-logs-carry-repo-url

## ADDED Requirements

### Requirement: Audit-module tracing carries the repository URL as a structured field
Every `tracing::warn!`, `tracing::info!`, AND `tracing::error!` call site under `autocoder/src/audits/` that fires DURING OR AFTER a per-repository audit context is established SHALL include a structured field named `url` whose value is the repository URL the audit is running against (typically `url = %ctx.repo.url` when the function has access to an `&AuditContext`, OR `url = %repo_url` when the function takes the URL as a parameter). The field name SHALL be exactly `url` — matching the convention used by `polling_loop.rs` informational log lines — so operators filtering by repository see a uniform attribution key across audit AND polling code paths.

Truly repository-agnostic tracing calls (e.g., audit-registry initialization at daemon startup, scheduler top-line `no audits configured for any repo` messages) MAY omit `url` ONLY when annotated with a `// no-url: <reason>` comment on the line immediately preceding the macro invocation. The annotation makes the attribution choice explicit AND keeps the regression test self-enforcing for future contributors.

A regression test SHALL scan every `.rs` file under `autocoder/src/audits/` via `std::fs::read_to_string` AND verify every `tracing::(warn|info|error)!` site either contains `url =` in its structured-field set OR is preceded by a `// no-url:` annotation. The test SHALL produce a combined failure listing (NOT first-failure-only) so an operator fixing many sites at once sees every offender in one run.

This requirement applies ONLY to the audit modules (`autocoder/src/audits/*.rs`) AND ONLY to the three log levels named. Other modules (`polling_loop.rs`, `chatops/`, `executor/`) follow their own tracing conventions AND are out of scope for this requirement.

#### Scenario: Validation-failure WARN carries the repo URL
- **GIVEN** a daemon configured with two repositories AND an active `missing_tests_audit` run on the first repository (`https://example.invalid/repo-alpha`)
- **WHEN** the audit produces an invalid proposal AND the validation-rejection WARN fires from `audits/specs_writing.rs`
- **THEN** the log line's structured-field set contains `url=https://example.invalid/repo-alpha`
- **AND** the operator filtering with `journalctl -u autocoder | grep repo-alpha` sees the WARN line
- **AND** the operator filtering with `journalctl -u autocoder | grep repo-beta` does NOT see the WARN line (the second repo's audit run, if any, has its own `url` field)

#### Scenario: Chatops-post-failed WARN carries the repo URL
- **GIVEN** an audit's `ValidationExhausted` chatops notification post errors out
- **WHEN** the WARN at `audits/specs_writing.rs::run_specs_writing_audit` (the chatops-post-failed branch) fires
- **THEN** the log line's structured-field set contains `url=<repo-url>` where the URL is the same one the failed chatops post would have named in its message body

#### Scenario: Shared helper threads the URL through
- **GIVEN** a helper in `audits/mod.rs` that takes `repo_url: &str` as a parameter (e.g., `post_validation_exhausted_notification`)
- **WHEN** that helper's internal tracing call fires
- **THEN** the log line's structured-field set contains `url=<repo_url>` (the helper's parameter, threaded into the tracing call's field set)

#### Scenario: Scheduler-startup tracing without per-repo context is annotated
- **GIVEN** the audit scheduler's startup phase logs `audit registry initialized with N audit types` before any per-repository context exists
- **WHEN** that INFO line fires
- **THEN** the line on the preceding row in source contains `// no-url: registry init runs once at startup, no repo context yet` (OR equivalent reason text)
- **AND** the regression test treats this site as acceptable (the annotation is the escape hatch)

#### Scenario: Regression test catches a new tracing call added without attribution
- **GIVEN** a hypothetical future change adds a `tracing::warn!("something went wrong")` to `autocoder/src/audits/drift.rs` without `url =` AND without a `// no-url:` annotation
- **WHEN** the regression test runs in CI
- **THEN** the test fails with a diagnostic naming `autocoder/src/audits/drift.rs:<lineno>: tracing call missing 'url' field AND no '// no-url:' annotation`
- **AND** the change cannot merge until the contributor either adds the `url` field OR explicitly annotates the call as repo-agnostic
- **AND** the test reports EVERY offending site in one run, not just the first
