//! `autocoder doctor` — run the dependency preflight on demand and print the
//! full report. Exits non-zero when a required dependency is missing/unusable
//! (a011 task 1.4).

use crate::config::Config;
use crate::dependency_preflight::{self, RealProbe};
use anyhow::Result;
use std::path::PathBuf;

pub async fn execute(config: Option<PathBuf>) -> Result<()> {
    let cfg = load_doctor_config(config);
    let report = dependency_preflight::build_report(&cfg, &RealProbe);
    print!("{}", report.render());
    let code = dependency_preflight::doctor_exit_code(&report);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Resolve the config that drives the configuration-implied checks. Uses the
/// same discovery as `run` (explicit → systemd unit → defaults). If nothing
/// resolves, or the resolved file fails to load, fall back to a minimal config
/// so the REQUIRED dependencies are still reported — `doctor` is a diagnostic
/// and must never refuse to run just because the config is absent or broken.
fn load_doctor_config(explicit: Option<PathBuf>) -> Config {
    match super::run::resolve_run_config_path(explicit) {
        Ok(path) => match Config::load_from(&path) {
            Ok(cfg) => {
                eprintln!("doctor: using config {}", path.display());
                cfg
            }
            Err(e) => {
                eprintln!(
                    "doctor: config at {} could not be loaded ({e:#}); \
                     checking required dependencies only",
                    path.display()
                );
                minimal_config()
            }
        },
        Err(_) => {
            eprintln!("doctor: no config found; checking required dependencies only");
            minimal_config()
        }
    }
}

/// A config carrying just enough to exercise the required-dependency checks
/// (no repositories, executor on its default `claude`, scout off, no RAG).
fn minimal_config() -> Config {
    const YAML: &str = r#"
repositories: []
executor:
  kind: claude_cli
  command: claude
  timeout_secs: 1800
github:
  token_env: GITHUB_TOKEN
features:
  scout:
    enabled: false
"#;
    serde_yml::from_str(YAML).expect("minimal doctor config parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_parses_and_has_no_repos() {
        let cfg = minimal_config();
        assert!(cfg.repositories.is_empty());
        assert_eq!(cfg.executor.command, "claude");
        assert!(!cfg.features.scout.enabled);
    }
}
