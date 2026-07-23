## 1. Three-way gate outcome

- [x] 1.1 In `autocoder/src/polling_loop/pr_open.rs`, change `open_pr_exists_for_agent_branch_at` (and its wrapper) to return a three-way outcome — open / none / unknown — with the URL-parse, token-resolution, and query-failure arms all returning unknown, each keeping its WARN.
- [x] 1.2 In `autocoder/src/polling_loop/pass.rs`, skip the pass on open AND on unknown; proceed only on a confirmed empty list. Keep the revision dispatcher running before the gate, unchanged.

## 2. Sustained-failure alert

- [x] 2.1 Track consecutive unknown outcomes per repository in the polling task's state (in-memory); on the third consecutive failure, post a throttled chatops alert naming the open-PR gate, the repository, and the most recent error, via the existing alert-throttle machinery. Reset the counter on any successful query.

## 3. Tests

- [x] 3.1 Mockito tests: a 5xx / transport error yields unknown and the pass is skipped (no branch init, no lane walk); a subsequent 200-empty response proceeds normally and resets the counter.
- [x] 3.2 Test: three consecutive failures post exactly one throttled alert; a fourth failure within the throttle window does not re-alert; a success then a new failure streak starts the count fresh.
- [x] 3.3 Regression test for the incident shape: with an open PR on the agent branch and a failing query, no executor run starts and no duplicate PR-open is attempted.
- [x] 3.4 Run the full `cargo test` suite; confirm the existing open-PR-gate scenarios (open → skip, empty → proceed, head qualifiers) pass unchanged.
