## ADDED Requirements

### Requirement: Status reply surfaces issues-lane state
When the issues lane is enabled for a repository, the `status` verb's reply SHALL include an issues-lane section alongside the existing changes-lane queue/marker sections, sourced from the same `repo_status` data path: the READY units (by slug, in selection order), the LOCKED units (slug plus lock age), AND the PARKED units (slug plus the park marker's marked-at time and last-reason detail when the marker carries one) — so an operator can see from chat why an issue is not being picked up, without access to the server workspace where the markers live. When the lane is enabled and `issues/` holds no units in any state, the section renders as a one-liner (e.g. `issues: 0 ready`); when the lane is disabled for the repository, the section is omitted entirely.

The section SHALL degrade like the reply's other sections: a failure reading the issues directory or a marker file logs a WARN and renders the affected entry (or section) as unavailable — it never breaks the rest of the reply.

#### Scenario: A parked issue is visible with its reason
- **WHEN** an operator issues `status <repo>` against a repo whose `issues/<slug>/` carries a `.perma-stuck.json` marker
- **THEN** the reply's issues-lane section lists `<slug>` as parked with the marker's marked-at time and its detail when present
- **AND** the changes-lane sections render exactly as before

#### Scenario: A locked issue is visible with its lock age
- **WHEN** an issue unit carries an `.in-progress` lock at status time
- **THEN** the issues-lane section lists the slug as locked with the lock's age

#### Scenario: Ready issues are listed in selection order
- **WHEN** two or more issue units are ready
- **THEN** the issues-lane section lists them alphabetically (the lane's selection order), so the operator can predict which runs next

#### Scenario: An empty enabled lane renders a one-liner
- **WHEN** the issues lane is enabled AND the repository has no issue units in any state
- **THEN** the issues-lane section renders as a one-liner (e.g. `issues: 0 ready`)

#### Scenario: A disabled lane omits the section
- **WHEN** `features.issues.enabled` is `false` for the repository's daemon
- **THEN** the status reply contains no issues-lane section

#### Scenario: A marker read failure degrades the entry, not the reply
- **WHEN** a park marker's JSON is unreadable at status time
- **THEN** the daemon logs a WARN
- **AND** the issue still appears as parked with its detail rendered as unavailable
- **AND** every other section of the reply renders normally
