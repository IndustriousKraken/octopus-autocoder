# Implementation tasks

## 1. Redaction-safe attribution accessor

- [x] 1.1 In `autocoder/src/config.rs` (OR a small `attribution` module), add an accessor that takes a resolved LLM-surface config block (reviewer, contradiction-check, or a named audit's config) and returns `format!("{}/{}", provider.as_str(), model)` using `LlmProvider::as_str()` for the provider name.
- [x] 1.2 The accessor SHALL read only a positive allowlist of fields (provider AND model). It MUST NOT read, embed, or derive its output from `api_key`, `api_key_env`-resolved values, `api_base_url`, or any other secret- or endpoint-bearing field. If the surface's config is structured so the accessor receives the whole block, copy out only the two allowlisted fields rather than formatting the block.
- [x] 1.3 Provide a small role-prefixed helper for composers: `attribution_line(role, surface_cfg) -> String` returning `*<Role>: <provider>/<model>*` (single line, italicized).

## 2. Wire attribution into the composers

- [x] 2.1 `autocoder/src/github.rs` (~700-731) — append `*Reviewer: <provider>/<model>*` to the initial-review `## Code Review` PR-body section (per-change sections in per_change mode each carry the reviewer attribution).
- [x] 2.2 `autocoder/src/revisions.rs` (~1281) — append `*Reviewer: <provider>/<model>*` to the `## Code Review (rerun N of M)` comment body.
- [x] 2.3 `autocoder/src/polling_loop.rs` — append `*Auditor (<audit-type>): <provider>/<model>*` to the audit-produced PR-body section (~5729) AND to the audit finding chatops notification(s).
- [x] 2.4 The change-internal contradiction-check finding composer — append `*Contradiction-check: <provider>/<model>*`.
- [x] 2.5 Do NOT add an attribution line to the executor's `## Agent implementation notes` section (~polling_loop.rs:5391): the executor has no daemon-known model in this change.

## 3. Tests

- [x] 3.1 Accessor redaction: given a reviewer config with `provider: openai_compatible`, `model: moonshotai/kimi-latest`, a non-empty `api_base_url`, AND an inline `api_key`, the accessor returns `openai_compatible/moonshotai/kimi-latest` AND the returned string contains NEITHER the api_key value NOR the base_url. (This is behavior — assert on the accessor output for a known-secret input.)
- [x] 3.2 Reviewer composer: the composed `## Code Review` (initial) AND `## Code Review (rerun N of M)` bodies each contain `*Reviewer: <provider>/<model>*` for a configured reviewer.
- [x] 3.3 Audit composer: an audit-produced PR section AND chatops finding contain `*Auditor (<type>): <provider>/<model>*`.
- [x] 3.4 Executor notes: the `## Agent implementation notes` composer output contains NO attribution line.

## 4. Documentation

- [x] 4.1 `docs/CODE-REVIEW.md` and/or `docs/OPERATIONS.md` — document the attribution line on reviewer/audit/contradiction-check output, the `<provider>/<model>` format (provider is the configured KIND, not the upstream brand), AND that the executor surface is not yet attributed. No kitsch.

## 5. Acceptance gate

- [x] 5.1 `cargo test` passes for the autocoder crate.
- [x] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 5.3 `openspec validate a49-model-attribution-on-llm-output --strict` passes.
