## Context

autocoder reads three categories of secret today:

1. **GitHub PATs** — one global (`github.token_env`) plus zero-or-more per
   owner (`github.owner_tokens: { owner: ENV_VAR_NAME }`).
2. **Reviewer API key** — `reviewer.api_key_env` for the LLM provider.
3. **Slack bot token** — `slack.bot_token_env`. Out of scope for this
   change: the `slack:` block is being renamed to `chatops:` in the
   in-flight `experimental-chatops-providers` change, which will adopt
   the same dual-source pattern established here when it lands.

Every field above names an env var and reads the value at startup
(GitHub PAT) or at construction (reviewer, Slack). The pattern is
uniform and security-defensible, but it forces a multi-step deployment:

1. Edit `config.yaml`.
2. Pick env-var names (or use the defaults).
3. Export the vars in the same shell that launches the daemon (or
   maintain a separate `~/.autocoder.env`).

For a single-operator, single-host deployment the env-var indirection is
ceremony without security benefit — both the config file and the
secret-bearing env file end up `chmod 600` and protected the same way.
This change adds an inline-value alternative that collapses the
deployment to one file.

## Goals / Non-Goals

**Goals:**

- Every secret-bearing config field SHALL accept either an env-var name
  (current behavior) or an inline value.
- Backward-compatible: any existing config that uses env-var names
  parses and behaves identically.
- Precedence is well-defined: when both forms are present for the same
  logical field, inline wins and the env-var form is ignored with a
  one-line warning.
- The `owner_tokens` map's value-type extension uses YAML's natural
  shape difference (bare string vs object) so existing configs need no
  changes.
- Startup logs name the resolved *source* (`env var X` vs
  `inline (github.token)`) without ever logging the value.

**Non-Goals:**

- **Encrypted secrets at rest.** Inline values are plaintext. Operators
  who need encryption use existing tools (`age`, `sops`, a vault) and
  inject the decrypted value via env var as today.
- **External secret managers.** No AWS Secrets Manager / Vault / GCP
  Secret Manager integration. Out of scope; would be a separate change.
- **Per-secret rotation.** Inline values change when the operator edits
  the file and restarts the daemon. No live-reload, no rotation API.
- **Slack/ChatOps secrets.** Touched only after the
  `experimental-chatops-providers` rename lands; that change will adopt
  the `SecretSource` pattern from this one.

## Decisions

### The `SecretSource` enum

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretSource {
    /// Bare string in YAML: the value names an environment variable.
    /// `token_env: GITHUB_TOKEN` deserializes to `EnvVar("GITHUB_TOKEN".into())`.
    EnvVar(String),
    /// `{ value: "..." }` object in YAML: the value is the secret itself.
    /// `token: { value: "github_pat_xxx" }` deserializes to
    /// `Inline { value: "github_pat_xxx".into() }`.
    Inline { value: String },
}

impl SecretSource {
    pub fn resolve(&self, field_label: &str) -> Result<String> {
        match self {
            Self::EnvVar(name) => std::env::var(name).map_err(|_| {
                anyhow!(
                    "secret env var `{name}` for `{field_label}` is not set"
                )
            }),
            Self::Inline { value } => Ok(value.clone()),
        }
    }

    /// Stable description for startup logs. NEVER returns the secret value.
    pub fn describe(&self, field_label: &str) -> String {
        match self {
            Self::EnvVar(name) => format!("env var {name}"),
            Self::Inline { .. } => format!("inline ({field_label})"),
        }
    }
}
```

`#[serde(untagged)]` makes the bare-string form parse as `EnvVar` and the
object form parse as `Inline`. Both `serde_yaml` and any future
`serde_json` use cases handle this cleanly. The error path is minimal:
if a YAML value is neither a string nor an object with a `value` field,
serde produces a generic "could not match any variant" error; we add a
covering test to confirm the message is intelligible.

### Field-level integration

For the two singleton secret fields (`github.token`, `reviewer.api_key`),
the existing `*_env` field stays as-is. A new `*` field is added:

