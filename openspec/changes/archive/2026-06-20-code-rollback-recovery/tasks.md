# Tasks

## 1. Commit-log listing

- [x] 1.1 Add a read-only commit-log lister for a resolved repo's base branch (short SHA, subject, date, newest-first, bounded by a count with a small default). Expose it as a CLI subcommand AND a chatops verb `@<bot> log <repo-substring> [<count>]`, using the existing repo-selector resolution and ambiguity/no-match replies. It modifies nothing.

## 2. Range → archived-units resolver

- [x] 2.1 Given a rollback target (a commit count N OR a target SHA) on the base branch, compute the rolled-back commit range AND the set of OpenSpec changes AND issues archived within it (map archive moves / `openspec/changes/archive/`, `issues/archive/` entries introduced in the range to their slugs). This set is what gets unarchived; everything archived outside the range is left alone.

## 3. The rollback operation (PR-based)

- [x] 3.1 Prepare the rolled-back state on the agent branch: restore every path OUTSIDE `openspec/` and the issues lane to the rollback target (discard the untrusted code); for each in-range change, unarchive it to `openspec/changes/<slug>/` with the canon fold undone (reuse `queue::unarchive`); for each in-range issue, move it from `issues/archive/` back to the active `issues/` lane.
- [x] 3.2 Open a PR (not a direct base push) with this state, reusing the normal PR-assembly path. The PR body enumerates the rolled-back commits AND the unarchived changes/issues AND states plainly that code was discarded while specs/issues were returned to the pipeline.
- [x] 3.3 Accept the rollback depth as a commit count OR a target SHA (both resolve to the same operation).

## 4. Safety: confirm + dry-run

- [x] 4.1 Require explicit confirmation before acting (CLI prompt; chatops two-step confirm), mirroring the other destructive operator commands. Provide a dry-run/preview (default for the CLI) that reports exactly what would be rolled back AND unarchived without changing any branch, workspace, archive, or canon.

## 5. Edge cases

- [x] 5.1 A code-only range (no archived changes/issues) opens a plain rollback PR with no unarchive step; the PR body says code-only.
- [x] 5.2 An in-range change/issue whose unarchive would collide (e.g. an active dir of the same slug already exists) is reported in the PR body / preview rather than silently overwritten.

## 6. Tests

- [x] 6.1 The resolver maps an N-commit range to exactly the changes/issues archived within it; units archived outside the range are excluded.
- [x] 6.2 The prepared tree has code at the target AND the in-range changes active (canon fold undone) AND the in-range issues active.
- [x] 6.3 Count and SHA forms produce identical structure.
- [x] 6.4 Dry-run/preview changes nothing; confirmation is required before acting.
- [x] 6.5 The commit-log verb lists newest-first and modifies nothing.
