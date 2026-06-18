## ADDED Requirements

### Requirement: autocoder generates and maintains OCTOPUS.md
`autocoder install` SHALL write `OCTOPUS.md` at the target repository's root, AND autocoder SHALL provide a regeneration path (on upgrade, on `autocoder reload`, OR via a dedicated subcommand) that rewrites it. Generation SHALL render from the single canonical definition of each documented format — the same source the agent prompts draw from — so a format change propagates to OCTOPUS.md without a hand edit. Generation SHALL be idempotent: regenerating when nothing has changed produces no diff.

The generated content SHALL reflect which features are enabled for the repository: the issues-format section is included when `features.issues` is enabled (and describes the single-file-or-directory form), AND the global-rules section is included when a global-rules corpus is configured; a section for a disabled feature is omitted rather than describing something the repo cannot use. The OpenSpec section is always included AND is stamped to the installed `openspec` version.

Generation SHALL also write (or refresh) the single idempotent, autocoder-managed pointer to OCTOPUS.md inside the repository's `AGENTS.md` (creating `AGENTS.md` if absent), between stable markers, leaving any other `AGENTS.md` content untouched.

#### Scenario: install writes OCTOPUS.md and the AGENTS.md pointer
- **WHEN** `autocoder install` runs for a repository
- **THEN** an `OCTOPUS.md` is written at the repository root
- **AND** an idempotent managed pointer to it is written between stable markers in `AGENTS.md` (created if absent)

#### Scenario: Regeneration tracks format changes and is idempotent
- **WHEN** OCTOPUS.md is regenerated after a documented format has changed
- **THEN** the regenerated file reflects the new format
- **WHEN** OCTOPUS.md is regenerated AND nothing has changed
- **THEN** the file (and the AGENTS.md pointer) are byte-identical — no spurious diff

#### Scenario: Sections reflect enabled features
- **WHEN** `features.issues` is enabled for the repository
- **THEN** OCTOPUS.md includes the issues-format section
- **WHEN** `features.issues` is disabled
- **THEN** the issues-format section is omitted
- **AND** the OpenSpec section is present regardless, stamped to the installed `openspec` version
