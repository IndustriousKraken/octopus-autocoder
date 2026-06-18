## MODIFIED Requirements

### Requirement: Issues lane for corrections
The daemon SHALL provide a second work lane, `issues/`, for corrections — fixes to code that is already correctly specified (bug fixes, behavior-preserving refactors) that carry NO spec delta. An issue SHALL take ONE of two on-disk forms:

- **Single file (the default):** `issues/<slug>.md` — a description of the problem and desired end state, OPTIONALLY followed by a `## Tasks` checklist of the fix steps. This is the form for the common case: a small, curated correction.
- **Directory (when more is needed):** `issues/<slug>/` containing `issue.md` (the report/diagnosis AND acceptance criteria stated against the EXISTING specification) AND `tasks.md` (the fix steps). The directory form is REQUIRED when the unit must carry a separate artifact — in particular a quarantined public report body (see below) — and MAY be used for any issue with attachments.

NEITHER form SHALL contain a `specs/` directory — that absence is the contract that an issue changes no spec; a unit carrying a `specs/` directory is malformed. A public-origin issue (one carrying an untrusted public report body) SHALL use the directory form so the quarantined `report-body.md` stays a separate file from the maintainer-approved task, preserving the quarantine boundary; collapsing an untrusted body into the single-file form is NOT permitted.

The lane SHALL be gated by a `features.issues` flag, off by default. The curated entry path is a maintainer committing an `issues/<slug>.md` (or `issues/<slug>/`) directly (repository write is the allowlist; no public surface). Per-issue markers (`.in-progress` lock, `.perma-stuck.json`, `.ignore-for-queue.json`, `.needs-spec-revision.json`) live INSIDE the directory for a directory-form issue, AND as sibling files for a single-file issue (e.g. `issues/<slug>.perma-stuck.json`); the lane's ready-list treats an `<slug>.md` file OR a `<slug>/` directory as a unit AND ignores marker siblings. On completion the unit SHALL move to `issues/archive/` — `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md`, `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/` — mirroring `changes/archive/`, AND no canonical spec SHALL be modified (the issues lane leaves an audit trail only).

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

#### Scenario: The lane is disabled by default
- **WHEN** `features.issues` is unset
- **THEN** the issues lane is inactive AND neither `issues/<slug>.md` files nor `issues/<slug>/` directories are worked
