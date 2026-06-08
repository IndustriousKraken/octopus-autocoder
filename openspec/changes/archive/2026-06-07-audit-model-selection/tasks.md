## 1. Configuration Schema Updates

- [x] 1.1 Add `model: Option<String>` to `AuditSettings` in `config.rs`.
- [x] 1.2 Update config validation to resolve the `model` field against the `models:` registry for each audit in `audits.settings` using `resolve_model_reference`, similar to the reviewer validation.
- [x] 1.3 Ensure validation fails fast with a clear error if the nickname is not found in the registry.

## 2. Audit Runner Updates

- [x] 2.1 Update `run_audit_cli` and `run_audit_cli_with_submit` in `audits/mod.rs` to accept an `Option<&ResolvedModel>` parameter.
- [x] 2.2 Replace the hardcoded `ClaudeStrategy::new` instantiation with a call to `crate::agentic_run::strategy_for_provider`, passing the resolved model's provider, the command, and any necessary args.
- [x] 2.3 Pass the `Option<&ResolvedModel>` to the `agentic_run` call so the CLI receives the correct `--model` flag.
- [x] 2.4 Update the callers of these functions (the audit execution logic in the polling loop) to pass the resolved model from the audit's settings.

## 3. Testing

- [x] 3.1 Add a unit test verifying that an audit configured with a valid registry nickname resolves correctly and selects the appropriate strategy (e.g., `OpencodeStrategy` for `openai_compatible`).
- [x] 3.2 Add a unit test verifying that config validation fails when an audit specifies a non-existent model nickname.
- [x] 3.3 Add a unit test verifying that an audit without a `model` field defaults to `ClaudeStrategy` with `None` for the model, preserving backward compatibility.

## 4. Acceptance Gate

- [x] 4.1 `cargo test` passes for the autocoder crate.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 4.3 `openspec validate audit-model-selection --strict` passes.
