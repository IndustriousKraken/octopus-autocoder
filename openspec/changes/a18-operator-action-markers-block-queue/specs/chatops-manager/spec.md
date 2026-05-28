## MODIFIED Requirements

### Requirement: Help verb returns the verb list
The dispatcher SHALL recognize `@<bot> help` (case-insensitive) as a verb and return `Some(Reply::Sync(text))` where `text` enumerates every currently-supported verb, its syntax, and a one-line description, plus a one-line pointer to the README's confirmation-flow section for the destructive verbs.

#### Scenario: help returns a multi-line synopsis
- **WHEN** `handle_message("@<bot> help", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))`
- **AND** `text` contains the strings `status`, `clear-perma-stuck`, `clear-revision`, `ignore-and-continue`, `clear-ignore`, `wipe-workspace`, `rebuild-specs`, AND `help` (the current verb set)

#### Scenario: help is case-insensitive
- **WHEN** `handle_message("@<bot> HELP", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))` matching the lowercase form

## ADDED Requirements

### Requirement: `ignore-and-continue` and `clear-ignore` verbs manage the `.ignore-for-queue.json` marker
The chatops dispatcher SHALL recognize `@<bot> ignore-and-continue <repo-substring> <change-slug>` AND `@<bot> clear-ignore <repo-substring> <change-slug>` (both case-insensitive on the verb). The verbs manage the `.ignore-for-queue.json` marker introduced by `a18`'s orchestrator-cli requirement.

`ignore-and-continue` writes the marker file inside the named change's directory AND commits/pushes the change. The verb refuses with a polite error when the named change has no underlying blocking marker (`.perma-stuck.json` OR `.needs-spec-revision.json`) — stamping ignore on a change with no problem is a confusing no-op.

`clear-ignore` removes the marker file AND commits/pushes the removal. The verb refuses with a polite error when no `.ignore-for-queue.json` exists for the named change.

#### Scenario: `ignore-and-continue` happy path
- **WHEN** the operator runs `@<bot> ignore-and-continue myrepo a07-foo`
- **AND** `myrepo` unambiguously resolves to a configured repository
- **AND** the change `a07-foo` has `.perma-stuck.json`
- **THEN** the daemon writes `<workspace>/openspec/changes/a07-foo/.ignore-for-queue.json` with the documented schema
- **AND** commits the file AND pushes to the agent branch (commit subject `chore: ignore-for-queue on a07-foo (operator <id>)`)
- **AND** the chatops reply: `✓ Marked a07-foo as ignored for queue. Subsequent changes will process; a07-foo stays excluded until the underlying marker is cleared.`

#### Scenario: `ignore-and-continue` rejects when no underlying marker exists
- **WHEN** the operator runs `@<bot> ignore-and-continue myrepo a07-foo`
- **AND** the change `a07-foo` has NEITHER `.perma-stuck.json` NOR `.needs-spec-revision.json`
- **THEN** the daemon refuses with: `✗ a07-foo has no operator-action marker (perma-stuck OR needs-spec-revision). Ignore is a no-op; rejecting to prevent confusion.`
- **AND** no file is written

#### Scenario: `clear-ignore` happy path
- **WHEN** the operator runs `@<bot> clear-ignore myrepo a07-foo`
- **AND** the change `a07-foo` has `.ignore-for-queue.json`
- **THEN** the daemon removes the file AND commits/pushes the removal (`chore: clear ignore-for-queue on a07-foo`)
- **AND** the chatops reply: `✓ Cleared ignore-for-queue on a07-foo. Queue resumes blocking on <original-marker>.`

#### Scenario: `clear-ignore` rejects when no marker exists
- **WHEN** the operator runs `@<bot> clear-ignore myrepo a07-foo`
- **AND** the change `a07-foo` has no `.ignore-for-queue.json`
- **THEN** the daemon refuses with: `✗ a07-foo has no .ignore-for-queue.json marker.`

### Requirement: Status reply annotates ignore-for-queue marker alongside the blocking marker
The `@<bot> status` reply's "active markers" section (when present) SHALL annotate every line whose change has BOTH a blocking marker AND `.ignore-for-queue.json` with the trailing text `(ignore-for-queue: yes — queue not blocked)`. Changes whose blocking markers are unaccompanied by ignore-markers get no annotation.

#### Scenario: Status annotates an ignored-blocked change
- **WHEN** an operator runs `@<bot> status myrepo`
- **AND** the workspace has change `a07-foo` with BOTH `.perma-stuck.json` AND `.ignore-for-queue.json`
- **AND** change `a09-bar` with `.needs-spec-revision.json` alone
- **THEN** the status reply's "active markers" section contains:
  ```
  active markers:
    a07-foo: .perma-stuck.json (ignore-for-queue: yes — queue not blocked)
    a09-bar: .needs-spec-revision.json (blocking queue)
  ```

#### Scenario: No annotation when no ignore-marker exists
- **WHEN** the workspace has only blocking markers AND no `.ignore-for-queue.json` files
- **THEN** the active-markers section names each marker without the annotation
- **AND** the trailing "(blocking queue)" hint MAY be appended for clarity (implementation choice — the spec doesn't mandate the hint, only the ignore-for-queue annotation)
