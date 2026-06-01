## Why

The implementer's `outcome_success` MCP tool description currently includes the sentence `Calling this tool IS the signal; no result inspection is required.` (`autocoder/src/mcp_askuser_server.rs:170`). That phrasing was correct in intent — the tool's RPC result is a content-free ack, so the agent doesn't need to inspect it — but in practice the agent reads the description as guidance about how to USE the tool, AND "the call IS the signal" reinforces an interpretation where the tool call is the message AND the `final_answer` text is decoration.

Observed symptom: PR #80's `## Agent implementation notes` section contained terse three-sentence bodies like `Outcome signal sent. Implementation complete.` per change. The agent had completed substantive work — multiple tasks across two changes, all tests passing, `openspec validate --strict` clean — AND chose to bury all of it because the tool description told it the call alone was sufficient.

The `prompts/implementer.md` rewrite in the prior session adds positive guidance about `final_answer` content (10-20 lines, list of categories to cover, a worked example). That fix is downstream of the tool description. Two other outcome-tool descriptions carry adjacent papercuts that are worth fixing in the same pass:

- `outcome_spec_needs_revision`'s description references "the legacy `=== AUTOCODER-OUTCOME ===` stdout block" — historical narrative about a mechanism the current agent has no context for; the description should describe the tool's job, not its predecessor.
- `outcome_request_iteration`'s description starts with `Use this when you completed some tasks but want another iteration to finish the rest. Use this when you started implementation honestly but...` — the word "honestly" is defensive narrative against the prior "narrative deferral" failure mode AND adds no operational value.

This change updates all three outcome-tool description fields to be operationally focused (what to do, what content to include, what NOT to do) without the historical framing. The canonical executor spec gains a new requirement defining the content shape of those description fields AND a regression test asserts the shape stays in place.

## What Changes

**`outcome_success` description rewritten.** New text:

> Signal successful completion of the implementation run. Pass `final_answer` with a substantive end-of-run summary (10-20 lines: what you implemented, test counts, clippy + `openspec validate` results, judgment calls, follow-ups). This text becomes the per-change body of the PR's `## Agent implementation notes` section AND is the reviewer's primary surface. Call once on the success path before exiting.

**`outcome_request_iteration` description trimmed.** New text:

> Signal that you completed some tasks but want another iteration to finish the rest. NOT for unimplementable tasks (use `outcome_spec_needs_revision` for those). The cumulative completed/remaining lists carry forward across iterations; the reason field documents the concrete blocker. Input is schema-validated at the MCP layer; empty arrays AND placeholder-shaped strings (e.g. `<concrete blocker>`) are rejected with a tool error you can correct AND retry in the same session.

**`outcome_spec_needs_revision` description trimmed.** New text:

> Signal that tasks.md names one or more tasks the agent cannot complete in this sandbox. Input is schema-validated at the MCP layer; placeholder-shaped strings (e.g. `<id-from-tasks-md>`) are rejected with a tool error you can correct AND retry in the same session.

**Required markers in each description.** The canonical executor spec gains a new requirement listing what each description SHALL contain AND SHALL NOT contain. The required markers are the load-bearing parts that a future contributor's edit must preserve; the regression test asserts presence.

- `outcome_success`: SHALL contain `final_answer`, `summary`, AND `PR`. SHALL NOT contain `IS the signal` OR `no result inspection`.
- `outcome_request_iteration`: SHALL contain `iteration`, `completed`, `remaining`, AND `reason`. SHALL NOT contain `honestly`.
- `outcome_spec_needs_revision`: SHALL contain `tasks.md`, `placeholder`, AND `MCP layer`. SHALL NOT contain `legacy` OR `AUTOCODER-OUTCOME`.

**Regression test asserts the markers.** A new test reads the rendered `tools/list` response from the MCP server (OR reads the description strings directly from `mcp_askuser_server.rs` via a small accessor used only in tests) AND verifies each tool's description contains the required substrings AND none of the forbidden substrings. The test produces a combined failure listing.

**No change to tool behavior, schemas, OR the daemon-side outcome store.** The descriptions are the only thing changing. `final_answer` remains an optional input on `outcome_success` (matching the existing API contract — the daemon writes an empty string if omitted). The new description text directs agents toward providing substantive content, but the API remains tolerant of omission for robustness.

## Impact

- **Affected specs:**
  - `executor` — ADDED a new requirement defining the content-shape rules (required AND forbidden substrings) for the three outcome tools' `description` fields. The existing canonical "Per-execution MCP child exposes outcome tools via control-socket relay" AND "Per-execution MCP child exposes `outcome_request_iteration` tool" requirements remain unchanged — those define schema AND behavior, NOT description text.
- **Affected code:**
  - `autocoder/src/mcp_askuser_server.rs` — three string literals (the `description` fields of the three outcome tools at lines ~170, ~183, ~209) replaced with the new text.
  - New test asserting the description content rules. Either added to an existing test in `mcp_askuser_server.rs`'s test module OR placed at `autocoder/tests/mcp_outcome_descriptions.rs` — pick whichever produces less file churn.
- **Operator-visible behavior:**
  - Indirectly: PR `## Agent implementation notes` sections become substantive again because the agent's tool description encourages content production rather than treating the call as sufficient.
  - No operator-facing API changes; no chatops changes.
- **Backward compatibility:** none affected. The tool schemas, behaviors, AND daemon-side outcome store are unchanged.
- **Dependencies:** none. Independent of every queued change.
- **Acceptance:** `cargo test` passes (including the new regression test); `openspec validate a44-mcp-outcome-tool-descriptions --strict` passes. Tests:
  - The regression test passes against the post-merge state (all three descriptions contain their required markers AND none of their forbidden markers).
  - The regression test fails with a combined diagnostic if any required marker is removed OR any forbidden marker is reintroduced.
  - An integration-style smoke test invokes the MCP `tools/list` RPC against the running MCP server AND confirms the three description strings match the source.
