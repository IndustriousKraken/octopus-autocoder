## MODIFIED Requirements

### Requirement: `changelog` subcommand harvests changelog entries from the OpenSpec archive
autocoder SHALL ship a `changelog` subcommand alongside `run`, `reload`, `rewind`, `audit run`, `install`, and `check-config`. The subcommand SHALL walk the OpenSpec archive directory (`openspec/changes/archive/`) AND the issues-lane archive (`issues/archive/`) of a target workspace, identify archives added within a tag range, extract per-archive summary text (`proposal.md` for changes, `issue.md` for issues), group change entries by primary affected capability and issue entries under a `Fixes` group, identify range commits attributable to neither lane, AND emit either markdown (default) or structured JSON to stdout.

The subcommand SHALL NOT spawn any daemon work, mutate any file, contact any external service, or invoke any LLM. It is a pure-data extractor — same archive contents + same tag range produce the same output every invocation.

**Flag surface:**

- `--workspace <path>`: directory containing `openspec/changes/archive/`. Defaults to the current working directory. Operators running against a managed workspace from the daemon host use this flag.
- `--since <tag-or-sentinel>`: lower bound (exclusive). Defaults to the most recent tag on `--to`'s ancestry that does NOT point at the `--to` commit itself — so when `--to` names a release tag, the default lower bound is the *previous* tag, and `changelog --to vX.Y.Z` documents the range ending at that release rather than resolving the empty self-range `(vX.Y.Z .. vX.Y.Z]`. The literal value `ever` is a sentinel meaning "from the beginning of archive history" — useful for first-release runs.
- `--to <tag-or-ref>`: upper bound (inclusive). Defaults to `HEAD`.
- `--format markdown|json`: output shape. Default `markdown`.

**Tag-range resolution edge cases:**

- `--since` unset AND no tag exists on `--to`'s ancestry other than tags pointing at the `--to` commit itself → fall back to "from ever" AND emit one stderr line: `No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice.` Exit 0.
- `--since <tag>` referencing a tag that does not exist → exit non-zero with a clear error naming the missing tag.
- A resolved range whose `since` and `to` are the same commit (however it arose — explicit flags or defaulting) → exit non-zero with an error naming both refs and the shared commit, and suggesting an explicit `--since` (or `--since ever`). An empty self-range SHALL NEVER produce a "no changes" document, because it silently misrepresents a release as empty.

**Issues-lane harvesting.** Entries under `issues/archive/` added within the range SHALL be harvested alongside OpenSpec archives: the slug is the archived issue's directory (or file stem) name, the summary is the first paragraph of body text in its `issue.md` (skipping any leading heading), and the same `changelog:` frontmatter overrides apply. Issue entries carry `lane: "issue"` (change entries carry `lane: "change"`) in JSON and group under a `### Fixes` heading in markdown, after the capability groups.

**Unattributed-commit sweep.** Commits in `(since .. to]` that added no OpenSpec archive entry AND no issues-lane archive entry (merge commits excluded) SHALL be reported as `unattributed_commits` — an array of `{sha, date, subject}` in JSON, and an `### Other changes` footer listing `<short-sha> <subject>` lines in markdown when at least one exists. The sweep is reporting, not interpretation: the extractor does not classify these commits; downstream consumers (the stylist, an operator) decide what they mean. A release window whose lanes are empty but whose unattributed list is not SHALL therefore never be presented by the extractor as containing no work.

**Frontmatter overrides** on a change's `proposal.md` (or an issue's `issue.md`):

- Absent OR no `changelog:` field → default behavior: use the first paragraph of `## Why` as the entry's summary (for issues: the first body paragraph).
- `changelog: skip` (or `internal`, `hidden` — accept synonyms) → omit the change from output AND record it in the `skipped` list (JSON output) or a footer (markdown output, when at least one change was skipped).
- `changelog: { summary: "<text>" }` → use the override summary instead of the first-`## Why` paragraph.
- Unrecognized `changelog:` value → emit a WARN log naming the value, fall through to default behavior.

