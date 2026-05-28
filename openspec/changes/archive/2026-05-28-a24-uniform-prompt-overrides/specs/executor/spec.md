## ADDED Requirements

### Requirement: Prompt loader applies a uniform embedded → per-workspace → daemon-level → embedded fallback precedence
The daemon SHALL load every embedded prompt template through a single `PromptLoader` helper. The loader SHALL accept a `PromptId` enum value (one variant per embedded prompt) AND the resolved per-repo configuration, AND SHALL return the prompt's content string. For each `(PromptId, config)` call the loader SHALL resolve in this precedence:

1. The per-workspace override path (when configured AND the file exists at the workspace-relative location).
2. The per-workspace LEGACY flat-name path (when the modernized nested form is unset AND a legacy field exists for this prompt AND its file exists).
3. The daemon-level legacy override path (when set AND the file exists).
4. The embedded default loaded via `include_str!` at compile time.

When a configured override path is present BUT the file at that path does NOT exist, the loader SHALL log a one-shot WARN naming the `(PromptId, missing-path)` pair AND fall through to the next precedence level. The one-shot tracking SHALL persist for the daemon's lifetime; repeated loads of the same `(PromptId, path)` SHALL NOT re-emit the WARN.

Every consumer of an embedded prompt — audits, the implementer executor mode, the implementer-revision flow, the code reviewer, the changelog stylist, the audit-triage flow, the chat-request-triage flow, the brownfield handler, AND any prompt added by future changes — SHALL invoke `PromptLoader::load(PromptId::X, &workspace_config)` instead of inlining `include_str!` at the call site.

#### Scenario: Embedded default loads when no override configured
- **WHEN** the workspace config has no override for `PromptId::Implementer` AND no daemon-level legacy field is set
- **THEN** `PromptLoader::load(PromptId::Implementer, &cfg)` returns the `include_str!`-embedded `prompts/implementer.md` contents

#### Scenario: Per-workspace nested override wins
- **WHEN** the workspace config has `executor.implementer.prompt_path: "./prompts/implementer-custom.md"` AND that file exists
- **THEN** the loader returns the file's contents
- **AND** does NOT consult the embedded default OR any legacy field

#### Scenario: Legacy daemon-level override applies when no per-workspace override exists
- **WHEN** the workspace config has no `executor.implementer.prompt_path` AND no `executor.implementer_prompt_path` AND the daemon-level config has `executor.implementer_prompt_path: /etc/autocoder/implementer.md` AND that file exists
- **THEN** the loader returns the daemon-level file's contents

#### Scenario: Per-workspace overrides preempt daemon-level legacy
- **WHEN** the workspace config has `executor.implementer.prompt_path: "./workspace-implementer.md"` AND the daemon-level config has `executor.implementer_prompt_path: /etc/autocoder/implementer.md` AND both files exist
- **THEN** the loader returns the workspace file's contents
- **AND** the daemon-level path is not read

#### Scenario: Missing override file logs WARN once and falls back
- **WHEN** the workspace config has `executor.implementer.prompt_path: "./missing.md"` AND that file does NOT exist
- **THEN** the loader logs a WARN naming `PromptId::Implementer` AND the missing path
- **AND** falls through to the next precedence level (daemon-level, then embedded)
- **WHEN** the same `(PromptId::Implementer, "./missing.md")` is loaded again later in the daemon's lifetime
- **THEN** no further WARN is logged

#### Scenario: Each embedded prompt has a `PromptId` variant
- **WHEN** the test suite enumerates `prompts/*.md` files via `std::fs::read_dir` at test time
- **THEN** every file corresponds to exactly one `PromptId` enum variant
- **AND** the registry-completeness test fails if a `prompts/<new>.md` file is added without a matching variant

### Requirement: `executor.audit_triage.prompt_path`, `executor.chat_request_triage.prompt_path`, AND `executor.implementer_revision.prompt_path` are per-workspace overrides for the three previously-unoverridable prompts
The per-repo config schema SHALL accept three new optional override blocks under `executor`:

- `audit_triage.prompt_path: Option<String>` — override for `prompts/audit-triage.md` (used by the polling-iteration triage flow that handles `send it` requests).
- `chat_request_triage.prompt_path: Option<String>` — override for `prompts/chat-request-triage.md` (used by the polling-iteration triage flow that handles `propose` requests).
- `implementer_revision.prompt_path: Option<String>` — override for `prompts/implementer-revision.md` (used by the implementer when iterating on revision-loop comments).

Each path is workspace-relative when set. Each defaults to `None`. The `PromptLoader` resolves them per the uniform precedence above.

#### Scenario: audit_triage override resolves
- **WHEN** the workspace config has `executor.audit_triage.prompt_path: "./prompts/triage-custom.md"` AND the file exists
- **THEN** the polling iteration's triage invocation loads the override
- **AND** the LLM receives the custom template

#### Scenario: chat_request_triage override resolves
- **WHEN** the workspace config has `executor.chat_request_triage.prompt_path: "./prompts/chat-triage-custom.md"` AND the file exists
- **THEN** the polling iteration's `propose`-flow triage invocation loads the override

#### Scenario: implementer_revision override resolves
- **WHEN** the workspace config has `executor.implementer_revision.prompt_path: "./prompts/revision-custom.md"` AND the file exists
- **THEN** the implementer-revision flow loads the override

#### Scenario: Missing override path falls back to embedded
- **WHEN** any of the three new override paths is configured to a path that does NOT exist
- **THEN** the loader logs the one-shot WARN per the uniform precedence
- **AND** the embedded default is used

### Requirement: New prompts SHALL declare their override field via the nested naming convention
Any new embedded prompt added in future changes SHALL declare its override field using the nested `<area>.<thing>.prompt_path` form (matching `audits.settings.<slug>.prompt_path` AND `features.brownfield.prompt_path` AND the three new fields above). Flat suffix forms (`<area>.<thing>_prompt_path`) MAY remain in use ONLY for the existing legacy fields documented in the registry; new prompts SHALL NOT introduce additional flat-suffix overrides.

#### Scenario: New prompt adds nested override field
- **WHEN** a future change introduces a new embedded prompt (e.g., `prompts/scout.md`)
- **THEN** its override field is named `<area>.scout.prompt_path` (nested), NOT `<area>.scout_prompt_path` (flat)

#### Scenario: Existing legacy fields remain accepted
- **WHEN** an operator config sets `executor.implementer_prompt_path` (the legacy flat field)
- **THEN** the config parses successfully AND the loader honors the field per the uniform precedence
- **AND** no deprecation error fires (the field is accepted indefinitely for backward compatibility)
