# Roadmap items: a lightweight future-feature record convention

## Why

Future-feature ideas have no lightweight home in the repo today. An issue
requires a code defect; an OpenSpec change requires full spec authorship. An
idea that is early, speculative, or deliberately deferred is forced into one of
those heavier forms or ends up in an ad-hoc file.

A `roadmap/` directory of one-file-per-idea records gives these ideas a home
that is searchable, linkable, and human- and agent-editable, and — unlike canon
— carries no gate enforcement. It is created naturally by the discuss → send it
flow (see `discuss-verb-conversational-propose`, which lists a roadmap item as
one of its output artifact types) and stays out of the implementation queue.

## What Changes

- A `roadmap/` directory is added at the repo root (tracked via `.gitkeep`).
- The roadmap convention — file format, frontmatter, status values, lifecycle —
  is documented in `OCTOPUS.md`.
- One canonical requirement defines the roadmap item format so agents produce
  and read them consistently.

Roadmap items are NOT queue input, and that needs no new code: the queue engine
enumerates only `openspec/changes/`, so files under `roadmap/` are already out
of its scope by position. No exclusion logic and no gitignore entry is required.

## Format

`roadmap/<slug>.md`:

```markdown
---
title: <one-line description>
status: proposed | considering | planned | deferred
added: YYYY-MM-DD
---

Free-text body. No required sections.
```

Status: `proposed` (raised, not evaluated) → `considering` (may become a change)
→ `planned` (accepted, timing uncommitted); `deferred` (set aside, kept for
reference). Operators and agents move an item by editing its `status` field.

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement (roadmap item
  format + documentation).
- Affected files: `OCTOPUS.md` (new section), `roadmap/.gitkeep` (new).
- No code behavior changes; no queue-engine changes.
