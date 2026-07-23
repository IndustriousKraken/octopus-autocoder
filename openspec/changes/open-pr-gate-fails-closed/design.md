## Context

`open_pr_exists_for_agent_branch_at` (`polling_loop/pr_open.rs:390`) returns `bool`, and every failure arm — URL parse, token resolution, and the `list_open_prs` query itself — returns `false` ("no open PR"), letting the pass proceed. The canonical requirement currently mandates this ("the check is best-effort — false negatives just degrade to the prior pre-check behavior"), but the "prior behavior" it degrades to is the set of harms the gate was built to prevent. Production incident (Abyssum, 2026-07-16): a transient query failure while issue-fix PR #30 was open let branch-init reset `agent-q` from base, the issues lane re-implemented `bump-rust-edition-2024` (~9 minutes of executor time), and duplicate PR #31 opened seconds after #30 merged. This is the last known fail-open control in the pass pipeline; the project's contributing standard ("an inability to run is a distinct non-passing state, never a pass") already names the principle.

## Goals / Non-Goals

**Goals:**
- An unconfirmed open-PR answer never triggers new work; a transient failure costs one polling interval.
- A sustained failure is loudly visible instead of silently idling a repository.

**Non-Goals:**
- Changing the gate's success-path semantics, query shape, or head-qualifier rules.
- Gating the revision dispatcher (it deliberately runs before this gate so revisions reach open PRs; it has its own failure handling).
- Persisting the consecutive-failure counter across restarts — a restart resetting it delays the alert by at most three intervals, which is not worth a state file.

## Decisions

- **Three-way outcome instead of `bool`.** The function returns open / none / unknown (an enum or `Option<bool>`); collapsing "unknown" into either boolean is how the bug existed. The call site in `pass.rs:115` skips on open AND on unknown; only a confirmed empty list proceeds.
- **All failure arms fail closed, not just the HTTP one.** URL-parse and token-resolution failures currently also return `false`; both mean "cannot confirm," so both skip. (Both are also config-shaped errors that startup validation should have caught — skipping is strictly safer than proceeding.)
- **Alert after three consecutive failures, via the existing throttle machinery.** One WARN per failed pass covers the journal; the chatops alert covers the operator who doesn't tail journals. Three consecutive failures (~30+ minutes at default cadence) separates a blip from an outage without new configuration. The counter lives in the polling task's per-repo state; a success resets it.
- **The cost asymmetry is the whole argument.** Fail-closed worst case: one wasted polling interval per transient blip, and during a real GitHub outage the repo pauses — which it effectively must anyway, since push + PR-open need the same API. Fail-open worst case (observed): a duplicate multi-minute agentic run, a force-push over a reviewer's in-flight PR, and a junk PR needing manual closure.

## Risks / Trade-offs

- [A long GitHub outage pauses all new work per repo] → Correct behavior: every downstream step of a pass needs GitHub anyway; working the queue just to fail at push would burn executor time. The alert makes the pause visible.
- [Chatops alert unavailable (no backend) during sustained failure] → The WARN-per-pass remains in the journal; the no-backend deployment accepted journal-only visibility everywhere else.
- [Callers of the old `bool` signature] → One production call site (`pass.rs`); the compiler finds the rest (tests, mockito harnesses) mechanically.
