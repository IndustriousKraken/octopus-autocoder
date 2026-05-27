## ADDED Requirements

### Requirement: CHATOPS.md and CLI.md document the `changelog` chatops verb and stylist prompt
`docs/CHATOPS.md` SHALL include a `### Generating a changelog: \`changelog\`` subsection within the `Chat-driven workflows` section, documenting the verb's syntax, flag surface, PR output shape, frontmatter propagation behavior, AND polite-refusal cases. `docs/CLI.md`'s existing `## \`changelog\`` section (from `a05`) SHALL gain a footer cross-link to the chatops verb so operators discovering the deterministic subcommand find the LLM-styled variant.

The stylist prompt template `prompts/changelog-stylist.md` SHALL ship in the repository alongside the other prompt templates (`prompts/implementer.md`, `prompts/code-review-default.md`, etc.) AND SHALL be embedded into the binary at compile time via `include_str!`. Operators MAY override the embedded prompt via a config knob parallel to the other prompt-override fields.

#### Scenario: CHATOPS.md subsection exists with full coverage
- **WHEN** an operator reads `docs/CHATOPS.md`
- **THEN** a subsection titled `### Generating a changelog: \`changelog\`` appears within the `Chat-driven workflows` section
- **AND** the subsection documents the verb syntax `@<bot> changelog <repo-substring> [<args>]`
- **AND** the subsection documents the accepted flags (`--since <tag>`, `--to <tag>`)
- **AND** the subsection documents the PR output shape (single PR; participates in the existing revision loop)
- **AND** the subsection documents frontmatter propagation (revisions implying durable classification may include `proposal.md` frontmatter edits in the same PR)
- **AND** the subsection enumerates the polite-refusal cases (`missing repo-substring`, `no repo matched`, `chatops backend not configured`, `could not post ack`)

#### Scenario: CLI.md cross-links to the chatops verb
- **WHEN** an operator reads `docs/CLI.md`'s `## \`changelog\`` section
- **THEN** the section ends with a footer paragraph: `For an LLM-styled draft that opens a PR for review, use the \`@<bot> changelog\` chatops verb instead. See [CHATOPS.md → Generating a changelog](CHATOPS.md#generating-a-changelog-changelog).`
- **AND** the link anchor resolves to the subsection's heading

#### Scenario: Stylist prompt is embedded and overridable
- **WHEN** an operator inspects the binary's behavior without setting any prompt-override config
- **THEN** the embedded `prompts/changelog-stylist.md` is used as the stylist prompt
- **WHEN** the operator sets `executor.changelog_stylist_prompt_path: /path/to/custom-prompt.md` AND restarts the daemon
- **THEN** the override file's contents replace the embedded prompt
- **AND** an empty override file is rejected at use-time so the daemon does not feed an empty prompt to the wrapped CLI (parallel to the audit prompt-path validation)

#### Scenario: Stylist prompt template explicitly handles the absent-CHANGELOG case
- **WHEN** a maintainer reads `prompts/changelog-stylist.md`
- **THEN** the template includes an explicit directive to check whether `CHANGELOG.md` exists in the workspace root
- **AND** describes both branches: matching the existing style when present, OR creating a fresh Keep a Changelog v1.1.0 file when absent
- **AND** the fresh-file branch specifies the file's expected structure (top-level project heading, `## [Unreleased]` placeholder, current release's `## [<version>] - <YYYY-MM-DD>` section)
