# executor — delta for a60-opencode-cli-strategy

## ADDED Requirements

### Requirement: OpencodeStrategy implements the opencode CLI for agentic roles
The daemon SHALL provide a second `CliStrategy` (a56), `OpencodeStrategy`, for the `opencode` CLI, so a role whose model provider resolves to `opencode` (a55's `provider → CLI` rule for `openai_compatible`/`ollama`, OR an explicit registry `cli: opencode`) runs agentically instead of erroring with "no registered strategy."

`OpencodeStrategy` SHALL build an `opencode run` invocation that: selects the model via `--model <provider>/<model>`; writes an `opencode.json` into the workspace carrying the MCP `mcp` block (`type: local`, the MCP-child command, AND env including `ORCH_MCP_ROLE`) AND the resolved provider config (base URL + key); AND maps a56's sandbox (allowed-tools list + deny patterns) onto opencode's permission configuration so a read-only role keeps its read-only profile. It SHALL set NO `ANTHROPIC_*` env (that is the `claude` strategy's mechanism), AND SHALL NOT write `.mcp.json` (the `claude` MCP format). The model SHALL be delivered the role's prompt by whichever mechanism headless `opencode run` accepts (stdin or positional argument), as determined by the integration spike.

`OpencodeStrategy` SHALL run in capture mode; the streaming-JSON event path (`final_answer` / `session_id` / incremental log) is `claude`-specific. opencode therefore serves the capture-mode structured-submission roles (the advisory audits, the reviewer, the contradiction check); the executor's streaming implementer path remains on the `claude` strategy. The opencode integration SHALL surface MCP tool calls AND surface a daemon-rejected submission to the model as a correctable tool error it can retry in the same session — the same submission contract a56 requires of the `claude` path.

Registering `opencode` unblocks the non-Anthropic agentic paths of the reviewer (a58) AND the contradiction check (a59); it does NOT change any role's default transport.

#### Scenario: Opencode provider resolves to a working strategy
- **WHEN** a role's model resolves (via a55's `provider → CLI` rule, OR an explicit `cli: opencode`) to the `opencode` CLI
- **THEN** strategy resolution returns `OpencodeStrategy` (NOT a "no registered strategy" error)
- **AND** it builds an `opencode run` invocation selecting the model via `--model <provider>/<model>`

#### Scenario: MCP and role env are delivered via opencode.json
- **WHEN** an `opencode` role runs with a structured-submission tool (e.g. `submit_review`)
- **THEN** the strategy writes `opencode.json` with an `mcp` block (`type: local`, the MCP-child command, env including `ORCH_MCP_ROLE`) so the role's `submit_*` tool is reachable
- **AND** NO `.mcp.json` is written for that run

#### Scenario: Model selection targets the configured provider, not Anthropic env
- **WHEN** the resolved model is `(openai_compatible, <model>, <base_url>, <key>)`
- **THEN** the invocation selects `--model openai_compatible/<model>` AND `opencode.json` carries the provider's base URL AND key
- **AND** none of `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_MODEL` is set

#### Scenario: Read-only sandbox is enforced via opencode permissions
- **WHEN** a read-only role (a56 sandbox: allow Read/Glob/Grep; deny Write/Edit/Bash) runs under opencode
- **THEN** the generated opencode permission configuration denies Write, Edit, AND Bash
- **AND** exposes only the read tools plus the role's MCP submission tool

#### Scenario: Capture mode only; streaming stays on claude
- **WHEN** an `opencode` role runs through `agentic_run`
- **THEN** it uses capture mode (stdout/stderr read at exit), NOT the streaming-JSON parse path
- **AND** the executor's streaming implementer path continues to use the `claude` strategy

#### Scenario: Submission contract holds under opencode
- **WHEN** an `opencode` role's agent calls its `submit_*` tool AND the daemon rejects the payload (schema-invalid)
- **THEN** the rejection reaches the model as a tool error it can correct AND retry within the same `opencode run` session
- **AND** this matches the correctable-tool-error contract a56 requires of the `claude` path

#### Scenario: Non-Anthropic agentic roles now function
- **WHEN** the reviewer (`reviewer.kind: agentic`) OR the contradiction check is configured with a model whose provider resolves to `opencode`
- **THEN** the role runs agentically via `OpencodeStrategy`
- **AND** it no longer errors / fails open on "no registered strategy"
