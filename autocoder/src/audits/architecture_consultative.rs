//! `architecture_consultative` audit — invokes the wrapped agent CLI
//! against a consultative architecture prompt sandboxed read-only.
//! Returns 0-5 anchored architecture *questions* (never directives) as
//! `AuditOutcome::Reported(findings)`.
//!
//! `requires_head_change = true` (no code change → no new architecture
//! to audit). `WritePolicy::None` (read-only; the post-hoc diff check
//! the foundation enforces will revert any leak).

use super::{
    Audit, AuditContext, AuditOutcome, AuditSettings, Finding, Severity, WritePolicy,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Embedded default prompt. Operator can override via
/// `audits.architecture_consultative.prompt_path` in `config.yaml`.
const DEFAULT_PROMPT: &str = include_str!("../../../prompts/architecture-consultative.md");

/// Maximum number of findings the audit will accept from the agent.
/// More than this is treated as a malformed run (the prompt explicitly
/// caps observations at 5 with a target of 3).
const MAX_FINDINGS: usize = 5;

/// Tools the read-only sandbox allows. Mirrors the foundation's
/// `WritePolicy::None` declaration — no `Write`/`Edit`.
const READ_ONLY_TOOLS: &[&str] = &["Read", "Glob", "Grep", "Bash"];

pub struct ArchitectureConsultativeAudit {
    settings: AuditSettings,
    executor_command: String,
    executor_timeout_secs: u64,
}

impl ArchitectureConsultativeAudit {
    pub fn new(settings: &AuditSettings, executor_command: &str, executor_timeout_secs: u64) -> Self {
        Self {
            settings: settings.clone(),
            executor_command: executor_command.to_string(),
            executor_timeout_secs,
        }
    }

    /// Resolve the prompt: operator override (if `prompt_path` is set
    /// and the file is readable + non-empty) else the embedded default.
    /// A configured override that fails to read or is empty is an
    /// error — silent fallback to the default would mask operator misconfig.
    fn resolve_prompt(&self) -> Result<String> {
        match &self.settings.prompt_path {
            Some(path) => {
                let s = std::fs::read_to_string(path).with_context(|| {
                    format!(
                        "reading architecture-consultative prompt override at {}",
                        path.display()
                    )
                })?;
                if s.trim().is_empty() {
                    return Err(anyhow!(
                        "architecture-consultative prompt override at {} is empty",
                        path.display()
                    ));
                }
                Ok(s)
            }
            None => Ok(DEFAULT_PROMPT.to_string()),
        }
    }

    /// Spawn the wrapped CLI in a read-only sandbox, write `prompt` on
    /// stdin, return collected stdout/stderr + exit info.
    async fn run_subprocess(&self, workspace: &Path, prompt: &str) -> Result<SubprocessOutcome> {
        let mut child = Command::new(&self.executor_command)
            .arg("--allowedTools")
            .arg(READ_ONLY_TOOLS.join(","))
            .arg("--permission-mode")
            .arg("default")
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "spawning audit executor command `{}` for architecture_consultative",
                    self.executor_command
                )
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes()).await;
        }
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let timeout = Duration::from_secs(self.executor_timeout_secs);
        let sleeper = tokio::time::sleep(timeout);
        tokio::pin!(sleeper);

        let exit_status: Option<std::io::Result<std::process::ExitStatus>> = tokio::select! {
            biased;
            () = &mut sleeper => None,
            res = child.wait() => Some(res),
        };

        match exit_status {
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                Ok(SubprocessOutcome {
                    timed_out: true,
                    exit_status: None,
                    stdout: String::new(),
                    stderr: "timeout".to_string(),
                })
            }
            Some(Err(e)) => Err(e).context("waiting on architecture_consultative child process"),
            Some(Ok(status)) => {
                let mut stdout_text = String::new();
                if let Some(ref mut p) = stdout_pipe {
                    let _ = p.read_to_string(&mut stdout_text).await;
                }
                let mut stderr_text = String::new();
                if let Some(ref mut p) = stderr_pipe {
                    let _ = p.read_to_string(&mut stderr_text).await;
                }
                Ok(SubprocessOutcome {
                    timed_out: false,
                    exit_status: Some(status),
                    stdout: stdout_text,
                    stderr: stderr_text,
                })
            }
        }
    }
}

