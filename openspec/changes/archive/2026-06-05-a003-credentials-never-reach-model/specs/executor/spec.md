# executor — delta for a003-credentials-never-reach-model

## ADDED Requirements

### Requirement: CLI strategies pass no LLM credential to the wrapped subprocess
No `CliStrategy` implementation SHALL pass an LLM credential (the resolved `api_key`) to the wrapped CLI — NOT by writing it into a config file in the workspace (`opencode.json`, `mcp_config.json`, `.gemini/*`, etc.), and NOT by setting it in the subprocess environment. A strategy SHALL select the model (e.g. `--model`) and rely on the CLI's **own** authentication — the CLI's own credential store or login (`claude login`, opencode / Big-Pickle, `agy` login), or the operator's out-of-band CLI provider config (e.g. opencode → OpenRouter configured in opencode's own config). This supersedes any prior per-strategy credential passing: the `claude` strategy SHALL NOT set `ANTHROPIC_AUTH_TOKEN`, AND the `opencode` strategy's `opencode.json` SHALL carry the MCP block + the permission/sandbox config + the provider's model/base-URL, but NOT the `api_key`.

The rationale is that the model never needs the credential: the CLI **process** authenticates by injecting the key into the request in its own memory; the model is tunneled across that connection. A credential written to a workspace file can be committed; a credential in the subprocess env is readable from the agent's Bash (and, for Anthropic, an env key also forces pay-per-token off the operator's subscription).

A resolved `api_key` SHALL flow only to autocoder's **in-process** HTTP clients (the non-agentic `oneshot` reviewer AND the contradiction-check LLM block), which the daemon calls directly so the key stays in the daemon's process and never reaches a model. When a role that resolves to a CLI strategy has a configured `api_key`, the strategy SHALL ignore it AND the daemon SHALL emit exactly one startup WARN noting the key is unused for CLI roles.

#### Scenario: opencode.json carries no api_key
- **WHEN** the `opencode` strategy writes `opencode.json` for a role whose resolved model has a non-empty `api_key`
- **THEN** the written `opencode.json` contains the MCP block, the permission/sandbox config, AND the provider's model + base URL
- **AND** it does NOT contain the `api_key`

#### Scenario: claude strategy sets no auth token
- **WHEN** the `claude` strategy builds an invocation for a role whose resolved model has an `api_key`
- **THEN** the invocation sets NO `ANTHROPIC_AUTH_TOKEN`
- **AND** claude authenticates from its own login/credential store

#### Scenario: no strategy writes a credential to a workspace file or env
- **WHEN** any `CliStrategy` builds its invocation AND/OR writes its config
- **THEN** no credential (the resolved `api_key`) appears in any file written into the workspace
- **AND** no credential appears in the subprocess environment

#### Scenario: a configured CLI-role key is ignored with one warning
- **WHEN** a role that resolves to a CLI strategy is configured with an `api_key`
- **THEN** the strategy ignores it (the CLI uses its own auth)
- **AND** the daemon emits exactly one startup WARN that the key is unused for CLI roles

#### Scenario: in-process HTTP roles still receive the key
- **WHEN** the non-agentic `oneshot` reviewer (or the contradiction-check LLM block) runs with a configured `api_key`
- **THEN** the key is used by the daemon's in-process HTTP client for that call
- **AND** the key is never passed to a subprocess (file or env)
