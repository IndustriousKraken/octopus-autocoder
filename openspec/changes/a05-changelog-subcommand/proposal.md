## Why

Tagged releases on GitHub today expose only the raw commit diff as their release notes — a wall of file changes that says nothing about *what an operator should care about* in this release. Most projects solve this with hand-written CHANGELOG.md files OR conventional-commits-driven generators. Neither fits autocoder's workflow: autocoder's commits are agent-generated AND don't follow a commit-prefix convention, AND a hand-written CHANGELOG.md duplicates work the operator already does when writing OpenSpec proposals.

The unrecognized asset is the OpenSpec archive itself. Every change ships with a `## Why` paragraph in `openspec/changes/<slug>/proposal.md` — written for humans, explaining motivation, sized for skim-reading. When the change archives, the directory moves to `openspec/changes/archive/YYYY-MM-DD-<slug>/` and the `## Why` content stays put. The archive is, structurally, already the changelog source-of-truth — it just needs a tool to harvest it.

Since OpenSpec's archive convention is identical across every repo that uses OpenSpec, the same tool works against any managed project, not just autocoder itself. Operators running `autocoder changelog` from a coterie or sound-cabinet checkout (or from the daemon host pointing at a managed workspace via `--workspace`) get the same harvesting against those repos' archives.

## What Changes

**New subcommand `autocoder changelog`.** Pure-data extractor: walks the OpenSpec archive, finds entries within a tag range, pulls `## Why` content, renders markdown or JSON. No LLM involvement — the harvested content IS the operator's prose. Deterministic: same archive + same range = same output every time.

**Flags:**

- `--workspace <path>` — directory containing `openspec/changes/archive/`. Defaults to the current working directory. Parallel to how `autocoder audit run --workspace <path>` operates against any OpenSpec checkout.
- `--since <tag-or-sentinel>` — lower bound (exclusive). Defaults to the most recent annotated/lightweight tag on `HEAD`'s ancestry. The sentinel `ever` means "from the beginning of archive history" — explicit form for first-release runs.
- `--to <tag-or-ref>` — upper bound (inclusive). Defaults to `HEAD`.
- `--format markdown|json` — output shape. Default `markdown`.

**Default behavior when no tags exist.** If `--since` is unset AND `git tag --merged HEAD` returns empty, the subcommand falls back to `ever` AND emits one INFO-level message to stderr: `No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice.` Exit 0; the operator gets useful output on the first-release case without inventing a fake tag.