#[async_trait]
impl Audit for ArchitectureConsultativeAudit {
    fn audit_type(&self) -> &'static str {
        "architecture_consultative"
    }

    fn requires_head_change(&self) -> bool {
        true
    }

    fn write_policy(&self) -> WritePolicy {
        WritePolicy::None
    }

    async fn run(&self, ctx: &AuditContext<'_>) -> Result<AuditOutcome> {
        let prompt = self.resolve_prompt()?;
        let outcome = self.run_subprocess(ctx.workspace, &prompt).await?;
        if outcome.timed_out {
            return Err(anyhow!(
                "architecture_consultative audit timed out after {}s",
                self.executor_timeout_secs
            ));
        }
        let status = outcome
            .exit_status
            .expect("non-timeout path always carries an exit status");
        if !status.success() {
            let stderr_excerpt: String =
                outcome.stderr.trim().chars().take(400).collect();
            return Err(anyhow!(
                "architecture_consultative agent exited {code:?}: {stderr_excerpt}",
                code = status.code(),
            ));
        }
        let findings = parse_findings(&outcome.stdout)?;
        Ok(AuditOutcome::Reported(findings))
    }
}

struct SubprocessOutcome {
    timed_out: bool,
    exit_status: Option<std::process::ExitStatus>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Deserialize)]
struct AgentOutput {
    findings: Vec<RawFinding>,
}

#[derive(Debug, Deserialize)]
struct RawFinding {
    subject: String,
    body: String,
    anchor: Option<String>,
    severity: Severity,
}

/// Parse the agent's stdout as the strict JSON shape declared in the
/// prompt. A leading code-fence (```json ... ```) wrap is tolerated
/// because LLMs do that even when told not to; anything else fails
/// loudly with a truncated excerpt for the audit-run log.
fn parse_findings(stdout: &str) -> Result<Vec<Finding>> {
    let candidate = strip_code_fence(stdout.trim());
    let parsed: AgentOutput = serde_json::from_str(candidate).map_err(|e| {
        let excerpt: String = stdout.chars().take(800).collect();
        anyhow!(
            "architecture_consultative output is not valid JSON: {e}. \
             stdout excerpt:\n{excerpt}"
        )
    })?;
    if parsed.findings.len() > MAX_FINDINGS {
        let excerpt: String = stdout.chars().take(800).collect();
        return Err(anyhow!(
            "architecture_consultative produced {n} findings (max {MAX_FINDINGS}). \
             stdout excerpt:\n{excerpt}",
            n = parsed.findings.len(),
        ));
    }
    Ok(parsed
        .findings
        .into_iter()
        .map(|r| Finding {
            severity: r.severity,
            subject: r.subject,
            body: r.body,
            anchor: r.anchor,
        })
        .collect())
}

