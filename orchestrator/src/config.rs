use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub repositories: Vec<RepositoryConfig>,
    pub executor: ExecutorConfig,
    pub github: GithubConfig,
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
}

fn default_github_token_env() -> String {
    "GITHUB_TOKEN".to_string()
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
