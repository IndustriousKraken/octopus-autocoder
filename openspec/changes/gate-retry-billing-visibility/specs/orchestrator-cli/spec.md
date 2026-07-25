# orchestrator-cli delta: gate-retry-billing-visibility

## ADDED Requirements

### Requirement: Verifier-gate no-submission retries are bounded and operator-visible
A verifier-gate session that runs to completion WITHOUT a schema-valid submission MAY be re-attempted, bounded by `executor.verifier_gate_retries` — the number of ADDITIONAL attempts beyond the first (default 2; `0` disables re-attempts). Session errors and timeouts SHALL NOT be re-attempted — they take the gate's existing fail-closed hold immediately. When every allowed attempt ends without a submission, the gate's existing fail-closed disposition applies unchanged (the retry budget delays the hold; it never weakens it).

Every re-attempt is a fresh, fully billed agentic session, so retries SHALL be operator-visible. When a daemon-run gate invocation consumed at least one re-attempt, autocoder SHALL post exactly ONE chatops notification for that invocation (never one per attempt) through the standard outbound notification path, naming: the gate's stable identifier, the change slug, attempts used vs. allowed, the final disposition (clean, findings, or held), and the model-attribution line. The notification fires regardless of the final disposition and is NOT throttled — each occurrence already represents at least one extra billed session, and its purpose is spend attribution: a model that habitually ends sessions without calling its submission tool multiplies gate cost silently while still failing closed, and the operator needs the signal to consider a model change. In daemon-absent contexts (the local `verify` subcommand, standalone runs) the same fields SHALL be emitted as a WARN log line instead of a chatops post.

#### Scenario: Config default and disable
- **WHEN** `config.yaml` omits `executor.verifier_gate_retries`
- **THEN** each gate invocation allows 2 additional attempts after a no-submission completion
- **WHEN** `executor.verifier_gate_retries: 0` is set
- **THEN** a no-submission session takes the fail-closed hold with no re-attempt AND no retry notification fires (no retry occurred)

#### Scenario: A retry that recovers still notifies
- **WHEN** a gate session ends with no schema-valid submission AND a re-attempt then submits a clean verdict
- **THEN** the invocation proceeds normally on the clean verdict
- **AND** one chatops notification names the gate identifier, the change slug, attempts used vs. allowed, the final disposition, and the model-attribution line

#### Scenario: Exhausted retries hold the change and the notification still fires
- **WHEN** every allowed attempt of a pre-executor gate invocation ends without a schema-valid submission
- **THEN** the gate's existing fail-closed hold and "gate FAILED TO RUN — change held" alert apply unchanged
- **AND** the retry notification also fires, naming the attempts used vs. allowed and the held disposition

#### Scenario: Errors and timeouts are not retried and not counted as retries
- **WHEN** a gate session fails with a session error or timeout on its first attempt
- **THEN** no re-attempt is made (the existing fail-closed hold applies)
- **AND** no retry notification fires

#### Scenario: Daemon-absent runs log instead of posting
- **WHEN** the local `verify` subcommand runs a gate AND the invocation consumed at least one re-attempt
- **THEN** a WARN log line carries the gate identifier, the change slug, attempts used vs. allowed, and the final disposition
- **AND** no chatops post is attempted
