## Why

Production-observed bug across multiple repos: `security_bug_audit` (and other spec-writing audits) fires the `🔍 created proposal …` chatops notification, commits the proposal directories to the agent branch, AND then the proposals vanish — no PR opens, no commits reach the remote, and the next iteration's workspace re-init silently destroys the local commits. The user-visible symptom is "notifications without PRs," and the repos appear idle while the audit re-creates the same proposals every cadence-due iteration and loses them again.

**Trace:**

1. `run_pass_through_commits` runs the pending queue walk first (no implementer work this iteration → `processed = []`), then runs the audit phase per the canonical "audit phase runs AFTER the queue walk" requirement. The audit writes `openspec/changes/secure-*` directories, validates them, fires the `🔍` notification, AND `git commit`s them as `audit: security-bug proposals (N change(s))`. The audit's commits are now on the local agent branch.

2. `run_pass_through_commits` returns `(processed = [], false)` to the caller in `polling_loop.rs`.

3. **`autocoder/src/polling_loop.rs:702-708` (the bug):**
   ```rust
   if processed.is_empty() {
       let _ = AlertState::clear(workspace);
       return Ok(());
   }
   ```
   Early return fires. **The push step at line 779 never executes.** The audit's commits sit on the local agent branch, never pushed to origin.

4. The downstream `commit_count == 0` check at line 712-719 IS the correct gate for "agent branch is ahead of base," but it sits BEHIND the wrong `processed.is_empty()` gate, so it never runs in this case.

5. Next iteration: `workspace::ensure_initialized` → `git::recreate_branch(agent_branch)` → `git checkout -B agent-q` resets the agent branch from `origin/base_branch`. The unpushed audit commits are silently destroyed. The `secure-*` proposal directories vanish. `git log` shows no record of the work.

6. Operator-visible: `🔍` notification fires (it's posted BEFORE the commit per `specs_writing.rs:285-297`); no PR opens; repos go idle; every subsequent audit cadence re-creates the same proposals and re-loses them.

The canonical `orchestrator-cli` spec at line 1237 already mandates the correct behavior: *"the audit's creation commits ship in iteration N's PR."* The implementation is non-conformant.

The bug class is broader than this single defect: any code path that decides "iteration done, nothing to ship" based on a HIGHER-LEVEL signal (implementer-queue processing outcome, audit-queue processing outcome, etc.) can miss the actual end-state of the agent branch. The correct termination gate is the agent-branch commit count itself.

## What Changes

**Remove the implementer-queue-based early return.** The check at `polling_loop.rs:702-708` (`if processed.is_empty()`) SHALL be removed. The downstream `commit_count == 0` check at line 712-719 is the correct gate for "no work to push" and handles all of:

- Implementer empty + audit empty → commit_count == 0 → return (existing behavior preserved)
- Implementer empty + audit produced commits → commit_count > 0 → proceed to push + PR (the bug fix)
- Implementer non-empty + audit empty → commit_count > 0 → proceed (existing happy path preserved)
- Implementer non-empty + audit non-empty → commit_count > 0 → proceed (existing happy path preserved)

**Reviewer behavior for audit-only iterations.** When `processed.is_empty()` but `commit_count > 0`, the reviewer SHALL be skipped — there are no implementer-modified files to review; the only commit is the audit's mechanical proposal-writing. The PR ships without a `## Code Review` section in this case. (Operator-controllable: a future change MAY add a flag for reviewing audit-only PRs, but the default behaviour ships them without review since the audit's own validation already gated the proposals.)

**PR-body construction for audit-only iterations.** When `processed.is_empty()` but `commit_count > 0`, `open_pull_request` SHALL build a PR body that names the audit-produced proposals from the agent-branch commit subjects (each audit commits with `audit: <type> proposals (N change(s))` per the existing audit-framework requirement). The PR title SHALL note this is an audit-only PR. Operators reviewing the PR see WHICH audits fired AND HOW MANY proposals were generated.

**Canonical requirement: iteration termination is commit-count-gated.** A new requirement codifies the invariant: any polling-iteration code path that decides "no work to ship" SHALL consult the agent-branch's commit count relative to base, NOT any higher-level signal that might miss commits produced after the signal was captured. This prevents the same class of bug from recurring when (for example) a future change introduces a new commit-producing iteration phase.

**Regression-prevention test.** A unit test SHALL exercise the audit-only iteration path: workspace with empty pending-changes queue, an audit fixture returning `SpecsWritten` after the (mock) git commit lands, AND assertions that:
1. `git::push_force_with_lease` is invoked.
2. `github::create_pull_request` (or its testable equivalent) is invoked.
3. The PR's head ref is the agent branch.
4. The PR's body names the audit-produced proposals.

This test fails against the pre-fix code (the early return prevents the push from being reached) and passes after the fix.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED requirement: `Polling iteration termination is gated on agent-branch commit count, not on implementer-queue outcome`. Codifies the invariant that prevents the bug class from recurring.
- **Affected code:**
  - `autocoder/src/polling_loop.rs` — remove the `if processed.is_empty()` early return at lines 702-708; the existing `commit_count == 0` check at line 712-719 becomes the sole gate. Skip the reviewer step when `processed.is_empty()` (no implementer-touched files to review); build the PR body from agent-branch commit subjects for the audit-only case.
  - `autocoder/src/polling_loop.rs::open_pull_request` (or its callees) — branch on `processed.is_empty()` to construct an audit-only PR title AND body that names the audit-produced proposals via the commit-subject convention.
  - `autocoder/src/polling_loop.rs` test module — new regression test covering the audit-only iteration path end-to-end.
- **Operator-visible behavior:**
  - Spec-writing audits' proposals now reliably reach GitHub as PRs. The `🔍 created proposal` notification is followed by an `✅ PR opened: <url>` notification (per the existing canonical notifications). Operators stop seeing "notifications without PRs."
  - The next iteration after the audit-only PR's merge sees the proposals in `list_pending` AND processes them through the implementer normally (per a12's stated semantics — implementation already conforms).
  - No new config knobs.
- **Breaking:** no. The change makes a previously-broken case work correctly. Operators who manually copied audit proposals out of run logs (the only pre-fix workaround) no longer need to.
- **Acceptance:** `cargo test` passes (new regression test + existing tests); `openspec validate a20a3-audit-only-iterations-push-and-pr --strict` passes; `cargo clippy --bin autocoder` produces no new warnings in touched files. Manual verification: on a live daemon, trigger an audit on a repo with no pending changes (`@<bot> audit security_bug <repo>`); observe both the `🔍 created proposal` AND the `✅ PR opened` notifications fire; verify the PR exists on GitHub AND the head ref carries the audit's commits.
