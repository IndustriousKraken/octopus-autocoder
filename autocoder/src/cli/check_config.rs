//! `autocoder check-config --config <path>` — validates a config file
//! without running the daemon. Surfaces the same checks `autocoder run`
//! executes at startup (parse, schema, token-route, workspace-collision,
//! audit-slug, path-collision, secret-source) and exits with one of three
//! codes: 0 (clean), 1 (warnings only), 2 (at least one error).
//!
//! The subcommand exists for two operator workflows:
//!   1. Unattended upgrades — `update.sh`'s preflight validates a new
//!      binary against the live config before swapping, avoiding the
//!      systemd `Restart=on-failure` loop on schema regressions.
//!   2. Hand-edited YAML — operators can ask "is this config valid?"
//!      without standing up a full daemon process.

use crate::config::{self, Config, Finding, FindingCategory, ValidationReport};
use anyhow::Result;
use clap::Args;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone)]
pub struct CheckConfigArgs {
    /// Path to the YAML configuration file to validate. Required;
    /// `check-config` has no global config-default file resolution.
    #[arg(long)]
    pub config: PathBuf,

    /// Emit one JSON object per finding on stdout instead of the
    /// human-readable `OK:` / `WARN:` / `ERROR:` lines. The final line
    /// is always a `{"level":"summary", ...}` object.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

/// Categorized result of a single `check-config` run. Carries every
/// signal `render_outcome` needs to produce stdout + stderr + exit code,
/// so tests can assert against a pure data structure.
pub enum Outcome {
    /// Could not read `--config <path>` (missing file, permission denied,
    /// etc.). Always exits 2.
    ReadError(String),
    /// Read OK but YAML parse failed (malformed YAML, schema mismatch
    /// rejected by serde). Always exits 2.
    ParseError(String),
    /// Read AND parsed; semantic validation completed. Exit code derives
    /// from the report.
    Validated(ValidationReport),
}

/// Subcommand entry point. Drives the I/O AND exits the process with the
/// resolved code. Logging is intentionally avoided so the operator's
/// stdout/stderr stays pristine for CI and `update.sh` parsing.
pub async fn execute(args: CheckConfigArgs) -> Result<()> {
    let outcome = load_and_validate(&args.config).await;
    let code = render_outcome(
        &args.config,
        &outcome,
        args.json,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )?;
    if code == 0 {
        Ok(())
    } else {
        std::process::exit(code);
    }
}

/// Read, parse, and validate `path`. Side-effect-free apart from reading
/// the file AND env vars consulted by the SecretSource check. Returns
/// the categorized outcome; the caller decides how to render it.
pub async fn load_and_validate(path: &Path) -> Outcome {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) => {
            return Outcome::ReadError(format!(
                "could not read config file `{}`: {e}",
                path.display()
            ));
        }
    };
    let cfg: Config = match serde_yml::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            return Outcome::ParseError(format!(
                "YAML parse error in `{}`: {e}",
                path.display()
            ));
        }
    };
    Outcome::Validated(config::validate_config(&cfg))
}

/// Categories the validator runs and reports on (excludes `Parse`,
/// which short-circuits the others). Used by the renderer to emit
/// `OK: <category> — <summary>` for every category that produced no
/// findings.
const REPORTED_CATEGORIES: &[FindingCategory] = &[
    FindingCategory::Schema,
    FindingCategory::TokenRoute,
    FindingCategory::WorkspaceCollision,
    FindingCategory::AuditSlug,
    FindingCategory::PathCollision,
    FindingCategory::SecretSource,
];

