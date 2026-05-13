## 1. SecretSource enum

- [x] 1.1 Add `pub enum SecretSource { EnvVar(String), Inline { value: String } }` to `src/config.rs` with `#[derive(Debug, Clone, Serialize, Deserialize)]` and `#[serde(untagged)]`. Place near the top of the file after the imports.
- [x] 1.2 Implement `pub fn resolve(&self, field_label: &str) -> Result<String>` per design.md: read env on `EnvVar`, return value on `Inline`. Error message MUST include the env var name on miss AND the field label.
- [x] 1.3 Implement `pub fn describe(&self, field_label: &str) -> String` for startup logs: returns `"env var <name>"` or `"inline (<field-label>)"`. NEVER returns the secret value.
- [x] 1.4 **Verify:** add unit tests in `config::tests`:
    - `secret_source_parses_bare_string_as_env_var`
    - `secret_source_parses_object_as_inline`
    - `secret_source_resolve_env_var_set`
    - `secret_source_resolve_env_var_unset_names_field`
    - `secret_source_resolve_inline`
    - `secret_source_describe_redacts_inline_value`

## 2. GithubConfig integration

- [x] 2.1 Add `pub token: Option<SecretSource>` to `GithubConfig` with `#[serde(default)]`. Keep existing `token_env: String` with its current default.
- [x] 2.2 Change `owner_tokens` field type from `Option<HashMap<String, String>>` to `Option<HashMap<String, SecretSource>>`. Because `SecretSource` parses bare strings, existing configs continue to parse.
- [x] 2.3 Update the two test-fixture `GithubConfig` constructions in `polling_loop.rs` to include `token: None` (`owner_tokens: None` unchanged).
- [x] 2.4 **Verify:** add `config::tests::loads_github_token_inline` (parses `token: { value: "abc" }`) and `loads_owner_tokens_mixed_env_and_inline` (parses one bare-string entry + one `{value: ...}` entry).

## 3. Token resolution

- [x] 3.1 Update `github_credentials::resolve_token` to consult, in order:
    - `cfg.owner_tokens` matching entry — call `.resolve(field_label)` on the matched `SecretSource`. Field label: `github.owner_tokens[<owner>]`.
    - `cfg.token` (if `Some`) — call `.resolve("github.token")`.
    - `cfg.token_env` — wrap as `SecretSource::EnvVar(cfg.token_env.clone())` and resolve with field label `github.token_env=<name>`.
- [x] 3.2 Add a returned-tuple variant or a sibling function so callers can also obtain the `SecretSource::describe(...)` string for startup logging: `pub fn resolve_token_with_source(cfg, owner) -> Result<(String, String)>` returning `(value, description)`. The original `resolve_token` keeps its signature and delegates internally.
- [x] 3.3 Detect "both global forms set" at the moment `token` is consulted: if `cfg.token` is `Some` AND `std::env::var(&cfg.token_env).is_ok()`, return an extra `Option<String>` warning message to the caller — or set a thread-local flag — for the startup wiring to log. Simpler approach: have `validate_github_token_routes` perform this check itself and log the warning.
- [x] 3.4 Update existing unit tests in `github_credentials::tests` to use the new resolver. Most pass unchanged because `SecretSource::EnvVar(s)` is backward-compatible. Add: `inline_token_resolves_without_env`, `inline_takes_precedence_over_token_env_when_both_set`.

## 4. Startup logging update

- [x] 4.1 In `cli/run::validate_github_token_routes`, replace the existing log line construction (which names the env-var only) with one that uses `SecretSource::describe(...)`. Format: `repository <url> will use GitHub token from <source>`.
- [x] 4.2 Emit one `tracing::warn!` line at startup, after the routing log lines, when ANY of these conditions hold:
    - `github.token` is set AND the env var named by `github.token_env` is also set.
    - `reviewer.api_key` is set AND the env var named by `reviewer.api_key_env` is also set.
    Each warn line names the field whose env-var form is being ignored.
- [x] 4.3 **Verify:** in `cli::run::tests`, add `startup_logs_inline_source_when_inline_set` (asserts the log message contains `inline (github.token)` rather than an env-var name).

## 5. ReviewerConfig integration

- [x] 5.1 Add `pub api_key: Option<SecretSource>` to `ReviewerConfig` with `#[serde(default)]`. Keep `api_key_env: String` unchanged.
- [x] 5.2 In `llm.rs` (or wherever the reviewer constructs its HTTP auth header), resolve the API key via: if `cfg.api_key` is `Some`, call `.resolve("reviewer.api_key")`; else `SecretSource::EnvVar(cfg.api_key_env.clone()).resolve("reviewer.api_key_env=<name>")`. Update both the Anthropic and openai_compatible branches.
- [x] 5.3 Update existing tests in `code_reviewer::tests` to construct `ReviewerConfig` with `api_key: None`.
- [x] 5.4 **Verify:** add `code_reviewer::tests::inline_api_key_takes_precedence_over_env_var` (sets both, asserts the inline value is sent in the auth header via a mockito fixture).

## 6. Documentation

- [x] 6.1 README's "AI Security & Guardrails" section: add a new subsection "Secrets in `config.yaml`" listing the env-var vs inline tradeoff, the `chmod 600 ~/config.yaml` recommendation, and a one-line `.gitignore` reminder.
- [x] 6.2 README's Configuration Reference: for both `github:` and `reviewer:` tables, add a note under each `_env` row pointing at the inline alternative (`token` / `api_key`) and link to the new Secrets subsection.
- [x] 6.3 README's "Multiple GitHub Tokens" section: add a brief mention that `owner_tokens` values can be inline (`{ value: "..." }`) as well as env-var names, with a small example.
- [x] 6.4 `config.example.yaml`: under each `_env` line, add a commented-out inline alternative. For `owner_tokens`, show a mixed example (one entry with env var name, one with `{ value: "..." }`).
- [x] 6.5 The `experimental-chatops-providers` change's `design.md` already references this pattern conceptually; no edit required — that change will adopt `SecretSource` for its provider blocks when it lands.

## 7. Verification

- [x] 7.1 `cargo test` passes; test count grows by at least: 6 SecretSource unit tests + 2 GithubConfig + 2 github_credentials + 1 cli::run + 1 code_reviewer = ~12 new tests.
- [x] 7.2 `cargo build --release` produces a binary that, given a single-file `~/config.yaml` containing all secrets inline (no env vars set), starts up successfully and emits log lines naming `inline (...)` sources.
- [x] 7.3 `openspec validate inline-secret-values --strict` passes.
