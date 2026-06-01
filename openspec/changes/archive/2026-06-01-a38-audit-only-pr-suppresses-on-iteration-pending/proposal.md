## Why

Two cross-spec bugs surfaced together when `a35-thread-daemon-paths-globals-removal` ran its second iteration AND called `outcome_request_iteration`:

**Bug A — Audit-only PR path opened a bogus PR.** The canonical `Polling iteration termination is gated on agent-branch commit count, NOT on implementer-queue outcome` requirement (from `a20a3`) fires push + PR when `commit_count > 0`. The intent was: if an audit produced proposal commits, push them AND open an audit-only PR. The unintended consequence: iteration_request WIP commits (introduced by `a27a1`) ALSO advance the agent-branch commit count, AND ALSO satisfy `commit_count > 0`, so the audit-only-PR path fires on iteration_request WIP too. The result was PR #77 on `IndustriousKraken/openspec-autocoder`: title "audit-only: agent-branch commits without implementer changes," body claiming "audit-produced proposals only," ACTUAL contents one iteration_request commit AND zero audit commits. Merging it would have locked in half-done a35 work — directly contradicting `a27a1`'s "iteration-pending means more iterations needed; don't ship yet" semantic.

`a20a3`'s canonical text was written before `a27a1` introduced iteration_request commits. The two requirements don't conflict in their stated text — they conflict in a case neither explicitly considered. The fix is narrow: `a20a3`'s termination-gate requirement gains an iteration-pending exclusion. When ANY `.iteration-pending.json` marker exists in `openspec/changes/`, the audit-only-PR step is suppressed for this iteration. Audit-produced proposals (if any exist this iteration) are NOT lost — they remain in the change directory on agent-q AND get picked up on the NEXT iteration's `list_pending` once the iteration-pending change concludes.

**Bug B — `a27a1`'s `.iteration-pending.json` marker isn't being written.** Inspecting the workspace after a35's iteration 2:

```
openspec/changes/a35-thread-daemon-paths-globals-removal/
  .in-progress           ← stale lock, not cleared
  proposal.md
  specs/
  tasks.md               ← updated by iteration 2's checkbox marks
  (no .iteration-pending.json)
```

`a27a1`'s canonical `Iteration-pending marker file in the change directory carries state across iteration boundaries` requirement REQUIRES the marker to be written when the polling-loop's `IterationRequested` arm handles the outcome. The implementation isn't writing it. Without the marker:

- The next iteration's prompt-builder won't inject the continuation-context block (`a27a1`'s "Implementer prompt includes a 'Prior iteration summary' block when an iteration-pending marker is present" requirement).
- The classifier's iteration-cap enforcement (`a27a1`'s 5-iteration cap) can't read the prior iteration_number AND defaults to "first iteration" — caps never fire.
- The queue engine's iteration-pending preference (per `a27a1`) doesn't order this change ahead of alphabetically-earlier siblings.

The implementation gap is silent — there's no test that asserts the marker write actually happens after a real iteration_request lifecycle. This change adds the regression test as a canonical requirement so the implementation gap surfaces in CI.

## What Changes

**Bug A fix — audit-only-PR path suppression on iteration-pending state.** The canonical "Polling iteration termination is gated on agent-branch commit count" requirement is MODIFIED to add an iteration-pending exclusion: when one or more `openspec/changes/*/.iteration-pending.json` markers exist at end-of-iteration, the audit-only-PR path SHALL be suppressed. The early-return-on-zero-commits behavior is unchanged. The implementer-changes-non-empty behavior is unchanged. ONLY the "non-empty commit count but zero archived implementer changes" case (which previously always fired the audit-only-PR path) gains the exclusion.

Specifically: when the iteration would otherwise open an audit-only PR per the existing logic, the polling-loop SHALL first scan `openspec/changes/*/.iteration-pending.json`. If any are present, the iteration SHALL log INFO `audit-only PR path suppressed: iteration-pending markers present for <change-list>` AND return `Ok(())` WITHOUT pushing OR opening a PR. Audit proposals committed during this iteration remain on agent-q AND on disk in `openspec/changes/<aXX>-*` directories; they ship in the next iteration's PR after the iteration-pending change concludes (via `outcome_success`, `outcome_spec_needs_revision`, OR the iteration cap).

**Audit-only PR body's "audit-produced proposals" claim becomes conditional.** The canonical "PR body for audit-only iterations names the audit-produced proposals" requirement is MODIFIED to require content-aware body rendering: the body SHALL enumerate commits by category (implementer-archived, audit-produced, iteration_request WIP, manual/unknown) AND SHALL only include the "audit-produced proposals" framing when audit commits actually exist. A PR whose entire diff is iteration_request WIP SHALL produce a body explicitly naming that — not the misleading "audit-produced proposals" boilerplate that PR #77 carried.

In practice with Bug A's fix above, this conditional-body case becomes vanishingly rare: the audit-only-PR path is suppressed when iteration-pending markers exist, so the "iteration WIP without audit commits" case never reaches the PR-body-rendering step. But the conditional-body rule is defensive — future commit-producing iteration phases (a new audit type, a new autonomous-fix mechanism) automatically get a correct body without needing a parallel fix to the renderer.

