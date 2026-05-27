## 1. Data types

- [x] 1.1 In a new `autocoder/src/cli/changelog.rs`, define the surface:
  ```rust
  #[derive(Args, Debug, Clone)]
  pub struct ChangelogArgs {
      #[arg(long)]
      pub workspace: Option<PathBuf>,
      #[arg(long)]
      pub since: Option<String>,
      #[arg(long, default_value = "HEAD")]
      pub to: String,
      #[arg(long, value_enum, default_value_t = ChangelogFormat::Markdown)]
      pub format: ChangelogFormat,
  }
  #[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
  pub enum ChangelogFormat { Markdown, Json }

  pub struct TagRange { pub since_commit: Option<String>, pub since_label: String, pub to_commit: String, pub to_label: String }
  pub struct ArchiveEntry {
      pub slug: String,
      pub archive_dir: PathBuf,
      pub primary_capability: Option<String>,
      pub summary: String,                  // first paragraph of ## Why OR frontmatter override
      pub shipped_commit: String,
      pub shipped_date: String,             // YYYY-MM-DD
  }
  pub struct SkippedEntry { pub slug: String, pub reason: String }
  pub async fn execute(args: ChangelogArgs) -> Result<()> { ... }
  ```

## 2. Tag range resolution

- [x] 2.1 `resolve_tag_range(workspace: &Path, since: Option<&str>, to: &str) -> Result<TagRange>`:
  - Workspace defaults to cwd if `args.workspace` is `None`.
  - If `since` is `Some("ever")`: return `TagRange { since_commit: None, since_label: "ever", to_commit: <resolve>, to_label: to.to_string() }`.
  - If `since` is `Some(<tag>)`: run `git -C <workspace> rev-parse <tag>` to resolve; bail with a clear error if the tag doesn't exist.
  - If `since` is `None`: run `git -C <workspace> describe --tags --abbrev=0 HEAD` to find the most recent tag. If a tag is found, use it. If `describe` exits non-zero (no tags), fall back to `TagRange { since_commit: None, since_label: "ever (no prior tags found)" }` AND emit one INFO line to stderr: `No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice.`
  - For `to`: run `git -C <workspace> rev-parse <to>` to resolve. Bail on failure.
- [x] 2.2 Tests:
  - Synthetic git history with two tags AND `--since` unset → range starts at the more recent tag.
  - No tags in history AND `--since` unset → range is `since_commit: None` AND the stderr INFO line fires.
  - Explicit `--since ever` → range is `since_commit: None` AND no INFO line.
  - Explicit `--since v0.1.0` against a history that has that tag → range starts at the tag's commit.
  - Explicit `--since v99.0.0` (nonexistent) → bail with a descriptive error.

## 3. Archive discovery

- [x] 3.1 `find_archives_in_range(workspace: &Path, range: &TagRange) -> Result<Vec<ArchiveEntry>>`:
  - Build the git log command:
    - If `since_commit` is `Some(sha)`: `git -C <workspace> log --diff-filter=A --pretty=format:%H%x09%ad --date=short --reverse <sha>..<to> -- openspec/changes/archive/`.
    - If `since_commit` is `None`: `git -C <workspace> log --diff-filter=A --pretty=format:%H%x09%ad --date=short --reverse <to> -- openspec/changes/archive/`.
  - Run the command and parse lines into `(commit_sha, shipped_date)` pairs.
  - For each commit, run `git -C <workspace> show --name-only --pretty=format: <sha>` and collect the changed paths. Filter to paths matching `openspec/changes/archive/<dir>/...` AND derive the top-level archive directory name.
  - Group: a single commit may add multiple archive entries (one PR can bundle changes); each top-level directory is a distinct `ArchiveEntry`.
  - For each entry, call `read_archive_metadata` AND `primary_capability` to populate the remaining fields.
- [x] 3.2 Tests:
  - Synthetic history with three commits adding archive entries; each is detected with its correct commit AND date.
  - Commit that adds two archive entries → two `ArchiveEntry` records, same `shipped_commit`.
  - Range that excludes pre-`--since` archive additions → those entries are NOT included.
  - Archive directory deletions (rare, but `--diff-filter=A` excludes them) are not falsely included.

## 4. Frontmatter + summary extraction

