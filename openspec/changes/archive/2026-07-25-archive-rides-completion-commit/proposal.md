# archive-rides-completion-commit

## Why

The waiting-change resume path completes a change in the wrong order: it commits the implementation diff first and performs the archive step afterwards, leaving the archive rename (and the canonical merge the archive subprocess performs) uncommitted in the working tree. The next pass's dirty-workspace recovery destroys that uncommitted state, so the completion commit that ships in the PR carries the change still active under `openspec/changes/`. Once that PR merges, the still-active change directory re-enters the pending queue and the full pipeline — pre-executor gates, executor, reviewer — re-runs and re-bills for work that is already done. This is the same doom-loop geometry as the July 13 completed-change re-implementation incident, surviving on the resume path only.

The pending path already does this correctly (archive before commit, one commit capturing both), but the ordering contract is stated inconsistently across the requirements: the queue-engine "Archive on completion" requirement and two orchestrator scenarios still describe the old commit-then-archive order, while the shared archive-helper requirement's self-heal scenario implies archive-then-commit.

## What Changes

- The completion ordering contract becomes uniform and explicit: the archive step runs BEFORE the completion commit is recorded, its output is staged with the implementation diff, and ONE commit captures both — on the pending path AND the resume path. No committed tree on the agent branch may show a completed change still active.
- The resume-completed handler is reordered to match the pending path.
- The two orchestrator scenarios that still narrate commit-then-archive are corrected to the same ordering.

## Impact

- Affected specs: `openspec-queue-engine` (Archive on completion), `orchestrator-cli` (Daemon entry point; autocoder invokes openspec archive for the archive step)
- Affected code: `autocoder/src/polling_loop/queue_waiting.rs` (resume_completed)
