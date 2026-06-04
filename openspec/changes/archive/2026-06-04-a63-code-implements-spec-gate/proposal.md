## Why

The code-reviewer deliberately reviews code quality only and explicitly defers spec-compliance: its prompt states "Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step." Nothing fills that slot today — once the executor implements a change, no check asks the targeted question "does this code actually satisfy the requirements AND scenarios in the change's spec delta?" Gaps (a requirement silently unimplemented, a scenario not honored, a partial implementation) reach the PR unflagged. The `[out]` gate of the verifier framework (a61) is that deferred verifier step: a post-executor, advisory check of the implementation against the change's spec.

It is advisory by design. It annotates — it does not auto-revise and does not block. The operator reads the verification and decides; a false "gap" never costs an unwanted revision cycle, and the gate never gates PR creation. This matches the chosen disposition for the `[out]` gate and keeps the executor → review → merge flow unchanged for operators who do not act on it.

## What Changes

**`[out]` gate — code-implements-spec verification (orchestrator-cli).** A new opt-in post-executor gate, `executor.code_implements_spec_check` (`disabled` default), runs an agentic session (a56) after the executor implements a change, in a read-only sandbox (`Read`/`Glob`/`Grep`, NO `Bash`/`Write`/`Edit`) with `ORCH_MCP_ROLE = code_implements_spec` and the `submit_verdict` MCP tool. The prompt carries the change's spec-delta files, the unified diff, and the changed-file list; the agent reads source on demand and judges, per requirement and scenario, whether the implementation satisfies it. It returns its verdict via `submit_verdict`. The daemon renders the verdict as an advisory `## Spec Verification` section in the PR body (parallel to the reviewer's `## Code Review` block), and posts a chatops note ONLY when gaps are found. It NEVER opens a revision and NEVER blocks PR creation. It is labeled `[out]` per a61; per the framework's advisory posture, a gate failure logs a WARN and omits the section (or notes "verification unavailable") — it never blocks.

**`submit_verdict` MCP tool (executor).** The last of a56's reserved per-role tools, advertised only when `ORCH_MCP_ROLE = code_implements_spec`. Payload `{ verdict: "implemented" | "gaps_found", summary, gaps: [{ requirement, scenario, status: "missing" | "partial", evidence }] }`; the schema requires a non-empty `gaps` array when `verdict: gaps_found`. Relays through `record_submission`; consumed after the session. Because the gate is advisory, a missing submission yields no annotation (a WARN), never a block.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED `Code-implements-spec verification (the [out] gate, advisory)`.
  - `executor` — ADDED `submit_verdict MCP tool returns the code-implements-spec verdict`.
- **Affected code:**
  - `autocoder/src/<verifier module>.rs` — the `[out]` gate run post-implementation via `agentic_run`; `consume_submission` → render the `## Spec Verification` PR section + optional chatops note; `[out]` label (a61).
  - `autocoder/src/config.rs` — `executor.code_implements_spec_check` + `_llm` + `_prompt_path`; startup fail-fast when enabled without `_llm`.
  - The PR-assembly path — splice the advisory `## Spec Verification` section into the PR body (alongside the reviewer's block), with no effect on whether the PR is created.
  - `autocoder/src/mcp_askuser_server.rs` — register `submit_verdict` + schema, gated on `ORCH_MCP_ROLE = code_implements_spec`.
  - Embedded prompt `prompts/code-implements-spec-check.md`.
- **Operator-visible behavior:** none unless enabled. When enabled, PRs gain an advisory `## Spec Verification` section; a gaps-found result also posts a chatops heads-up. No revision and no block ever result from this gate.
- **Acceptance:** `cargo test` passes; `openspec validate a63-code-implements-spec-gate --strict` passes. Tests: default-disabled spawns no session; an enabled run reads delta + diff and submits a verdict; an `implemented` verdict renders a clean section and posts no chatops; a `gaps_found` verdict renders the gaps section AND posts a chatops note but opens NO revision and does NOT block the PR; a session failure WARNs and omits the section; `submit_verdict` advertised only for the `code_implements_spec` role; diagnostics carry the `[out]` label.
- **Dependencies:** stacks on **a56** (`agentic_run`, the `submit_*` framework, the reserved `submit_verdict`) and **a61** (the verifier framework + `[out]` label, advisory posture). Fills the verifier slot the code-reviewer requirement defers to. Independent of a62 (the two gates are orthogonal).
