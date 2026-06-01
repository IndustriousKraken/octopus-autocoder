# executor — delta for a44-mcp-outcome-tool-descriptions

## ADDED Requirements

### Requirement: MCP outcome-tool description fields encourage substantive content AND drop narrative history
The `description` field of each outcome tool advertised by the per-execution MCP child (currently `autocoder/src/mcp_askuser_server.rs`) SHALL be operationally focused — directing the agent what to do AND what content to produce — without narrative history about prior failure modes OR legacy mechanisms. The agent reads the `description` field from the MCP `tools/list` response to decide how to use the tool; that text is the primary surface for shaping agent behavior, so it SHALL contain the load-bearing operational guidance.

Required AND forbidden substrings, per tool:

- `outcome_success` — SHALL contain `final_answer`, `summary`, AND `PR`. SHALL NOT contain `IS the signal` OR `no result inspection`. The required substrings ensure the description names the input field carrying the agent's content AND names the reviewer-facing destination of that content. The forbidden substrings are the phrasings that previously pushed agents toward treating the tool call as sufficient (terse `final_answer` text) when the goal is a content-rich summary.
- `outcome_request_iteration` — SHALL contain `iteration`, `completed`, `remaining`, AND `reason`. SHALL NOT contain `honestly`. The required substrings ensure the description names the cumulative state lists AND the blocker-naming field. The forbidden `honestly` was a defensive-narrative artifact AND adds no operational value.
- `outcome_spec_needs_revision` — SHALL contain `tasks.md`, `placeholder`, AND `MCP layer`. SHALL NOT contain `legacy` OR `AUTOCODER-OUTCOME`. The required substrings ensure the description names the file the agent reads, the placeholder-rejection rule, AND where validation runs. The forbidden substrings reference a prior stdout-block mechanism the current agent has no context for; the description should describe the tool's job, not its predecessor.

A regression test SHALL read the rendered `tools/list` response from the MCP server (OR the description strings via a test-only accessor) AND verify each tool's `description` against the required AND forbidden substring rules. The test SHALL produce a combined failure listing — every offending tool AND every failed marker reported in one run — so a contributor editing several descriptions sees all problems at once.

This requirement does NOT mandate the exact prose; future contributors MAY rewrite the descriptions for clarity OR style as long as the required substrings stay present AND no forbidden substrings appear. The substring rules are the load-bearing contract; the surrounding prose is flexible.

This requirement covers description CONTENT ONLY. The tool schemas (`inputSchema`), behaviors (control-socket relay), AND output shapes are governed by the existing canonical "Per-execution MCP child exposes outcome tools via control-socket relay" AND "Per-execution MCP child exposes `outcome_request_iteration` tool" requirements AND are unchanged by this requirement.

#### Scenario: All three descriptions satisfy the marker rules
- **GIVEN** the repository is in its post-merge state for `a44-mcp-outcome-tool-descriptions`
- **WHEN** the regression test reads the MCP server's `tools/list` response
- **THEN** the `outcome_success` description contains `final_answer`, `summary`, AND `PR`
- **AND** the `outcome_success` description does NOT contain `IS the signal` OR `no result inspection`
- **AND** the `outcome_request_iteration` description contains `iteration`, `completed`, `remaining`, AND `reason`
- **AND** the `outcome_request_iteration` description does NOT contain `honestly`
- **AND** the `outcome_spec_needs_revision` description contains `tasks.md`, `placeholder`, AND `MCP layer`
- **AND** the `outcome_spec_needs_revision` description does NOT contain `legacy` OR `AUTOCODER-OUTCOME`
- **AND** the test passes with no diagnostic output

#### Scenario: Removing a required substring fails the test
- **GIVEN** a hypothetical future change removes `final_answer` from the `outcome_success` description
- **WHEN** the regression test runs in CI for that change
- **THEN** the test fails with a diagnostic naming `outcome_success: missing required substring 'final_answer'`
- **AND** the failure surfaces before the change can merge

#### Scenario: Reintroducing a forbidden substring fails the test
- **GIVEN** a hypothetical future change reintroduces the phrase `IS the signal; no result inspection is required` into the `outcome_success` description
- **WHEN** the regression test runs
- **THEN** the test fails with a diagnostic naming `outcome_success: forbidden substring 'IS the signal' is present`

#### Scenario: Multiple offending tools are reported in one run
- **GIVEN** a hypothetical future change removes a required substring from `outcome_success` AND reintroduces `legacy` into `outcome_spec_needs_revision`
- **WHEN** the regression test runs
- **THEN** the test fails with a single combined diagnostic naming both offending tools AND both failed checks
- **AND** the contributor can fix both without re-running the test repeatedly

#### Scenario: Rewording within the marker rules is permitted
- **GIVEN** a future change rewrites the `outcome_success` description for clarity, preserving all required substrings AND avoiding all forbidden substrings
- **WHEN** the regression test runs
- **THEN** the test passes
- **AND** no diagnostic is produced (the prose is flexible; only the substring contract is binding)

#### Scenario: Description content rule is independent of tool schema rule
- **GIVEN** a future change rewrites a description AND inadvertently breaks the tool's `inputSchema` shape
- **WHEN** the regression test for THIS requirement runs
- **THEN** it asserts only the description content rules
- **AND** schema violations surface via the existing canonical "Per-execution MCP child exposes outcome tools via control-socket relay" requirement's scenarios, not via this requirement's test
