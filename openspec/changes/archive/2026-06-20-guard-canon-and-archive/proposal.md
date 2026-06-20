# Guard canon and the archive from executor sessions

## Why

Folding a change's deltas into `openspec/specs/` and archiving the change are
autocoder's post-merge responsibilities. Nothing currently stops an executor
session from doing them mid-run, and one did: a spec-implementing/revision
session both archived a change AND wrote four canonical spec files in the same
PR. That bypasses the post-executor gate and double-applies the deltas on merge
(the archive folds them again), which is why that PR had to be discarded.

Documentation (OCTOPUS.md) states the rule, but a prompt-driven, sandboxed agent
needs more than a document it might not read. The robust fix is defense in depth:
teach the agent (prompt → OCTOPUS.md), prevent the action (sandbox), and catch it
regardless (post-session revert) — the last being fail-closed, so the guarantee
does not rest on the agent having complied.

The invariant underneath is simple and general: **canon and the archive are
autocoder-only.** Every session may write its own planning artifact — a change's
`openspec/changes/<slug>/` (deltas included) or an issue's `issues/<slug>` unit,
plus the implementer's code — but `openspec/specs/` and the archive are the
daemon's alone, after implementation and merge.

## What Changes

- A new `orchestrator-cli` invariant: no executor session (implementer,
  spec-revision executor, spec-writing audit, triage, proposer) modifies
  `openspec/specs/` or archives a change, enforced in three layers:
  1. **Teach** — the session prompt points at `OCTOPUS.md` for the conventions
     and constraints when present (graceful no-op when absent, so this does not
     depend on `octopus-md-agent-guide` landing first).
  2. **Prevent** — the session sandbox denies `openspec archive` execution and
     writes under `openspec/specs/`; the change's own `changes/<slug>/specs/`
     delta dir and the implementer's code edits stay writable.
  3. **Catch** — after the session and before commit, any `openspec/specs/` edit
     or new archive entry the session produced is reverted and surfaced, reusing
     the audit planning-lane post-hoc-revert pattern.

## Impact

- Affected specs: `orchestrator-cli` (ADD the canon/archive invariant + the
  three-layer enforcement).
- Affected code: the executor sandbox config for spec-writing/implementing/
  revision roles (deny `openspec archive`, deny `openspec/specs/` writes); the
  spec-session prompts (reference OCTOPUS.md when present); and a post-session
  diff check (revert `openspec/specs/` edits + new `changes/archive/` or
  `issues/archive/` entries the session produced, alert on violation) — modeled
  on the audit `PlanningLanes` post-hoc revert.
- Independent of `octopus-md-agent-guide` (the prompt reference degrades
  gracefully when OCTOPUS.md is absent) and of `single-file-issues`. This is the
  change that actually closes the canon/archive hole the discarded PR exposed.
