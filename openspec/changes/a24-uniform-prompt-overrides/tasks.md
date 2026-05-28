## 1. PromptLoader helper

- [ ] 1.1 New module `autocoder/src/prompts/loader.rs` with:
  - `PromptId` enum: one variant per embedded prompt. Initial set:
    - `Implementer`, `ImplementerRevision`, `ChangelogStylist`, `CodeReview`,
    - `AuditTriage`, `ChatRequestTriage`,
    - `AuditArchitectureBrightline`, `AuditArchitectureConsultative`, `AuditDrift`,
    - `AuditMissingTests`, `AuditSecurityBug`,
    - `AuditDocumentation` (added by `a22`), `BrownfieldDraft` (added by `a23`),
    - (further variants added by subsequent stack changes as they ship.)
  - `PromptLoader::load(id: PromptId, workspace_config: &PerRepoConfig) -> Result<String>` returning the resolved prompt template content.
- [ ] 1.2 Each `PromptId` variant maps via an internal table to:
  - The embedded template (via `include_str!`).
  - The per-workspace override config-key path (e.g., `executor.implementer.prompt_path` OR `audits.settings.drift_audit.prompt_path`).
  - The legacy daemon-level override field (if one exists) OR `None`.
- [ ] 1.3 Loader precedence per call:
  1. If per-workspace path is set AND the file exists → load AND return.
  2. Else if per-workspace path is set BUT file does NOT exist → log WARN once-per-daemon-start naming the path AND fall through.
  3. Else if legacy daemon-level path is set AND the file exists → load AND return.
  4. Else if legacy daemon-level path is set BUT file does NOT exist → log WARN once-per-daemon-start AND fall through.
  5. Else → return embedded default.
- [ ] 1.4 One-shot WARN tracking: an internal `HashSet<(PromptId, PathBuf)>` records WARNs already emitted so missing paths don't spam logs across reloads.
- [ ] 1.5 Tests:
  - Each precedence branch resolves to the expected content.
  - Missing-file WARN fires once AND not again on subsequent loads of the same `(id, path)`.
  - A registry-completeness test enumerates `prompts/*.md` files (via `std::fs::read_dir` at test time) AND asserts every file has a corresponding `PromptId` variant.

## 2. Per-workspace config fields for the three new overrides

- [ ] 2.1 In `autocoder/src/config.rs`, extend `PerRepoConfig.executor` with:
  - `audit_triage: Option<PromptOverrideBlock>` (where `PromptOverrideBlock { prompt_path: Option<String> }`).
  - `chat_request_triage: Option<PromptOverrideBlock>`.
  - `implementer_revision: Option<PromptOverrideBlock>`.
- [ ] 2.2 Add a similar `executor.implementer: Option<PromptOverrideBlock>` AND `executor.changelog_stylist: Option<PromptOverrideBlock>` AND `reviewer.<nested>` for the existing prompts, so all consumers can prefer the nested form. The legacy flat fields (`executor.implementer_prompt_path` etc.) continue to deserialize alongside the nested fields.
- [ ] 2.3 The loader prefers the nested form when both forms are set in the same config (per-workspace nested > per-workspace flat-legacy > daemon-level flat-legacy > embedded).
- [ ] 2.4 Tests:
  - Each new field round-trips through serde.
  - A config with BOTH nested AND legacy fields set picks the nested form.
  - A config with only the legacy field set still works.

## 3. Refactor existing consumers to call PromptLoader

- [ ] 3.1 Audit consumers: replace direct `include_str!` calls in each audit's `run()` with `PromptLoader::load(PromptId::Audit<X>, &workspace_config)`.
- [ ] 3.2 Executor consumers:
  - `executor.run_implementer(...)` → `PromptLoader::load(PromptId::Implementer, ...)`.
  - Revision flow → `PromptLoader::load(PromptId::ImplementerRevision, ...)`.
  - Audit-triage flow → `PromptLoader::load(PromptId::AuditTriage, ...)`.
  - Chat-request-triage flow → `PromptLoader::load(PromptId::ChatRequestTriage, ...)`.
  - Changelog stylist flow → `PromptLoader::load(PromptId::ChangelogStylist, ...)`.
- [ ] 3.3 Code reviewer: `prompts/code-review-default.md` resolution → `PromptLoader::load(PromptId::CodeReview, ...)`.
- [ ] 3.4 Brownfield handler (added by `a23`): `PromptLoader::load(PromptId::BrownfieldDraft, ...)`.
- [ ] 3.5 Tests: each refactored consumer still passes its existing tests; loader integration is verified via at least one end-to-end test per consumer.

## 4. Docs

- [ ] 4.1 In `docs/CONFIG.md`, add a `## Prompt overrides` section near the existing `audits.settings.<slug>.prompt_path` discussion. Contents:
  - A short paragraph explaining the precedence (per-workspace nested → per-workspace flat-legacy → daemon-level flat-legacy → embedded).
  - The registry table listing every prompt: logical id, embedded path, primary (nested) override field, legacy flat field (where applicable).
  - A short note that future prompts SHALL use the nested form.
- [ ] 4.2 In `README.md`, add a single sentence in the "Configuration" section pointing operators at the `docs/CONFIG.md` prompt overrides table.
- [ ] 4.3 In `config.example.yaml`, include the three new override blocks commented out, with comments showing the per-workspace path semantics.

## 5. Spec deltas

- [ ] 5.1 `openspec/changes/a24-uniform-prompt-overrides/specs/executor/spec.md` ADDs the uniform-load-semantics requirement AND the three new override-field requirements.
- [ ] 5.2 `openspec/changes/a24-uniform-prompt-overrides/specs/orchestrator-cli/spec.md` ADDs the triage-prompts-honor-overrides requirement.
- [ ] 5.3 `openspec/changes/a24-uniform-prompt-overrides/specs/project-documentation/spec.md` ADDs the CONFIG.md prompt-overrides-table requirement.

## 6. Verification

- [ ] 6.1 `cargo test` passes (new + existing).
- [ ] 6.2 `openspec validate a24-uniform-prompt-overrides --strict` passes.
- [ ] 6.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 6.4 Manual verification: in a test workspace, set one nested + one flat-legacy + one missing override path. Daemon start logs the missing-path WARN exactly once. Each consumer sees the expected resolved template.