/// If `s` is wrapped in a fenced JSON block (with or without a `json`
/// language tag), return the inner content. Otherwise return `s`
/// unchanged.
fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    let after_open = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|rest| rest.trim_start_matches('\n'));
    match after_open {
        Some(rest) => rest.strip_suffix("```").unwrap_or(rest).trim(),
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit() -> ArchitectureConsultativeAudit {
        ArchitectureConsultativeAudit::new(&AuditSettings::default(), "/bin/true", 30)
    }

    #[test]
    fn audit_metadata_matches_spec() {
        let a = audit();
        assert_eq!(a.audit_type(), "architecture_consultative");
        assert!(a.requires_head_change());
        assert_eq!(a.write_policy(), WritePolicy::None);
    }

    #[test]
    fn parses_well_formed_findings_json() {
        let stdout = r#"{
            "findings": [
                {
                    "subject": "Should foo be split from bar?",
                    "body": "These two modules import from each other across most files; the boundary may have eroded.",
                    "anchor": "src/foo.rs:10-200",
                    "severity": "low"
                },
                {
                    "subject": "Why does baz own both parsing and rendering?",
                    "body": "baz.rs grew from a small helper to ~700 lines spanning two distinct concerns.",
                    "anchor": "src/baz.rs:1-700",
                    "severity": "medium"
                }
            ]
        }"#;
        let findings = parse_findings(stdout).expect("well-formed JSON parses");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].subject, "Should foo be split from bar?");
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].anchor.as_deref(), Some("src/foo.rs:10-200"));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn parses_zero_findings_as_no_findings_outcome() {
        let stdout = r#"{ "findings": [] }"#;
        let findings = parse_findings(stdout).expect("empty findings parses");
        assert!(findings.is_empty());
    }

    #[test]
    fn rejects_runs_with_more_than_5_findings() {
        let mut s = String::from("{ \"findings\": [");
        for i in 0..6 {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{ \"subject\": \"q{i}?\", \"body\": \"b\", \"anchor\": \"f.rs:1-2\", \"severity\": \"low\" }}",
            ));
        }
        s.push_str("] }");
        let err = parse_findings(&s).expect_err("6 findings must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("max 5") || msg.contains("max ") , "error must name the cap: {msg}");
        assert!(msg.contains("6") , "error must name the actual count: {msg}");
    }

    #[test]
    fn malformed_json_returns_err_with_excerpt() {
        let stdout = "I am not JSON, I am sorry.\nLet me try again later.";
        let err = parse_findings(stdout).expect_err("non-JSON must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("not valid JSON"), "error must say 'not valid JSON': {msg}");
        // Excerpt of the bad stdout must appear so the audit-run log is useful.
        assert!(msg.contains("I am not JSON"), "error must include stdout excerpt: {msg}");
    }

    #[test]
    fn parses_findings_inside_json_code_fence() {
        // Common LLM behavior: wraps output in ```json ... ``` even when
        // told not to. Tolerate it rather than fail the whole run.
        let stdout = "```json\n{ \"findings\": [] }\n```";
        let findings = parse_findings(stdout).expect("fenced JSON parses");
        assert!(findings.is_empty());
    }

    #[test]
    fn prompt_contains_anti_microservices_clause() {
        // Protects against accidental prompt drift: the spec REQUIRES
        // the default prompt forbid microservices/process/binary splits.
        // Lower-cased compare so future word-order edits don't break us.
        let lower = DEFAULT_PROMPT.to_lowercase();
        assert!(
            lower.contains("microservices"),
            "default prompt must explicitly forbid microservices"
        );
        assert!(
            lower.contains("separate processes")
                || lower.contains("separate process")
                || lower.contains("separate binaries")
                || lower.contains("separate binary"),
            "default prompt must forbid splits into separate processes / binaries"
        );
    }

    #[test]
    fn prompt_contains_language_agnostic_clause() {
        let lower = DEFAULT_PROMPT.to_lowercase();
        // The prompt directs the agent NOT to assume a language.
        assert!(
            lower.contains("language-agnostic")
                || lower.contains("you do not know what language"),
            "default prompt must declare language-agnostic survey method"
        );
        // Rewrites in another language are explicitly forbidden.
        assert!(
            lower.contains("rewrite in a different programming language")
                || lower.contains("rewrite in a different language"),
            "default prompt must forbid 'rewrite in a different language' suggestions"
        );
    }

    #[test]
    fn prompt_forbids_writes_and_edits() {
        // The audit's WritePolicy::None contract requires the prompt
        // tell the agent it cannot write or edit anything. The post-hoc
        // diff check is belt-and-suspenders, but if the prompt itself
        // doesn't say so, we'll waste runs on agents that try.
        let lower = DEFAULT_PROMPT.to_lowercase();
        assert!(
            lower.contains("do not use the `write`") || lower.contains("do not use the `write` or `edit`"),
            "default prompt must forbid Write/Edit tools explicitly"
        );
    }

    #[test]
    fn resolve_prompt_uses_default_when_no_override() {
        let p = audit().resolve_prompt().expect("default loads");
        assert!(p.contains("Architecture Consultative Audit"));
    }

    #[test]
    fn resolve_prompt_uses_override_when_set() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("custom.md");
        std::fs::write(&path, "CUSTOM_CONSULTATIVE_PROMPT_SENTINEL").unwrap();
        let settings = AuditSettings {
            prompt_path: Some(path),
            notify_on_clean: false,
            extra: Default::default(),
        };
        let a = ArchitectureConsultativeAudit::new(&settings, "/bin/true", 30);
        let p = a.resolve_prompt().expect("override loads");
        assert!(p.contains("CUSTOM_CONSULTATIVE_PROMPT_SENTINEL"));
    }

    #[test]
    fn resolve_prompt_errors_on_empty_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.md");
        std::fs::write(&path, "   \n  \n").unwrap();
        let settings = AuditSettings {
            prompt_path: Some(path),
            notify_on_clean: false,
            extra: Default::default(),
        };
        let a = ArchitectureConsultativeAudit::new(&settings, "/bin/true", 30);
        let err = a.resolve_prompt().expect_err("empty override must error");
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn resolve_prompt_errors_on_missing_override() {
        let settings = AuditSettings {
            prompt_path: Some("/definitely/not/a/real/consultative/prompt.md".into()),
            notify_on_clean: false,
            extra: Default::default(),
        };
        let a = ArchitectureConsultativeAudit::new(&settings, "/bin/true", 30);
        let err = a.resolve_prompt().expect_err("missing override must error");
        assert!(format!("{err:#}").contains("architecture-consultative prompt override"));
    }
}
