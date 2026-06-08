## ADDED Requirements

### Requirement: Audit model selection
Periodic audits SHALL support an optional `model` field under `audits.settings.<audit_type>` in the configuration file. When specified, the value SHALL be a nickname referencing an entry in the top-level `models:` registry. At config load, this nickname SHALL be resolved to its full `(provider, model, api_base_url, api_key)` tuple. If the nickname does not exist in the registry, config validation SHALL fail fast with an error naming the missing nickname.

#### Scenario: Audit configured with a valid registry nickname
- **WHEN** an operator configures `audits.settings.drift_audit.model: "my_audit_model"`
- **AND** `my_audit_model` exists in the `models:` registry
- **THEN** config validation succeeds
- **AND** the audit runner receives the resolved model configuration for that audit

#### Scenario: Audit configured with an invalid registry nickname
- **WHEN** an operator configures `audits.settings.security_bug_audit.model: "nonexistent_model"`
- **AND** `nonexistent_model` is not present in the `models:` registry
- **THEN** config validation fails at startup
- **AND** the error message names the missing nickname and the referencing audit setting

#### Scenario: Audit without a model field defaults to existing behavior
- **WHEN** an operator does not specify a `model` field under `audits.settings.<audit_type>`
- **THEN** the audit runner receives `None` for the model configuration
- **AND** the audit executes using the default `claude` CLI strategy with no model override, preserving backward compatibility
