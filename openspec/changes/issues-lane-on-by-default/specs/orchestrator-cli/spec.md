## MODIFIED Requirements

### Requirement: Issues lane for corrections
The daemon SHALL provide a second work lane, `issues/`, for corrections — fixes to code that is already correctly specified (bug fixes, behavior-preserving refactors) that carry NO spec delta. An issue SHALL take ONE of two on-disk forms:

- **Single file (the default):** `issues/<slug>.md` — a description of the problem and desired end state, OPTIONALLY followed by a `## Tasks` checklist of the fix steps. This is the form for the common case: a small, curated correction.
- **Directory (when more is needed):** `issues/<slug>/` containing `issue.md` (the report/diagnosis AND acceptance criteria stated against the EXISTING specification) AND `tasks.md` (the fix steps). The directory form is REQUIRED when the unit must carry a separate artifact — in particular a quarantined public report body (see below) — and MAY be used for any issue with attachments.

NEITHER form SHALL contain a `specs/` directory — that absence is the contract that an issue changes no spec; a unit carrying a `specs/` directory is malformed. A public-origin issue (one carrying an untrusted public report body) SHALL use the directory form so the quarantined `report-body.md` stays a separate file from the maintainer-approved task, preserving the quarantine boundary; collapsing an untrusted body into the single-file form is NOT permitted.

The lane SHALL be gated by a `features.issues` flag that is ON by default, because the issues lane is one of the two fundamental work paths; an operator who tracks corrections in an external system SHALL disable it by setting `features.issues.enabled: false`. The curated entry path is a maintainer committing an `issues/<slug>.md` (or `issues/<slug>/`) directly (repository write is the allowlist; no public surface). Per-issue markers (the `.in-progress` lock AND the `.perma-stuck.json` park marker — the only markers the issues lane writes) live INSIDE the directory for a directory-form issue, AND as sibling files for a single-file issue (e.g. `issues/<slug>.in-progress`, `issues/<slug>.perma-stuck.json`); the lane's ready-list treats an `<slug>.md` file OR a `<slug>/` directory as a unit AND ignores marker siblings AND any other non-`.md`, non-directory sibling. On completion the unit SHALL move to `issues/archive/` — `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md`, `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/` — mirroring `changes/archive/`, AND no canonical spec SHALL be modified (the issues lane leaves an audit trail only).

#### Scenario: An enabled lane works a committed issue
- **WHEN** `features.issues` is on AND an `issues/<slug>.md` (OR an `issues/<slug>/` with `issue.md` and `tasks.md`) is present
- **THEN** the issue is selected and worked
- **AND** no spec delta is required for it

#### Scenario: A single-file issue is a valid unit
- **WHEN** `features.issues` is on AND an `issues/<slug>.md` carries a description AND an optional `## Tasks` checklist, with no accompanying `specs/`
- **THEN** it loads as a well-formed issue AND is worked like a directory-form issue
- **AND** its `## Tasks` checklist (when present) is the fix-step list the implementer follows

#### Scenario: An issue carrying a specs directory is rejected
- **WHEN** a directory-form `issues/<slug>/` contains a `specs/` directory
- **THEN** it is rejected as malformed, because an issue carries no spec delta

#### Scenario: A public-origin issue uses the directory form to keep the body quarantined
- **WHEN** a public-origin issue is written (it carries an untrusted public report body)
- **THEN** it uses the directory form `issues/<slug>/` with the body in a separate `report-body.md`, NOT the single-file form
- **AND** the untrusted body is never merged into the same file as the maintainer-approved task

#### Scenario: Completion archives without touching canon
- **WHEN** an issue's fix completes
- **THEN** a single-file issue moves `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md`, AND a directory-form issue moves `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/`
- **AND** no canonical spec file is modified

#### Scenario: The lane is enabled by default
- **WHEN** the config has no `features.issues` entry
- **THEN** the issues lane is active AND `issues/<slug>.md` files and `issues/<slug>/` directories are worked (the schema's default-on representation)

#### Scenario: An operator disables the lane
- **WHEN** the config sets `features.issues.enabled: false`
- **THEN** the issues lane is inactive AND neither `issues/<slug>.md` files nor `issues/<slug>/` directories are worked

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

#### Scenario: Non-interactive explicit disable
- **WHEN** an operator passes `--issues-lane disabled`
- **THEN** the rendered config.yaml contains `features.issues.enabled: false`
