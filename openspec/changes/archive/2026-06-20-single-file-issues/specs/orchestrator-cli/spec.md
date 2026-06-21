## MODIFIED Requirements

### Requirement: Issues lane for corrections
The daemon SHALL provide a second work lane, `issues/`, for corrections — fixes to code that is already correctly specified (bug fixes, behavior-preserving refactors) that carry NO spec delta. An issue SHALL take ONE of two on-disk forms:

- **Single file (the default):** `issues/<slug>.md` — a description of the problem and desired end state, OPTIONALLY followed by a `## Tasks` checklist of the fix steps. This is the form for the common case: a small, curated correction.
- **Directory (when more is needed):** `issues/<slug>/` containing `issue.md` (the report/diagnosis AND acceptance criteria stated against the EXISTING specification) AND `tasks.md` (the fix steps). The directory form is REQUIRED when the unit must carry a separate artifact — in particular a quarantined public report body (see below) — and MAY be used for any issue with attachments.

NEITHER form SHALL contain a `specs/` directory — that absence is the contract that an issue changes no spec; a unit carrying a `specs/` directory is malformed. A public-origin issue (one carrying an untrusted public report body) SHALL use the directory form so the quarantined `report-body.md` stays a separate file from the maintainer-approved task, preserving the quarantine boundary; collapsing an untrusted body into the single-file form is NOT permitted.

The lane SHALL be gated by a `features.issues` flag, off by default. The curated entry path is a maintainer committing an `issues/<slug>.md` (or `issues/<slug>/`) directly (repository write is the allowlist; no public surface). Per-issue markers (the `.in-progress` lock AND the `.perma-stuck.json` park marker — the only markers the issues lane writes) live INSIDE the directory for a directory-form issue, AND as sibling files for a single-file issue (e.g. `issues/<slug>.in-progress`, `issues/<slug>.perma-stuck.json`); the lane's ready-list treats an `<slug>.md` file OR a `<slug>/` directory as a unit AND ignores marker siblings AND any other non-`.md`, non-directory sibling. On completion the unit SHALL move to `issues/archive/` — `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md`, `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/` — mirroring `changes/archive/`, AND no canonical spec SHALL be modified (the issues lane leaves an audit trail only).

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

### Requirement: Issues lane parks a non-progressing issue
The issues lane SHALL NOT re-attempt the same issue indefinitely. The issues walker SHALL track a per-issue consecutive-failure counter (its own lane state, per the independent-lane-walkers requirement) AND, once an issue stops making progress, SHALL PARK it: write a `.perma-stuck.json` marker for the issue — INSIDE `issues/<slug>/` for a directory-form issue, OR as the sibling `issues/<slug>.perma-stuck.json` for a single-file issue — exclude the issue from selection while the marker is present, AND post an operator-visible chatops alert. The threshold SHALL be the existing `executor.perma_stuck_after_failures` value (no new configuration). The operator unparks an issue by removing the marker, exactly as for a parked change.

Progress is defined by outcome:
- A RETRYABLE failure (executor error, a `Completed` outcome that left the workspace unmodified, an unsupported iteration request, OR a precondition-unmet outcome) SHALL increment the counter; the issue is parked when the counter reaches `executor.perma_stuck_after_failures`.
- An outcome that retrying cannot resolve — the agent escalating a question (the issues lane does not escalate) OR the agent kicking the fix back to the changes lane (it requires a behavior change) — SHALL park the issue IMMEDIATELY (a single attempt, not the full threshold). Immediate parking on kick-back also stops the kick-back notice from re-posting on every pass.
- A daemon-shutdown abort SHALL NOT count toward the threshold (operator-initiated shutdown is not an issue failure).

Parking SHALL be fail-loud, never silent: the chatops alert names the issue, the attempt count, AND the last reason, so the lane is never silently re-attempting an issue NOR silently abandoning one. Completion (the fix landed AND the issue archived) SHALL clear both the counter AND the marker, so a later issue reusing the slug starts clean. The park marker SHALL be gitignored regardless of issue form: the `.git/info/exclude` set SHALL match BOTH the in-directory `.perma-stuck.json` AND the single-file sibling `issues/<slug>.perma-stuck.json` (a suffix pattern such as `*.perma-stuck.json`, because a bare-basename exclude matches only the in-directory form), so the marker is gitignored at any depth AND survives the per-iteration branch reset AND `git clean` in either form.

#### Scenario: A repeatedly failing issue is parked after the threshold
- **WHEN** an issue's fix fails on `executor.perma_stuck_after_failures` consecutive passes
- **THEN** a `.perma-stuck.json` marker is written for the issue (inside `issues/<slug>/` for a directory issue, OR as the sibling `issues/<slug>.perma-stuck.json` for a single-file issue)
- **AND** an operator-visible chatops alert names the issue, the attempt count, AND the last reason

#### Scenario: A parked issue is skipped until the operator removes the marker
- **WHEN** an issue carries a `.perma-stuck.json` marker (inside `issues/<slug>/`, OR as the sibling `issues/<slug>.perma-stuck.json`)
- **THEN** the issue is excluded from selection (it is not worked)
- **AND** removing the marker makes the issue selectable again

