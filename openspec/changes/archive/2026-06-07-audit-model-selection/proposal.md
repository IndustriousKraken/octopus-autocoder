## Why

Periodic audits are currently hardcoded to use the `claude` CLI strategy without model selection, preventing operators from routing them to cost-effective or specialized models (e.g., OpenRouter via `opencode`, or Kimi for large context). This breaks the consistency of the `models:` registry, which already successfully supports model selection for the executor, reviewer, and pre-flight checks.

## What Changes

- **Audit Configuration**: Adds an optional `model` field to `AuditSettings` in `config.yaml`, allowing operators to specify a model nickname from the `models:` registry for any periodic audit.
- **Strategy Resolution**: Updates the audit execution path to dynamically resolve the correct `CliStrategy` (e.g., `OpencodeStrategy`, `ClaudeStrategy`) based on the resolved model's provider, rather than hardcoding `ClaudeStrategy`.
- **Model Flag Propagation**: Ensures the resolved model's provider and model name are passed to the CLI via the `--model <provider>/<model>` flag (or equivalent) when applicable.

## Capabilities

### New Capabilities
- `audit-model-selection`: Allows periodic audits to resolve and use a specific LLM model and CLI strategy from the global `models:` registry.

### Modified Capabilities
- `orchestrator-cli`: Modifies the periodic audit configuration schema to accept a `model` field and validates it against the registry.
- `executor`: Modifies the agentic run primitive to accept and apply model resolution for audit roles, enabling dynamic CLI strategy selection.

## Impact

- **Affected specs**: `orchestrator-cli`, `executor`.
- **Affected code**: `config.rs` (add `model` to `AuditSettings` and validation), `audits/mod.rs` (update `run_audit_cli` and `run_audit_cli_with_submit` to accept `Option<&ResolvedModel>` and use `strategy_for_provider`), `agentic_run.rs` (ensure strategy resolution works for audit roles).
- **Operator-visible behavior**: Operators can now add `model: <nickname>` under `audits.settings.<audit_type>` to route specific audits to specific CLI-driven models defined in the `models:` registry. Existing configs without this field continue to default to the `claude` CLI with no model override.
- **Dependencies**: Builds on `a55` (model registry), `a56` (agentic run primitive + `strategy_for_provider`), and `a57` (audit MCP submission).
