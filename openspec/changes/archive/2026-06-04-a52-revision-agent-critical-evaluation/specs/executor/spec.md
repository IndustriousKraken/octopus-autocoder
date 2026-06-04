# executor — delta for a52-revision-agent-critical-evaluation

## ADDED Requirements

### Requirement: Revision prompt instructs critical evaluation of the reviewer's request
`prompts/implementer-revision.md` SHALL instruct the revision agent to evaluate the triggering request critically rather than assume it is correct. Before applying a requested change, the agent reads the actual code at the cited location, verifies the request's claim against the current state, and — when the claim is wrong (mistaken about the code, would break a passing or spec-traced test, references a symbol that does not exist, or churns working idiomatic code for protection that does not apply) — declines OR partially honors the request AND reports what it declined and why via the `outcome_success` `final_answer` summary.

Declining a wrong request is a valid, successful outcome the agent reports; it is NOT a failure AND NOT grounds to fabricate a change that satisfies the literal request at the cost of correctness. The agent reports its evaluation through the existing `final_answer` surface (no new outcome tool); the no-change declination path is handled by the orchestrator-cli `Revision execution updates the agent branch and posts a reply comment` requirement.

The guidance SHALL be language-neutral — it references "the project's test and lint commands" rather than a specific toolchain, so it applies to any managed repository.

This is design intent for the revision prompt's content. It is verified by review AND the drift audit's semantic judgment — NOT by a unit test asserting the prompt's wording (per the project-documentation requirement `Tests assert behavior or derivation, never message wording`).

#### Scenario: Revision prompt instructs claim verification before applying
- **WHEN** the revision prompt is reviewed against this requirement (by a human reviewer OR the drift audit)
- **THEN** it instructs the agent to read the cited code AND verify the request's claim against the current state before applying any change
- **AND** it states that declining or partially honoring a wrong request is a valid outcome the agent SHALL report via `final_answer`, not a failure and not grounds to fabricate a change

#### Scenario: A reasoned declination is reported, not engineered around
- **GIVEN** a request whose claim is mistaken (e.g. it references a test or symbol that does not exist, or asks to remove a spec-traced test)
- **WHEN** the agent evaluates it per the prompt's guidance
- **THEN** the prompt directs the agent NOT to make a change that satisfies the literal request at the cost of correctness
- **AND** to call `outcome_success` with a `final_answer` naming the request, the verification it performed, AND why it declined or partially honored the request
