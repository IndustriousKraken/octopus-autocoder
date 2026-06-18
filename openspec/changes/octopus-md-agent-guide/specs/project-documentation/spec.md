## ADDED Requirements

### Requirement: OCTOPUS.md orients agents to autocoder's workflow conventions
A repository under autocoder management SHALL carry an `OCTOPUS.md` at the repository root — the location where an `AGENTS.md` would live — that orients any agent OR human working in the repo to autocoder's workflow conventions: the OpenSpec change workflow, the issues format, AND the global-rules format. Its audience is both autocoder's own offline agents (no web access) AND non-autocoder agents/humans (e.g. a coding assistant or speccing agent run directly on the repo, which never receives autocoder's internal prompts).

OCTOPUS.md SHALL be **autocoder-owned and generated, NOT hand-authored**: it is regenerated from a single canonical definition of each format (the same source the agent prompts draw from), AND version-stamped, so it cannot silently drift from the formats it documents. A hand-maintained copy of these conventions would be one more drift surface — and the most dangerous one, since it is the file agents are told to trust without retrieval.

For OpenSpec, OCTOPUS.md SHALL inline the irreducible core needed to work WITHOUT retrieval — creating a change, the `## ADDED`/`## MODIFIED`/`## REMOVED`/`## RENAMED Requirements` delta shape, the rule that a `MODIFIED` block reproduces the canonical requirement's exact title AND every existing scenario, and `openspec validate --strict` — AND SHALL link to the full OpenSpec documentation for agents that have web access. The inline core SHALL be stamped to the installed `openspec` version rather than copied from memory.

OCTOPUS.md SHALL instruct any agent writing a spec change that it MUST:
- ensure the change does NOT contradict itself;
- ensure it does NOT contradict canon UNLESS it explicitly modifies the contradicted canonical requirement to align — via a `MODIFY`/`RENAME`/`REMOVE` delta — so the change and canon can hold together;
- NOT duplicate work OpenSpec already does — in particular, NOT add tasks that "sync the specs" or "apply/copy the change into canon"; folding a change's deltas into `openspec/specs/` is the archive step, which autocoder performs AFTER implementation, and pre-folding it breaks the archive;
- NOT edit canonical specs under `openspec/specs/` directly;
- NOT archive the change (run `openspec archive`, OR move the change into `changes/archive/`) — autocoder archives a change only after it is implemented AND merged.

These instructions are documentation for agents that are not otherwise constrained; for autocoder's own agents the same invariants are enforced by the verifier gates AND the session sandbox, which OCTOPUS.md does not replace.

OCTOPUS.md SHALL be discoverable via the `AGENTS.md` convention without clobbering a repository's own `AGENTS.md`: autocoder adds a single idempotent, autocoder-managed pointer (between stable markers) to `AGENTS.md` referencing OCTOPUS.md, leaving any existing `AGENTS.md` content intact.

#### Scenario: A managed repo carries a generated OCTOPUS.md
- **WHEN** autocoder manages a repository
- **THEN** an `OCTOPUS.md` exists at the repository root describing the OpenSpec workflow, the issues format, AND (when enabled) the global-rules format
- **AND** it is version-stamped AND was generated, not hand-authored

#### Scenario: OCTOPUS.md carries OpenSpec essentials inline plus links
- **WHEN** an offline agent reads OCTOPUS.md
- **THEN** it finds enough of the OpenSpec workflow inline to author a change without retrieval (create, delta shape, the MODIFIED-reproduces-title-and-scenarios rule, validate)
- **AND** it finds links to the full OpenSpec docs for agents with web access

#### Scenario: OCTOPUS.md states the spec-writing guardrails
- **WHEN** an agent consults OCTOPUS.md before writing a spec change
- **THEN** it is told the change must not contradict itself, must not contradict canon without an explicit `MODIFY`/`RENAME`/`REMOVE` aligning canon, must not add spec-sync / apply-to-canon tasks, must not edit `openspec/specs/` directly, AND must not archive the change (autocoder archives after implementation)

#### Scenario: Discoverable via AGENTS.md without clobbering it
- **WHEN** a repository has its own `AGENTS.md`
- **THEN** autocoder adds (or refreshes) only a single managed pointer to OCTOPUS.md between stable markers
- **AND** the repository's existing `AGENTS.md` content is left intact
