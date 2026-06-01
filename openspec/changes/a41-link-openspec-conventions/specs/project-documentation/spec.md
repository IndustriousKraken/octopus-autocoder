# project-documentation — delta for a41-link-openspec-conventions

## ADDED Requirements

### Requirement: OpenSpec upstream-docs pointer in spec-drafting prompts AND human-facing docs
The repository SHALL maintain a pointer to OpenSpec's upstream documentation (https://github.com/Fission-AI/OpenSpec/tree/main/docs) in two surfaces: every agent-facing prompt that drafts OR materially modifies OpenSpec change content, AND the human-facing `docs/README.md`. The pointer's purpose is to give both agents AND human contributors a canonical reference for scenario syntax (`GIVEN`/`WHEN`/`THEN`), delta format (`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`), AND requirement-header rules without authoring a parallel convention document.

The agent-facing prompt set covered by this requirement is:

- `prompts/implementer.md`
- `prompts/implementer-revision.md`
- `prompts/chat-request-triage.md`
- `prompts/audit-triage.md`
- `prompts/missing-tests-audit.md`
- `prompts/security-bug-audit.md`
- `prompts/brownfield-draft.md`
- `prompts/scout.md`

Each file in this set SHALL contain the literal substring `https://github.com/Fission-AI/OpenSpec` AND at least one of the topical hints: `GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`. The exact prose surrounding the link MAY vary per prompt's voice; the load-bearing parts are the URL AND the topical hint. Pointer placement SHOULD be near the prompt's existing OpenSpec framing (where the prompt first mentions drafting changes, editing specs, or reading canonical specs) so the link is in-context for the agent's first relevant operation.

`docs/README.md` SHALL contain the literal substring `https://github.com/Fission-AI/OpenSpec` in a one-line entry under the file's existing section taxonomy (under `## Internals` OR a new `## Contributing` section near the bottom). The entry SHALL be written in the project's documentation tone — no exclamation marks, no "tip:" framing, no faux-friendly hooks — per the broader project-documentation tone rules.

When a future change introduces a new spec-drafting prompt OR splits an existing one, the new prompt SHALL be added to the covered set above AND inherit the pointer requirement. When a prompt is removed, the requirement's covered set SHALL be updated in the same change to keep the regression test consistent.

This requirement does NOT mandate authoring a project-local convention document (e.g., `openspec/AGENTS.md` OR `docs/SPEC-CONVENTIONS.md`). Project-specific deviations from upstream conventions, when they accumulate enough to confuse contributors, MAY be consolidated into a local file by a separate change; that change would update this requirement to add the local-file cross-link alongside the upstream pointer.

#### Scenario: All covered prompts contain the upstream pointer
- **GIVEN** the repository is in its post-merge state for `a41-link-openspec-conventions`
- **WHEN** a regression test reads each of the eight covered prompt files via `std::fs::read_to_string`
- **THEN** every file's contents contain the substring `https://github.com/Fission-AI/OpenSpec`
- **AND** every file's contents contain at least one of the topical hints (`GIVEN`, `WHEN`, `scenario`, `delta`, `Requirement`)
- **AND** the test fails with a diagnostic naming the offending file path AND the missing element if either substring check fails

#### Scenario: `docs/README.md` contains the human-facing pointer
- **GIVEN** the repository is in its post-merge state for `a41-link-openspec-conventions`
- **WHEN** a regression test reads `docs/README.md` via `std::fs::read_to_string`
- **THEN** the file contents contain the substring `https://github.com/Fission-AI/OpenSpec`
- **AND** the entry is under a section heading appropriate for contributor-oriented references (`## Internals` OR `## Contributing`)
- **AND** the entry's surrounding prose does NOT contain exclamation marks AND does NOT use "tip:" framing (the no-kitsch tone rule)

#### Scenario: New spec-drafting prompt added in a future change
- **GIVEN** a future change introduces `prompts/<new-name>.md` whose role is to draft OR materially modify OpenSpec change content
- **WHEN** that change is drafted
- **THEN** the change's spec deltas SHALL include a MODIFIED block on this requirement that adds `prompts/<new-name>.md` to the covered set
- **AND** the change's tasks SHALL include adding the pointer text to the new file
- **AND** the regression test SHALL be updated in the same change to cover the new file path

#### Scenario: Pointer removal in a future change blocks the regression test
- **GIVEN** a hypothetical future change removes the upstream-docs pointer from `prompts/implementer.md` without updating this requirement OR the regression test
- **WHEN** the regression test runs in CI for that change
- **THEN** the test fails with `prompts/implementer.md: missing required substring 'https://github.com/Fission-AI/OpenSpec'`
- **AND** the failure surfaces before the change can merge
- **AND** the change author's choices are: restore the pointer, OR explicitly update this requirement AND the test together (an intentional convention shift)
