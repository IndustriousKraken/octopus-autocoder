# Tasks

## 1. Lane-aware WritePolicy

- [ ] 1.1 Add `WritePolicy::PlanningLanes` in `audits/mod.rs` (`workspace_writable()` returns `true`). Its post-run write-scope check accepts any modified path under `openspec/changes/` OR `issues/` and reverts anything else through the existing `git reset --hard HEAD && git clean -fd` path (generalize the current `OpenSpecOnly` single-prefix check to the two-prefix allowlist).
- [ ] 1.2 Switch `security_bug_audit` and `missing_tests_audit` to `WritePolicy::PlanningLanes`. Leave `canon_consolidation_audit` on `OpenSpecOnly` (spec-lane by definition). Update the `audits/mod.rs` assertion that specs-writing audits get a writable workspace so it covers the new policy.
- [ ] 1.3 The post-run commit step stages both planning lanes (`git add openspec/changes/ issues/`) and counts the produced units in the commit subject (`audit: <type> proposals (N unit(s))`).

## 2. Lane-choice plumbing

- [ ] 2.1 Resolve `features.issues.enabled` for the repository the audit runs against and thread it into the audit's prompt input, so the issue lane is offered only when the lane is enabled. When it is off, the audit offers the spec lane only.
- [ ] 2.2 Collect produced unit directory names across BOTH lanes and return them via `AuditOutcome::SpecsWritten(names)`, so the same iteration's lane walkers (changes walker, issues walker) pick them up under the established `issues > changes` precedence.

## 3. Prompts

- [ ] 3.1 In `prompts/security-bug-audit.md` and `prompts/missing-tests-audit.md`, instruct the agent to read the canonical spec(s) for the area of each finding BEFORE proposing a fix, reuse canonical vocabulary, and prefer a `MODIFIED` delta of an existing requirement over an `ADDED` requirement that coins a parallel term.
- [ ] 3.2 Add the lane-choice instruction: a fix that changes an observable contract → spec lane (`openspec/changes/<slug>/`); a behavior-preserving fix to already-correctly-specified code → issue lane (`issues/<slug>/` with `issue.md` stating acceptance against the EXISTING specification, plus `tasks.md`, no `specs/`). Offer the issue lane only when `features.issues` is enabled (per the threaded flag). Forbid defaulting to the spec lane.
- [ ] 3.3 Add the legibility rule: when a fix genuinely requires changing a canonical contract, write a spec and state the contract change plainly in the proposal's rationale — never bury a contract change inside an issue.

## 4. Tests

- [ ] 4.1 `PlanningLanes`: a run whose only writes are under `issues/` survives and commits; a run whose only writes are under `openspec/changes/` survives; a write to any other path triggers the revert and a failed-run outcome. Assert behavior (surviving artifacts / revert), not prompt or message wording.
- [ ] 4.2 Lane routing (derive from produced artifacts, not prompt substrings): with `features.issues` enabled, a behavior-preserving finding yields an `issues/<slug>/` unit with no `specs/` directory, and a contract-changing finding yields an `openspec/changes/<slug>/` unit; with `features.issues` disabled, only `openspec/changes/` units are produced.
- [ ] 4.3 The commit stages both lanes and `SpecsWritten` carries unit names regardless of which lane produced them.
- [ ] 4.4 Regression: `canon_consolidation_audit` still runs under `OpenSpecOnly` and reverts a write under `issues/`.

## 5. Docs

- [ ] 5.1 Update `docs/OPERATIONS.md` (and the audit reference in `docs/CHATOPS.md` if present) to state that the two bug/gap audits choose their lane by canon judgment, that issues are a first-class audit output, and that the issue lane is offered only when `features.issues` is enabled.
