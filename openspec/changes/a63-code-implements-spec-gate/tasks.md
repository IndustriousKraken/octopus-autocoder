# Implementation tasks

## 1. `[out]` gate config (orchestrator-cli)

- [ ] 1.1 `config.rs` — add `executor.code_implements_spec_check` (`disabled` default, `enabled` opt-in), `executor.code_implements_spec_check_llm` (provider/model/api_key/api_base_url), AND `executor.code_implements_spec_check_prompt_path` (override for the embedded `prompts/code-implements-spec-check.md`).
- [ ] 1.2 Startup fail-fast: enabling the check without `_llm` fails daemon startup with a named error.

## 2. `[out]` gate (orchestrator-cli)

- [ ] 2.1 Run the gate AFTER the executor implements a change (the agent branch carries the implementation), before PR-body assembly, via a56's `agentic_run`: read-only sandbox (`["Read","Glob","Grep"]`; `Bash`/`Write`/`Edit` denied), `ORCH_MCP_ROLE = code_implements_spec`, the `submit_verdict` MCP tool, capture mode.
- [ ] 2.2 The prompt carries the change's spec-delta files, the unified diff, AND the changed-file list; the embedded `prompts/code-implements-spec-check.md` directs the agent to judge, per requirement and scenario, whether the implementation satisfies it, reading source on demand.
- [ ] 2.3 After the session, `consume_submission` → verdict. Render an advisory `## Spec Verification` section into the PR body (parallel to the reviewer's `## Code Review` block). Post a chatops note ONLY when `verdict: gaps_found`.
- [ ] 2.4 Advisory posture (a61): the gate NEVER opens a revision AND NEVER blocks PR creation. A gate failure (session error / no submission) logs a WARN (labeled `[out]`) AND omits the section (or writes "verification unavailable"); it never blocks.

## 3. `submit_verdict` MCP tool (executor)

- [ ] 3.1 `mcp_askuser_server.rs` — register `submit_verdict` under a56's framework, gated on `ORCH_MCP_ROLE = code_implements_spec`; not advertised for any other role. Relay via `relay_submission` → `record_submission`.
- [ ] 3.2 Schema `{ verdict: "implemented" | "gaps_found", summary: string, gaps: [{ requirement: string, scenario: string|null, status: "missing"|"partial", evidence: string }] }`; require a non-empty `gaps` array when `verdict: gaps_found`. Schema-invalid → correctable tool error (a56).
- [ ] 3.3 Missing submission consumed as empty (advisory: no section, WARN) — the no-block decision lives in the orchestrator-cli caller.

## 4. Tests

- [ ] 4.1 Default-disabled spawns no `[out]` session; PR assembly is unchanged.
- [ ] 4.2 An enabled run invokes `agentic_run` with the read-only sandbox + `submit_verdict` after implementation; the verdict is consumed.
- [ ] 4.3 An `implemented` verdict renders a clean `## Spec Verification` section AND posts no chatops note.
- [ ] 4.4 A `gaps_found` verdict renders the gaps in the section AND posts a chatops note BUT opens NO revision AND does NOT block PR creation.
- [ ] 4.5 A session failure / no submission WARNs (labeled `[out]`) AND omits the section; the PR is still created.
- [ ] 4.6 `submit_verdict` advertised only when `ORCH_MCP_ROLE = code_implements_spec`; a `gaps_found` payload with an empty `gaps` array is a correctable tool error.

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate.
- [ ] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 5.3 `openspec validate a63-code-implements-spec-gate --strict` passes.
