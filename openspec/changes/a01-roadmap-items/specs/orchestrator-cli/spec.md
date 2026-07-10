## ADDED Requirements

### Requirement: Roadmap items are lightweight per-file future-feature records
The repository SHALL support a `roadmap/` directory at the repo root. Each roadmap item is a single markdown file at `roadmap/<slug>.md` with YAML frontmatter and a free-text body. The slug follows the same character rules as change slugs (`^[a-zA-Z0-9_-]{1,64}$`). Roadmap items are operator-editable idea records — NOT queue inputs, NOT autocoder-owned, NOT subject to gate enforcement.

Required frontmatter fields:
- `title` (string) — one-line description of the feature or idea.
- `status` (enum) — one of `proposed`, `considering`, `planned`, or `deferred`.
- `added` (date, `YYYY-MM-DD`) — creation date set by the creator.

No other frontmatter fields are required. The body is free-text markdown with no required sections.

Status vocabulary:
- `proposed` — idea raised; not yet evaluated.
- `considering` — under active consideration; may become an OpenSpec change.
- `planned` — accepted for a future change; timing not yet committed.
- `deferred` — explicitly set aside; kept for reference.

The queue engine (`list_pending`) SHALL NOT enumerate `roadmap/` items. Roadmap items are invisible to the implementation pipeline by position (they are not under `openspec/changes/`), so no exclusion logic is required. Agents implementing OpenSpec changes MUST NOT delete roadmap items without explicit operator instruction; lifecycle transitions (`proposed` → `considering` → `planned`, or `planned` → `deferred`) are made via operator or agent edits to the `status` field.

#### Scenario: Roadmap item is not visible to list_pending
- **WHEN** `roadmap/concurrent-executor.md` exists with valid frontmatter
- **AND** the queue engine enumerates pending changes
- **THEN** `concurrent-executor` does NOT appear in the returned list
- **AND** no exclusion logic or gitignore entry is required to achieve this

#### Scenario: Valid roadmap item round-trips through git
- **WHEN** a roadmap item is created, committed, and pushed
- **THEN** `roadmap/<slug>.md` is a tracked file in the repository
- **AND** human contributors and agents can read and edit it without a gate or lock

### Requirement: `discuss`-mode artifact creation routes future-feature ideas to roadmap items
When the discuss artifact-creation agent (invoked after `send it`) determines that the operator's request is an aspirational or explicitly deferred future feature — one that is not yet scoped, not urgent, and does not have a clear implementation plan — the agent SHALL create `roadmap/<slug>.md` with the appropriate frontmatter and body rather than a full `openspec/changes/<slug>/` directory.

The agent SHOULD choose `roadmap/` when the operator's language is aspirational ("it would be nice if", "in the future", "someday", "thinking ahead", "a rough idea"), when the feature lacks a concrete spec-ready description, or when the operator has not expressed intent to prioritize it. The agent SHALL choose `openspec/changes/` when the operator's request is a concrete, ready-to-spec behavior change and the discussion has produced a clear implementation plan. When uncertain, the agent SHALL ask the operator in the thread before running `send it`.

The `prompts/discuss-mode.md` prompt SHALL instruct the agent on this routing decision AND reference the format defined in `OCTOPUS.md`'s `## Roadmap items` section and the `roadmap/` directory as the correct destination.

#### Scenario: Aspirational feature idea produces a roadmap item
- **WHEN** an operator's discuss thread contains "it would be nice if the executor could run in parallel someday" AND `send it` is received
- **THEN** the agent creates `roadmap/<slug>.md` (not `openspec/changes/<slug>/`)
- **AND** the frontmatter contains `status: proposed` and a suitable `title`
- **AND** the PR body names the roadmap item and explains it is a roadmap entry, not a spec

#### Scenario: Concrete scoped request produces an openspec change
- **WHEN** the operator's discuss thread has a concrete implementation plan for a new feature AND `send it` is received
- **THEN** the agent creates `openspec/changes/<slug>/` with `proposal.md` and `tasks.md` (not a roadmap item)
