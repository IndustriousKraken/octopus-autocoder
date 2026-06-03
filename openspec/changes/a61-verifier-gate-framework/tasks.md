# Implementation tasks

## 1. Gate vocabulary + labeling (orchestrator-cli)

- [ ] 1.1 Define the verifier-gate identifiers as a small enum/constants (`in`, `canon`, `out`) with their lifecycle position (pre-executor for `in`/`canon`, post-executor for `out`) in a shared module.
- [ ] 1.2 Add a labeling helper that prefixes a gate's log/diagnostic lines with its stable identifier (e.g. `[verifier:in]`), so a finding is attributable to the gate that produced it.

## 2. Reframe the `[in]` gate (orchestrator-cli)

- [ ] 2.1 The a59 change-internal contradiction-check call site adopts the `[in]` gate identifier in its WARN / `SpecNeedsRevision` alert / log lines via the labeling helper. No change to what the check decides, its config key, or its alert category.
- [ ] 2.2 Map the `[in]` gate identifier to the contradiction-check entry point so the gate is resolvable by name (the registry a62/a63 will extend with `canon`/`out`).

## 3. Unrealized gates are inert

- [ ] 3.1 The `canon` and `out` identifiers exist in the vocabulary but resolve to no installed gate; the framework invokes nothing for an unrealized gate. (a62/a63 register their gates.)

## 4. Tests

- [ ] 4.1 Running the `[in]` gate executes the a59 contradiction check unchanged AND its emitted diagnostics carry the `[in]` gate identifier.
- [ ] 4.2 An unrealized gate (`canon` / `out`) is treated as absent: resolving it yields "no installed gate" and nothing is invoked.

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate.
- [ ] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 5.3 `openspec validate a61-verifier-gate-framework --strict` passes.
