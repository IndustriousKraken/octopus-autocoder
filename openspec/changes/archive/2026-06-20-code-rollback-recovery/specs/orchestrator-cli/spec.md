## ADDED Requirements

### Requirement: List recent commits for a managed repository
The orchestrator SHALL provide a way to list a managed repository's recent commits from one place, so an operator choosing a rollback depth can see the history without leaving the management surface. It SHALL be available BOTH as a CLI subcommand AND as a chatops verb (`@<bot> log <repo-substring> [<count>]`), resolving the repository by the same selector rule the other operator commands use. The listing SHALL show, per commit, at least the short SHA, the subject, AND the commit date, newest first, bounded by a count (default a small page, e.g. 20). This is a read-only command: it SHALL NOT modify any branch, workspace, or marker.

#### Scenario: Listing shows recent commits newest-first
- **WHEN** an operator requests the commit log for a resolved repository with a count N
- **THEN** the response lists up to N of the base branch's most recent commits, newest first, each with its short SHA, subject, AND date
- **AND** no branch, workspace, or marker is modified

#### Scenario: Selector resolution matches the other operator commands
- **WHEN** the repo-substring resolves to zero OR multiple repositories
- **THEN** the response reports the ambiguity / no-match the same way the other operator commands do (listing candidates), rather than acting on a guess

### Requirement: Code-rollback recovery rolls back code while unarchiving its specs and issues
The orchestrator SHALL provide a recovery operation that rolls a managed repository's CODE back by a chosen depth WHILE preserving the OpenSpec changes AND issues that were archived in the rolled-back range — moving them back to the active lanes rather than discarding them. The motivating case: code that merged WITHOUT being gate-checked is not to be trusted, but the spec/issue work that drove it is sound AND should re-enter the pipeline to be re-implemented under the controls. A plain `git reset`/`revert` cannot express this, because the orchestrator commits the implementation, the archive move, AND the canonical-spec fold together — so reverting the commits would discard the spec work entirely, back to before it existed.

The operation SHALL accept a rollback depth as EITHER a commit count (roll back the last N commits) OR a target commit SHA (roll back to that commit), resolved against the repository's base branch.

The operation SHALL ride the normal push + PR flow rather than force-pushing the base branch directly: it prepares the rolled-back state on the agent branch AND goes through the SAME push + PR-creation path as any change, honoring the per-repo `auto_submit_pr` setting — a pull request the operator reviews AND merges when `auto_submit_pr` is enabled (the default), OR a pushed agent branch with no PR (the `BranchPushedNoPr` outcome) when an installation has set it false. The operation SHALL NOT special-case a force-push to the base branch; it produces reviewable commits through the established flow, AND git history remains the backstop.

Within the rolled-back range, the operation SHALL:

- Restore the CODE (every path outside `openspec/` AND outside the issues lane) to its state at the rollback target — the untrusted implementation is discarded.
- For each OpenSpec change archived in the range, UNARCHIVE it: the change returns to `openspec/changes/<slug>/` (active), its canonical-spec fold is undone, so it is pending again AND will be re-gated AND re-implemented. It is NOT reverted to non-existence.
- For each issue archived in the range, UNARCHIVE it: the issue unit returns from `issues/archive/` to the active `issues/` lane.
- Leave changes/issues archived OUTSIDE the range untouched (still archived, canon intact).

The operation SHALL be fail-loud AND reviewable: the PR body SHALL enumerate the commits rolled back, the changes/issues unarchived, AND state plainly that the code was discarded while the specs/issues were returned to the pipeline. Because it discards code, the operation SHALL require explicit confirmation before it acts (a confirmation prompt for the CLI, OR a two-step confirm for the chatops verb), mirroring the other destructive operator commands. A dry run (default for the CLI, OR an explicit preview) SHALL report exactly what WOULD be rolled back AND unarchived without changing anything.

#### Scenario: Rollback by count discards code and unarchives specs via the normal flow
- **WHEN** an operator rolls a repository back by N commits AND those commits archived one or more changes/issues
- **THEN** it rides the normal push + PR flow (opening a PR when `auto_submit_pr` is enabled, the default; otherwise a pushed branch with no PR), NOT a force-push to base, with the agent branch's tree holding the code restored to the rollback target
- **AND** each change archived in the range is moved back to `openspec/changes/<slug>/` with its canon fold undone (active, to be re-gated and re-implemented)
- **AND** each issue archived in the range is moved back to the active `issues/` lane
- **AND** the PR body enumerates the rolled-back commits AND the unarchived changes/issues

#### Scenario: Rollback to a SHA is equivalent to the count form
- **WHEN** an operator rolls back to a specific commit SHA instead of a count
- **THEN** the same restore-code / unarchive-specs-and-issues operation runs against that target
- **AND** the result is a PR with identical structure to the count form

#### Scenario: Specs and issues archived outside the range are untouched
- **WHEN** the rollback range covers some archived changes/issues but not others
- **THEN** only the changes/issues archived WITHIN the range are unarchived
- **AND** changes/issues archived before the range stay archived AND their canon fold is intact

#### Scenario: Confirmation is required and a dry run changes nothing
- **WHEN** the operation is invoked without confirmation (OR in dry-run/preview mode)
- **THEN** it reports the commits it WOULD roll back AND the changes/issues it WOULD unarchive
- **AND** it does NOT modify any branch, workspace, archive, or canon until the operator confirms

#### Scenario: Code-only range is a plain rollback through the normal flow
- **WHEN** the rolled-back range archived NO changes AND NO issues (code-only commits)
- **THEN** the rolled-back state restores the code to the target with no unarchive step AND rides the normal push + PR flow (a PR when `auto_submit_pr` is enabled)
- **AND** the PR body (or push notification) says the rollback was code-only
