## ADDED Requirements

### Requirement: Documentation-audit chatops notification uses 📚 emoji
The chatops audit-notification surface SHALL emit `documentation_audit` findings with a `📚`-prefixed top-line, parallel to the existing per-audit emoji conventions (`📐` brightline, `🧭` drift, `📋` consultative, `🔍` proposal-created). The notification SHALL use the existing threaded-notification path (top-line in channel, findings body as a thread reply when length warrants).

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
- **AND** shorter findings inline into a single message per the existing length threshold

#### Scenario: Operator can act on findings via `send it`
- **WHEN** an operator replies `@<bot> send it` inside a `documentation_audit` thread that is fresh, tracked, AND open
- **THEN** the existing `audit-reply-acts` mechanism handles the verb (per its existing requirement)
- **AND** the triage produces a doc-fix PR
- **AND** no special-casing of `documentation_audit` is needed in the `send it` handler — the audit's `Reported` outcome surface is identical to other reported-outcome audits
