## ADDED Requirements

### Requirement: Inbound listener recognizes the `prioritize` verb AND manages the `.priority.json` marker
The chatops dispatcher SHALL recognize `@<bot> prioritize <repo-substring> <change-slug> <N>` (case-insensitive on the verb) as a known operator verb alongside the existing operator verbs (`status`, `clear-perma-stuck`, `clear-revision`, `ignore-and-continue`, `clear-ignore`, `help`). It resolves the repo substring via the existing case-insensitive substring-match rule used by `status` / `clear-revision` (ambiguous match → the standard "be more specific" reply listing each candidate URL, with no action submitted; missing repo substring → a polite `✗ prioritize: ...` error with no action submitted).

The trailing argument SHALL be parsed as follows: a non-negative integer `N` sets the change's priority (lower N = higher priority); the literal `clear` OR `none` (case-insensitive) removes the priority. A missing trailing argument, a negative number, or any other non-numeric token that is not `clear`/`none` SHALL be refused with a polite `✗ prioritize: ...` error AND no action submitted.

On a valid parse the dispatcher SHALL submit a `PrioritizeAction { repo_url, change_slug, priority, channel, thread_ts }` over the daemon's Unix-domain control socket, where `priority` is `Some(N)` for a numeric argument AND `None` for `clear`/`none`. The action handler SHALL write (atomic tempfile + rename) `<workspace>/openspec/changes/<change-slug>/.priority.json` carrying `{ priority: N }` when `priority` is `Some`, OR remove that file when `priority` is `None`. The `.priority.json` marker is untracked daemon bookkeeping — gitignored, never committed, parallel to `.iteration-pending.json`. The handler SHALL refuse (polite error, no file written/removed) when the named change-slug does not resolve to a pending change in the workspace. The verb SHALL participate in the existing event-dedup cache so a redelivered Slack event submits exactly one `PrioritizeAction`.

The dispatcher SHALL reply with a confirmation ack: on a set, `✓ Prioritized <change-slug> at priority <N>. It will be worked ahead of unprioritized changes (lower number = higher priority); a change already mid-iteration still goes first.`; on a clear, `✓ Cleared priority on <change-slug>. It returns to the default alphabetical order (or remains in mid-iteration position if it is currently mid-iteration).`

#### Scenario: Setting a numeric priority writes the marker
- **WHEN** the operator runs `@<bot> prioritize myrepo a07-foo 3`
- **AND** `myrepo` unambiguously resolves to a configured repository AND `a07-foo` is a pending change
- **THEN** the dispatcher submits a `PrioritizeAction` over the control socket with `repo_url = <resolved URL>`, `change_slug = "a07-foo"`, AND `priority = Some(3)`
- **AND** the handler writes `<workspace>/openspec/changes/a07-foo/.priority.json` carrying `{ priority: 3 }`
- **AND** the reply is `✓ Prioritized a07-foo at priority 3. It will be worked ahead of unprioritized changes (lower number = higher priority); a change already mid-iteration still goes first.`

#### Scenario: `clear` removes the marker
- **WHEN** the operator runs `@<bot> prioritize myrepo a07-foo clear` (or `... none`)
- **AND** `a07-foo` has a `.priority.json` marker
- **THEN** the dispatcher submits a `PrioritizeAction` with `priority = None`
- **AND** the handler removes `<workspace>/openspec/changes/a07-foo/.priority.json`
- **AND** the reply is `✓ Cleared priority on a07-foo. It returns to the default alphabetical order (or remains in mid-iteration position if it is currently mid-iteration).`
- **NOTE** if `a07-foo` also carries `.iteration-pending.json`, the queue-engine places it in tier 1 (mid-iteration) regardless; the ack's "alphabetical" clause applies only to changes without that marker

#### Scenario: Malformed priority argument is refused
- **WHEN** the operator runs `@<bot> prioritize myrepo a07-foo -1` (or a non-numeric token that is not `clear`/`none`, or omits the argument entirely)
- **THEN** the dispatcher refuses with a polite `✗ prioritize: ...` error explaining the `<N>` (non-negative integer) / `clear` / `none` grammar
- **AND** no `PrioritizeAction` is submitted AND no marker file is written

#### Scenario: Ambiguous repo substring lists candidates
- **WHEN** the operator runs `@<bot> prioritize my-repo a07-foo 2` AND `my-repo` matches multiple configured URLs
- **THEN** the dispatcher does NOT submit a `PrioritizeAction`
- **AND** posts the standard "be more specific" reply with each candidate URL listed
- **AND** no marker file is written

#### Scenario: Priority on a non-pending change is refused
- **WHEN** the operator runs `@<bot> prioritize myrepo a99-missing 2` AND `a99-missing` is not a pending change in the workspace
- **THEN** the handler refuses with a polite error AND writes no marker file

#### Scenario: Help verb lists the prioritize verb
- **WHEN** an operator runs `@<bot> help`
- **THEN** the help text lists `prioritize` with its syntax `prioritize <repo> <change> <N>|clear|none` AND a one-line description (`rank a pending change ahead of the default alphabetical order; lower N = higher priority`)

### Requirement: Status reply surfaces the change-queue priority ordering
The `@<bot> status` reply's queue section SHALL annotate every pending change that carries a `.priority.json` marker with its priority value, rendered as a trailing `(priority <N>)` after the change name, so the operator can see the effective queue order they set. This annotation applies in BOTH the queue one-liner form AND the per-line fallback form. Pending changes WITHOUT a `.priority.json` marker render exactly as they do today (no annotation). The annotation reflects only the changes lane; the issues AND audits lanes are unaffected.

#### Scenario: Prioritized pending change is annotated in the queue section
- **WHEN** an operator runs `@<bot> status myrepo`
- **AND** the workspace has pending change `a07-foo` with `.priority.json` (`priority: 3`) AND pending change `a09-bar` with no priority marker
- **THEN** the queue section renders `a07-foo` with a trailing `(priority 3)` annotation
- **AND** `a09-bar` renders with no priority annotation

#### Scenario: No annotation when no priority markers exist
- **WHEN** an operator runs `@<bot> status myrepo` AND no pending change carries a `.priority.json` marker
- **THEN** the queue section renders exactly as it does today, with no priority annotations
