# chatops-manager (delta)

OpenSpec: https://github.com/Fission-Labs/openspec

## ADDED Requirements

### Requirement: Preempting in-flight work is acknowledged to the operator
When a workspace-mutating operator command (e.g. the confirmed `rollback` verb) preempts a repository's in-flight pass — cancelling the change that pass was working — the chatops surface SHALL acknowledge the preempt to the operator before, or alongside, the operation's result, so the cancelled change is not a silent surprise. The acknowledgement SHALL name the operation being performed AND the change that was cancelled (e.g. "preempting in-flight work on `<slug>` to roll back"). The change slug SHALL come from the busy marker's recorded `change` field, which the operation reads before preempting.

When the operation finds no pass in flight (no busy marker held, OR a marker with no recorded change), NO preempt acknowledgement SHALL be emitted — the operator sees only the normal operation result. The acknowledgement is conditional on an actual preempt occurring, not on the command being invoked.

The preempt acknowledgement SHALL be best-effort: a chatops post failure SHALL NOT abort the operation, AND the operation SHALL run identically when no chatops backend is configured (the daemon log is the operator's signal in that case). This mirrors the degradation contract the other lifecycle notifications follow.

#### Scenario: Confirmed rollback mid-pass acknowledges the preempt naming the cancelled change
- **WHEN** an operator confirms `rollback` on a repository whose pass is mid-flight working a change AND the operation preempts that pass
- **THEN** the chatops surface posts an acknowledgement naming the operation AND the cancelled change slug (read from the busy marker's `change` field) before the operation proceeds
- **AND** the operation then performs the rollback under the acquired busy marker

#### Scenario: No pass in flight emits no preempt acknowledgement
- **WHEN** an operator confirms a workspace-mutating command on a repository with no busy marker held (no pass in flight)
- **THEN** no preempt acknowledgement is posted
- **AND** the operator sees only the normal operation result

#### Scenario: Preempt acknowledgement is best-effort and degrades without a backend
- **WHEN** a preempt occurs AND the chatops post fails, OR no chatops backend is configured
- **THEN** the operation still preempts, acquires the marker, AND performs the operation identically
- **AND** the daemon logs the preempt so an operator reading the log sees the cancelled change
