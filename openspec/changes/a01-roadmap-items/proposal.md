# Roadmap items: lightweight per-file future-feature records

## Why

Capturing future-feature ideas currently has no lightweight home in the repo.
Issues require a code defect; changes require full spec authorship. A future
feature idea that is too early, too speculative, or deliberately deferred has
nowhere to land without either forcing a premature spec or cluttering an ad-hoc
file.

A `roadmap/` directory of individual markdown files gives these ideas a home
that:

- Is searchable and linkable (one file per idea, named by slug).
- Is easy to create via the `discuss` → `send it` flow without requiring full
  spec authorship.
- Stays OUT of the queue engine (autocoder does not automatically implement
  roadmap items — they are ideas, not work orders).
- Is editable by humans and agents alike (unlike `openspec/specs/` which is
  autocoder-owned and immutable between changes).

## Format

Each roadmap item lives at `roadmap/<slug>.md`:

```markdown
---
title: <one-line description>
status: proposed | considering | planned | deferred
added: YYYY-MM-DD
---

Free-text body describing the feature, motivation, and any known constraints or
open questions. No required sections. Keep it as short or long as the idea warrants.
```

`status` values:
- `proposed` — idea has been raised; not yet evaluated.
- `considering` — under active consideration; may become a spec.
- `planned` — accepted for a future spec; timing not yet committed.
- `deferred` — explicitly set aside; kept for reference.

The `added` date is the creation date (set by the creator; not auto-updated).
No other frontmatter fields are required.

## What Changes

- `roadmap/` directory is created at the repo root (tracked by git).
- `OCTOPUS.md` gains a `## Roadmap items` section explaining the format to
  agents and human contributors.
- The `discuss` verb's artifact-creation prompt (`prompts/discuss-mode.md`)
  is updated to instruct the agent that when the operator's request is a
  future-feature idea (not yet specced, not urgent, explicitly aspirational),
  the appropriate artifact is a new `roadmap/<slug>.md` rather than a full
  `openspec/changes/<slug>/` directory.
- Roadmap items are NOT automatically processed by the queue engine and MUST
  NOT appear under `openspec/changes/` or `issues/`.
- Agents implementing OpenSpec changes SHALL treat `roadmap/` as a searchable
  reference (the `discuss` prompt proactively reads it; implementing agents MAY
  read it for context). Agents MUST NOT delete roadmap items without operator
  instruction; items transition to `planned` or `deferred` via operator edits.

## Impact

- Affected specs: `orchestrator-cli` — two ADDED requirements (roadmap item
  format, discuss-mode prompt roadmap guidance). `project-documentation` spec
  update is NOT needed; this change owns OCTOPUS.md and `roadmap/`.
- Affected files: `OCTOPUS.md` (new section), `roadmap/` (new directory, empty
  initially), `prompts/discuss-mode.md` (new file, or add section when that
  file is created by `discuss-verb-conversational-propose`).
- No queue-engine changes: the `list_pending` enumeration already excludes
  `roadmap/` (it only walks `openspec/changes/`). No exclusion logic needed.
