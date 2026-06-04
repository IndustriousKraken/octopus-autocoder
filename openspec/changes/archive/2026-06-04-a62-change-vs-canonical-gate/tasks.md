# Implementation tasks

## 1. `[canon]` gate config (orchestrator-cli)

- [x] 1.1 `config.rs` — add `executor.change_canonical_contradiction_check` (`disabled` default, `enabled` opt-in), `executor.change_canonical_contradiction_check_llm` (provider/model/api_key/api_base_url, parallel to the `[in]` gate's block), AND `executor.change_canonical_contradiction_check_prompt_path` (override for the embedded `prompts/change-vs-canonical-check.md`).
- [x] 1.2 Startup fail-fast: enabling the check without `_llm` configured fails daemon startup with a named error, exactly as the `[in]` gate does.

## 2. `[canon]` gate (orchestrator-cli)

- [x] 2.1 Run the gate pre-executor (alongside the a59 `[in]` gate) via a56's `agentic_run`: read-only sandbox (`["Read","Glob","Grep"]`; `Bash`/`Write`/`Edit` denied), `ORCH_MCP_ROLE = canon_contradiction_check`, the `submit_canon_contradictions` MCP tool, capture mode. Add `query_canonical_specs` as a common tool when a21 RAG is enabled.
- [x] 2.2 The embedded `prompts/change-vs-canonical-check.md` directs the agent to read the change's spec-delta files AND the canonical specs (`openspec/specs/*/spec.md` directly, OR via `query_canonical_specs` when present) and submit contradictions between the change and canon.
- [x] 2.3 After the session, `consume_submission` → contradictions. Non-empty → write `.needs-spec-revision.json` (revision_suggestion from the canon-contradiction narrative; empty structural arrays), fire `AlertCategory::SpecNeedsRevision`, halt the queue walk. Empty → proceed.
- [x] 2.4 Fail-open per the a61 framework: session error / never-corrected schema rejection / no submission → WARN + treat as "no contradictions" + proceed. Label all gate diagnostics `[canon]` (a61 helper).

## 3. `submit_canon_contradictions` MCP tool (executor)

- [x] 3.1 `mcp_askuser_server.rs` — register `submit_canon_contradictions` under a56's framework, gated on `ORCH_MCP_ROLE = canon_contradiction_check`; not advertised for any other role. Relay via `relay_submission` → `record_submission`.
- [x] 3.2 Schema `{ contradictions: [{ change_requirement: string, canonical_capability: string, canonical_requirement: string, summary: string }] }`. Schema-invalid → correctable tool error (a56).
- [x] 3.3 Missing submission consumed as empty (the fail-open decision lives in the orchestrator-cli caller).

## 4. Tests

- [x] 4.1 Default-disabled spawns no `[canon]` session and proceeds to the executor.
- [x] 4.2 An enabled run invokes `agentic_run` with the read-only sandbox + `submit_canon_contradictions`; the agent's submission is consumed.
- [x] 4.3 An empty submission proceeds with no marker/alert; a non-empty submission writes `.needs-spec-revision.json`, fires `SpecNeedsRevision`, and halts the queue walk.
- [x] 4.4 A session error AND a no-submission session both WARN and fail open (proceed).
- [x] 4.5 The gate runs correctly with a21 RAG enabled (uses `query_canonical_specs`) AND with it disabled (direct `Read` of `openspec/specs`).
- [x] 4.6 `submit_canon_contradictions` advertised only when `ORCH_MCP_ROLE = canon_contradiction_check`; gate diagnostics carry the `[canon]` label.

## 5. Acceptance gate

- [x] 5.1 `cargo test` passes for the autocoder crate.
- [x] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 5.3 `openspec validate a62-change-vs-canonical-gate --strict` passes.