#### Scenario: Default invocation emits markdown grouped by capability
- **WHEN** an operator runs `autocoder changelog` from a repo root with two prior tags AND three archives added since the most recent tag (`drift-audit-spec-contradictions`, `chatops-slack-event-dedup`, `executor-streams-output-incrementally`)
- **THEN** stdout contains a markdown document headed `## <to-ref> — <YYYY-MM-DD>`
- **AND** the changes group under `### chatops-manager` (one entry), `### executor` (one entry), AND `### orchestrator-cli` (one entry — whichever capability owns drift-audit's spec delta)
- **AND** each entry's bullet form is `- **<summary-first-line>** (<slug>) — <rest-of-summary-if-any>`
- **AND** stderr is empty

#### Scenario: `--to` naming a tag documents the range ending at that tag
- **WHEN** the repo has tags `v1.0.0` and `v1.1.0` AND archives were added between them
- **AND** the operator runs `autocoder changelog --to v1.1.0` with no `--since`
- **THEN** the default `since` resolves to `v1.0.0` (the most recent tag on `v1.1.0`'s ancestry excluding tags pointing at `v1.1.0` itself)
- **AND** stdout documents the archives added in `(v1.0.0 .. v1.1.0]`
- **AND** the output is identical to an explicit `autocoder changelog --since v1.0.0 --to v1.1.0` run

#### Scenario: A degenerate equal range is an error, not an empty success
- **WHEN** the resolved `since` and `to` name the same commit (e.g. `--since v1.1.0 --to v1.1.0` passed explicitly)
- **THEN** the subcommand exits non-zero
- **AND** the error names both refs and the shared commit AND suggests passing an explicit `--since` (or `--since ever`)
- **AND** no "no archived changes" document is emitted

#### Scenario: No prior tags falls back to "ever" with an INFO line
- **WHEN** the operator runs `autocoder changelog` from a repo root with no tags AND `--since` unset
- **THEN** the subcommand emits one stderr INFO line naming the fallback AND pointing at `--since ever` as the explicit form
- **AND** stdout contains every archive in the repo's history, sorted by shipped-commit order
- **AND** the subcommand exits 0

#### Scenario: `--since ever` explicit form suppresses the INFO line
- **WHEN** the operator runs `autocoder changelog --since ever` from a repo (with or without tags)
- **THEN** stdout contains every archive in history
- **AND** stderr is empty (the INFO line only fires under the implicit fallback path)

#### Scenario: Issues-lane corrections appear under Fixes
- **WHEN** an issues-lane correction is archived under `issues/archive/<slug>/` in a commit within the range
- **AND** the operator runs `autocoder changelog` over that range
- **THEN** the markdown output contains a `### Fixes` group with an entry for `<slug>` whose summary is the first body paragraph of its `issue.md`
- **AND** the JSON output's corresponding entry carries `"lane": "issue"`

#### Scenario: Issue frontmatter `changelog: skip` omits the issue
- **WHEN** an archived issue's `issue.md` carries frontmatter `changelog: skip`
- **AND** the changelog is generated for a range that includes it
- **THEN** the issue does NOT appear among the entries
- **AND** it DOES appear in the `skipped` list with reason `changelog: skip`

#### Scenario: Unattributed commits are reported, not hidden
- **WHEN** the range contains commits that added no OpenSpec archive entry and no issues-lane archive entry (e.g. a direct bugfix or docs commit pushed by a human)
- **THEN** the JSON output carries an `unattributed_commits` array with one `{sha, date, subject}` object per such commit (merge commits excluded)
- **AND** the markdown output ends with an `### Other changes` footer listing `<short-sha> <subject>` per commit
- **AND** a range with empty lanes but non-empty unattributed commits is NOT presented as containing no work

#### Scenario: Frontmatter `changelog: skip` omits the change
- **WHEN** an archive's `proposal.md` carries frontmatter `changelog: skip`
- **AND** `autocoder changelog --format json` is run against a range that includes this archive
- **THEN** the change does NOT appear in the JSON output's `entries` array
- **AND** the change DOES appear in the `skipped` array with `{"slug": "...", "reason": "changelog: skip"}`

#### Scenario: Frontmatter `changelog.summary` override replaces the default summary
- **WHEN** an archive's `proposal.md` carries frontmatter `changelog: { summary: "Adds /healthz endpoint for liveness probes" }`
- **AND** the changelog is generated for a range that includes this archive
- **THEN** the entry's summary text is `Adds /healthz endpoint for liveness probes` exactly
- **AND** the first paragraph of `## Why` is NOT used

#### Scenario: JSON output is machine-readable
- **WHEN** the operator runs `autocoder changelog --format json`
- **THEN** stdout contains a single JSON object with `version`, `date`, `since`, `to`, `entries`, `skipped`, and `unattributed_commits` top-level fields
- **AND** each entry object includes `slug`, `archive_dir`, `primary_capability`, `summary`, `shipped_commit`, `shipped_date`, and `lane`
- **AND** the JSON parses without error via `serde_json::from_str`
- **AND** the output is pretty-printed (2-space indent) for human readability

#### Scenario: Cross-project usage via `--workspace`
- **WHEN** an operator runs `autocoder changelog --workspace /path/to/another-openspec-repo`
- **THEN** the subcommand reads the named workspace's archive AND git history
- **AND** the operator's cwd is irrelevant
- **AND** the subcommand works against any repo whose `openspec/changes/archive/` directory exists, not just autocoder's own repo (a workspace with no `issues/archive/` simply harvests no issue entries)

#### Scenario: Archive discovery uses git addition commits, not directory date prefixes
- **WHEN** an archive entry is added to `openspec/changes/archive/` (or an issue to `issues/archive/`) in commit `<sha>`
- **AND** the operator runs `autocoder changelog --since <tag>` where `<tag>` is reachable from before `<sha>`
- **THEN** the entry appears in the output if and only if `<sha>` is reachable from `--to` BUT NOT from `--since`
- **AND** the directory's `YYYY-MM-DD` prefix is used only for the entry's `shipped_date` field, never for range filtering (so a manually-renamed archive directory does not affect what changelogs include)

#### Scenario: Subcommand is testable against synthetic fixtures
- **WHEN** the changelog tests run under `cargo test`
- **THEN** each test stands up a tempdir with a synthetic git repo (`git init`, a few commits adding archive entries, optional tags)
- **AND** the test invokes `execute` with a `ChangelogArgs` pointing at the tempdir
- **AND** assertions cover the markdown / JSON output text exactly
- **AND** no test depends on autocoder's own archive history