/// Render `outcome` to `stdout` + `stderr` (in the human or JSON format)
/// and return the exit code to use. Exit codes: 2 for any error,
/// 1 for warnings-only, 0 for clean.
pub fn render_outcome(
    config_path: &Path,
    outcome: &Outcome,
    use_json: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> std::io::Result<i32> {
    match outcome {
        Outcome::ReadError(msg) | Outcome::ParseError(msg) => {
            let finding = Finding {
                category: FindingCategory::Parse,
                message: msg.clone(),
                config_pointer: None,
            };
            if use_json {
                write_json_finding(stdout, "error", &finding)?;
                write_json_summary(stdout, 1, 0, config_path)?;
            } else {
                write_text_finding(stdout, "ERROR", &finding)?;
            }
            write_summary_to_stderr(stderr, 1, 0, config_path)?;
            Ok(2)
        }
        Outcome::Validated(report) => {
            let n_err = report.errors.len();
            let n_warn = report.warnings.len();
            if use_json {
                render_json(stdout, report, config_path)?;
            } else {
                render_text(stdout, report)?;
            }
            let exit = if n_err > 0 {
                2
            } else if n_warn > 0 {
                1
            } else {
                0
            };
            if exit != 0 {
                write_summary_to_stderr(stderr, n_err, n_warn, config_path)?;
            }
            Ok(exit)
        }
    }
}

fn render_text(stdout: &mut impl Write, report: &ValidationReport) -> std::io::Result<()> {
    for category in REPORTED_CATEGORIES {
        let has_finding = report
            .errors
            .iter()
            .chain(report.warnings.iter())
            .any(|f| f.category == *category);
        if !has_finding {
            writeln!(
                stdout,
                "OK: {} — {}",
                category.slug(),
                category.ok_summary()
            )?;
        } else {
            for f in report.errors.iter().filter(|f| f.category == *category) {
                write_text_finding(stdout, "ERROR", f)?;
            }
            for f in report.warnings.iter().filter(|f| f.category == *category) {
                write_text_finding(stdout, "WARN", f)?;
            }
        }
    }
    Ok(())
}

fn write_text_finding(
    stdout: &mut impl Write,
    level: &str,
    f: &Finding,
) -> std::io::Result<()> {
    match &f.config_pointer {
        Some(p) => writeln!(stdout, "{level}: {}: {} ({p})", f.category.slug(), f.message),
        None => writeln!(stdout, "{level}: {}: {}", f.category.slug(), f.message),
    }
}

fn render_json(
    stdout: &mut impl Write,
    report: &ValidationReport,
    config_path: &Path,
) -> std::io::Result<()> {
    for category in REPORTED_CATEGORIES {
        let has_finding = report
            .errors
            .iter()
            .chain(report.warnings.iter())
            .any(|f| f.category == *category);
        if !has_finding {
            let ok = serde_json::json!({
                "level": "ok",
                "category": category.slug(),
                "message": category.ok_summary(),
                "config_pointer": serde_json::Value::Null,
            });
            writeln!(stdout, "{ok}")?;
        } else {
            for f in report.errors.iter().filter(|f| f.category == *category) {
                write_json_finding(stdout, "error", f)?;
            }
            for f in report.warnings.iter().filter(|f| f.category == *category) {
                write_json_finding(stdout, "warn", f)?;
            }
        }
    }
    write_json_summary(stdout, report.errors.len(), report.warnings.len(), config_path)?;
    Ok(())
}

fn write_json_finding(
    stdout: &mut impl Write,
    level: &str,
    f: &Finding,
) -> std::io::Result<()> {
    let pointer = match &f.config_pointer {
        Some(p) => serde_json::Value::String(p.clone()),
        None => serde_json::Value::Null,
    };
    let line = serde_json::json!({
        "level": level,
        "category": f.category.slug(),
        "message": f.message,
        "config_pointer": pointer,
    });
    writeln!(stdout, "{line}")
}

fn write_json_summary(
    stdout: &mut impl Write,
    errors: usize,
    warnings: usize,
    config_path: &Path,
) -> std::io::Result<()> {
    let summary = serde_json::json!({
        "level": "summary",
        "errors": errors,
        "warnings": warnings,
        "config": config_path.display().to_string(),
    });
    writeln!(stdout, "{summary}")
}

fn write_summary_to_stderr(
    stderr: &mut impl Write,
    errors: usize,
    warnings: usize,
    config_path: &Path,
) -> std::io::Result<()> {
    writeln!(
        stderr,
        "check-config: {errors} error(s), {warnings} warning(s) in {}",
        config_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Env-var mutation is process-global; serialize tests that touch
    /// it so concurrent test runs do not race. tokio::sync::Mutex so
    /// the guard can be safely held across `.await` calls in async
    /// tests.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn write_yaml(yaml: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, yaml).unwrap();
        (dir, path)
    }

    fn run_render(outcome: &Outcome, path: &Path, json: bool) -> (String, String, i32) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = render_outcome(path, outcome, json, &mut stdout, &mut stderr).unwrap();
        (
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
            code,
        )
    }

    fn valid_inline_yaml() -> &'static str {
        r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
  command: claude
github:
  token: { value: "inline-pat" }
"#
    }

    #[tokio::test]
    async fn missing_file_exits_2_with_read_error() {
        let path = PathBuf::from("/nonexistent/check-config-fixture.yaml");
        let outcome = load_and_validate(&path).await;
        assert!(matches!(outcome, Outcome::ReadError(_)));
        let (stdout, stderr, code) = run_render(&outcome, &path, false);
        assert_eq!(code, 2);
        assert!(
            stdout.contains("ERROR: parse:") && stdout.contains("could not read"),
            "stdout must name the read failure; got: {stdout}"
        );
        assert!(
            stderr.contains("1 error(s)"),
            "stderr summary must report 1 error; got: {stderr}"
        );
        assert!(
            stderr.contains(path.to_string_lossy().as_ref()),
            "stderr must name the config path; got: {stderr}"
        );
    }

    #[tokio::test]
    async fn malformed_yaml_exits_2_with_parse_error() {
        let (_dir, path) = write_yaml(":::not-yaml::: -[[[\n");
        let outcome = load_and_validate(&path).await;
        assert!(matches!(outcome, Outcome::ParseError(_)));
        let (stdout, stderr, code) = run_render(&outcome, &path, false);
        assert_eq!(code, 2);
        assert!(
            stdout.contains("ERROR: parse:"),
            "stdout must mention a parse error; got: {stdout}"
        );
        assert!(stderr.contains("1 error(s)"), "got: {stderr}");
    }

    #[tokio::test]
    async fn valid_config_exits_0_with_ok_lines() {
        let _g = ENV_LOCK.lock().await;
        let (_dir, path) = write_yaml(valid_inline_yaml());
        let outcome = load_and_validate(&path).await;
        let (stdout, stderr, code) = run_render(&outcome, &path, false);
        assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
        for cat in REPORTED_CATEGORIES {
            let needle = format!("OK: {} — ", cat.slug());
            assert!(
                stdout.contains(&needle),
                "missing OK line for {}; got: {stdout}",
                cat.slug()
            );
        }
        assert!(stderr.is_empty(), "stderr must be empty on success; got: {stderr}");
    }

    #[tokio::test]
    async fn schema_violation_exits_2_with_error_line_and_summary() {
        let _g = ENV_LOCK.lock().await;
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 0
executor:
  kind: claude_cli
github:
  token: { value: "x" }
"#;
        let (_dir, path) = write_yaml(yaml);
        let outcome = load_and_validate(&path).await;
        let (stdout, stderr, code) = run_render(&outcome, &path, false);
        assert_eq!(code, 2);
        assert!(
            stdout.contains("ERROR: schema:")
                && stdout.contains("poll_interval_sec")
                && stdout.contains("(repositories/0/poll_interval_sec)"),
            "stdout must contain the schema error with pointer; got: {stdout}"
        );
        assert!(
            stderr.contains("1 error(s)"),
            "stderr summary must report 1 error; got: {stderr}"
        );
    }

    #[tokio::test]
    async fn missing_env_exits_1_with_warn_line_and_zero_errors() {
        let _g = ENV_LOCK.lock().await;
        let env_var = "AUTOCODER_CHECK_CONFIG_TEST_MISSING_ENV";
        unsafe { std::env::remove_var(env_var) };
        let yaml = format!(
            r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60
executor:
  kind: claude_cli
github:
  token_env: {env_var}
  owner_tokens:
    owner: {{ value: "inline-owner-pat" }}
"#
        );
        let (_dir, path) = write_yaml(&yaml);
        let outcome = load_and_validate(&path).await;
        let (stdout, stderr, code) = run_render(&outcome, &path, false);
        assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
        assert!(
            stdout.contains("WARN: secret-source:") && stdout.contains(env_var),
            "stdout must contain the WARN line naming the env var; got: {stdout}"
        );
        assert!(
            stderr.contains("0 error(s), 1 warning(s)"),
            "stderr summary must report 0 errors and 1 warning; got: {stderr}"
        );
    }

    // -----------------------------------------------------------------
    // --json output format
    // -----------------------------------------------------------------

    fn parse_jsonl(s: &str) -> Vec<serde_json::Value> {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("line is not valid JSON: {l}: {e}"))
            })
            .collect()
    }

    #[tokio::test]
    async fn json_summary_is_always_last_line_on_success() {
        let _g = ENV_LOCK.lock().await;
        let (_dir, path) = write_yaml(valid_inline_yaml());
        let outcome = load_and_validate(&path).await;
        let (stdout, _stderr, code) = run_render(&outcome, &path, true);
        assert_eq!(code, 0);
        let lines = parse_jsonl(&stdout);
        assert!(!lines.is_empty(), "stdout must contain at least one JSON line");
        let last = lines.last().unwrap();
        assert_eq!(last.get("level").and_then(|v| v.as_str()), Some("summary"));
        assert_eq!(last.get("errors").and_then(|v| v.as_u64()), Some(0));
        assert_eq!(last.get("warnings").and_then(|v| v.as_u64()), Some(0));
    }

    #[tokio::test]
    async fn json_emits_error_level_for_schema_violation() {
        let _g = ENV_LOCK.lock().await;
        let yaml = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 0
executor:
  kind: claude_cli
github:
  token: { value: "x" }
"#;
        let (_dir, path) = write_yaml(yaml);
        let outcome = load_and_validate(&path).await;
        let (stdout, _stderr, code) = run_render(&outcome, &path, true);
        assert_eq!(code, 2);
        let lines = parse_jsonl(&stdout);
        let schema_err = lines
            .iter()
            .find(|l| {
                l.get("level").and_then(|v| v.as_str()) == Some("error")
                    && l.get("category").and_then(|v| v.as_str()) == Some("schema")
            })
            .expect("must include a schema error line");
        assert_eq!(
            schema_err.get("config_pointer").and_then(|v| v.as_str()),
            Some("repositories/0/poll_interval_sec")
        );
        let last = lines.last().unwrap();
        assert_eq!(last.get("level").and_then(|v| v.as_str()), Some("summary"));
        assert_eq!(last.get("errors").and_then(|v| v.as_u64()), Some(1));
    }

    #[tokio::test]
    async fn json_parse_error_emits_single_error_then_summary() {
        let (_dir, path) = write_yaml(":::not-yaml::: -[[[\n");
        let outcome = load_and_validate(&path).await;
        let (stdout, _stderr, code) = run_render(&outcome, &path, true);
        assert_eq!(code, 2);
        let lines = parse_jsonl(&stdout);
        assert_eq!(lines.len(), 2, "expected 1 finding + 1 summary; got: {stdout}");
        assert_eq!(lines[0].get("level").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(lines[0].get("category").and_then(|v| v.as_str()), Some("parse"));
        assert_eq!(lines[1].get("level").and_then(|v| v.as_str()), Some("summary"));
    }
}
