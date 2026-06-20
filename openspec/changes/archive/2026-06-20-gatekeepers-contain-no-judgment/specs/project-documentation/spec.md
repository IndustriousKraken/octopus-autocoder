## MODIFIED Requirements

### Requirement: Control-plane gatekeepers fail closed, never to a passing verdict

A **control-plane gatekeeper** — any component whose role is to decide whether work may proceed OR to attest that work meets a standard (the pre-flight contradiction gates `[in]` AND `[canon]`, the code-implements-spec gate `[out]`, the code reviewer, any future verifier, AND audits that gate an operator's `send it`) — SHALL NOT represent an inability to run as a passing OR permissive outcome. The absence of a SUCCESSFUL evaluation SHALL be a distinct, surfaced, non-passing state. A control that fails open is not a control.

This invariant SHALL hold by inspection — so the periodic `drift_audit` AND the `[canon]`/`[out]` gates can flag a violation — and applies across every gatekeeper:

- **Verdict defaults AND initializers SHALL be the non-passing state.** A verdict variable, accumulator, or struct default SHALL initialize to blocked / errored / unknown — NEVER to approve / pass. An aggregation over zero evaluated items SHALL NOT yield a passing result.
- **Error paths SHALL NOT collapse into pass.** A spawn OR timeout failure, an unavailable or unregistered CLI / tool, a missing or unparseable result, a schema-rejected submission the agent never corrects, OR "no result recorded" SHALL be treated as ERRORED — never as "no findings" / "approved" / "verified".
- **The errored state SHALL be operator-visible**, surfaced via chatops AND/OR the artifact the gatekeeper writes, naming the gatekeeper AND the cause — so "ran AND passed" is distinguishable from "could not run".
- **The action on error follows the gatekeeper's role, but none is silent-pass.** A BLOCKING gatekeeper SHALL NOT let the gated work proceed as if it passed: it holds the work in an explicit failed-to-run state an operator clears (distinct from a "found a problem" verdict). An ADVISORY gatekeeper SHALL render an explicit "failed to run" result rather than omit its output OR report success.
- **Transient-failure tolerance is bounded retry, NOT fail-open.** Where a gatekeeper retries transient failures to avoid wedging on a blip, it SHALL — after exhausting the retry bound — enter the errored state, never pass.

The clauses above forbid turning an inability to run into a pass. A gatekeeper whose verdict is a matter of judgment SHALL delegate that judgment to an agent AND SHALL NOT manufacture the verdict in code — a manufactured verdict is fail-open even when nothing errored:

- **The verdict is the agent's, surfaced verbatim — the code synthesizes none.** For a gatekeeper that delegates judgment to an agent, the code's responsibilities are exactly: initialize to the non-passing state, assemble the inputs (prompt + materials), invoke the agent, AND surface the agent's returned verdict. No code path SHALL derive, infer, OR short-circuit the verdict by inspecting the inputs — a code-authored conclusion such as "the materials to evaluate are absent, so this passes" is a manufactured pass, NOT a judgment, even though no error occurred. When a genuine agent evaluation is not possible, the outcome is the failed-to-run state (per the clauses above), NEVER a synthesized pass.
- **The agent is never presented an option set that forecloses failure.** The verdict mechanism (the MCP submission tool's schema, AND the prompt that frames it) SHALL let the agent express a FAILING verdict with its own words ALONE. Structured detail (gap lists, concern arrays) MAY accompany a verdict but SHALL NEVER be a precondition for returning a failing verdict. A prompt OR schema constructed so that pass is the only expressible answer is fail-open by construction.

A developer-facing standards doc SHALL record this invariant so contributors apply it to new gatekeepers.

#### Scenario: A gatekeeper that cannot run does not pass
- **WHEN** a gatekeeper's evaluation cannot complete (CLI/tool unavailable, spawn/timeout error, no result recorded, OR an uncorrected schema-rejected submission)
- **THEN** the outcome is the errored state, surfaced with the gatekeeper name AND the cause
- **AND** it is NOT reported as passed / approved / verified / "no findings"

#### Scenario: A blocking gatekeeper holds rather than waving work through
- **WHEN** a blocking gatekeeper (e.g. an `[in]` or `[canon]` pre-flight) enters the errored state
- **THEN** the gated work does NOT proceed as if the gate passed
- **AND** the work is held in an explicit failed-to-run state an operator clears, distinct from a "found a problem" verdict

#### Scenario: An advisory gatekeeper reports "failed to run", not success
- **WHEN** an advisory gatekeeper (e.g. the `[out]` gate) enters the errored state
- **THEN** it renders an explicit "failed to run" result naming the cause
- **AND** it does NOT omit its output NOR report success / verified

#### Scenario: Verdict defaults and zero-item aggregations are non-passing
- **WHEN** a gatekeeper initializes a verdict OR aggregates a verdict over zero evaluated items
- **THEN** the initial / default / zero-item result is a non-passing state (blocked / errored / unknown)
- **AND** no code path yields approve / pass from a default OR from zero evaluated items

#### Scenario: The code does not synthesize a verdict from the inputs
- **WHEN** an agent-backed gatekeeper could reach a verdict by inspecting its inputs in code (for example, the materials to evaluate are missing or empty)
- **THEN** the code does NOT emit a pass / approve from that inspection
- **AND** the verdict comes only from the agent's evaluation, OR — when no genuine evaluation is possible — the outcome is the failed-to-run state, never a synthesized pass

#### Scenario: The agent can always return a failing verdict
- **WHEN** an agent-backed gatekeeper invokes its agent
- **THEN** the verdict mechanism accepts a failing verdict accompanied by the agent's prose ALONE, with no structured detail required
- **AND** no prompt OR schema constraint makes pass the only expressible verdict
