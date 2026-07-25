# openspec-queue-engine delta: archive-rides-completion-commit

## MODIFIED Requirements

### Requirement: Archive on completion
The queue engine SHALL move successfully implemented changes into a dated archive subdirectory, and the completion commit SHALL capture that move: the archive step runs BEFORE the completion commit is recorded, its file move (and any canonical-spec merge the archive subprocess performs) is staged together with the implementation diff, and ONE commit carries them all. No committed tree on the agent branch may show a completed change still active under `openspec/changes/`. This ordering applies to every completion path — the pending-change walk AND the waiting-change resume path alike. A committed-but-still-active completed change is a re-implementation hazard: once its commit merges to the base branch, the still-active change directory re-enters the pending queue and the full pipeline (pre-executor gates, executor, reviewer) re-runs — and re-bills — for work that is already done.

#### Scenario: Archiving a completed change
- **WHEN** the executor returns `Completed` for `<change>` with a workspace diff
- **THEN** the queue engine renames `<workspace>/openspec/changes/<change>/` to `<workspace>/openspec/changes/archive/<YYYY-MM-DD>-<change>/`, where `<YYYY-MM-DD>` is the UTC date of the rename, BEFORE the completion commit is recorded
- **AND** the completion commit attributable to that change captures both the implementation diff AND the archive move — its committed tree contains the dated archive entry and does NOT contain `openspec/changes/<change>/`
- **AND** if the destination path already exists, the engine returns an error naming the conflict and does NOT overwrite the existing archive entry

#### Scenario: Resume path uses the same ordering
- **WHEN** a waiting change resumes after a human answer AND the resumed executor outcome is `Completed` with a workspace diff
- **THEN** the archive step runs before the completion commit and that one commit captures both the implementation diff and the archive move, exactly as on the pending path
- **AND** there is no window in which the branch's committed history carries the completed change as active with the archive move left uncommitted

#### Scenario: Archive failure records no completion commit
- **WHEN** the archive step fails for `<change>` on either completion path
- **THEN** no completion commit is recorded for `<change>` (the failure short-circuits before staging or commit)
- **AND** the change stays at its active path and the existing per-change failure handling applies