**Bug B fix — regression test for the iteration-pending marker write.** A new canonical requirement SHALL pin: an integration test fires an `IterationRequested` outcome through the polling-loop's actual code path (NOT just the unit-level helper) AND asserts that `<workspace>/openspec/changes/<change>/.iteration-pending.json` is written on disk with the expected payload (`completed_tasks`, `remaining_tasks`, `reason`, `iteration_number`) after the commit + force-push step completes. This test fails the build if the implementation drops the marker-write step, OR writes it to the wrong path, OR writes corrupt content.

The new test pins the `a27a1` canonical "marker is filesystem-inspectable" semantic at the integration level. Today's a27a1 spec has unit-test-level scenarios (per the canonical "Marker written on IterationRequested AFTER successful push" requirement), but the bug we just hit shows the unit test passing while the end-to-end implementation drops the write. The integration test closes that gap.

**`.in-progress` marker is also cleared on `IterationRequested`.** Inspecting the workspace after the broken a35 iteration showed `.in-progress` still present at the change directory, even though the iteration had concluded. The canonical "Unlocking after any executor outcome" requirement from openspec-queue-engine ALREADY requires this; the implementation gap is the same shape as Bug B. The same integration test SHALL also assert `.in-progress` is removed at end-of-iteration regardless of which outcome fired (Completed, Failed, SpecNeedsRevision, AskUser, IterationRequested).

## Impact

- **Affected specs:**
  - `orchestrator-cli` — MODIFIED the canonical "Polling iteration termination is gated on agent-branch commit count, NOT on implementer-queue outcome" requirement to add iteration-pending exclusion. MODIFIED the canonical "PR body for audit-only iterations names the audit-produced proposals" scenario to require content-aware rendering. ADDED a requirement pinning the integration test for `.iteration-pending.json` write + `.in-progress` cleanup on `IterationRequested`.
- **Affected code:**
  - `autocoder/src/polling_loop.rs` — the commit-count-gate check gains a sibling check for `.iteration-pending.json` markers; the audit-only PR step suppresses when markers are present. The `IterationRequested` arm of the outcome dispatch gains (OR is fixed to actually invoke) the marker-write call + the `.in-progress` removal call.
  - `autocoder/src/git_workflow_manager.rs` (OR wherever PR-body composition lives) — the audit-only PR body template becomes content-aware, branching on commit-message category (implementer-archived, audit-produced, iteration WIP).
  - New integration test in `autocoder/tests/` (OR an extension to an existing integration test file) exercising the IterationRequested lifecycle end-to-end AND asserting both filesystem outcomes.
- **Operator-visible behavior:**
  - PRs like #77 stop opening. iteration_request commits stay on agent-q awaiting the iteration sequence's conclusion.
  - Audit-only PRs that DO open (when actual audit commits exist AND no iteration-pending markers are active) get accurate body content describing exactly what commits they contain.
  - `.iteration-pending.json` markers reliably appear on disk after an iteration_request, enabling the next iteration's continuation-context injection (per a27a1) AND the cap enforcement (per a27a1) AND the queue front-insertion (per a27a1).
  - `.in-progress` markers reliably clear after every outcome, ending the "stale lock blocks next iteration" failure mode.
- **Backward compatibility:** no config changes. Existing iterations on changes WITHOUT iteration_request behavior are unaffected. The audit-only-PR path continues to fire on actual audit-produced commits.
- **Dependencies:** sits structurally on `a20a3` (audit-only iterations push and PR) AND `a27a1` (iteration request + continuation context). Both are merged on master. Can land independently.
- **Acceptance:** `cargo test` passes; `openspec validate a38-audit-only-pr-suppresses-on-iteration-pending --strict` passes. Tests:
  - Integration: a polling iteration whose only commits-ahead-of-master are iteration_request WIP, with a `.iteration-pending.json` marker present, returns `Ok(())` WITHOUT opening a PR. The `audit-only PR path suppressed: iteration-pending markers present` log line fires.
  - Integration: a polling iteration with BOTH iteration_request WIP commits AND `.iteration-pending.json` markers present AND audit-produced commits on agent-q ALSO suppresses (audit proposals defer to next iteration).
  - Integration: a polling iteration with audit-produced commits AND NO `.iteration-pending.json` markers anywhere opens the audit-only PR as today (regression test for the existing happy path).
  - Integration: a polling iteration whose IterationRequested arm completes successfully writes `<workspace>/openspec/changes/<change>/.iteration-pending.json` with the expected payload AND removes `<workspace>/openspec/changes/<change>/.in-progress`.
  - Integration: PR body for an audit-only PR with three audit commits AND zero implementer commits contains an "audit-produced proposals" section listing the three commits. Body does NOT include any iteration-WIP framing (there is none).
  - Integration: a `.in-progress` marker present at start-of-iteration AND no live PID owning it is cleared by the existing stale-lock-cleanup path (regression test; existing canonical behavior preserved).