#### Scenario: A single-file issue's sibling park marker is gitignored
- **WHEN** a single-file issue `issues/<slug>.md` is parked with a sibling `issues/<slug>.perma-stuck.json` marker
- **THEN** the marker is gitignored — the exclude set matches the sibling form, not only the in-directory `.perma-stuck.json`
- **AND** it does not appear as an untracked change in the pre-pass dirty check, AND it survives `git clean` AND the per-iteration branch reset

#### Scenario: An escalated issue is parked immediately
- **WHEN** the agent escalates a question while working an issue
- **THEN** the issue is parked on that single attempt (not after the full threshold)
- **AND** the operator is alerted

#### Scenario: A kicked-back issue is parked immediately and not re-reported
- **WHEN** the agent reports that an issue requires a behavior change (a kick-back to the changes lane)
- **THEN** the issue is parked on that single attempt
- **AND** the kick-back notice is not re-posted on subsequent passes

#### Scenario: A daemon-shutdown abort does not count toward the threshold
- **WHEN** an issue's session is aborted by the daemon's shutdown cascade
- **THEN** the issue's consecutive-failure counter is not incremented
- **AND** the issue is not parked for the abort

#### Scenario: Completion clears the counter and the marker
- **WHEN** an issue's fix completes AND the issue is archived
- **THEN** the per-issue failure counter is cleared
- **AND** no `.perma-stuck.json` marker remains for that slug (in either form)

### Requirement: Hybrid issue ingestion with maintainer promotion
The daemon SHALL ingest reported issues without giving public authors the ability to trigger code work. It SHALL triage reported GitHub issues read-only (reusing scout's issue read), classify AND dedup each against open AND archived issues, draft a candidate `issues/<slug>/`, AND post the candidate to chatops WITHOUT queuing it. A maintainer SHALL promote a candidate with a "send it" (reusing the audit send-it pattern); ONLY on promotion does the daemon write the issue unit AND queue it. The public can REPORT but SHALL NOT TRIGGER code work — promotion is the authorization gate. The curated path (a009) is this path minus the auto-triage step.

The candidate notification SHALL be posted in a way that a later promotion reply can be matched to it: the daemon SHALL capture the posted message's `thread_ts` AND `channel` AND persist them on the candidate's stored state. A candidate whose thread was not captured (a degraded post) is simply not matchable by a reply — graceful degradation, never an error. The notification SHALL instruct the maintainer to reply `@<bot> send it` (the mention form that the verb recognizes), retaining the statement that nothing is written OR queued until they do.

Promotion SHALL be performed by a control-socket action reachable from the `send it` dispatcher. The action SHALL resolve the matched candidate AND write the issue unit in the form appropriate to its origin: a CURATED candidate (carrying no untrusted body) as the default single file `issues/<slug>.md` (a description plus an optional `## Tasks` checklist); a PUBLIC-ORIGIN candidate as the directory form `issues/<slug>/` (its `issue.md` AND `tasks.md`, plus the quarantined `report-body.md`) so the untrusted body stays a separate file from the maintainer-approved task. The action SHALL flip the candidate's status to promoted; writing the unit IS the queue (the issues-lane walker picks up any ready issue unit). The action SHALL be idempotent: an already-promoted candidate writes nothing further AND reports that it is already promoted.

#### Scenario: A triaged report posts a candidate and queues nothing
- **WHEN** a reported issue is triaged
- **THEN** a candidate `issues/<slug>/` is drafted and posted to chatops
- **AND** nothing is written to `issues/` or queued

#### Scenario: Promotion writes and queues
- **WHEN** a maintainer "send it"s a posted candidate
- **THEN** the daemon writes the issue unit in the form appropriate to its origin (a single file `issues/<slug>.md` for a curated candidate, OR `issues/<slug>/` for a public-origin candidate)
- **AND** queues it for the issues lane

#### Scenario: A curated candidate is promoted as a single file
- **WHEN** the promotion action runs for a curated candidate (carrying no untrusted body)
- **THEN** the daemon writes the single file `issues/<slug>.md` (description plus an optional `## Tasks` checklist), NOT a directory
- **AND** the written unit is ready for the issues-lane walker

#### Scenario: An unpromoted candidate does no work
- **WHEN** a candidate is posted but no maintainer promotes it
- **THEN** no issue is written or queued

#### Scenario: Duplicates are deduped
- **WHEN** a report duplicates an open or an archived issue
- **THEN** it is deduped AND no candidate is queued

#### Scenario: The candidate notification is matchable and instructs the mention form
- **WHEN** a candidate is posted to chatops
- **THEN** the posted message's `thread_ts` AND `channel` are persisted on the candidate's stored state
- **AND** the notification instructs the maintainer to reply `@<bot> send it`

#### Scenario: The promotion action writes, queues, and flips status
- **WHEN** the promotion control-socket action runs for a posted candidate
- **THEN** the daemon writes the issue unit in the form appropriate to origin (a single file `issues/<slug>.md` for a curated candidate, OR `issues/<slug>/` including the quarantined `report-body.md` for a public-origin candidate)
- **AND** the candidate's stored status becomes promoted
- **AND** the written unit is ready for the issues-lane walker

#### Scenario: The promotion action is idempotent
- **WHEN** the promotion control-socket action runs for a candidate that is already promoted
- **THEN** no further filesystem write is performed
- **AND** the action reports that the candidate is already promoted
