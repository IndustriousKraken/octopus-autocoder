# orchestrator-cli — delta for a49-model-attribution-on-llm-output

## ADDED Requirements

### Requirement: Redaction-safe model-attribution accessor
The resolved configuration SHALL expose a redaction-safe accessor that, given an LLM-driven surface (reviewer, contradiction-check, or a named audit), returns a stable attribution string of the form `<provider>/<model>`, where `<provider>` is the `LlmProvider` canonical name (`anthropic`, `openai_compatible`, `ollama`) AND `<model>` is the configured model identifier. The accessor SHALL read only an explicit positive allowlist of non-secret fields (provider AND model) AND SHALL NEVER return, embed, or derive its output from `api_key`, `api_key_env`-resolved values, `api_base_url`, or any other secret- or endpoint-bearing field.

The displayed `<provider>` is the configured provider KIND, not the upstream brand — a model served via an OpenAI-compatible gateway renders as `openai_compatible/<model>` (e.g. `openai_compatible/moonshotai/kimi-latest`), not the gateway's name.

#### Scenario: Accessor returns provider/model without secrets
- **GIVEN** a reviewer config with `provider: openai_compatible`, `model: moonshotai/kimi-latest`, a non-empty `api_base_url`, AND an inline `api_key`
- **WHEN** the attribution accessor is called for the reviewer surface
- **THEN** it returns `openai_compatible/moonshotai/kimi-latest`
- **AND** the returned string contains neither the `api_key` value NOR the `api_base_url`

#### Scenario: Allowlist is positive — a new secret-bearing field cannot leak
- **GIVEN** a future config field is added to an LLM-surface config block
- **WHEN** the attribution accessor runs
- **THEN** it returns only the allowlisted provider AND model fields
- **AND** the new field is NOT included in the output unless it is explicitly added to the safe allowlist

### Requirement: Operator-facing LLM-driven output carries a model-attribution line
Each operator-facing output the daemon composes from an LLM-driven surface with a configured `(provider, model)` SHALL carry a one-line model attribution produced by the redaction-safe accessor, so operators can associate output quality with the model that produced it. The attribution line SHALL have the form `*<Role>: <provider>/<model>*` (e.g. `*Reviewer: openai_compatible/moonshotai/kimi-latest*`). The covered surfaces are:

- the initial-review `## Code Review` PR-body section AND the `## Code Review (rerun N of M)` re-review comment — role `Reviewer` (in per_change mode each per-change section carries the reviewer attribution).
- each audit's operator-facing PR-body section AND chatops finding notification — role `Auditor (<audit-type>)`.
- the change-internal contradiction-check findings — role `Contradiction-check`.

The executor's `## Agent implementation notes` section is OUT of scope for this requirement: the executor wraps the Claude CLI AND has no daemon-known `(provider, model)` (it uses the CLI's configured model). Its attribution is deferred to the model-registry work that gives the executor a resolvable model; this change SHALL NOT add a false or placeholder attribution to it.

#### Scenario: Reviewer output carries attribution
- **WHEN** the daemon composes a `## Code Review` section (initial OR rerun) from a reviewer configured with `provider: anthropic`, `model: claude-opus-4-8`
- **THEN** the composed output contains the line `*Reviewer: anthropic/claude-opus-4-8*`
- **AND** the line is produced via the redaction-safe accessor (no secret material)

#### Scenario: Audit finding carries attribution
- **WHEN** the daemon composes an audit's operator-facing PR section OR chatops finding notification from an audit configured with a `(provider, model)`
- **THEN** the output carries `*Auditor (<audit-type>): <provider>/<model>*`

#### Scenario: A surface without a daemon-known model is not falsely attributed
- **WHEN** the daemon composes the executor's `## Agent implementation notes` section
- **THEN** no model-attribution line is added (the executor has no daemon-known model in this change)
- **AND** the deferral is documented so the gap is intentional, not an omission
