# Implementation tasks

## 1. Strip credentials from the CLI strategies (`agentic_run.rs`)

- [x] 1.1 `OpencodeStrategy::provider_block` (and the `opencode.json` assembly): remove the `api_key` from the written config entirely. Keep the MCP block, the permission/sandbox config, and the provider model + base URL.
- [x] 1.2 `ClaudeStrategy`: stop setting `ANTHROPIC_AUTH_TOKEN`. (Model selection via the existing mechanism is fine; auth comes from claude's own login.) Re-check the `ANTHROPIC_BASE_URL` / `ANTHROPIC_MODEL` env: keep only what's needed for model/endpoint selection that is NOT a credential.
- [x] 1.3 Antigravity / any future strategy: same rule — no `api_key` into `mcp_config.json` or any file, none into env.

## 2. Route the key to the in-process HTTP path only

- [x] 2.1 Confirm the resolved `api_key` is consumed only by the daemon's in-process HTTP clients (the `oneshot` reviewer's `LlmClient`, the contradiction-check LLM block) — never handed to a `CliStrategy`.
- [x] 2.2 When a CLI-resolving role carries a configured `api_key`, ignore it in the strategy AND emit exactly one startup WARN that the key is unused for CLI roles.

## 3. Tests

- [x] 3.1 `opencode.json` written for a keyed model is parsed and asserted to contain no `api_key` (and to still contain the MCP/permission/provider-base-URL blocks).
- [x] 3.2 The `claude` strategy's built invocation sets no `ANTHROPIC_AUTH_TOKEN` even with a keyed model.
- [x] 3.3 Across every strategy: no file written into the workspace and no subprocess env entry contains the resolved `api_key` (assert against a sentinel key value).
- [x] 3.4 The in-process HTTP `oneshot` reviewer still receives the key for its call.
- [x] 3.5 A CLI role with a configured key produces exactly one WARN and ignores the key.

## 4. Acceptance gate

- [x] 4.1 `cargo test` passes for the autocoder crate.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 4.3 `openspec validate a003-credentials-never-reach-model --strict` passes.
