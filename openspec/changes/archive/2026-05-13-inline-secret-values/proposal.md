## Why

autocoder currently requires every secret (GitHub PATs, reviewer API key,
Slack bot token) to live in an environment variable, with `config.yaml`
naming only the env var. For an operator deploying on a single host they
control, the env-var indirection adds an extra step (export the var, or
maintain a separate `~/.autocoder.env` file) without adding security —
they're going to chmod-700 the secret-bearing file either way.

This change adds an *inline* alternative to the existing env-var-name
pattern. The operator can write the secret value directly in
`config.yaml` and skip the env-var dance. The env-var form remains
supported and is still the recommended path for multi-user hosts and
systemd-managed deployments.

The motivating workflow is the Quick Start: get from `git clone` to a
running daemon in one terminal session without juggling shell env state
between the build and the run.

## What Changes

- Introduce a `SecretSource` enum in `src/config.rs`:
  ```rust
  #[serde(untagged)]
  enum SecretSource {
      EnvVar(String),                    // bare string → env var NAME
      Inline { value: String },          // { value: "..." } → inline value
  }
  ```
  Resolves to a `String` via `SecretSource::resolve(field_label: &str) -> Result<String>`,
  reading the env var on `EnvVar(name)` and returning the value verbatim
  on `Inline { value }`. Error messages name the originating config field.

- `GithubConfig`: add `pub token: Option<SecretSource>`. Precedence when
  both are set: `token` wins, `token_env` is ignored with a warning. When
  `token` is absent, `token_env` behaves exactly as today.

- `GithubConfig.owner_tokens`: change map value type from
  `HashMap<String, String>` to `HashMap<String, SecretSource>`. A bare
  string value in YAML continues to parse as `SecretSource::EnvVar(name)`
  for backward compatibility; the new `{ value: "..." }` form parses as
  `SecretSource::Inline`.

- `ReviewerConfig`: add `pub api_key: Option<SecretSource>`. Precedence:
  `api_key` wins over `api_key_env`. Same warning when both are set.

- `github_credentials::resolve_token` consults `SecretSource::resolve`
  for both the owner-specific lookup and the fallback.

- `llm::build_from_config` reads the resolved key via the same
  mechanism.

- README: a new "Secrets in config.yaml" subsection under Security
  documenting the inline form, recommended file permissions
  (`chmod 600 ~/config.yaml`), and the explicit recommendation NOT to
  commit `config.yaml` with inline secrets. The default `.gitignore`
  guidance covers `*.yaml` already; the section reinforces this.

- `config.example.yaml`: both forms shown commented for every
  secret-bearing field.

- Startup logging: when an inline value is resolved, the startup log
  line names the field (`github.token` / `owner_tokens[my-org]` /
  `reviewer.api_key`) instead of an env-var name. The token value is
  never logged.

## Capabilities

### Modified Capabilities

- `orchestrator-cli`: GitHub token resolution and startup logging now
  accept either env-var names or inline values via `SecretSource`.
- `code-reviewer`: the reviewer's API key may be sourced inline OR via
  the existing env-var-name field.

## Impact

Operators on a single-host setup can collapse their deployment to a
single edited file: `~/config.yaml` carries the daemon's full
configuration including secrets, with `chmod 600` for protection. The
env-var path is unchanged for operators on multi-user hosts or systemd
deployments who want secrets out of the config file. Both paths run
side-by-side; per-field precedence is unambiguous (inline wins) and is
flagged when both are set on the same field.
