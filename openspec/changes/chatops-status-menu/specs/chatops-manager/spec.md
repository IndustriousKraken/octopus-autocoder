## ADDED Requirements

### Requirement: Bare `status` returns the per-repo menu
The chatops dispatcher SHALL recognise `@<bot> status` with no arguments as the `StatusMenu` command and SHALL return a `Sync` reply containing a one-line announcement plus one two-line section per configured repository. The existing `@<bot> status <repo-substring>` SHALL continue to behave as the per-repo deep-dive. Argument count after the verb token is the disambiguator: zero args → `StatusMenu`; one arg → `Status { repo_substring }`; two or more args → the existing "invalid" error.

#### Scenario: Bare status produces the menu reply
- **WHEN** an operator posts `@<bot> status` (no further arguments) in an allowlisted channel
- **THEN** the dispatcher returns `Some(Reply::Sync(text))` whose first line is `📊 Watching <N> repositories. Reply \`@<bot> status <repo-substring>\` for details.`
- **AND** the reply contains one section per configured repository

#### Scenario: Status with a substring still works
- **WHEN** an operator posts `@<bot> status myrepo`
- **THEN** the dispatcher returns the existing per-repo `Sync` reply
- **AND** the dispatcher does NOT return the menu reply

#### Scenario: Trailing whitespace and casing tolerated
- **WHEN** an operator posts `@<bot> Status   ` (trailing whitespace; verb in mixed case)
- **THEN** the message parses as `StatusMenu` and the menu reply is returned

#### Scenario: Empty configured-repos slice
- **WHEN** the daemon has zero configured repositories AND an operator posts `@<bot> status`
- **THEN** the reply is `📊 No repositories configured.`

### Requirement: Menu reply renders queue, busy, and last-iteration clauses per repo
Each section of the menu reply SHALL render the repo URL on its own line and a summary line containing three clauses joined by ` · `: a queue clause, a busy clause, and a last-iteration clause. Empty / zero values render as documented placeholders rather than blank fields. User-controlled fields (change names) pass through the Slack-escape helper before assembly.

#### Scenario: Idle empty-queue repo renders the empty-queue collapse
- **WHEN** a repo has zero pending, zero waiting, zero excluded, no busy marker, and a last iteration 5m ago
- **THEN** the summary line reads `empty queue · idle · last iteration 5m ago`

#### Scenario: Busy repo with pending entries
- **WHEN** a repo has 2 pending (`a06-foo`, `a07-bar`), 0 waiting, 0 excluded, busy marker on `a05-foo` started 2m ago, last iteration just now
- **THEN** the summary line reads `2 pending (a06-foo, a07-bar), 0 waiting, 0 excluded · working on a05-foo (started 2m ago) · last iteration just now`

#### Scenario: Pending-list truncates after 5 entries
- **WHEN** a repo has 7 pending entries (`a01`, `a02`, `a03`, `a04`, `a05`, `a06`, `a07`)
- **THEN** the queue clause renders `7 pending (a01, a02, a03, a04, a05 …+2 more)`

#### Scenario: Fresh daemon with no iteration history
- **WHEN** a repo's `last_iteration` is `None` (daemon just started)
- **THEN** the last-iteration clause reads `no iteration yet`

#### Scenario: User-controlled change name is Slack-escaped
- **WHEN** a change name passed in by the parser somehow contains `<` (despite the parser's allowlist — belt-and-braces)
- **THEN** the change name renders with the angle bracket escaped to `&lt;` in the menu reply

### Requirement: Partial-degradation: one repo's failure does not block the menu
When the dispatcher cannot assemble a complete `RepoStatusResponse` for a specific repository (control-socket call errored, repo-not-found, etc.), the menu SHALL still render every other repository's section normally AND SHALL render the failing repository's section with `(unavailable: <error excerpt>)` in place of the summary line. A WARN log is emitted for each unavailable repository.

#### Scenario: One repo unavailable, two healthy
- **WHEN** a three-repo daemon returns Ok for two of the three and Err for one
- **THEN** the menu reply contains three sections in total
- **AND** the two Ok sections render normally
- **AND** the one Err section renders `(unavailable: <error excerpt>)` in place of the summary line
- **AND** the URL line for the unavailable section is still present

#### Scenario: All repos unavailable
- **WHEN** every per-repo lookup errors
- **THEN** the menu reply contains one section per repo, each with `(unavailable: ...)`
- **AND** the leading announcement line still names the count

### Requirement: Help verb mentions the bare-status menu
The `help` verb's reply SHALL include a line documenting that `@<bot> status` with no repo argument returns the per-repo menu. The line distinguishes the two `status` forms so operators discovering the help text learn both modes.

#### Scenario: Help mentions bare status
- **WHEN** an operator posts `@<bot> help`
- **THEN** the reply text contains a phrase describing that bare `@<bot> status` returns the per-repo menu
- **AND** the reply text also mentions the per-repo form `@<bot> status <repo-substring>` for the detailed view
