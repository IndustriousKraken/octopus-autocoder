## 1. Config schema

- [ ] 1.1 In `autocoder/src/config.rs`, extend `ExecutorConfig`:
  ```rust
  #[serde(default)]
  pub change_internal_contradiction_check: ContradictionCheckMode,
  pub change_internal_contradiction_check_prompt_path: Option<PathBuf>,
  pub change_internal_contradiction_check_llm: Option<ContradictionCheckLlmConfig>,
  ```
- [ ] 1.2 Define the enum + sub-config:
  ```rust
  #[derive(Copy, Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
  #[serde(rename_all = "snake_case")]
  pub enum ContradictionCheckMode { #[default] Disabled, Enabled }
  pub struct ContradictionCheckLlmConfig {
      pub provider: ProviderKind,  // anthropic | openai_compatible
      pub model: String,
      pub api_key_env: Option<String>,
      pub api_key: Option<SecretSource>,
      pub api_base_url: Option<String>,
  }
  ```
- [ ] 1.3 When the check is `Enabled` AND no `change_internal_contradiction_check_llm` is set, startup config-validation MUST fail with `executor.change_internal_contradiction_check is enabled but executor.change_internal_contradiction_check_llm is not configured`. Fail-fast at startup, not at the per-change check time.
- [ ] 1.4 Update `config.example.yaml` AND the project-documentation config-coverage test list.
- [ ] 1.5 Tests: default parses (disabled); enabled-without-llm-config fails startup validation with the expected message; enabled-with-llm-config passes validation.

## 2. Embedded prompt template

- [ ] 2.1 Create `prompts/change-contradiction-check.md` per the proposal's prompt text.
- [ ] 2.2 Embed via `include_str!("../../prompts/change-contradiction-check.md")`.
- [ ] 2.3 The override config `change_internal_contradiction_check_prompt_path` resolves at use time. Empty override file → error at use, naming the path (don't feed an empty prompt to the LLM).
- [ ] 2.4 Test: embedded template loads; override path replaces it; empty override file rejected.

## 3. Pre-flight check function

- [ ] 3.1 New module `autocoder/src/preflight/change_contradiction.rs`. Public surface:
  ```rust
  pub struct ContradictionFinding {
      pub requirement_a: String,
      pub requirement_b: String,
      pub summary: String,
  }
  pub async fn check_change_internal_contradictions(
      workspace_root: &Path,
      change_slug: &str,
      llm: &dyn LlmClient,
      prompt_template: &str,
  ) -> Result<Vec<ContradictionFinding>>;
  ```
- [ ] 3.2 Body:
  - Read every `<workspace>/openspec/changes/<change>/specs/<cap>/spec.md` file.
  - Concatenate into a single input, prefixed by `## File: openspec/changes/<change>/specs/<cap>/spec.md` headers (same convention the reviewer uses).
  - Build the prompt: template + input.
  - Invoke `llm.complete(prompt)`.
  - Parse the response as JSON conforming to `{ contradictions: [{ requirement_a, requirement_b, summary }] }`.
  - On parse failure: log WARN naming the response excerpt, return Ok(empty Vec) (fail-open).
  - On LLM error (network, rate-limit): log WARN, return Ok(empty Vec) (fail-open).
  - On parse success: return the contradictions list.
- [ ] 3.3 Tests:
  - LLM mock returns `{"contradictions": []}` → returns empty Vec.
  - LLM mock returns `{"contradictions": [{ ... }]}` → returns the parsed list.
  - LLM mock returns malformed JSON → WARN logged, empty Vec returned (fail-open).
  - LLM mock returns Err (network error) → WARN logged, empty Vec returned.
  - Change with no spec deltas (rare; check is invoked anyway) → LLM receives empty input; behavior depends on the LLM's response.

## 4. Pre-executor pipeline integration

- [ ] 4.1 In the polling loop's per-change pre-executor pipeline, AFTER `a17`'s archivability check AND BEFORE `executor.run(...)`:
  - If `config.executor.change_internal_contradiction_check != ContradictionCheckMode::Enabled`: skip (no-op).
  - If enabled: construct the LLM client from `change_internal_contradiction_check_llm`, load the prompt template (embedded OR override), call `check_change_internal_contradictions(...)`.
  - If returned Vec is empty: proceed to executor.
  - If returned Vec is non-empty: write `.needs-spec-revision.json` with `revision_suggestion` populated from the contradictions narrative (see §5), post the existing `AlertCategory::SpecNeedsRevision` chatops alert (subject to 24h throttle), halt the queue walk.
- [ ] 4.2 The marker's `unarchivable_deltas` field (from `a17`) is left EMPTY for this case — the issue isn't an unarchivable delta. The `unimplementable_tasks` field is also empty. The contradictions narrative goes entirely into `revision_suggestion`.
- [ ] 4.3 Tests:
  - Disabled mode → no LLM call, executor invoked normally.
  - Enabled mode + LLM returns empty → no LLM call (yes there IS an LLM call; it's the empty-result case), executor invoked.
  - Enabled mode + LLM returns contradictions → marker written, executor NOT invoked, alert fires.
  - Enabled mode + LLM fails → fail-open: WARN, executor IS invoked (the daemon doesn't gate work on a failed check).

## 5. `revision_suggestion` population

- [ ] 5.1 When the contradiction check produces findings, the marker's `revision_suggestion` text is auto-generated:
  ```
  Pre-flight contradiction check found N issue(s) where this change's
  requirements appear to contradict each other:

  1. Requirement A: <requirement_a>
     Requirement B: <requirement_b>
     <summary>

  2. ... (one block per finding)

  Edit the conflicting requirements so they can hold simultaneously,
  OR REMOVE one of them. Push the spec change AND clear this marker
  via @<bot> clear-revision <repo> <change>.
  ```
- [ ] 5.2 The narrative is informational — the contradiction check is opt-in AND new; operators should be able to disagree with the LLM AND clear the marker without editing if they assess it as a false positive.
- [ ] 5.3 Test: marker file written with N findings has the expected `revision_suggestion` text.

## 6. Docs

- [ ] 6.1 In `docs/CONFIG.md`'s `executor:` table, add rows for `change_internal_contradiction_check`, `change_internal_contradiction_check_prompt_path`, AND `change_internal_contradiction_check_llm`.
- [ ] 6.2 In `docs/OPERATIONS.md`'s "Spec marked as needing revision" section, add a paragraph describing the contradiction-check failure mode AND the opt-in posture. Note that the check is LLM-based AND has a small per-change cost; operators trading cost for safety can enable it.
- [ ] 6.3 In `docs/OPERATIONS.md`, add a "Pre-flight checks" section enumerating the layered pre-executor checks: `openspec validate --strict` (well-formedness), `a17`'s archivability check (mechanical), AND `a19`'s contradiction check (LLM, opt-in). Each check's purpose AND cost.

## 7. Spec deltas

- [ ] 7.1 `openspec/changes/a19-change-internal-non-contradiction-preflight/specs/orchestrator-cli/spec.md` ADDs `Change-internal contradiction pre-flight check (opt-in)`.
- [ ] 7.2 `openspec/changes/a19-change-internal-non-contradiction-preflight/specs/project-documentation/spec.md` ADDs `CONFIG.md and OPERATIONS.md document the contradiction-check fields and cost model`.

## 8. Verification

- [ ] 8.1 `cargo test` passes (new + existing).
- [ ] 8.2 `openspec validate a19-change-internal-non-contradiction-preflight --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
