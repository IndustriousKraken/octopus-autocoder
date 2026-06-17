# Issue: issues lane lives under `openspec/` instead of the canonical root `issues/`

## Report

The issues-lane implementation stores units under `openspec/issues/<slug>/`
(active) and `openspec/issues/archive/` (completed). The canonical spec mandates
the lane live at the repository root, NOT under `openspec/`.

## Diagnosis (code drifted from existing canon)

The `Issues lane for corrections` requirement (orchestrator-cli) states:

> "a second work lane, `issues/`, for corrections … An issue SHALL be a
> directory `issues/<slug>/` … On completion the issue directory SHALL move to
> `issues/archive/`, mirroring `changes/archive/` …"

Canon refers to the path as `issues/<slug>/` and `issues/archive/` throughout —
there is no `openspec/issues` anywhere in `openspec/specs/`. The code is the
deviation: `lanes/issues.rs` defines `ISSUES_SUBDIR = "openspec/issues"`, and the
path is then echoed in agent-facing prompts, operator docs, and tests. Nesting
the lane under `openspec/` also mislabels it as an OpenSpec artifact, when issues
are autocoder's own construct (the `openspec` CLI never reads or manages them).

This carries NO spec delta: the fix brings the code into conformance with what
canon already specifies. It is a behavior-preserving correction — the lane's
logic, lifecycle, precedence, and dedup are unchanged; only the on-disk location
moves from `openspec/issues/` to `issues/`.

Path is centralized: `lanes::issues::{issues_dir, issue_dir, archive_root}` all
derive from the single `ISSUES_SUBDIR` constant, so the logic move is one
constant. The remaining references are hardcoded strings in prompts, docs, and
tests, plus a per-repo migration concern for repositories that already hold an
`openspec/issues/` tree.

## Acceptance criteria (against the existing specification)

- Active issues are created, selected, worked, and archived at `issues/<slug>/`
  and `issues/archive/<date>-<slug>/`, exactly as the `Issues lane for
  corrections` requirement states — confirmed by `lanes::issues` resolving to
  the root `issues/` path.
- No `openspec/issues` reference remains in code, agent-facing prompts, or
  operator docs (grep-clean).
- A repository that already holds an `openspec/issues/` tree (active and/or
  archived units) is not broken by the move: its active issues are still worked
  and its archived issues are still seen by ingestion dedup. No active issue is
  orphaned and no archived issue becomes invisible to dedup (which would risk a
  re-created duplicate).
- The walker still skips `archive/`; archived issues are never re-implemented by
  the move.
