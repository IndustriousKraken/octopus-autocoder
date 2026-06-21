# OCTOPUS.md — in-repo agent & contributor guide

This repository is managed by [Octopus Autocoder](https://github.com/rab-autocode/octopus-autocoder).
It carries two workflow protocols any agent or human working in the repo must
follow: an **issues** lane for corrections, and an **OpenSpec change** lane for
behavior changes. This file states both, plus the ownership rules for canon and
the gate model. It is the in-repo reference for readers who are not autocoder's
own gated agents — a coding assistant or speccing agent run directly on the
repo, and human teammates. Autocoder's own agents have these same rules enforced
by the verifier gates and the session sandbox; this file documents them but does
not replace that enforcement.

## Issues protocol (corrections, no spec delta)

An **issue** is a correction: a fix to code that is already correctly specified.
An issue carries **no spec delta** and never contains a `specs/` directory.

An issue takes ONE of two on-disk forms under `issues/`:

- **Single file** — `issues/<slug>.md` (the default): a description, plus an
  optional `## Tasks` checklist.
- **Directory** — `issues/<slug>/` containing `issue.md` AND `tasks.md`:
  required only when the unit must carry a separate artifact (for example a
  quarantined public report body).

Use an issue when the spec is right and the code is wrong. Use a change (below)
when the desired behavior is not yet specified.

## OpenSpec change protocol (behavior changes)

A **change** lives in `openspec/changes/<slug>/` and contains:

- `proposal.md` — why and what changes,
- `tasks.md` — the implementation checklist,
- `design.md` — optional, for non-trivial design decisions,
- spec deltas at `specs/<capability>/spec.md`.

Spec deltas use `## ADDED Requirements`, `## MODIFIED Requirements`,
`## REMOVED Requirements`, and `## RENAMED Requirements` blocks.

A `## MODIFIED Requirements` block **reproduces the canonical requirement's
title EXACTLY** and **retains every existing scenario**. A scenario dropped from
a MODIFIED block silently deletes that scenario from canon when the change is
archived — so a MODIFIED block must restate the full requirement, not just the
part being changed.

Every change MUST pass `openspec validate --strict` before it is considered
ready.

## Canon and archive are autocoder-owned

The canonical specifications under `openspec/specs/` and the archive under
`openspec/changes/archive/` are **autocoder-owned**. A working session writes
ONLY its own change/issue planning artifacts and code. It:

- never edits `openspec/specs/` (canon) directly, and
- never runs `openspec archive`.

Autocoder folds a change's deltas into canon at archive time, after the change
is merged. A change must not pre-apply its own ADDED delta into
`openspec/specs/` — doing so makes the archive abort on a duplicate
requirement.

## The gate model (gatekeepers fail closed)

A change passes through gatekeepers before its work lands. Each fails closed —
an inability to run a gate is a non-passing outcome, never a pass:

- `[in]` — the change does not contradict itself.
- `[canon]` — the change does not contradict canon, unless it explicitly
  modifies the contradicted requirement.
- `[rules]` — the change conforms to the global engineering rules.
- `[out]` — the merged code implements the spec.

## Further reading

For the fuller OpenSpec documentation, see https://github.com/Fission-AI/OpenSpec.
