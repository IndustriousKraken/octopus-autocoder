# Implementation tasks

## 1. Spike: verify headless opencode behaviors (do first)

- [ ] 1.1 Determine prompt delivery: does `opencode run` read the prompt from stdin, or require it as a positional argument? Record which, so `build_command` delivers the prompt the right way.
- [ ] 1.2 Confirm `opencode run` (headless) invokes MCP tools configured via `opencode.json` (`mcp` block, `type: local`) — i.e. a `submit_*` tool is actually called by the model.
- [ ] 1.3 Confirm a daemon-rejected `submit_*` call (schema-invalid → `record_submission` error) reaches the model as a tool error it can correct and retry within the same `opencode run` session. This is the load-bearing contract for the submission roles.
- [ ] 1.4 Determine opencode's permission/sandbox configuration shape AND confirm a read-only profile (allow Read/Glob/Grep equivalents; deny Write/Edit/Bash) is enforceable from `opencode.json` or invocation flags.
- [ ] 1.5 If any of 1.2–1.4 cannot be satisfied under the current opencode release, STOP and report the blocker (do not ship a strategy that silently drops submissions or sandboxing). Capture the opencode version probed.

## 2. `OpencodeStrategy` (executor)

- [ ] 2.1 `struct OpencodeStrategy` implementing a56's `CliStrategy`. `build_command` produces `opencode run` with the prompt delivered per task 1.1, `--model <provider>/<model>`, and writes `opencode.json` into the workspace.
- [ ] 2.2 `opencode.json` writer: the `mcp` block (`type: local`, the MCP-child command, env including `ORCH_MCP_ROLE`) AND the provider config (base URL + key) for the resolved model. Do NOT write `.mcp.json` (that is the claude strategy's format).
- [ ] 2.3 `apply_model_selection`: translate `(provider, model, api_base_url, api_key)` into `--model <provider>/<model>` + the `opencode.json` provider entry. Set NO `ANTHROPIC_*` env.
- [ ] 2.4 Map a56's sandbox (allowed-tools list + deny patterns) onto opencode's permission configuration so a read-only role denies Write/Edit/Bash and exposes only the read tools + the role's MCP tool (per task 1.4).
- [ ] 2.5 Run opencode roles in capture mode (read stdout/stderr at exit). The streaming-JSON `final_answer`/`session_id`/incremental-log path stays claude-specific; the executor's streaming implementer path remains on the claude strategy.

## 3. Registration

- [ ] 3.1 Register `opencode` in the strategy resolver so a55's `provider → CLI` rule (`openai_compatible`/`ollama` default, or an explicit registry `cli: opencode`) resolves to `OpencodeStrategy` instead of erroring.

## 4. Tests

- [ ] 4.1 A role whose model resolves (via a55) to `opencode` returns `OpencodeStrategy` and builds an `opencode run` invocation — no "no registered strategy" error.
- [ ] 4.2 `OpencodeStrategy` writes `opencode.json` with the `mcp` block (incl. `ORCH_MCP_ROLE`) + provider config, AND writes NO `.mcp.json`.
- [ ] 4.3 `apply_model_selection` for `(openai_compatible, <model>, <base>, <key>)` sets `--model openai_compatible/<model>` + the provider entry AND sets no `ANTHROPIC_*` env.
- [ ] 4.4 A read-only role under opencode denies Write/Edit/Bash via the generated permission config.
- [ ] 4.5 An `opencode` role runs through `agentic_run` in capture mode (no streaming-JSON parse).

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate.
- [ ] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 5.3 `openspec validate a60-opencode-cli-strategy --strict` passes.