- [x] 4.1 `read_archive_metadata(workspace: &Path, archive_dir: &Path) -> Result<ArchiveMetadataRaw>`:
  - Read `<archive_dir>/proposal.md`.
  - If the file starts with `---` followed by a frontmatter block ending in `---`, parse the YAML between via `serde_yaml`. Otherwise treat as no-frontmatter.
  - Extract the first paragraph after `## Why`:
    - Scan for the literal line `## Why` (case-sensitive; the OpenSpec convention).
    - Skip any blank lines immediately after.
    - Collect lines until the next blank line OR the next `##` heading.
    - Return the joined string with trailing whitespace stripped.
  - Tests:
    - Proposal with `## Why\n\nFirst paragraph here.\n\nSecond paragraph.` → returns "First paragraph here."
    - Proposal with `## Why` followed by indented code block then prose → returns the prose (skip pure code-fence-only paragraphs? — for v1, accept the first paragraph as-is, even if it's a code block; LLMs in a06 can re-style).
    - Proposal with no `## Why` heading → returns the first paragraph of the file with a WARN log; do not crash.
- [x] 4.2 Apply frontmatter overrides:
  - `changelog: skip` (or `internal`, `hidden`) → return `SkippedEntry { reason: "changelog: skip" }` to the caller.
  - `changelog: { summary: "..." }` → use the summary verbatim instead of the `## Why` paragraph.
  - Unrecognized `changelog:` value → emit a WARN naming the value AND fall through to default behavior.
- [x] 4.3 Tests covering each frontmatter shape, including the `summary` override AND an unrecognized value.

## 5. Primary-capability detection

- [x] 5.1 `primary_capability(workspace: &Path, archive_dir: &Path) -> Option<String>`:
  - Read `<archive_dir>/specs/` directory entries.
  - Filter to subdirectories (each is a capability slug).
  - Return the alphabetically-first slug, or `None` if the directory is empty / missing.
- [x] 5.2 Tests:
  - Archive with `specs/chatops-manager/` AND `specs/orchestrator-cli/` → returns `chatops-manager`.
  - Archive with no `specs/` dir (rare but possible for docs-only changes) → returns `None`.

## 6. Renderers

- [x] 6.1 `render_markdown(version: &str, range: &TagRange, entries: &[ArchiveEntry]) -> String`:
  - Header: `## <version> — <YYYY-MM-DD>` where date is the `--to` commit's date in UTC.
  - Group entries by `primary_capability`. Capabilities sort alphabetically; entries within each capability sort by `shipped_commit` order (matches release chronology).
  - Entries with `primary_capability: None` group under `### Other`.
  - Each entry renders as `- **<summary-first-line>** (<slug>) — <rest-of-summary-if-any>`.
- [x] 6.2 `render_json(version: &str, range: &TagRange, entries: &[ArchiveEntry], skipped: &[SkippedEntry]) -> String`:
  - Single JSON object with `version`, `date`, `since`, `to`, `entries: [...]`, `skipped: [...]`.
  - Pretty-printed (2-space indent) for human readability AND scripting alike.
- [x] 6.3 Tests for each renderer against fixture entries; assert exact output text.

## 7. Subcommand wiring

- [x] 7.1 In `autocoder/src/cli/mod.rs` (or wherever subcommand dispatch lives), register `changelog`.
- [x] 7.2 `execute(args: ChangelogArgs) -> Result<()>`:
  - Resolve workspace (`args.workspace.unwrap_or_else(|| std::env::current_dir().unwrap())`).
  - Resolve tag range via `resolve_tag_range`.
  - Discover archives via `find_archives_in_range`.
  - For each, attempt metadata read; skipped entries collect into `Vec<SkippedEntry>`.
  - Render per `args.format`.
  - Write to stdout. Exit 0.
- [x] 7.3 Integration test: tempdir with a synthetic git repo, two tags, three archive entries (one with `changelog: skip` frontmatter, one with `changelog.summary` override, one default). Run `execute` and assert the markdown output matches expectation.

## 8. Release workflow integration

- [x] 8.1 In `.github/workflows/release.yml`, after the test gate AND before the binary publish job, add a `notes` job (OR a step in the existing publish job) that:
  - Resolves the previous tag: `previous_tag=$(git describe --tags --abbrev=0 HEAD^ 2>/dev/null || echo "ever")`.
  - Runs `./target/release/autocoder changelog --since "$previous_tag" --to "${{ github.ref_name }}" > /tmp/release-notes.md`.
  - Sets `notes_file=/tmp/release-notes.md` as a step output.
- [x] 8.2 Extend the existing `gh release create` step (or `softprops/action-gh-release` invocation) to pass `--notes-file <path>` (or `body_path: /tmp/release-notes.md` for the action variant).
- [x] 8.3 Test the workflow change by running it locally with `act` (or against a test branch with a test tag). Assert the resulting release notes contain the expected markdown.
- [x] 8.4 If the changelog step itself fails (no archives in range, unexpected error), the release workflow continues — log the error and pass an empty notes file. A failing changelog generation should NOT block a binary release; the binary is the primary artifact.

## 9. CLI.md documentation

- [x] 9.1 In `docs/CLI.md`, add a new section `## \`changelog\`` documenting:
  - The subcommand and its flags.
  - The `--since ever` sentinel AND the no-tags-fallback behavior.
  - The frontmatter overrides (`changelog: skip`, `changelog.summary: "..."`).
  - The markdown AND JSON output shapes.
  - Cross-project usage via `--workspace` (run from another OpenSpec checkout OR from the daemon host pointing at a managed workspace).
- [x] 9.2 Cross-link from `docs/DEPLOYMENT.md`'s `Upgrading` section: operators tagging a new release can preview the notes locally via `autocoder changelog` before pushing the tag.

## 10. Spec deltas

- [x] 10.1 `openspec/changes/a05-changelog-subcommand/specs/orchestrator-cli/spec.md` ADDs one requirement: `changelog subcommand harvests changelog entries from the OpenSpec archive`.
- [x] 10.2 `openspec/changes/a05-changelog-subcommand/specs/project-documentation/spec.md` ADDs two requirements: `CLI.md documents the changelog subcommand` AND `Release workflow uses the changelog subcommand for release-body notes`.

## 11. Verification

- [x] 11.1 `cargo test` passes (new + existing).
- [x] 11.2 `openspec validate a05-changelog-subcommand --strict` passes.
- [x] 11.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
