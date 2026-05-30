## ADDED Requirements

### Requirement: `docs/CHATOPS.md`, `docs/OPERATIONS.md`, AND `docs/CONFIG.md` document `brownfield-survey`, `send it`-in-survey-thread, `clear-survey`, AND the `features.brownfield_survey` config block
`docs/CHATOPS.md` SHALL contain:

- A `### brownfield-survey` subsection under chat-driven workflow with syntax, output shape (numbered items with slug, complexity, summary, scope_in/out, source_modules), lifecycle-thread behavior, AND the disabled-verb refusal.
- An extension to the existing `### send it` subsection naming the brownfield-survey-thread context as a second valid invocation site (alongside audit threads). The extension describes the batch-generation behavior at a high level: one item per iteration; per-item status updates AND a final summary.
- A `### clear-survey` subsection under operator-recovery verbs alongside `clear-perma-stuck`, `clear-revision`, `clear-scout`, `wipe-workspace`.

`docs/OPERATIONS.md` SHALL contain a "Bootstrapping specs for an existing project" section that:

- Describes the survey → review → send-it batch loop as the recommended workflow for previously-unspecced projects.
- Cross-references `a23`'s single-capability brownfield AS the right shape for narrow gaps (one capability to add) AND positions survey-and-batch as the right shape for whole-project bootstrap.
- Includes a worked example showing the typical operator interaction: invoke survey, scan the list, optionally re-invoke with refined guidance, then `send it` AND let the daemon work through the list.
- Notes the per-iteration discipline (one spec PR at a time) AS a deliberate choice to avoid context-compression failures.

`docs/CONFIG.md` SHALL document `features.brownfield_survey.{enabled, prompt_path, max_capabilities}` with defaults, valid ranges, AND a cross-link to the OPERATIONS.md bootstrap section.

The `a24` Prompt overrides table SHALL be extended with the `BrownfieldSurvey` entry (logical id `BrownfieldSurvey`, embedded path `prompts/brownfield-survey.md`, per-workspace override `features.brownfield_survey.prompt_path`, legacy field `—`).

`config.example.yaml` SHALL include the `features.brownfield_survey` block commented out, with each field's default in a comment.

#### Scenario: CHATOPS.md documents brownfield-survey
- **WHEN** an operator reads `docs/CHATOPS.md`'s chat-driven-workflow section
- **THEN** a `### brownfield-survey` subsection appears naming the syntax, output shape, lifecycle behavior, AND refusal cases

#### Scenario: CHATOPS.md extends send-it to name the survey context
- **WHEN** an operator reads the `### send it` subsection
- **THEN** the text names BOTH valid invocation contexts (audit thread AS the canonical case AND brownfield-survey thread AS the new case)
- **AND** describes the batch-generation behavior at a high level (one item per iteration; per-item status; final summary)

#### Scenario: CHATOPS.md documents clear-survey under recovery verbs
- **WHEN** an operator reads `docs/CHATOPS.md`'s operator-recovery section
- **THEN** a `### clear-survey` subsection appears with the wipe-all-surveys behavior AND its idempotence

#### Scenario: OPERATIONS.md bootstrap section is complete
- **WHEN** an operator reads the "Bootstrapping specs for an existing project" section
- **THEN** the section names the survey → review → send-it loop AS the recommended workflow
- **AND** cross-references `a23`'s single-capability brownfield for narrow gaps
- **AND** includes a worked-example operator transcript
- **AND** names the per-iteration discipline AS the mechanism for avoiding context compression

#### Scenario: CONFIG.md documents features.brownfield_survey
- **WHEN** an operator reads the `features.brownfield_survey` subsection
- **THEN** each field is documented with its default
- **AND** `max_capabilities`'s valid range `1..=50` is named
- **AND** the `prompt_path` entry cross-links to the Prompt overrides table

#### Scenario: Prompt overrides table includes BrownfieldSurvey
- **WHEN** an operator reads `docs/CONFIG.md`'s `## Prompt overrides` table
- **THEN** a `BrownfieldSurvey` row appears with embedded path `prompts/brownfield-survey.md`, per-workspace override `features.brownfield_survey.prompt_path`, legacy field `—`
