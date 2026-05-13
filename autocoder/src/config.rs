use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A secret value sourced from EITHER an environment variable name (bare
/// YAML string) OR an inline value (`{ value: "..." }` object). Used for
/// any config field that carries a credential.
///
/// Parsing relies on `#[serde(untagged)]`: a YAML string deserializes to
/// `EnvVar(name)`; a YAML mapping with a `value` key deserializes to
/// `Inline { value }`. Any other shape produces a deserialize error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretSource {
    /// Bare string: names an environment variable holding the secret.
    EnvVar(String),
    /// `{ value: "..." }`: the secret value itself, verbatim.
    Inline { value: String },
}

impl SecretSource {
    /// Read the secret. For `EnvVar`, reads the named env var and errors if
    /// unset, naming both the env var and the originating config field. For
    /// `Inline`, returns the value verbatim.
    pub fn resolve(&self, field_label: &str) -> Result<String> {
        match self {
            Self::EnvVar(name) => std::env::var(name).map_err(|_| {
                anyhow!("secret env var `{name}` for `{field_label}` is not set")
            }),
            Self::Inline { value } => Ok(value.clone()),
        }
    }

    /// Source description for startup logs. NEVER returns the secret value.
    pub fn describe(&self, field_label: &str) -> String {
        match self {
            Self::EnvVar(name) => format!("env var {name}"),
            Self::Inline { .. } => format!("inline ({field_label})"),
        }
    }

    /// True when this source is an inline value (used to detect "both forms
    /// set" precedence warnings at startup).
    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub repositories: Vec<RepositoryConfig>,
    pub executor: ExecutorConfig,
    pub github: GithubConfig,
    #[serde(default)]
    pub reviewer: Option<ReviewerConfig>,
    #[serde(default)]
    pub slack: Option<SlackConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConfig {
    pub url: String,
    #[serde(default)]
    pub local_path: Option<PathBuf>,
    pub base_branch: String,
    pub agent_branch: String,
    pub poll_interval_sec: u64,
    #[serde(default)]
    pub slack_channel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorConfig {
    pub kind: ExecutorKind,
    #[serde(default = "default_executor_command")]
    pub command: String,
    #[serde(default = "default_executor_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    ClaudeCli,
}

fn default_executor_command() -> String {
    "claude".to_string()
}

fn default_executor_timeout() -> u64 {
    1800
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    #[serde(default = "default_github_token_env")]
    pub token_env: String,
    #[serde(default)]
    pub token: Option<SecretSource>,
    #[serde(default)]
    pub owner_tokens: Option<HashMap<String, SecretSource>>,
}

fn default_github_token_env() -> String {
    "GITHUB_TOKEN".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerConfig {
    #[serde(default)]
    pub enabled: bool,
    pub provider: ReviewerProvider,
    pub model: String,
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: Option<SecretSource>,
    #[serde(default)]
    pub api_base_url: Option<String>,
    #[serde(default)]
    pub prompt_template_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerProvider {
    Anthropic,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    pub bot_token_env: String,
    pub default_channel_id: String,
}

impl RepositoryConfig {
    /// Resolve the Slack channel to use for this repo: explicit per-repo
    /// `slack_channel_id` if set, otherwise the global default.
    pub fn slack_channel<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.slack_channel_id.as_deref().unwrap_or(fallback)
    }
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Config = serde_yaml::from_str(&raw)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(yaml: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, yaml).unwrap();
        (dir, path)
    }

    const VALID_TWO_REPO_YAML: &str = r#"
repositories:
  - url: "git@github.com:owner/repo-a.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 300
  - url: "git@github.com:owner/repo-b.git"
    local_path: /tmp/workspaces/repo-b
    base_branch: dev
    agent_branch: agent-q
    poll_interval_sec: 1800
executor:
  kind: claude_cli
  command: claude
  timeout_secs: 1800
github:
  token_env: GITHUB_TOKEN
"#;

    /// Parses the actual `config.example.yaml` file shipped at the repo
    /// root. This guards against the example drifting out of sync with the
    /// parser — operators who `cp config.example.yaml config.yaml` should
    /// always end up with a parseable file.
    #[test]
    fn config_example_yaml_parses() {
        let example_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("manifest dir has a parent")
            .join("config.example.yaml");
        assert!(
            example_path.exists(),
            "config.example.yaml must exist at {}",
            example_path.display()
        );
        let cfg = Config::load_from(&example_path)
            .expect("config.example.yaml must be parseable as Config");
        // Single-repo by default per the design.
        assert_eq!(cfg.repositories.len(), 1);
        assert_eq!(cfg.repositories[0].base_branch, "main");
        assert_eq!(cfg.repositories[0].agent_branch, "agent-q");
        // Reviewer and Slack blocks are commented out by default.
        assert!(cfg.reviewer.is_none(), "reviewer must be off by default");
        assert!(cfg.slack.is_none(), "slack must be off by default");
    }

    #[test]
    fn loads_example() {
        let (_dir, path) = write_config(VALID_TWO_REPO_YAML);
        let cfg = Config::load_from(&path).expect("config should parse");
        assert_eq!(cfg.repositories.len(), 2);
        assert_eq!(cfg.repositories[0].url, "git@github.com:owner/repo-a.git");
        assert_eq!(cfg.repositories[0].poll_interval_sec, 300);
        assert!(cfg.repositories[0].local_path.is_none());
        assert_eq!(
            cfg.repositories[1].local_path.as_deref(),
            Some(Path::new("/tmp/workspaces/repo-b"))
        );
        assert_eq!(cfg.executor.kind, ExecutorKind::ClaudeCli);
        assert_eq!(cfg.executor.command, "claude");
        assert_eq!(cfg.executor.timeout_secs, 1800);
        assert_eq!(cfg.github.token_env, "GITHUB_TOKEN");
    }

    #[test]
    fn applies_defaults_for_executor_and_github() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("config should parse");
        assert_eq!(cfg.executor.command, "claude");
        assert_eq!(cfg.executor.timeout_secs, 1800);
        assert_eq!(cfg.github.token_env, "GITHUB_TOKEN");
    }

    #[test]
    fn rejects_unknown_field() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
    typo_field: oops
executor:
  kind: claude_cli
github: {}
"#;
        let (_dir, path) = write_config(yaml);
        let err = Config::load_from(&path).expect_err("should reject unknown field");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("typo_field") || msg.to_lowercase().contains("unknown"),
            "error should mention unknown field; got: {msg}"
        );
    }

    #[test]
    fn missing_config_path_errors_with_path_in_message() {
        // 13.1.2 attestation: orchestrator-cli baseline says missing config
        // "exits with a non-zero status code AND stderr contains a single
        // error line naming the offending file path". Config::load_from is
        // the only step in the dispatch chain that reads the file; if it
        // returns an Err whose message names the path, anyhow's `main`
        // formatting will print that to stderr and the process will exit
        // non-zero (a Result::Err from `main`).
        let path = Path::new("/nonexistent/orchestrator-test-config.yaml");
        let err = Config::load_from(path).expect_err("missing path must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/nonexistent/orchestrator-test-config.yaml"),
            "error must name the offending path; got: {msg}"
        );
    }

    #[test]
    fn loads_with_reviewer() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
