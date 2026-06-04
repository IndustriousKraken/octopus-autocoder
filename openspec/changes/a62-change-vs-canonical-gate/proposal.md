## Why

The `[in]` gate (a59) catches a change that contradicts *itself*. It cannot catch a change that is internally coherent but contradicts an *already-canonical* requirement — e.g. a delta that re-specifies a behavior the project has already locked elsewhere, or asserts a default that a canonical requirement forbids. Those land on the executor, get implemented, and surface late (a failed review, a confused operator, or a silent canonical drift). The `[canon]` gate of the verifier framework (a61) closes that gap with a pre-executor check of the change's deltas against existing canonical specs.

It is the natural sibling of the `[in]` gate: same lifecycle position (pre-executor), same opt-in + fail-open posture, same agentic transport (a56 `agentic_run` + a `submit_*` tool). The only differences are what it reads (the change's deltas PLUS canonical specs) and what it reports (contradictions between the change and canon, not within the change).

## What Changes

**`[canon]` gate — change-vs-canonical contradiction pre-flight (orchestrator-cli).** A new opt-in pre-executor gate, `executor.change_canonical_contradiction_check` (`disabled` default), runs an agentic session (a56) in a read-only sandbox (`Read`/`Glob`/`Grep`, NO `Bash`/`Write`/`Edit`) with `ORCH_MCP_ROLE = canon_contradiction_check` and the `submit_canon_contradictions` MCP tool. The agent reads the change's spec-delta files AND the canonical specs, then submits contradictions between the change and canon. Canon access follows the `documentation_audit` pattern: the gate reads `openspec/specs/*/spec.md` directly via the sandbox, AND additionally uses the `query_canonical_specs` MCP tool when a21's RAG is enabled (focused retrieval for large canon); it functions correctly with or without RAG. On non-empty findings the gate writes `.needs-spec-revision.json` (revision_suggestion from the canon-contradiction narrative), posts the `AlertCategory::SpecNeedsRevision` alert, AND halts the queue walk — identical disposition to the `[in]` gate. It is labeled `[canon]` per a61, fail-open per the framework (session error / no submission → WARN + proceed), and fails fast at startup if enabled without its LLM config.

**`submit_canon_contradictions` MCP tool (executor).** Built on a56's per-role framework, advertised only when `ORCH_MCP_ROLE = canon_contradiction_check`. Payload `{ contradictions: [{ change_requirement, canonical_capability, canonical_requirement, summary }] }` — distinct from a59's within-change `submit_contradictions` because each finding names the canonical requirement it conflicts with. Relays through `record_submission`; consumed after the session; a missing submission is consumed as empty (the gate's fail-open policy lives in the orchestrator-cli caller).

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED `Change-vs-canonical contradiction pre-flight check (the [canon] gate)`.
  - `executor` — ADDED `submit_canon_contradictions MCP tool returns change-vs-canonical contradictions`.
- **Affected code:**
  - `autocoder/src/<contradiction module>.rs` — the `[canon]` gate alongside the a59 `[in]` gate, sharing the agentic pre-flight machinery; `consume_submission` → marker/alert/halt on findings; fail-open otherwise; `[canon]` label (a61).
  - `autocoder/src/config.rs` — `executor.change_canonical_contradiction_check` + `_llm` + `_prompt_path`; startup fail-fast when enabled without `_llm`.
  - `autocoder/src/mcp_askuser_server.rs` — register `submit_canon_contradictions` + schema, gated on `ORCH_MCP_ROLE = canon_contradiction_check`.
  - Embedded prompt `prompts/change-vs-canonical-check.md`.
- **Operator-visible behavior:** none unless enabled. When enabled, a change that contradicts canon is halted pre-executor with a `SpecNeedsRevision` alert, exactly like the `[in]` gate halts a self-contradictory change.
- **Acceptance:** `cargo test` passes; `openspec validate a62-change-vs-canonical-gate --strict` passes. Tests: default-disabled spawns no session; an enabled run reads deltas + canon and submits; an empty submission proceeds; a non-empty submission writes the marker + alert + halts; a session error and a no-submission session fail open; the gate runs with AND without a21 RAG; `submit_canon_contradictions` advertised only for the `canon_contradiction_check` role; its diagnostics carry the `[canon]` label.
- **Dependencies:** stacks on **a56** (`agentic_run`, the `submit_*` framework), **a61** (the verifier framework + `[canon]` label), and structurally parallels **a59** (the `[in]` gate it mirrors). Uses **a21** (`query_canonical_specs`) opportunistically. Independent of a57/a58/a60/a63.
