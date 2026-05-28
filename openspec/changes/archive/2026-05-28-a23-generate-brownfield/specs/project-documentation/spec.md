## ADDED Requirements

### Requirement: `docs/CHATOPS.md`, `docs/OPERATIONS.md`, AND `docs/CONFIG.md` document the `brownfield` verb
`docs/CHATOPS.md` SHALL document the `brownfield` verb under the chat-driven-workflow verbs section (alongside `propose`, `audit`, `send it`) with syntax, refusal cases, AND the lifecycle-thread behavior. `docs/OPERATIONS.md` SHALL include an onboarding-existing-projects paragraph that names brownfield-drafting as the first step AND describes its relationship to `propose` for ongoing changes. `docs/CONFIG.md` SHALL document the `features.brownfield.{enabled, prompt_path}` schema with defaults AND override semantics.

#### Scenario: CHATOPS.md documents the verb syntax AND refusals
- **WHEN** an operator reads `docs/CHATOPS.md`'s chat-driven-workflow section
- **THEN** a `brownfield` subsection appears with:
  - Syntax: `@<bot> brownfield <repo-substring> <capability-name> [optional guidance]`
  - The slug-pattern constraint `^[a-z][a-z0-9-]*$`
  - The pre-existing-spec refusal AND its suggested alternative (`propose`)
  - The disabled-verb refusal
  - The lifecycle-thread behavior (top-level ack + threaded follow-ups)

#### Scenario: OPERATIONS.md onboarding paragraph names brownfield
- **WHEN** an operator reads `docs/OPERATIONS.md`'s onboarding-existing-projects content
- **THEN** a paragraph names brownfield-drafting as the first step for retrofitting spec-driven development onto a project that predates it
- **AND** the paragraph contrasts brownfield (one-shot per capability, documents existing behavior) with `propose` (used for changes to capabilities once their spec exists)
- **AND** the paragraph notes the recommended cadence: one brownfield run per capability, reviewed AND merged before moving to the next

#### Scenario: CONFIG.md documents the `features.brownfield` block
- **WHEN** an operator reads `docs/CONFIG.md`'s features-block discussion
- **THEN** a `features.brownfield` subsection describes:
  - `enabled: bool` (default `true`) with the disabled-verb behavior
  - `prompt_path: Option<String>` (default `None`) with the workspace-relative path semantics AND the fall-back-to-embedded behavior when the path is unset OR the file is missing
- **AND** the subsection notes that the per-workspace prompt override is a forward-compatible knob: when the broader per-workspace-prompt schema lands, brownfield's override SHALL conform to it
