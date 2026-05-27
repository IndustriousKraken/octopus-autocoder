## ADDED Requirements

### Requirement: `changelog` subcommand harvests changelog entries from the OpenSpec archive
autocoder SHALL ship a `changelog` subcommand alongside `run`, `reload`, `rewind`, `audit run`, `install`, and `check-config`. The subcommand SHALL walk the OpenSpec archive directory (`openspec/changes/archive/`) of a target workspace, identify archives added within a tag range, extract per-archive summary text from `proposal.md`, group by primary affected capability, AND emit either markdown (default) or structured JSON to stdout.

The subcommand SHALL NOT spawn any daemon work, mutate any file, contact any external service, or invoke any LLM. It is a pure-data extractor — same archive contents + same tag range produce the same output every invocation.

**Flag surface:**

- `--workspace <path>`: directory containing `openspec/changes/archive/`. Defaults to the current working directory. Operators running against a managed workspace from the daemon host use this flag.
- `--since <tag-or-sentinel>`: lower bound (exclusive). Defaults to the most recent tag on `HEAD`'s ancestry as reported by `git describe --tags --abbrev=0 HEAD`. The literal value `ever` is a sentinel meaning "from the beginning of archive history" — useful for first-release runs.
- `--to <tag-or-ref>`: upper bound (inclusive). Defaults to `HEAD`.
- `--format markdown|json`: output shape. Default `markdown`.

**Tag-range resolution edge cases:**

- `--since` unset AND `git describe --tags --abbrev=0 HEAD` exits non-zero (no tags exist) → fall back to "from ever" AND emit one stderr line: `No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice.` Exit 0.
- `--since <tag>` referencing a tag that does not exist → exit non-zero with a clear error naming the missing tag.

**Frontmatter overrides** on a change's `proposal.md`:

- Absent OR no `changelog:` field → default behavior: use the first paragraph of `## Why` as the entry's summary.
- `changelog: skip` (or `internal`, `hidden` — accept synonyms) → omit the change from output AND record it in the `skipped` list (JSON output) or a footer (markdown output, when at least one change was skipped).
- `changelog: { summary: "<text>" }` → use the override summary instead of the first-`## Why` paragraph.
- Unrecognized `changelog:` value → emit a WARN log naming the value, fall through to default behavior.

#### Scenario: Default invocation emits markdown grouped by capability
- **WHEN** an operator runs `autocoder changelog` from a repo root with two prior tags AND three archives added since the most recent tag (`drift-audit-spec-contradictions`, `chatops-slack-event-dedup`, `executor-streams-output-incrementally`)
- **THEN** stdout contains a markdown document headed `## <to-ref> — <YYYY-MM-DD>`
- **AND** the changes group under `### chatops-manager` (one entry), `### executor` (one entry), AND `### orchestrator-cli` (one entry — whichever capability owns drift-audit's spec delta)
- **AND** each entry's bullet form is `- **<summary-first-line>** (<slug>) — <rest-of-summary-if-any>`
- **AND** stderr is empty

#### Scenario: No prior tags falls back to "ever" with an INFO line
- **WHEN** the operator runs `autocoder changelog` from a repo root with no tags AND `--since` unset
- **THEN** the subcommand emits one stderr INFO line naming the fallback AND pointing at `--since ever` as the explicit form
- **AND** stdout contains every archive in the repo's history, sorted by shipped-commit order
- **AND** the subcommand exits 0

#### Scenario: `--since ever` explicit form suppresses the INFO line
- **WHEN** the operator runs `autocoder changelog --since ever` from a repo (with or without tags)
- **THEN** stdout contains every archive in history
- **AND** stderr is empty (the INFO line only fires under the implicit fallback path)

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
- **THEN** stdout contains a single JSON object with `version`, `date`, `since`, `to`, `entries`, and `skipped` top-level fields
- **AND** each entry object includes `slug`, `archive_dir`, `primary_capability`, `summary`, `shipped_commit`, `shipped_date`
- **AND** the JSON parses without error via `serde_json::from_str`
- **AND** the output is pretty-printed (2-space indent) for human readability

#### Scenario: Cross-project usage via `--workspace`
- **WHEN** an operator runs `autocoder changelog --workspace /path/to/another-openspec-repo`
- **THEN** the subcommand reads the named workspace's archive AND git history
- **AND** the operator's cwd is irrelevant
- **AND** the subcommand works against any repo whose `openspec/changes/archive/` directory exists, not just autocoder's own repo

#### Scenario: Archive discovery uses git addition commits, not directory date prefixes
- **WHEN** an archive entry is added to `openspec/changes/archive/` in commit `<sha>`
- **AND** the operator runs `autocoder changelog --since <tag>` where `<tag>` is reachable from before `<sha>`
- **THEN** the entry appears in the output if and only if `<sha>` is reachable from `--to` BUT NOT from `--since`
- **AND** the directory's `YYYY-MM-DD` prefix is used only for the entry's `shipped_date` field, never for range filtering (so a manually-renamed archive directory does not affect what changelogs include)

#### Scenario: Subcommand is testable against synthetic fixtures
- **WHEN** the changelog tests run under `cargo test`
- **THEN** each test stands up a tempdir with a synthetic git repo (`git init`, a few commits adding archive entries, optional tags)
- **AND** the test invokes `execute` with a `ChangelogArgs` pointing at the tempdir
- **AND** assertions cover the markdown / JSON output text exactly
- **AND** no test depends on autocoder's own archive history