```rust
pub struct GithubConfig {
    #[serde(default = "default_github_token_env")]
    pub token_env: String,
    #[serde(default)]
    pub token: Option<SecretSource>,    // NEW
    #[serde(default)]
    pub owner_tokens: Option<HashMap<String, SecretSource>>,  // CHANGED type
}

pub struct ReviewerConfig {
    pub provider: ReviewerProvider,
    pub model: String,
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: Option<SecretSource>,  // NEW
    // ...
}
```

For the owner_tokens map, the value type changes from `String` (an
env-var name) to `SecretSource`. Backward compatibility is automatic:
a bare-string YAML value parses as `SecretSource::EnvVar(name)`, which
the resolver handles identically to today.

### Resolution & precedence

```rust
// In github_credentials.rs:
pub fn resolve_token(cfg: &GithubConfig, owner: &str) -> Result<String> {
    if let Some(map) = cfg.owner_tokens.as_ref() {
        if let Some((_k, source)) = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(owner)) {
            return source.resolve(&format!("github.owner_tokens[{owner}]"));
        }
    }
    if let Some(inline) = cfg.token.as_ref() {
        return inline.resolve("github.token");
    }
    SecretSource::EnvVar(cfg.token_env.clone())
        .resolve(&format!("github.token_env={}", cfg.token_env))
}
```

When both `github.token` and `github.token_env` resolve (i.e. inline is
set AND the env var is also set), the inline value wins. A `tracing::warn!`
at startup notes "github.token (inline) takes precedence; the env var
named by github.token_env is being ignored." Same pattern for the
reviewer.

### Startup log labels

Today: `repository <url> will use GitHub token from env var X`.

With this change: `repository <url> will use GitHub token from <source-description>`,
where `<source-description>` is one of:

- `env var GITHUB_TOKEN` (when source resolved to `EnvVar`)
- `inline (github.token)` (when source resolved to `Inline`)
- `inline (github.owner_tokens[my-org-a])` (when the matched owner_tokens
  entry was an inline value)

This way an operator skimming logs can confirm which fields contain
inline secrets and audit the file's permissions accordingly.

### Security guidance in the README

A new "Secrets in `config.yaml`" subsection lands under the existing
"AI Security & Guardrails" section. Bullet points:

- Either form (env var or inline) is fine; pick based on your
  deployment.
- For inline secrets: `chmod 600 ~/config.yaml` and never commit the
  file. The default `.gitignore` recommendation already covers this.
- For multi-user hosts or systemd-managed deployments, the env-var
  form is still preferred: it keeps secrets out of YAML so the config
  file can be readable for audit without exposing tokens.

## Risks / Trade-offs

- **Risk:** inline values get accidentally committed to a repo.
  - **Mitigation:** README explicitly warns; `.gitignore` guidance is
    reinforced; the existing systemd deployment example (which uses
    `EnvironmentFile`) is unchanged and remains the recommended path
    for "production-style" deployments.

- **Risk:** the YAML enum shape (bare string vs `{value: "..."}`) is
  unfamiliar; an operator writes `token: "github_pat_xxx"` and gets a
  confusing "could not match any variant" error.
  - **Mitigation:** a covering serde test asserts the error message
    quality; the README shows both forms side by side; `config.example.yaml`
    keeps both commented as well.

- **Risk:** dual fields invite "both set, which wins?" confusion.
  - **Mitigation:** inline wins, deterministic, documented; the
    startup warn-line is the operator's signal that the env-var field
    is being ignored.

- **Risk:** the `SecretSource` enum is a foundation other capabilities
  will want (chatops, future webhook secrets, etc.); landing it in
  `config.rs` rather than its own module ties it to config-loading,
  which is mostly fine but means importers reach across the module.
  - **Mitigation:** `SecretSource` is a small `pub` type in
    `config.rs`. If a future change wants it elsewhere it can be moved
    to its own module without breaking semver — internal-only refactor.
