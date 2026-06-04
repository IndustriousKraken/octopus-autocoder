## Why

PR #95's reviewer found a real, live credential hole: `OpencodeStrategy::provider_block` writes the resolved `api_key` in plaintext into `opencode.json` at the workspace root (`agentic_run.rs`), and `opencode.json` is not in the workspace git-exclude list — so a `git add -A` (by the daemon or by the agent's own Bash during a revision) could commit a plaintext key to the fork/repo. It only bites when an opencode role is configured with a key, but it must close before opencode is used.

The deeper rule the incident exposes: **the model never needs an LLM credential, so it should never be handed one.** The CLI *process* authenticates by injecting its key into the TLS header in its own memory; the model is just tunneled across that authenticated connection. Handing the key to the subprocess at all — via a workspace file (committable) OR via env (readable from the agent's Bash, and for Anthropic it also forces pay-per-token *off* the operator's subscription) — is a leak. With multiple CLIs that means every model ends up able to read keys it has no business seeing.

This change is the **key-flow** half: nothing passes a credential to a subprocess. (The OS-sandbox half — a model can't *reach* a credential even so — is `docs/design/agentic-subprocess-sandbox.md`, specced separately once the mechanism is chosen. The two are complementary.)

## What Changes

**No `CliStrategy` passes an LLM credential to the wrapped CLI** — not into a config file (`opencode.json` / `mcp_config.json` / `.gemini`), not via env. A strategy selects the model (`--model`) and relies on the CLI's **own** authentication (its own credential store / login — `claude login`, opencode/Big-Pickle, `agy` login, or the operator's out-of-band CLI provider config for e.g. opencode → OpenRouter). This supersedes the credential-passing in the `claude` strategy (it no longer sets `ANTHROPIC_AUTH_TOKEN`) and corrects `a60` (the `opencode.json` it writes carries the MCP block + permissions + provider base-URL/model, but **no** `api_key`).

**A resolved `api_key` flows only to autocoder's in-process HTTP clients** — the non-agentic (`oneshot`) reviewer and the contradiction-check LLM block, which autocoder calls directly so the key stays in the daemon's process and never reaches a model. A model resolving to a CLI strategy ignores any configured `api_key` (and the daemon emits one startup WARN that the key is unused for CLI roles).

## Impact

- **Affected specs:** `executor` — ADD `CLI strategies pass no LLM credential to the wrapped subprocess`.
- **Affected code:** `agentic_run.rs` — `OpencodeStrategy::provider_block` drops the `api_key` from `opencode.json`; the `claude` strategy stops setting `ANTHROPIC_AUTH_TOKEN`; no strategy writes a key to a file or env. The `ResolvedModel.api_key` is consumed only on the in-process HTTP path.
- **Operator-visible behavior:** CLI roles authenticate via their own CLI login (no behavior change for operators already using login, e.g. you on Claude Max / opencode Big-Pickle); a configured `api_key` on a CLI role is ignored with one WARN. The HTTP `oneshot` reviewer is unchanged (key stays in-process). No credential lands in a workspace file or a subprocess env.
- **Dependencies:** none for the key-flow itself. Complementary to the sandbox design (which enforces "can't reach a key"); independent of a004/a005.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a003-credentials-never-reach-model --strict` passes. Tests: `opencode.json` written for a keyed model contains no `api_key`; the `claude` strategy sets no `ANTHROPIC_AUTH_TOKEN`; no strategy writes any credential to a workspace file or env; the in-process HTTP reviewer still receives the key.
