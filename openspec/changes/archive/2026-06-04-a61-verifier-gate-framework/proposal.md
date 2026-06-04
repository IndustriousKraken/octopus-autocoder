## Why

The daemon is growing a set of LLM-driven consistency checks around a change's lifecycle: the change-internal contradiction pre-flight (a59, the `[in]` gate), a change-vs-canonical pre-flight (a62, `[canon]`), and a post-implementation code-implements-spec check (a63, `[out]`). Built ad hoc, these read as three unrelated features with three vocabularies. This change establishes the **verifier-gate framework**: the shared naming, lifecycle positions, and labeling that make the three checks one coherent verification story, and reframes the existing a59 contradiction check as the framework's first concrete gate (`[in]`).

It is deliberately a thin, low-behavior-change reframe. It does NOT rename a59's config, does NOT add a new gate, and does NOT change what the `[in]` gate decides. It introduces the gate vocabulary (`[in]` / `[canon]` / `[out]`), the lifecycle positions (two pre-executor, one post-executor), the shared posture rules, and a stable per-gate identifier carried in diagnostics — so a62 and a63 plug into an established frame rather than inventing their own.

## What Changes

**Verifier-gate framework (orchestrator-cli).** A new requirement defines exactly three named verifier gates positioned around the executor run: `[in]` (change-internal consistency, pre-executor), `[canon]` (change-vs-canonical consistency, pre-executor), and `[out]` (code-implements-spec, post-executor). Each gate is individually opt-in and owns its disposition — the pre-executor gates are fail-open (a gate's own failure never blocks the iteration); the `[out]` gate is advisory (it annotates, it never auto-acts). Each gate's diagnostics (logs, and any operator surface it writes) carry its stable gate identifier so a finding is attributable to the gate that produced it. The `[in]` gate IS the existing change-internal contradiction pre-flight check (a59), unchanged in what it decides; the `[canon]` and `[out]` gates are realized by subsequent changes (a62, a63) and are inert until then.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED `Verifier-gate framework`.
- **Affected code:**
  - A small shared module defining the gate identifiers (`in` / `canon` / `out`) and the labeling helper used in gate diagnostics; the a59 contradiction-check call site adopts the `[in]` identifier in its WARN/alert/log lines.
- **Operator-visible behavior:** the `[in]` gate's log/diagnostic lines now carry an `[in]` gate label; no decision, config, or alert category changes. The `[canon]`/`[out]` gates do not exist yet and nothing is invoked for them.
- **Acceptance:** `cargo test` passes; `openspec validate a61-verifier-gate-framework --strict` passes. Tests: running the `[in]` gate executes the a59 contradiction check unchanged and its diagnostics carry the `[in]` identifier; an unrealized gate (`[canon]`/`[out]`) is treated as absent and nothing is invoked for it.
- **Dependencies:** stacks on **a59** (the change-internal contradiction check it reframes as `[in]`). It defines the frame that **a62** (`[canon]`) and **a63** (`[out]`) fill. Independent of a57/a58/a60.