reviewer:
  enabled: true
  provider: anthropic
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY
  api_base_url: https://api.anthropic.com
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("config with reviewer should parse");
        let rv = cfg.reviewer.expect("reviewer block should be present");
        assert!(rv.enabled);
        assert_eq!(rv.provider, ReviewerProvider::Anthropic);
        assert_eq!(rv.model, "claude-sonnet-4-6");
        assert_eq!(rv.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(rv.api_base_url.as_deref(), Some("https://api.anthropic.com"));
        assert!(rv.prompt_template_path.is_none());
    }

    #[test]
    fn reviewer_disabled_by_default() {
        // Absent block parses to None — opt-in semantics.
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.reviewer.is_none());
    }

    #[test]
    fn reviewer_openai_compatible_provider() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
reviewer:
  provider: openai_compatible
  model: gpt-4o
  api_key_env: OPENAI_API_KEY
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).unwrap();
        let rv = cfg.reviewer.unwrap();
        assert_eq!(rv.provider, ReviewerProvider::OpenAiCompatible);
        assert!(!rv.enabled); // default false when omitted
    }

    #[test]
    fn loads_with_slack() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
    slack_channel_id: C01234OVERRIDE
executor:
  kind: claude_cli
github: {}
slack:
  bot_token_env: SLACK_BOT_TOKEN
  default_channel_id: C0DEFAULT
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).unwrap();
        let slack = cfg.slack.expect("slack block present");
        assert_eq!(slack.bot_token_env, "SLACK_BOT_TOKEN");
        assert_eq!(slack.default_channel_id, "C0DEFAULT");
        assert_eq!(
            cfg.repositories[0].slack_channel_id.as_deref(),
            Some("C01234OVERRIDE")
        );
    }

    #[test]
    fn repo_overrides_channel() {
        let repo_with_override = RepositoryConfig {
            url: "x".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            slack_channel_id: Some("C_REPO_LEVEL".into()),
        };
        assert_eq!(repo_with_override.slack_channel("C_DEFAULT"), "C_REPO_LEVEL");

        let repo_default = RepositoryConfig {
            url: "x".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            slack_channel_id: None,
        };
        assert_eq!(repo_default.slack_channel("C_DEFAULT"), "C_DEFAULT");
    }

    #[test]
    fn slack_block_absent_parses_to_none() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.slack.is_none());
    }

    #[test]
    fn loads_with_owner_tokens() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
  owner_tokens:
    rabbeverly: PERSONAL_GH_TOKEN
    my-org-a: ORG_A_GH_TOKEN
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("config with owner_tokens should parse");
        let map = cfg
            .github
            .owner_tokens
            .expect("owner_tokens block should be present");
        match map.get("rabbeverly").unwrap() {
            SecretSource::EnvVar(name) => assert_eq!(name, "PERSONAL_GH_TOKEN"),
            _ => panic!("expected env-var source for rabbeverly"),
        }
        match map.get("my-org-a").unwrap() {
            SecretSource::EnvVar(name) => assert_eq!(name, "ORG_A_GH_TOKEN"),
            _ => panic!("expected env-var source for my-org-a"),
        }
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn owner_tokens_optional() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: GITHUB_TOKEN
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("config without owner_tokens should parse");
        assert!(cfg.github.owner_tokens.is_none());
    }

    #[test]
    fn secret_source_parses_bare_string_as_env_var() {
        let s: SecretSource = serde_yaml::from_str("MY_VAR").unwrap();
        match s {
            SecretSource::EnvVar(name) => assert_eq!(name, "MY_VAR"),
            _ => panic!("bare string must parse as EnvVar"),
        }
    }

    #[test]
    fn secret_source_parses_object_as_inline() {
        let s: SecretSource = serde_yaml::from_str("value: \"abc123\"").unwrap();
        match s {
            SecretSource::Inline { value } => assert_eq!(value, "abc123"),
            _ => panic!("`{{value: ...}}` must parse as Inline"),
        }
    }

    #[test]
    fn secret_source_resolve_env_var_set() {
        // SAFETY: unique env var name per test, no parallel mutator.
        unsafe { std::env::set_var("AUTOCODER_TEST_SECRET_RESOLVE_SET", "x") };
        let s = SecretSource::EnvVar("AUTOCODER_TEST_SECRET_RESOLVE_SET".into());
        assert_eq!(s.resolve("test.field").unwrap(), "x");
        unsafe { std::env::remove_var("AUTOCODER_TEST_SECRET_RESOLVE_SET") };
    }

    #[test]
    fn secret_source_resolve_env_var_unset_names_field() {
        unsafe { std::env::remove_var("AUTOCODER_TEST_SECRET_RESOLVE_UNSET") };
        let s = SecretSource::EnvVar("AUTOCODER_TEST_SECRET_RESOLVE_UNSET".into());
        let err = s.resolve("my.field.label").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("AUTOCODER_TEST_SECRET_RESOLVE_UNSET"),
            "error must name env var; got: {msg}"
        );
        assert!(
            msg.contains("my.field.label"),
            "error must name field label; got: {msg}"
        );
    }

    #[test]
    fn secret_source_resolve_inline() {
        let s = SecretSource::Inline {
            value: "verbatim".into(),
        };
        assert_eq!(s.resolve("any.label").unwrap(), "verbatim");
    }

    #[test]
    fn secret_source_describe_redacts_inline_value() {
        let inline = SecretSource::Inline {
            value: "super-secret-token-xyz".into(),
        };
        let desc = inline.describe("github.token");
        assert!(
            !desc.contains("super-secret-token-xyz"),
            "describe must NEVER expose the inline value; got: {desc}"
        );
        assert_eq!(desc, "inline (github.token)");

        let env = SecretSource::EnvVar("MY_VAR".into());
        assert_eq!(env.describe("anything"), "env var MY_VAR");
    }

    #[test]
    fn loads_github_token_inline() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token:
    value: "ghp_inlinepat"
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("config with inline github.token should parse");
        match cfg.github.token.unwrap() {
            SecretSource::Inline { value } => assert_eq!(value, "ghp_inlinepat"),
            _ => panic!("expected inline source"),
        }
        // token_env default still present:
        assert_eq!(cfg.github.token_env, "GITHUB_TOKEN");
    }

    #[test]
    fn loads_owner_tokens_mixed_env_and_inline() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  owner_tokens:
    org-with-env-var: ORG_ENV_VAR
    org-with-inline:
      value: "ghp_inlinevalue"
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("mixed owner_tokens should parse");
        let map = cfg.github.owner_tokens.expect("present");
        match map.get("org-with-env-var").unwrap() {
            SecretSource::EnvVar(n) => assert_eq!(n, "ORG_ENV_VAR"),
            _ => panic!("env-var entry mis-parsed"),
        }
        match map.get("org-with-inline").unwrap() {
            SecretSource::Inline { value } => assert_eq!(value, "ghp_inlinevalue"),
            _ => panic!("inline entry mis-parsed"),
        }
    }

    #[test]
    fn loads_reviewer_inline_api_key() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github: {}
reviewer:
  enabled: true
  provider: anthropic
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY
  api_key:
    value: "sk-ant-inline"
"#;
        let (_dir, path) = write_config(yaml);
        let cfg = Config::load_from(&path).expect("inline reviewer api_key should parse");
        let rv = cfg.reviewer.unwrap();
        match rv.api_key.unwrap() {
            SecretSource::Inline { value } => assert_eq!(value, "sk-ant-inline"),
            _ => panic!("expected inline reviewer key"),
        }
        // api_key_env still present:
        assert_eq!(rv.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn rejects_unknown_executor_kind() {
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: gpt_cli
github: {}
"#;
        let (_dir, path) = write_config(yaml);
        let err = Config::load_from(&path).expect_err("unknown executor kind should fail");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("gpt_cli") || msg.to_lowercase().contains("variant"),
            "error should reject unknown variant; got: {msg}"
        );
    }
}
