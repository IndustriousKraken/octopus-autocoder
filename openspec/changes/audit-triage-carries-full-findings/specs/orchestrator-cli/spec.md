## ADDED Requirements

### Requirement: The audit-thread excerpt handed to triage carries each finding's full body

The excerpt stamped into the audit-thread state SHALL carry the same rich,
body-bearing rendering as the posted audit thread body: per finding, the
`severity glyph + subject + anchor` title line FOLLOWED BY that finding's full
`body` field when non-empty. Because that stamped excerpt is exactly the value
handed to the triage executor as `TriageContext.findings` when an operator runs
`send it` in the audit thread (per the existing "`send it` verb in an audit thread
schedules a triage executor run" AND "Triage mode runs the executor with an
explore-then-classify prompt" requirements), the downstream triage agent SHALL
receive the divergence reasoning the audit already produced — what the spec
requires, what the code does, and why — rather than a one-line title it would
otherwise have to re-derive.

This requirement constrains the CONTENT of the stamped excerpt AND of
`TriageContext.findings`; it does NOT add, remove, OR reorder any
`TriageContext` field, NOR change how `send it` schedules or how triage mode runs.
The stamped excerpt remains subject to its existing 35,000-character cap: when the
rich rendering would exceed 35,000 characters it is truncated to that cap and ends
with the existing pointer-to-daemon-log tail, mirroring the thread-body truncation.
A finding whose `body` is empty contributes only its one-line title, with no stray
blank-body artifact.

#### Scenario: The stamped excerpt carries the finding bodies

- **WHEN** a `drift_audit` posts an audit notification whose findings carry divergence `body` paragraphs
- **THEN** the excerpt stamped into the audit-thread state contains each finding's title line AND its full `body` paragraph
- **AND** the stamped excerpt is the rich body-bearing form, not the title-only string

#### Scenario: Triage receives the rich excerpt on `send it`

- **WHEN** an operator runs `@<bot> send it` in the audit thread AND the next polling iteration drains the triage and invokes `run_triage`
- **THEN** the `TriageContext.findings` value equals the stamped rich excerpt — the divergence body text is present
- **AND** the triage agent is NOT handed the title-only form

#### Scenario: The stamped excerpt is still capped at 35,000 characters

- **WHEN** the rich rendering stamped as the excerpt would exceed 35,000 characters
- **THEN** the stamped excerpt is truncated to 35,000 characters AND ends with the existing pointer-to-daemon-log tail
- **AND** the 35,000 cap value AND the tail text are unchanged by this requirement
