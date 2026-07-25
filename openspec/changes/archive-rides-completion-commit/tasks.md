# Tasks: archive-rides-completion-commit

## 1. Reorder the resume completion path

- [ ] 1.1 In `autocoder/src/polling_loop/queue_waiting.rs` `resume_completed`, compute the commit subject first (the change directory must still be at its active path when the subject is derived), then run the archive step, then `git::add_all` + `git::commit` — matching the pending path in `polling_loop/outcome.rs`, so the single completion commit captures the implementation diff and the directory move together.
- [ ] 1.2 Keep failure semantics aligned with the pending path: when the archive step fails, return the error before any staging or commit — no partial completion commit is recorded, and the change stays at its active path.

## 2. Tests

- [ ] 2.1 Add a test for the resume-to-completed flow asserting: exactly one new commit is recorded, its tree contains the dated archive entry for the change, and its tree does NOT contain the change's active directory; the working tree is clean afterwards.
- [ ] 2.2 Add (or adjust) a test asserting the archive-failure path on resume records no commit.

## 3. Validation

- [ ] 3.1 Run `openspec validate archive-rides-completion-commit --strict` and fix any findings.
