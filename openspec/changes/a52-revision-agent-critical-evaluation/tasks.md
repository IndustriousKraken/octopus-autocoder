# Implementation tasks

## 1. Extend the revision prompt with critical-evaluation guidance

- [ ] 1.1 In `prompts/implementer-revision.md`, extend the outcome-signal section (added by a45) with guidance directing the agent to evaluate the request critically before applying it: read the actual code at the cited location, verify the request's claim against the current state, and decline OR partially honor the request when the claim is wrong (mistaken about the code, would break a passing or spec-traced test, references a symbol that does not exist, churns working idiomatic code for protection that does not apply).
- [ ] 1.2 State that declining a wrong request is a valid, successful outcome the agent reports via `outcome_success`'s `final_answer` (naming the request, the verification performed, and why it declined or partially honored) — NOT a failure, and NOT grounds to fabricate a change that satisfies the literal request at the cost of correctness.
- [ ] 1.3 Keep the guidance language-neutral: reference "the project's test and lint commands" rather than a specific toolchain. Do NOT introduce `cargo`/`clippy`/language-specific commands into the prompt (a51 sweeps the prompts for these; this change must not add new instances).

## 2. Make a no-change declination a reported success (not a false failure)

- [ ] 2.1 In `autocoder/src/revisions.rs`, in the `Ok(ExecutorOutcome::Completed { .. })` arm (~line 943), determine whether the agent produced code changes before committing — query the working-tree dirty state (e.g. a `git status --porcelain` helper) rather than committing unconditionally.
- [ ] 2.2 Dirty tree: keep the existing path — `apply_revision_commit` (add_all + commit + force-with-lease push) + the a45 success comment carrying `final_answer`. A genuine commit/push failure still routes to the failure comment + cap increment (unchanged).
- [ ] 2.3 Clean tree (deliberate no-change declination): do NOT call `apply_revision_commit` (no empty commit, no push) and do NOT post a `✗ Revision attempt failed` comment. Post a success comment whose first line marks an evaluation with no change made — distinct from `✅ Revision applied:` (e.g. `✅ Revision evaluated, no change made: <subject>. Revision count: <n> of <cap>.`) — followed, when `final_answer` is non-empty after trimming, by a blank line AND the `final_answer` text (passed through the same `truncate_to_fit` helper a45 uses). When `final_answer` is empty, post the no-change line alone.
- [ ] 2.4 Both branches increment `state.revisions_applied`, advance the seen-marker, and write state. The clean-tree branch fires the same chatops success notification path as the dirty branch (the revision was processed, just with no diff).

## 3. Tests

- [ ] 3.1 Unit test: a `Completed { final_answer: Some("Declined: <reason>") }` outcome with a CLEAN working tree (stub the dirty-check to clean) posts a success comment whose first line marks no-change evaluation AND contains the `final_answer` reasoning; asserts NO commit/push occurred AND NO `✗ Revision attempt failed` comment; asserts the cap counter incremented.
- [ ] 3.2 Unit test: a `Completed` outcome with a DIRTY working tree commits + pushes + posts `✅ Revision applied:` with `final_answer` (a45 behavior preserved).
- [ ] 3.3 Unit test: a genuine commit/push failure on the dirty path still posts the `✗ Revision attempt failed` comment AND increments the cap (existing behavior unregressed).
- [ ] 3.4 The revision prompt's critical-evaluation guidance (task 1) is prompt content, NOT under test — its fitness is governed by review AND the drift audit (per `Tests assert behavior or derivation, never message wording`).

## 4. Spec deltas

- [ ] 4.1 `specs/executor/spec.md` — ADD `Revision prompt instructs critical evaluation of the reviewer's request`.
- [ ] 4.2 `specs/orchestrator-cli/spec.md` — MODIFY `Revision execution updates the agent branch and posts a reply comment` per this change's delta (stacks on a45; reproduces a45's version + the clean-tree declination scenario).

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate.
- [ ] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 5.3 `openspec validate a52-revision-agent-critical-evaluation --strict` passes.
