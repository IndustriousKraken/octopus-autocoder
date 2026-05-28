## ADDED Requirements

### Requirement: `docs/CONFIG.md` contains a Prompt overrides section with a registry table covering every embedded prompt
`docs/CONFIG.md` SHALL contain a `## Prompt overrides` section located near the existing audits-configuration discussion. The section SHALL contain:

1. A short prose paragraph (3-5 sentences) explaining the loader's uniform precedence (per-workspace nested → per-workspace flat-legacy → daemon-level flat-legacy → embedded fallback) AND the one-shot WARN behavior on missing override files.
2. A single registry table listing every embedded prompt with these columns: **Logical id**, **Embedded path**, **Per-workspace override field**, **Legacy daemon-level field**. The table SHALL include one row per `PromptId` enum variant.
3. A short note that new prompts in future changes SHALL declare their override field using the nested `<area>.<thing>.prompt_path` form.

`README.md` SHALL include one sentence in its Configuration section pointing operators at the `docs/CONFIG.md` Prompt overrides table as the canonical reference for customizing prompts.

`config.example.yaml` SHALL include the three new override blocks (`executor.audit_triage`, `executor.chat_request_triage`, `executor.implementer_revision`) commented out, with comments showing the workspace-relative path semantics.

#### Scenario: CONFIG.md registry table is complete
- **WHEN** an operator reads `docs/CONFIG.md`'s `## Prompt overrides` section
- **THEN** the registry table lists every embedded prompt the daemon ships
- **AND** each row names the prompt's logical id (e.g., `Implementer`, `AuditTriage`, `AuditDrift`), its embedded path (e.g., `prompts/implementer.md`), its per-workspace override field (e.g., `executor.implementer.prompt_path` OR `audits.settings.drift_audit.prompt_path`), AND its legacy daemon-level field where one exists (e.g., `executor.implementer_prompt_path`)
- **AND** rows with no legacy field show `—` (em-dash) in the legacy column

#### Scenario: CONFIG.md precedence paragraph names all four levels
- **WHEN** an operator reads the prose paragraph above the table
- **THEN** the paragraph explicitly names the four precedence levels in order: per-workspace nested, per-workspace flat-legacy, daemon-level flat-legacy, embedded fallback
- **AND** the paragraph documents the one-shot WARN on missing override files

#### Scenario: README points at the prompt overrides table
- **WHEN** an operator reads `README.md`'s Configuration section
- **THEN** a sentence names the `docs/CONFIG.md` Prompt overrides table as the canonical reference for customizing prompts
- **AND** the sentence does NOT duplicate the full table contents (single source of truth lives in `docs/CONFIG.md`)

#### Scenario: config.example.yaml shows the three new override blocks
- **WHEN** an operator opens `config.example.yaml`
- **THEN** the file contains commented-out examples for `executor.audit_triage.prompt_path`, `executor.chat_request_triage.prompt_path`, AND `executor.implementer_revision.prompt_path`
- **AND** the comments describe the workspace-relative path semantics AND the loader's fall-back behavior when the file is missing
