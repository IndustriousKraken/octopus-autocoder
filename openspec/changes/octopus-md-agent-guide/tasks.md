# Tasks

## 1. Single source for the documented formats

- [ ] 1.1 Factor the canonical description of each documented format (OpenSpec essentials, issues format, rules format) into ONE source that both the OCTOPUS.md generator AND the agent prompts render from, so they cannot diverge. Where a format is already authoritatively defined (e.g. the issues form in canon), the source references it rather than restating it.
- [ ] 1.2 Capture the installed `openspec` version (from the resolved `openspec` binary) for the OpenSpec section's version stamp; do not write the version from memory.

## 2. Generator

- [ ] 2.1 Render `OCTOPUS.md`: an OpenSpec section (essentials inline — create, the ADDED/MODIFIED/REMOVED/RENAMED delta shape, the MODIFIED-reproduces-title-and-all-scenarios rule, `openspec validate --strict` — plus links to the full docs, version-stamped); an issues-format section (single-file-or-directory form) included only when `features.issues` is enabled; a global-rules section included only when a global-rules corpus is configured; and the spec-writing guardrails block (no self-contradiction; no canon contradiction without an explicit MODIFY/RENAME/REMOVE; no spec-sync/apply-to-canon tasks; no direct `openspec/specs/` edits; no archiving).
- [ ] 2.2 Generation is idempotent: regenerating with no underlying change produces a byte-identical file (stable ordering, no timestamps in the body beyond the version stamp).

## 3. AGENTS.md pointer

- [ ] 3.1 Write (or refresh) a single managed pointer to OCTOPUS.md inside `AGENTS.md`, between stable markers (e.g. `<!-- autocoder:octopus start -->` / `<!-- autocoder:octopus end -->`), creating `AGENTS.md` if absent. Content outside the markers is never modified. Refresh updates only the marked region.

## 4. Wiring

- [ ] 4.1 `autocoder install` writes OCTOPUS.md + the AGENTS.md pointer for the target repo.
- [ ] 4.2 Provide a regeneration entry point (on upgrade / `autocoder reload` / a dedicated subcommand) that rewrites both from the canonical source.

## 5. Tests

- [ ] 5.1 Generated OCTOPUS.md contains the OpenSpec essentials, the spec-writing guardrails, and the version stamp; the issues section is present iff `features.issues` is enabled; the rules section is present iff a global-rules corpus is configured. (Assert on section presence/derivation, not on exact prose.)
- [ ] 5.2 Regeneration with no change is byte-identical (idempotent); regeneration after a format change reflects the new format.
- [ ] 5.3 The AGENTS.md pointer is written between markers; an existing AGENTS.md's other content is preserved across a refresh; a second refresh is idempotent.

## 6. Docs

- [ ] 6.1 Note in OPERATIONS.md that autocoder maintains OCTOPUS.md (generated, version-stamped) and adds a managed pointer to AGENTS.md.
