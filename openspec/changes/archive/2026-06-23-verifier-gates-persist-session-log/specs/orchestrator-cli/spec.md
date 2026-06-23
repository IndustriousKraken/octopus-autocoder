## ADDED Requirements

### Requirement: Verifier gate sessions persist a discoverable run log
Each verifier-framework gate session — `[in]`, `[canon]`, `[rules]`, AND `[out]` — SHALL persist its agentic session's full captured output to a discoverable per-session log file, regardless of the session's outcome. The daemon already holds this output; it SHALL write it rather than discard all but a short excerpt.

The persisted log SHALL contain the session's full captured output: the agent's streamed actions/output, its final message, its captured standard-error, the process exit status, AND the timed-out flag. The log SHALL be written for EVERY outcome — a clean pass, a findings result, a fail-closed no-submission hold, a timeout, OR a session error — so a held change is diagnosable AND a surprising clean-or-findings result is auditable. The log SHALL be stored under the run-logs directory, uniquely named by gate, change, AND a timestamp, mirroring the audit logs' per-run-log pattern. The fail-closed "gate FAILED TO RUN — change held" chatops alert AND the corresponding WARN SHALL name the log-file path, as the executor's failure reason names its log. This is provider-agnostic: it persists the raw captured output for any wrapped CLI; it SHALL NOT parse the output for any decision, AND it SHALL NOT change any gate's disposition or fail-closed posture.

#### Scenario: A held gate writes a session log named in the alert
- **WHEN** a verifier gate session ends with no submission and the change is held (fail closed)
- **THEN** the session's full captured output is persisted to a per-session log under the run-logs directory
- **AND** the "gate FAILED TO RUN — change held" alert AND the WARN name that log-file path
- **AND** the change is still held (the disposition is unchanged — this adds only the log)

#### Scenario: A clean or findings run also writes a log
- **WHEN** a verifier gate session completes with a clean result OR with findings
- **THEN** its captured output is persisted to a per-session log too
- **AND** the gate proceeds or holds exactly as before (the log does not alter the outcome)

#### Scenario: A timeout or error run writes a log
- **WHEN** a verifier gate session times out OR errors before producing a result
- **THEN** its captured output, including the timeout/error indicator (exit status / timed-out flag), is persisted to a per-session log
- **AND** the operator-facing failure surfacing names the log-file path

### Requirement: A held gate's log distinguishes the no-submission failure modes
When a verifier gate is held because its session produced no submission, the persisted session log together with the daemon's submission-side recording SHALL make the failure mode determinable, so an operator can act on the real cause rather than guess. The distinguishable modes are: (a) the submission tool was not advertised to the session for its role; (b) the tool was advertised but the session ended without calling it (including emitting prose instead of a tool call); (c) the session called the tool but no submission reached the daemon (a relay / control-socket failure); AND (d) the session errored or timed out.

To make these distinguishable, the MCP submission server SHALL record which submission tool (if any) it advertised for the session's role, AND the daemon's submission listener SHALL record whether a submission was received for that session before the consume that found none. These records, with the session's own captured output (which shows the model's tool calls and any CLI/API error), SHALL be cross-referenceable to the held session by its role/change/timestamp.

#### Scenario: The advertised submission tool is recorded for the session's role
- **WHEN** a gate session is started with a submission role
- **THEN** the MCP submission server records which submission tool it advertised for that role, or that none matched the role
- **AND** an operator can therefore tell, for a held session, whether the tool was ever presented (mode a) versus presented-but-not-called (mode b)

#### Scenario: A received submission is recorded distinctly from consume-found-none
- **WHEN** the daemon consumes a gate session's submission slot AND finds none
- **THEN** the daemon's recording distinguishes "a submission was relayed but not consumed" from "no submission was ever relayed"
- **AND** an operator can therefore tell called-but-not-relayed (mode c) from advertised-but-not-called (mode b)