**Determining which archives belong to a release window.** The archive directory name carries a `YYYY-MM-DD` prefix (autocoder's archive step uses this AND `rebuild-canonical-specs` preserves it). The right "this archive shipped between tags X and Y" signal is the commit that *moved the directory into archive*, not the date in the directory name. The subcommand uses `git log --diff-filter=A --pretty=format:%H -- openspec/changes/archive/` to find the addition commits for each archive entry, then checks which of those commits are reachable from `--to` but not from `--since`.

**Frontmatter overrides.** A change's `proposal.md` MAY carry frontmatter that the extractor honors:

```markdown
---
changelog: skip                    # this change does not appear in changelogs
# OR
changelog:
  summary: "One-line override for the changelog entry"
---
## Why
...
```

Three behaviors:

- No frontmatter OR no `changelog:` field → default behavior: use the first paragraph of `## Why` as the changelog entry.
- `changelog: skip` (or `changelog: internal`, `changelog: hidden` — accept synonyms; emit a WARN on unrecognized values rather than error) → omit the change from output.
- `changelog.summary: "<text>"` → use the override instead of the first-`## Why` paragraph.

The frontmatter is harmless when no extractor reads it — pre-spec proposals work unchanged.

**Default markdown output shape.** Grouped by primary affected capability (the first directory under the change's `specs/` tree):

```markdown
## v0.4.0 — 2026-05-28

### chatops-manager
- **Chat-driven proposals via `@<bot> propose`** (chat-request-triage) — Operators can now ask autocoder to act on a free-form request from chat. The agent classifies the request as DIRECTIVE, QUESTION, or AMBIGUOUS and produces a fixes PR and/or a spec PR.
- **Audit-finding triage via `@<bot> send it`** (audit-reply-acts) — Reply inside an audit's threaded findings to spawn a triage run.

### orchestrator-cli
- **Streaming JSON output capture** (executor-streams-output-incrementally) — Per-change logs gain PROMPT / ACTIONS / FINAL ANSWER / STDERR sections.

### Fixes
- Sentinel parser no longer false-positives on tool_result content (hotfix).
```

Headings for the version + date use `## v<tag> — <ISO-date>`. Date is the `--to` ref's commit date in UTC. Capability names use the on-disk directory names verbatim. Each entry's parenthetical is the archive's slug (without date prefix) so a reader can grep back to the source proposal.

**JSON output shape** (one object, not line-delimited like `check-config`):

```json
{
  "version": "v0.4.0",
  "date": "2026-05-28",
  "since": "v0.3.0",
  "to": "HEAD",
  "entries": [
    {
      "slug": "chat-request-triage",
      "archive_dir": "openspec/changes/archive/2026-05-22-chat-request-triage",
      "primary_capability": "chatops-manager",
      "summary": "Chat-driven proposals via @<bot> propose...",
      "shipped_commit": "abc123",
      "shipped_date": "2026-05-22"
    }
  ],
  "skipped": [
    { "slug": "...", "reason": "changelog: skip" }
  ]
}
```

JSON exists for downstream tooling — most obviously `a06`'s chat-driven version, which feeds this into an LLM stylist.

**Release workflow integration.** `.github/workflows/release.yml` gains a step that runs `autocoder changelog --since <previous-tag>` against the just-tagged commit AND passes the output to `gh release create --notes-file`. The release body becomes the harvested changelog instead of the auto-generated diff. The step runs after the test gate AND before the binary upload (so a failing changelog generation doesn't block the binary release, but operators reviewing the release see the notes alongside the assets).

**CHANGELOG.md is NOT automatically maintained by a05.** The deterministic extractor emits to stdout; appending or prepending to `CHANGELOG.md` is the consumer's job. The release workflow uses it for release-body notes only. `a06`'s chat-driven version handles the CHANGELOG.md maintenance because that's where human judgment over phrasing matters.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one ADDED requirement: `changelog subcommand harvests changelog entries from the OpenSpec archive`. Names the flag surface, the tag-range resolution, the frontmatter overrides, the output formats, and the cross-project applicability via `--workspace`.
  - `project-documentation` — TWO ADDED requirements: `CLI.md documents the changelog subcommand` AND `Release workflow uses the changelog subcommand for release-body notes`.
- **Affected code:**
  - `autocoder/src/cli/changelog.rs` (new) — module implementing the subcommand. Sub-functions:
    - `resolve_tag_range(workspace: &Path, since: Option<&str>, to: &str) -> Result<TagRange>` — handles the `ever` sentinel, the no-tags-fallback, and the default-to-last-tag logic. Returns `TagRange { since_commit: Option<String>, to_commit: String }` where `since_commit: None` means "from the dawn of history."
    - `find_archives_in_range(workspace: &Path, range: &TagRange) -> Result<Vec<ArchiveEntry>>` — uses `git log --diff-filter=A --pretty=format:%H%x09%ad --date=short --reverse <since>..<to> -- openspec/changes/archive/` to find the addition commits, then for each commit's `--name-only` changed paths, scopes to top-level directories under `openspec/changes/archive/`. Returns one entry per archive directory.
    - `read_archive_metadata(workspace: &Path, archive_dir: &str) -> Result<ArchiveMetadata>` — reads `proposal.md`, parses any frontmatter via `serde_yaml`, extracts the first paragraph of `## Why` (markdown-aware: skip the `## Why` heading line, take everything until the next blank line OR the next `##` heading).
    - `primary_capability(workspace: &Path, archive_dir: &str) -> Option<String>` — reads the change's `specs/` directory; returns the alphabetically-first capability slug. Returns `None` for changes with no spec deltas (rare; treated as "Other" in the markdown output).
    - `render_markdown(version: &str, range: &TagRange, entries: &[ArchiveEntry]) -> String` — the formatter.
    - `render_json(version: &str, range: &TagRange, entries: &[ArchiveEntry], skipped: &[SkippedEntry]) -> String` — the JSON formatter.
  - `autocoder/src/cli/mod.rs` (or equivalent dispatch) — register `changelog`.
  - `.github/workflows/release.yml` — add a step between the test gate and the publish step:
    ```yaml
    - name: Generate release notes from OpenSpec archive
      id: notes
      run: |
        previous_tag=$(git describe --tags --abbrev=0 HEAD^ 2>/dev/null || echo "ever")
        ./target/release/autocoder changelog --since "$previous_tag" --to "${{ github.ref_name }}" > /tmp/release-notes.md
        echo "notes_file=/tmp/release-notes.md" >> $GITHUB_OUTPUT
    ```
    The existing `gh release create` step is extended with `--notes-file ${{ steps.notes.outputs.notes_file }}`.
  - `docs/CLI.md` — new `## \`changelog\`` section documenting the verb, flags, defaults, and JSON / markdown output.
- **Operator-visible behavior:**
  - Running `autocoder changelog` from a repo root emits the changelog since the most recent tag to stdout.
  - The first time an operator tags a fresh repo (no prior tags), `autocoder changelog` falls back to "ever" with an INFO line on stderr.
  - GitHub release pages for autocoder (AND any other project where the workflow is wired in) gain human-readable notes instead of the raw diff.
- **Breaking:** no. The new subcommand is additive. Existing behavior of every other autocoder subcommand is unchanged. The frontmatter overrides are opt-in (changes without them behave identically to today).
- **Acceptance:** `cargo test` passes; `openspec validate a05-changelog-subcommand --strict` passes. Unit tests cover: tag-range resolution with and without prior tags; frontmatter parsing for each accepted shape; primary-capability detection; markdown and JSON formatter output against fixture archives. An integration test runs the subcommand against a tempdir fixture with a synthetic git history + archive entries AND asserts the output matches expectation.
