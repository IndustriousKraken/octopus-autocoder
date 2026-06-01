# Implementation tasks

## 1. Rewrite the three outcome-tool description strings

In `autocoder/src/mcp_askuser_server.rs`, replace the `description` field of each outcome tool with the new text. The other JSON fields (`name`, `inputSchema`, etc.) are unchanged.

- [ ] 1.1 `outcome_success` description (~line 170) becomes:

  ```
  Signal successful completion of the implementation run. Pass `final_answer` with a substantive end-of-run summary (10-20 lines: what you implemented, test counts, clippy + `openspec validate` results, judgment calls, follow-ups). This text becomes the per-change body of the PR's `## Agent implementation notes` section AND is the reviewer's primary surface. Call once on the success path before exiting.
  ```

- [ ] 1.2 `outcome_request_iteration` description (~line 183) becomes:

  ```
  Signal that you completed some tasks but want another iteration to finish the rest. NOT for unimplementable tasks (use `outcome_spec_needs_revision` for those). The cumulative completed/remaining lists carry forward across iterations; the reason field documents the concrete blocker. Input is schema-validated at the MCP layer; empty arrays AND placeholder-shaped strings (e.g. `<concrete blocker>`) are rejected with a tool error you can correct AND retry in the same session.
  ```

- [ ] 1.3 `outcome_spec_needs_revision` description (~line 209) becomes:

  ```
  Signal that tasks.md names one or more tasks the agent cannot complete in this sandbox. Input is schema-validated at the MCP layer; placeholder-shaped strings (e.g. `<id-from-tasks-md>`) are rejected with a tool error you can correct AND retry in the same session.
  ```

## 2. Regression test asserting description content rules

- [ ] 2.1 Add a test (in `mcp_askuser_server.rs`'s `#[cfg(test)] mod tests` OR at `autocoder/tests/mcp_outcome_descriptions.rs` — pick the lower-churn location) that constructs the MCP server's `tools/list` response AND verifies each tool's `description` string against required AND forbidden substring rules:

  - `outcome_success`: SHALL contain `final_answer`, `summary`, AND `PR`. SHALL NOT contain `IS the signal` OR `no result inspection`.
  - `outcome_request_iteration`: SHALL contain `iteration`, `completed`, `remaining`, AND `reason`. SHALL NOT contain `honestly`.
  - `outcome_spec_needs_revision`: SHALL contain `tasks.md`, `placeholder`, AND `MCP layer`. SHALL NOT contain `legacy` OR `AUTOCODER-OUTCOME`.

- [ ] 2.2 The test SHALL produce a combined failure listing (NOT first-failure-only). Each entry SHALL name the tool, the failed check (missing required substring OR present forbidden substring), AND the offending substring text.
- [ ] 2.3 The test SHALL be deterministic — pure assertion against the static description strings, no network or env interaction.

## 3. Smoke test against the running MCP server

- [ ] 3.1 Extend (OR add) a small integration-style test that spawns the MCP server in-process AND issues a `tools/list` JSON-RPC request, then asserts that the three description strings in the response satisfy the same required/forbidden marker rules as task 2. This catches the case where the test fixture asserts against a static string that drifts from the actual rendered output.
- [ ] 3.2 If `tools/list` is already exercised by an existing test (e.g., `outcome_success_description_present` or similar), extend that test rather than creating a new one.

## 4. Acceptance gate

- [ ] 4.1 `cargo test` passes for the autocoder crate, including the new regression AND smoke tests.
- [ ] 4.2 `openspec validate a44-mcp-outcome-tool-descriptions --strict` passes.
- [ ] 4.3 Sanity check: revert one description string to its prior text, run the test, AND confirm it fails with a clear diagnostic. Restore the new text. (Smoke test for the failure path; do NOT leave the codebase in a broken state.)
