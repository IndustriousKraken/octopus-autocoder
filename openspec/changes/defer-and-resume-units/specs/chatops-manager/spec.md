## ADDED Requirements

### Requirement: `defer` and `undefer` operator verbs set a unit aside and resume it
The chatops listener SHALL recognize two operator verbs that set a work unit aside without deleting or revising it, and later resume it: `@<bot> defer <repo-substring> <slug>` AND `@<bot> undefer <repo-substring> <slug>`. The repository SHALL be resolved by the SAME selector rule the other operator commands use (case-insensitive substring against the configured repositories, with the same not-found and ambiguous replies). `<slug>` names a single unit; the verb SHALL auto-detect whether it is a change OR an issue by where the unit lives — the operator never spells out which lane.

Both verbs SHALL be dispatched as actions through the existing Unix-domain control socket and replied to in the same channel where the command arrived. Because defer is reversible AND discards no code, both verbs SHALL reply with a SINGLE acknowledgement — there SHALL be no two-step confirmation AND no `defer-confirm` verb. This is the deliberate contrast with the destructive operator commands (`wipe-workspace`, `rollback`), which require a channel-keyed two-step confirm.

The reply SHALL be one line: a `✓` acknowledgement on a performed move OR a no-op success (already-deferred for `defer`, already-active for `undefer`), naming the slug AND the repo; OR a `✗` clear error on a failure (no such unit, ambiguous slug). The verbs SHALL appear in the `@<bot> help` verb list with a one-line description each.

#### Scenario: defer resolves the repo and acknowledges in one step
- **WHEN** an operator posts `@<bot> defer your-repo a06-foo` AND `your-repo` resolves to exactly one configured repository AND `a06-foo` is a unit present in that repo's active lane
- **THEN** the bot resolves the repo, submits a `defer_unit` action to the control socket, AND on success posts a single one-line acknowledgement naming the slug AND the repo
- **AND** the bot does NOT prompt for a confirmation reply AND does NOT store a pending-confirmation entry
- **AND** if `your-repo` matches multiple configured repos, the reply lists the matches AND asks for a more specific substring (the same shape as the other verbs)
- **AND** if no repo matches, the reply lists every configured repo's URL

#### Scenario: undefer resumes a set-aside unit in one step
- **WHEN** an operator posts `@<bot> undefer your-repo a06-foo` AND `a06-foo` is a unit currently set aside in that repo
- **THEN** the bot submits an `undefer_unit` action AND posts a single one-line `✓` acknowledgement
- **AND** no confirmation step is required

#### Scenario: the verb auto-detects change versus issue
- **WHEN** an operator defers a slug that lives in the changes lane, AND separately a slug that lives in the issues lane
- **THEN** the verb detects each unit's kind from where it lives, with no per-lane flag in the command
- **AND** the operator's command text is identical for both kinds (`defer <repo> <slug>`)

#### Scenario: a slug present in neither lane is a clear error
- **WHEN** an operator posts `@<bot> defer your-repo nonesuch` AND no unit named `nonesuch` exists in either lane of the resolved repo
- **THEN** the bot posts a `✗` error stating no change or issue by that slug exists on that repo (informational; not retried)

#### Scenario: a slug naming a unit in both lanes is reported as ambiguous
- **WHEN** an operator defers a slug that names BOTH a change AND an issue in the same repo
- **THEN** the bot posts a `✗` error reporting the ambiguity AND naming both candidate locations, rather than guessing
- **AND** no move is performed

#### Scenario: deferring an already-deferred unit is a no-op success
- **WHEN** an operator defers a slug that is already set aside (absent from its lane, present in the deferred area)
- **THEN** the bot reports the unit is already deferred — a `✓` no-op success, NOT an error AND NOT a second move

#### Scenario: undefer of a unit not set aside is a clear error
- **WHEN** an operator posts `@<bot> undefer your-repo a06-foo` AND no deferred unit by that slug exists
- **THEN** the bot posts a `✗` error stating no deferred change or issue by that slug exists on that repo

#### Scenario: defer and undefer use the same control socket as the other verbs
- **WHEN** either verb's action is performed
- **THEN** the chatops listener submits the action via the existing Unix-domain control socket (the same socket the other operator commands AND `autocoder reload` use)
- **AND** the control socket's existing authn (Unix-socket-perms, daemon-user-only) applies identically

#### Scenario: the verbs appear in help
- **WHEN** an operator posts `@<bot> help`
- **THEN** the verb list includes `defer <repo-substring> <slug>` AND `undefer <repo-substring> <slug>`, each with a one-line description
