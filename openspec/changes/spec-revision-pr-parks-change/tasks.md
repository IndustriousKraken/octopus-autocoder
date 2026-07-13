# Tasks

## 1. Code — blocking-markers gate

- [ ] 1.1 In `handle_blocking_markers_gate` (or its caller in `commits.rs`): after the `.needs-spec-revision.json` check short-circuits (marker present → block), add a branch for the no-marker case that calls `forge.find_pr_by_head` for `agent_branch + "-spec-revision-" + change_slug`. On an open PR result, return the same blocking/halt signal.
- [ ] 1.2 On a forge-query error for the spec-revision branch, fail open (log WARN, do NOT block) — consistent with the existing open-PR-check error policy (a transient GitHub failure should not park a change indefinitely).
- [ ] 1.3 Log an INFO when a change is parked by an open spec-revision PR, naming the PR number and URL, mirroring the existing repo-level park log.

## 2. Tests

- [ ] 2.1 Test: change with open spec-revision PR is blocked without marker re-write. Mock the forge to return an open PR on `agent-q-spec-revision-<change>` and assert the gate returns blocking and no `.needs-spec-revision.json` is written.
- [ ] 2.2 Test: change with BOTH a marker and an open spec-revision PR short-circuits on the marker (no forge call made for the PR check).
- [ ] 2.3 Test: forge query error for spec-revision branch fails open — gate proceeds as if no spec-revision PR exists, does not block.
