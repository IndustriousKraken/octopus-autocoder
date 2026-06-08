## Context

Currently, the periodic audit execution path (`run_audit_cli` and `run_audit_cli_with_submit` in `audits/mod.rs`) is hardcoded to instantiate `ClaudeStrategy` and passes `model: None` to `agentic_run`. This prevents operators from leveraging the `models:` registry to route audits to alternative providers (e.g., OpenRouter via `opencode`) or specialized models (e.g., Kimi for large context security sweeps). The executor, reviewer, and pre-flight checks already support this pattern via `strategy_for_provider` and `ResolvedModel`.

## Goals / Non-Goals

**Goals:**
- Allow any periodic audit to specify a `model` nickname in `config.yaml` under `audits.settings.<audit_type>`.
- Resolve the nickname against the `models:` registry at config load.
- Dynamically select the correct `CliStrategy` (e.g., `OpencodeStrategy`, `ClaudeStrategy`) based on the resolved model's provider.
- Pass the resolved model to `agentic_run` so the CLI receives the appropriate `--model` flag.

**Non-Goals:**
- Changing the default behavior for audits that do not specify a `model` (they will continue to use the `claude` CLI with no model override).
- Adding new audit types (this change only enables model selection for existing ones).

## Decisions

1. **Reuse `strategy_for_provider`**: Instead of creating a new audit-specific strategy resolver, we will reuse the existing `crate::agentic_run::strategy_for_provider` function. This ensures consistency with how the reviewer and executor resolve their strategies.
2. **Optional `model` field in `AuditSettings`**: The `model` field will be `Option<String>`. If omitted, the audit runner will pass `None` for the model, and the existing hardcoded `ClaudeStrategy` fallback will be used to maintain backward compatibility.
3. **Config Validation**: The `model` field will be validated at config load time using the same `resolve_model_reference` logic as the reviewer, ensuring typos or missing registry entries fail fast.

## Risks / Trade-offs

- **Risk**: If an operator specifies a model with a provider that lacks a registered `CliStrategy` (e.g., a hypothetical future provider), `strategy_for_provider` will return an error. 
  - **Mitigation**: The config validation step will catch this, and the error message will clearly state which provider lacks a strategy, guiding the operator to fix the config or add the strategy.
- **Risk**: Audits running with `opencode` might behave slightly differently than `claude` in terms of tool usage or output formatting.
  - **Mitigation**: The `agentic_run` primitive and MCP submission framework (`run_audit_cli_with_submit`) already abstract these differences. The read-only sandbox enforcement (`WritePolicy::None`) remains unchanged regardless of the CLI used.
