## MODIFIED Requirements

### Requirement: Audit top-line uses per-type emoji and audit-specific summary
The top-line of each audit notification SHALL be formatted per audit type so operators can scan the channel and immediately recognize the audit producing each message:

- `architecture_advisor`: `🏛 architecture_advisor on <repo>: <N> refactor recommendation(s)`
- `drift_audit`: `🧭 drift_audit on <repo>: <N> spec/code divergence(s) detected`
- The proposal-creating audits (`missing_tests_audit`, `security_bug_audit`) use the `🔍 created proposal` form from `a02-audit-proposal-created-notification` (unchanged by this requirement; their notifications are already concise and do not need threading).

When an audit has zero findings AND `notify_on_clean=true`, the top-line is `✅ <audit_type> on <repo>: no findings` (uniform across audit types).

#### Scenario: Advisor summary names the recommendation count
- **WHEN** an `architecture_advisor` notification fires with 3 refactor recommendations
- **THEN** the top-line is `🏛 architecture_advisor on <repo>: 3 refactor recommendation(s)`

#### Scenario: Drift summary names the divergence count
- **WHEN** a `drift_audit` notification fires with 2 divergences detected
- **THEN** the top-line is `🧭 drift_audit on <repo>: 2 spec/code divergence(s) detected`

#### Scenario: No-findings top-line uses the `✅` form uniformly
- **WHEN** any audit fires with zero findings AND `notify_on_clean=true`
- **THEN** the top-line is `✅ <audit_type> on <repo>: no findings` regardless of audit type

### Requirement: Documentation-audit chatops notification uses 📚 emoji
The chatops audit-notification surface SHALL emit `documentation_audit` findings with a `📚`-prefixed top-line, parallel to the existing per-audit emoji conventions (`🏛` advisor, `🧭` drift, `🔍` proposal-created). The notification SHALL use the existing threaded-notification path (top-line in channel, findings body as a thread reply when length warrants).

#### Scenario: Top-line format
- **WHEN** `documentation_audit` returns `Reported(findings)` with non-empty findings
- **THEN** the chatops top-line reads `📚 documentation_audit on <repo-url>: <N> finding(s)`
- **AND** the threaded body lists findings grouped by category (`Coverage`, `Stale references`, `Organization`)
- **AND** each finding renders as `- <severity> at <anchor>: <body>` (one-line per finding; long bodies wrap)

#### Scenario: Clean run honors `notify_on_clean`
- **WHEN** `documentation_audit` returns `Reported(vec![])` AND `notify_on_clean: true`
- **THEN** the chatops post reads `✅ documentation_audit on <repo-url>: no findings`
- **WHEN** `notify_on_clean: false` (the default) AND the audit returns `Reported(vec![])`
- **THEN** no chatops post fires (silence is success, consistent with other audits)

#### Scenario: Findings body uses the existing threaded path
- **WHEN** `documentation_audit` produces findings whose total body exceeds 3 lines OR 300 characters
- **THEN** the chatops post routes through the threaded-notification path (per the existing `Audit findings post via the threaded-notification path` requirement) — top-line in channel, body in thread reply

## REMOVED Requirements

### Requirement: Brightline chatops top-line admits a stale-ignore-cleanup clause

**Reason:** The stale-ignore clause described the `.brightline-ignore` validation
surfaced by the `architecture_brightline` `📐` top-line. Both the brightline
audit and the `.brightline-ignore` file are removed by this change, so the clause
has no producer. `architecture_advisor` posts a recommendation-count top-line and
maintains no ignore file.
