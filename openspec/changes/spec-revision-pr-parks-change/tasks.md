# Tasks

## 1. Spec delta — orchestrator-cli

- [ ] 1.1 In the `Spec-needs-revision executor outcome + marker` requirement, add a corollary: "When the spec-revision executor opens a revision PR and clears the `.needs-spec-revision.json` marker, the open PR on `<agent_branch>-spec-revision-<change-slug>` SHALL serve as the authoritative blocking signal for that change, preventing the gate from re-running until the PR is merged or closed."
- [ ] 1.2 In (or immediately after) the `Skip iteration when an open PR exists for the agent branch` requirement, add a sibling requirement: "When the blocking-markers gate processes a change that has NO `.needs-spec-revision.json` marker, it SHALL also query GitHub for an open PR on branch `<agent_branch>-spec-revision-<change-slug>`. If such a PR exists, the change is treated as parked (same blocking/halt-queue-walk result as a present marker) and the spec gate is NOT re-run."
- [ ] 1.3 Add a scenario: "Open spec-revision PR parks change without re-adding marker": GIVEN change X has no `.needs-spec-revision.json` marker AND an open PR exists on `agent-q-spec-revision-X`, THEN the change is blocked, no gate runs, no marker is written, and the queue halts (consistent with existing halt-on-block semantics).
- [ ] 1.4 Add a scenario: "Park resolves on PR close or merge": GIVEN the spec-revision PR is merged or closed, THEN the next iteration finds no open spec-revision PR AND no marker, so the gate runs normally with the (potentially updated) spec.

## 2. Code — blocking-markers gate

- [ ] 2.1 In `handle_blocking_markers_gate` (or its caller in `commits.rs`): after the `.needs-spec-revision.json` check short-circuits (marker present → block), add a branch for the no-marker case that calls `forge.find_pr_by_head` for `agent_branch + "-spec-revision-" + change_slug`. On an open PR result, return the same blocking/halt signal.
- [ ] 2.2 On a forge-query error for the spec-revision branch, fail open (log WARN, do NOT block) — consistent with the existing open-PR-check error policy (a transient GitHub failure should not park a change indefinitely).
- [ ] 2.3 Log an INFO when a change is parked by an open spec-revision PR, naming the PR number and URL, mirroring the existing repo-level park log.

## 3. Tests

- [ ] 3.1 Test: change with open spec-revision PR is blocked without marker re-write. Mock the forge to return an open PR on `agent-q-spec-revision-<change>` and assert the gate returns blocking and no `.needs-spec-revision.json` is written.
- [ ] 3.2 Test: change with BOTH a marker and an open spec-revision PR short-circuits on the marker (no forge call made for the PR check).
- [ ] 3.3 Test: forge query error for spec-revision branch fails open — gate proceeds as if no spec-revision PR exists, does not block.
