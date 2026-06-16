## MODIFIED Requirements

### Requirement: Issues lane for corrections
The daemon SHALL provide a second work lane, `issues/`, for corrections — fixes to code that is already correctly specified (bug fixes, behavior-preserving refactors) that carry NO spec delta. An issue SHALL be a directory `issues/<slug>/` containing `issue.md` (the report and diagnosis AND the acceptance criteria stated against the EXISTING specification) AND `tasks.md` (the fix steps), with NO `specs/` directory — that absence is the contract that an issue changes no spec. The lane SHALL be gated by a `features.issues` flag that is ON by default, because the issues lane is one of the two fundamental work paths; an operator who tracks corrections in an external system SHALL disable it by setting `features.issues.enabled: false`. The curated entry path is a maintainer committing `issues/<slug>/` directly (repository write is the allowlist; no public surface). On completion the issue directory SHALL move to `issues/archive/`, mirroring `changes/archive/`, AND no canonical spec SHALL be modified (the issues lane leaves an audit trail only).

#### Scenario: An enabled lane works a committed issue
- **WHEN** `features.issues` is on AND an `issues/<slug>/` with `issue.md` and `tasks.md` is present
- **THEN** the issue is selected and worked
- **AND** no spec delta is required for it

#### Scenario: An issue carrying a specs directory is rejected
- **WHEN** an `issues/<slug>/` contains a `specs/` directory
- **THEN** it is rejected as malformed, because an issue carries no spec delta

#### Scenario: Completion archives without touching canon
- **WHEN** an issue's fix completes
- **THEN** `issues/<slug>/` moves to `issues/archive/`
- **AND** no canonical spec file is modified

#### Scenario: The lane is enabled by default
- **WHEN** the config has no `features.issues` entry
- **THEN** the issues lane is active AND `issues/<slug>/` directories are worked (the schema's default-on representation)

#### Scenario: An operator disables the lane
- **WHEN** the config sets `features.issues.enabled: false`
- **THEN** the issues lane is inactive AND `issues/<slug>/` directories are not worked

### Requirement: Install wizard configures the issues lane
The `autocoder install` wizard SHALL prompt operators about the issues lane during first-time install, after the periodic-audits prompts AND before the config-assembly step, as a single yes/no gate defaulting to YES (keep the lane on). Because the issues lane is one of the two fundamental work paths — where behavior-preserving corrections, including audit-found implementation fixes, are worked — it is ON by default; an operator who tracks corrections in an external system (Jira, Linear, and similar) may opt out. Enabling the lane makes per-iteration unit selection `issues > changes > audits`; autonomous triage of open GitHub issues remains separately gated by `features.scout.include_issues` AND is NOT turned on by the issues lane, so default-on introduces no autonomous public-issue behavior. The prompt body SHALL state these effects so the operator decides informed. The wizard SHALL write `features.issues.enabled: false` to config.yaml ONLY when the operator opts out; keeping the default SHALL write no `features.issues` entry, matching the schema's default-on representation. The non-interactive mode SHALL mirror the gate with a `--issues-lane <enabled|disabled>` flag whose default (`enabled`) matches the interactive default.

#### Scenario: Default interactive path keeps the issues lane on
- **WHEN** an operator runs `autocoder install` AND accepts the issues-lane default (bare-Enter on the gate → keep on)
- **THEN** the rendered config.yaml contains no `features.issues` entry
- **AND** the issues lane is on (the schema's default-on representation)

#### Scenario: Operator opts out interactively
- **WHEN** the operator answers to disable the issues lane
- **THEN** the rendered config.yaml contains `features.issues.enabled: false`

#### Scenario: Non-interactive default keeps the issues lane on
- **WHEN** an operator runs `autocoder install --non-interactive` with all required flags AND no `--issues-lane` flag
- **THEN** the rendered config.yaml contains no `features.issues` entry
- **AND** the issues lane is on (the default); an install that omits the flag gains the active lane

#### Scenario: Non-interactive explicit enable
- **WHEN** an operator runs `autocoder install --non-interactive --issues-lane enabled` with all other required flags
- **THEN** the rendered config.yaml contains `features.issues.enabled: true`

#### Scenario: Non-interactive explicit disable
- **WHEN** an operator passes `--issues-lane disabled`
- **THEN** the rendered config.yaml contains `features.issues.enabled: false`
