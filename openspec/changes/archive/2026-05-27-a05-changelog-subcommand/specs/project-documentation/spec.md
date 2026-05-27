## ADDED Requirements

### Requirement: CLI.md documents the `changelog` subcommand
`docs/CLI.md` SHALL include a `## \`changelog\`` section documenting the subcommand's flags, default behavior, output formats, and intended use cases.

#### Scenario: CLI.md section exists with full coverage
- **WHEN** an operator reads `docs/CLI.md`
- **THEN** a section titled `## \`changelog\`` appears alongside the other subcommand entries
- **AND** the section documents `--workspace`, `--since`, `--to`, and `--format` with their defaults
- **AND** the section documents the `--since ever` sentinel AND the no-tags-fallback INFO line
- **AND** the section documents the `changelog:` frontmatter overrides (`skip`, `internal`, `hidden`, `summary`)
- **AND** the section includes at least one example markdown output AND one example JSON output

#### Scenario: Section describes cross-project applicability
- **WHEN** an operator reads the section
- **THEN** the text explains that the subcommand works against any OpenSpec checkout, not just autocoder's own repo
- **AND** the text provides examples for both `cd` + `autocoder changelog` AND `autocoder changelog --workspace <path>`
- **AND** the text cross-links to `docs/OPERATIONS.md` for the managed-workspace path under `<cache_dir>/workspaces/<sanitized-url>/`

### Requirement: Release workflow uses the changelog subcommand for release-body notes
`.github/workflows/release.yml` SHALL invoke `autocoder changelog` between the test gate AND the publish step AND pass the output to `gh release create --notes-file` (or the equivalent `body_path` field on the release-action variant in use). The release body on GitHub Releases SHALL display the harvested changelog instead of the auto-generated diff.

A failure in the changelog generation step SHALL NOT block the binary release — the step writes an empty notes file on error AND logs the error. The binary upload is the primary artifact; notes are a best-effort enhancement.

#### Scenario: Tagged release publishes a release body with the harvested notes
- **WHEN** a maintainer pushes a production tag matching `v\d+\.\d+\.\d+`
- **AND** the test gate passes
- **THEN** the workflow runs `autocoder changelog --since <previous-tag> --to <new-tag>` against the just-tagged commit
- **AND** the resulting markdown is written to a temp file
- **AND** the `gh release create` step passes `--notes-file <path>` so the release body on GitHub displays the markdown
- **AND** the release page shows human-readable section headings + bullets, NOT a raw commit diff

#### Scenario: No prior tag falls back to "ever"
- **WHEN** a maintainer pushes the FIRST tag in a repo
- **THEN** the workflow's `previous_tag` resolution (`git describe --tags --abbrev=0 HEAD^`) exits non-zero
- **AND** the workflow falls back to `--since ever` so the first release's notes cover every archive in history
- **AND** the resulting release body is non-empty (a first-release operator gets a meaningful notes block, not an empty one)

#### Scenario: Changelog step failure does not block the binary release
- **WHEN** the `autocoder changelog` invocation fails (binary panics, workspace has no archive, etc.)
- **THEN** the workflow step logs the error AND writes an empty `release-notes.md` AND continues
- **AND** the subsequent binary-upload step runs to completion
- **AND** the resulting GitHub Release has the binaries attached with an empty (or fallback-text) body
- **AND** the operator sees the failed workflow step in the Actions tab AND can investigate manually
