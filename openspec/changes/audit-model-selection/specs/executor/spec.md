## ADDED Requirements

### Requirement: Agentic run model resolution for audits
The agentic run primitive SHALL accept an optional `ResolvedModel` parameter when invoked for periodic audits. When a model is provided, the audit runner SHALL dynamically select the appropriate `CliStrategy` (e.g., `ClaudeStrategy`, `OpencodeStrategy`, `AntigravityStrategy`) based on the resolved model's provider using the `strategy_for_provider` function, rather than hardcoding a single strategy. The resolved model SHALL be passed to the CLI execution command, ensuring the CLI receives the appropriate `--model <provider>/<model>` flag (or equivalent) when supported by the strategy.

#### Scenario: Audit runs with a resolved OpenRouter model
- **WHEN** an audit is executed with a `ResolvedModel` where the provider is `openai_compatible`
- **THEN** the audit runner selects the `OpencodeStrategy`
- **AND** the CLI command includes the `--model openai_compatible/<model_name>` flag
- **AND** the CLI is invoked with the provider's configured API key and base URL

#### Scenario: Audit runs with a resolved Anthropic model
- **WHEN** an audit is executed with a `ResolvedModel` where the provider is `anthropic`
- **THEN** the audit runner selects the `ClaudeStrategy`
- **AND** the CLI command includes the `--model anthropic/<model_name>` flag (if applicable to the CLI)
- **AND** the CLI is invoked with the provider's configured API key

#### Scenario: Audit runs without a model (backward compatibility)
- **WHEN** an audit is executed with `None` for the model parameter
- **THEN** the audit runner defaults to the `ClaudeStrategy`
- **AND** no `--model` flag is appended to the CLI command
- **AND** the CLI uses its locally configured default model and authentication
