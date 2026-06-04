## Why

Operator-facing output from the daemon's LLM-driven surfaces — code-review sections, audit findings, contradiction-check findings — does not identify which model produced it. With multiple providers/models configurable across these surfaces, and operators actively experimenting across reviewer tiers, the missing attribution makes it hard to associate a comment's quality with the model behind it. Today operators (and Claude, when helping them debug) bridge the gap from memory.

The fix is small and self-contained: a redaction-safe accessor that returns a stable `<provider>/<model>` string for an LLM-driven surface, and a one-line attribution appended to each operator-facing output composed from such a surface.

## What Changes

**A redaction-safe model-attribution accessor (orchestrator-cli/config).** Given an LLM-driven surface (reviewer, contradiction-check, or a named audit), the accessor returns `<provider>/<model>` — `<provider>` being the `LlmProvider` canonical name (`anthropic` / `openai_compatible` / `ollama`) and `<model>` the configured model. It reads only an explicit positive allowlist of non-secret fields (provider AND model) and can never return an `api_key`, resolved env-var value, or `api_base_url`. The displayed provider is the configured provider KIND, not the upstream brand — a gateway-served model renders `openai_compatible/moonshotai/kimi-latest`, not the gateway name.

**Operator-facing LLM-driven output carries an attribution line.** Each daemon-composed operator-facing output from a surface with a configured `(provider, model)` carries `*<Role>: <provider>/<model>*`: the initial `## Code Review` section and the `## Code Review (rerun N of M)` comment (`Reviewer`); each audit's PR section and chatops finding (`Auditor (<type>)`); the contradiction-check findings (`Contradiction-check`).

**Executor implementation-notes are out of scope.** The executor wraps the Claude CLI and has no daemon-known `(provider, model)` — it uses the CLI's configured model. Attributing it cleanly requires the model-registry work (the planned `models:` block that gives every surface a resolvable model). a49 attributes the surfaces whose model the daemon already knows; the executor surface is explicitly deferred.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED `Redaction-safe model-attribution accessor` AND `Operator-facing LLM-driven output carries a model-attribution line`.
- **Affected code:**
  - `autocoder/src/config.rs` (OR a small attribution module) — the accessor: a function/method that, given a surface's resolved config block, returns `format!("{}/{}", provider.as_str(), model)`. Positive allowlist (provider, model); no secret-bearing field is reachable from its output.
  - `autocoder/src/github.rs` (~700-731) — the initial-review PR-body `## Code Review` composer appends the `*Reviewer: …*` line.
  - `autocoder/src/revisions.rs` (~1281) — the `## Code Review (rerun N of M)` composer appends the `*Reviewer: …*` line.
  - `autocoder/src/polling_loop.rs` audit composers (PR-body audit section ~5729 AND the audit chatops finding notifications) — append `*Auditor (<type>): …*`.
  - the change-internal contradiction-check finding composer — append `*Contradiction-check: …*`.
- **Operator-visible behavior:** code-review, audit, and contradiction-check output gains a one-line model attribution. Executor implementation-notes are unchanged (deferred).
- **Acceptance:** `cargo test` passes; `openspec validate a49-model-attribution-on-llm-output --strict` passes. Tests: the accessor returns `<provider>/<model>` AND its output contains neither the api_key nor the base_url for a config carrying both; the reviewer composer output contains `*Reviewer: <provider>/<model>*`; an audit composer output contains `*Auditor (<type>): …*`; the executor implementation-notes composer adds no attribution line.
- **Dependencies:** none hard. Complements the planned model-registry work (which will extend attribution to the executor); independent of a44–a48/a51–a53.
