## ADDED Requirements

### Requirement: Contradiction gates report every contradiction in a single pass
The `[in]` (change-internal) AND `[canon]` (change-vs-canonical) contradiction gates SHALL enumerate the COMPLETE set of contradictions found in a single evaluation AND SHALL NOT stop after the first. A single requirement the change introduces or modifies MAY conflict with more than one other requirement — with another requirement in the same change (for `[in]`), or with more than one canonical requirement, possibly across more than one capability (for `[canon]`); each distinct conflict SHALL be reported as its own finding.

The gate prompts SHALL direct an exhaustive sweep. The `[in]` prompt SHALL direct the agent to evaluate every requirement in the change against every other requirement in the change. The `[canon]` prompt SHALL direct the agent to read the canonical specs of EVERY capability whose invariants the change's behavior bears on — not only the capability that shares the delta's name or is most obviously related — so a change requirement that violates an invariant in a second capability is not missed. The single-element JSON example in each prompt SHALL be presented as illustrative of a set, so it does not anchor the agent toward reporting exactly one finding.

The submission tools (`submit_contradictions`, `submit_canon_contradictions`) accept an unbounded array, AND every downstream surface — the `.needs-spec-revision.json` marker's `revision_suggestion` AND the chatops alert — SHALL carry every submitted finding without truncation or cap.

Completeness SHALL NOT erode precision: the gates' existing false-positive guardrails remain in force, in particular that a `## MODIFIED Requirements` delta is never a contradiction with the same-titled canonical requirement it supersedes. An exhaustive sweep reports every REAL conflict; it does not invent conflicts to lengthen the list.

#### Scenario: Every submitted finding reaches the marker and the alert
- **WHEN** a gate session submits multiple distinct contradictions in one pass
- **THEN** the `.needs-spec-revision.json` marker's `revision_suggestion` enumerates all of them
- **AND** the chatops alert lists all of them
- **AND** none is dropped or truncated away by a per-run cap

#### Scenario: One requirement conflicting with multiple canonical requirements yields one finding each
- **WHEN** a requirement the change introduces or modifies conflicts with two distinct canonical requirements — including the case where the second is in a different capability than the first
- **THEN** the `[canon]` gate reports two findings, one naming each conflicting canonical requirement
- **AND** it does not report only the first and consider the change evaluated

#### Scenario: Exhaustiveness preserves the MODIFIED guardrail
- **WHEN** a change carries a `## MODIFIED Requirements` delta of a canonical requirement AND also has one genuine conflict with a different, unmodified canonical requirement
- **THEN** the `[canon]` gate reports the one genuine conflict
- **AND** it does NOT report the MODIFIED delta as a contradiction with the same-titled canonical requirement it supersedes

### Requirement: Contradiction gate findings carry a concrete, actionable suggested fix
Each contradiction a gate reports SHALL carry a concrete suggested fix — a specific proposed edit that would resolve it — recorded distinctly from the one-line summary of WHY the two requirements conflict. The summary states why they conflict; the suggested fix states WHAT to change and HOW, so an operator can tell what the revision would actually do from the first output rather than re-deriving it.

The submission tools SHALL accept a `suggested_fix` field per finding, alongside the existing identity AND summary fields. The gate prompts SHALL direct the agent to produce, for each contradiction, a concrete edit plan — which requirement(s) to ADD, MODIFY, RENAME, or REMOVE, and a sketch of the resulting text — NOT a restatement of the contradiction. The marker's `revision_suggestion` AND the chatops alert SHALL render each finding's suggested fix prominently AND labeled distinctly from its summary. A finding whose `suggested_fix` is absent (an older daemon, or a model omission) SHALL still render its identity AND summary — the suggested fix is additive, never a parse-or-render precondition.

#### Scenario: A finding's suggested fix appears in the operator-facing output
- **WHEN** a gate reports a contradiction carrying a `suggested_fix`
- **THEN** the marker's `revision_suggestion` AND the chatops alert show that concrete proposed edit — naming the requirement(s) to change and the resulting text — labeled distinctly from the summary
- **AND** the output is not merely a restatement of the contradiction

#### Scenario: Summary and suggested fix are distinct fields
- **WHEN** a finding is recorded
- **THEN** its summary (why the two conflict) AND its suggested fix (what edit resolves it) are carried as separate fields
- **AND** the operator-facing rendering presents both, each labeled

#### Scenario: A finding without a suggested fix still renders
- **WHEN** a finding is reported with no `suggested_fix`
- **THEN** the marker AND alert still render the finding's identity AND summary
- **AND** no parse or render error occurs (the suggested fix is additive)
