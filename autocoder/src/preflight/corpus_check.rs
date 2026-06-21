//! Corpus-parameterized pre-flight check core (the shared machinery behind the
//! `[canon]` and `[rules]` verifier gates).
//!
//! Both gates do the SAME work: run a read-only, CLI-wrapped agentic session
//! ([`crate::agentic_run`], a56) that reads a change's spec-delta files AND a
//! comparison **corpus**, then returns its findings through a per-role
//! `submit_*` MCP tool. They differ ONLY in:
//!
//! - the comparison corpus — the `[canon]` gate compares against the project's
//!   canonical specs (`openspec/specs/`); the `[rules]` gate compares against
//!   the global rule corpus;
//! - what a finding names — a canonical requirement vs. a violated rule id;
//! - the MCP role + `submit_*` tool the session uses.
//!
//! This module factors the corpus-INVARIANT part of that work — spawning the
//! read-only session with a role, draining its submission, the bounded retry on
//! the flaky no-submission case, AND the fail-closed disposition — into a single
//! core. The `[canon]` gate
//! ([`crate::preflight::canon_contradiction`]) AND the `[rules]` gate
//! ([`crate::preflight::global_rules`]) are thin INSTANTIATIONS of this core:
//! each supplies its prompt (which lists its corpus), its role + allowed-tools,
//! AND its own payload→findings mapping. Neither forks the session/retry/
//! fail-closed logic — it lives here, once.
//!
//! Fail-closed posture (gatekeepers-fail-closed standard): the core returns
//! [`CorpusCheckSession::Errored`] on EVERY could-not-run path — a session
//! error (spawn, timeout, unregistered strategy) OR a session that ends with no
//! submission. A consumed submission is returned as
//! [`CorpusCheckSession::Submitted`] for the caller to map into findings; the
//! caller decides clean-vs-found from the mapped result. An empty submission is
//! a clean result the caller proceeds on.

use crate::agentic_run::ResolvedModel;
use crate::verifier_gate::{SessionSubmission, VerifierGate};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Truncation bound for the no-submission fail-closed WARN's stdout excerpt.
const RESPONSE_EXCERPT_MAX: usize = 200;

/// Read-only CLI tool permissions shared by the corpus-check gates. NO `Bash`,
/// NO `Write`, NO `Edit` — the agent reads the change's spec-delta files AND
/// the comparison corpus on demand AND returns its findings through its per-role
/// `submit_*` MCP tool.
pub const CORPUS_CHECK_ALLOWED_TOOLS: &[&str] = &["Read", "Glob", "Grep"];

/// The read-only file tools PLUS the qualified `submit_*` MCP tool for `role`.
/// Notably absent: `Bash`, `Write`, `Edit`. The common `query_canonical_specs`
/// tool is added separately by the agentic-run layer. Exposed so gate modules
/// AND tests can assert the surface.
pub fn allowed_tools_for_role(role: &str) -> Vec<String> {
    let mut tools: Vec<String> = CORPUS_CHECK_ALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if let Some(t) = crate::mcp_askuser_server::submission_tool_name_for_role(role) {
        tools.push(crate::mcp_askuser_server::qualified_tool_name(t));
    }
    tools
}

/// Outcome of ONE corpus-check session: the consumed submission (or `None` when
/// the agent recorded nothing) AND a truncated stdout/stderr excerpt for the
/// no-submission fail-closed WARN. The shared retry loop
/// ([`crate::verifier_gate::run_session_with_retry`]) tests
/// [`SessionSubmission::has_submission`] to retry ONLY the flaky no-submission
/// case.
pub struct CorpusCheckSessionOutcome {
    pub submission: Option<serde_json::Value>,
    pub stdout_excerpt: String,
}

impl SessionSubmission for CorpusCheckSessionOutcome {
    fn has_submission(&self) -> bool {
        self.submission.is_some()
    }
}

/// Abstracts "run ONE corpus-check session AND drain its submission" so the
/// orchestration ([`run_corpus_check_with_runner`]) is unit-testable without
/// spawning a CLI. Production is [`CliCorpusCheckSessionRunner`]; tests inject
/// canned/scripted submissions.
#[async_trait]
pub trait CorpusCheckSessionRunner: Send + Sync {
    async fn run_session(&self, prompt: &str) -> Result<CorpusCheckSessionOutcome>;
}

/// Production session runner shared by the `[canon]` AND `[rules]` gates: writes
/// the per-execution MCP config (`ORCH_MCP_ROLE = role`), runs the wrapped CLI
/// through [`crate::agentic_run::agentic_run_with_session`] in a read-only
/// capture sandbox, AND drains the stored submission via the control socket.
/// Parameterized by `role` + `allowed_tools` so each gate advertises its own
/// `submit_*` tool while sharing this body.
pub struct CliCorpusCheckSessionRunner<'a> {
    pub workspace: &'a Path,
    /// The MCP role AND submission routing key (`canon_contradiction_check` /
    /// `global_rules_check`). The MCP child advertises this role's `submit_*`
    /// tool ONLY; the runner consumes the same key after exit.
    pub role: &'static str,
    /// The role's full `--allowedTools` list (read-only file tools + the
    /// qualified `submit_*` tool), from [`allowed_tools_for_role`].
    pub allowed_tools: Vec<String>,
    pub strategy: &'a dyn crate::agentic_run::CliStrategy,
    pub model: &'a ResolvedModel,
    pub settings_dir: Option<&'a Path>,
    pub timeout: Duration,
    /// Human-readable noun for diagnostics (e.g. `change-vs-canonical-check`,
    /// `global-rules-check`). Used only in the timeout error message.
    pub subject_noun: &'static str,
}

#[async_trait]
impl CorpusCheckSessionRunner for CliCorpusCheckSessionRunner<'_> {
    async fn run_session(&self, prompt: &str) -> Result<CorpusCheckSessionOutcome> {
        // Write the per-execution MCP config advertising this role's `submit_*`
        // tool. `change == role` keys the submission-store entry; this runner
        // consumes the same key after exit.
        crate::executor::claude_cli::ClaudeCliExecutor::write_mcp_config(
            self.workspace,
            self.role,
            Some(self.role),
        )
        .with_context(|| format!("writing {} MCP config", self.subject_noun))?;

        // a70: a single-shot role — prune the session it creates on completion.
        let result = crate::agentic_run::agentic_run_with_session(
            crate::agentic_run::AgenticRunOpts {
                workspace: self.workspace,
                change: self.role,
                strategy: self.strategy,
                prompt,
                sandbox: crate::agentic_run::SandboxConfig {
                    allowed_tools: self.allowed_tools.clone(),
                    disallowed_bash_patterns: Vec::new(),
                    disallowed_read_paths: Vec::new(),
                    deny_writes: true,
                },
                model: Some(self.model),
                output_mode: crate::agentic_run::OutputMode::Capture,
                timeout: self.timeout,
                paths: None,
                settings_dir: self.settings_dir,
                // a21: `include_autocoder_tools` advertises the common
                // `query_canonical_specs` tool (among others); it activates only
                // when the daemon set the control-socket env (RAG configured),
                // AND fails open to empty hits otherwise — so the gate runs
                // correctly with OR without RAG.
                include_autocoder_tools: true,
                emit_stream_json_in_capture: false,
                resume_session_id: None,
                track_subprocess_marker: false,
                etxtbsy_retry_spawn: true,
                // a006: read-only role — read-only workspace; self-store from the
                // resolved model's provider.
                os_sandbox: crate::sandbox::current_run_sandbox(
                    crate::config::default_cli_for(self.model.provider),
                    false,
                ),
            },
            true,
            None,
        )
        .await;

        // Always remove the config we wrote, regardless of run outcome.
        crate::executor::claude_cli::ClaudeCliExecutor::delete_mcp_config(self.workspace);

        let outcome = result.with_context(|| format!("spawning {} subprocess", self.subject_noun))?;
        if outcome.timed_out {
            return Err(anyhow!(
                "{} session timed out after {}s",
                self.subject_noun,
                self.timeout.as_secs()
            ));
        }
        // Include stderr — opencode/agy write their real failure there, leaving
        // stdout empty, so a stdout-only excerpt is blank when it matters most.
        let stdout_excerpt = crate::agentic_run::failure_excerpt(&outcome, RESPONSE_EXCERPT_MAX);
        let submission = crate::audits::try_consume_submission(self.workspace, self.role).await;
        Ok(CorpusCheckSessionOutcome {
            submission,
            stdout_excerpt,
        })
    }
}

/// The fail-closed result of a corpus-check session BEFORE the caller maps the
/// payload into its own finding shape. The gatekeepers-fail-closed standard
/// lives HERE: every could-not-run path is [`CorpusCheckSession::Errored`]
/// (the change is held), never silently clean.
pub enum CorpusCheckSession {
    /// The session ran AND recorded a (schema-valid) submission payload. The
    /// caller maps it into findings: an empty mapped result is a clean pass; a
    /// non-empty one holds the change.
    Submitted(serde_json::Value),
    /// The session could NOT run (CLI unavailable, session error, timeout, OR no
    /// submission). Hold the change — never treat as clean.
    Errored { cause: String },
}

/// Run ONE corpus-check session via `runner` for `subject_slug`, applying the
/// bounded retry (`retries`) on the flaky no-submission case AND the shared
/// fail-closed disposition. Returns [`CorpusCheckSession::Submitted`] with the
/// consumed payload for the caller to map, OR [`CorpusCheckSession::Errored`]
/// on any could-not-run path. Every diagnostic carries `gate.label()` so a held
/// change is attributable to the gate that could not run.
///
/// This is the corpus-INVARIANT orchestration both gates share; the prompt
/// (which lists the corpus) AND the payload mapping (the finding shape) are the
/// caller's, supplied per-gate.
pub async fn run_corpus_check_with_runner(
    gate: VerifierGate,
    subject_slug: &str,
    retries: u32,
    prompt: &str,
    runner: &dyn CorpusCheckSessionRunner,
) -> CorpusCheckSession {
    let label = gate.label();
    // Bounded retry of the agentic session on the flaky no-submission case
    // (`executor.verifier_gate_retries`); a successful submission, a session
    // error, a timeout, AND an unregistered-strategy / CLI-unavailable error
    // are NOT retried. After the bound is exhausted the gate still fails closed.
    let session = crate::verifier_gate::run_session_with_retry(
        gate,
        subject_slug,
        retries,
        || runner.run_session(prompt),
    )
    .await;
    match session {
        Err(e) => {
            let cause = format!("session failed: {e:#}");
            tracing::warn!(
                subject = %subject_slug,
                "{label} corpus check could not run ({cause}); holding (fail-closed)"
            );
            CorpusCheckSession::Errored { cause }
        }
        Ok(outcome) => match outcome.submission {
            None => {
                let cause = format!(
                    "session ended with no submission (excerpt: {})",
                    outcome.stdout_excerpt
                );
                tracing::warn!(
                    subject = %subject_slug,
                    "{label} corpus check could not run ({cause}); holding (fail-closed)"
                );
                CorpusCheckSession::Errored { cause }
            }
            Some(payload) => CorpusCheckSession::Submitted(payload),
        },
    }
}

/// Enumerate every `openspec/changes/<change>/specs/<cap>/spec.md` path
/// (workspace-relative) for the change, sorted by capability. Returns an empty
/// `Vec` when the change has no `specs/` subdir or no per-capability spec files.
/// Shared by the corpus-check gates: the agent reads each on demand via the
/// read-only sandbox to compare the change's deltas against the corpus.
pub fn change_spec_delta_paths(workspace_root: &Path, change_slug: &str) -> Vec<String> {
    let specs_dir = workspace_root
        .join("openspec/changes")
        .join(change_slug)
        .join("specs");
    let Ok(read) = std::fs::read_dir(&specs_dir) else {
        return Vec::new();
    };
    let mut caps: Vec<(String, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            caps.push((name, path));
        }
    }
    caps.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::new();
    for (cap_name, cap_path) in caps {
        if cap_path.join("spec.md").is_file() {
            out.push(format!(
                "openspec/changes/{change_slug}/specs/{cap_name}/spec.md"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_tools_are_read_only() {
        // For a role with no registered submit tool, the surface is the bare
        // read-only file tools — no Bash/Write/Edit.
        let tools = allowed_tools_for_role("no_such_role");
        assert!(tools.contains(&"Read".to_string()));
        assert!(tools.contains(&"Glob".to_string()));
        assert!(tools.contains(&"Grep".to_string()));
        assert!(
            !tools.iter().any(|t| t == "Bash" || t == "Write" || t == "Edit"),
            "sandbox must deny Bash/Write/Edit: {tools:?}"
        );
    }

    /// A test runner that simulates a session error (spawn/timeout/strategy).
    struct ErrorRunner;
    #[async_trait]
    impl CorpusCheckSessionRunner for ErrorRunner {
        async fn run_session(&self, _prompt: &str) -> Result<CorpusCheckSessionOutcome> {
            Err(anyhow!("simulated session spawn error"))
        }
    }

    /// A canned runner returning a fixed submission (or None).
    struct CannedRunner {
        submission: Option<serde_json::Value>,
    }
    #[async_trait]
    impl CorpusCheckSessionRunner for CannedRunner {
        async fn run_session(&self, _prompt: &str) -> Result<CorpusCheckSessionOutcome> {
            Ok(CorpusCheckSessionOutcome {
                submission: self.submission.clone(),
                stdout_excerpt: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn submitted_payload_is_returned() {
        let payload = serde_json::json!({ "violations": [] });
        let runner = CannedRunner {
            submission: Some(payload.clone()),
        };
        let out =
            run_corpus_check_with_runner(VerifierGate::Rules, "c1", 0, "PROMPT", &runner).await;
        match out {
            CorpusCheckSession::Submitted(p) => assert_eq!(p, payload),
            CorpusCheckSession::Errored { cause } => panic!("expected Submitted, got {cause}"),
        }
    }

    #[tokio::test]
    async fn no_submission_fails_closed() {
        let runner = CannedRunner { submission: None };
        let out =
            run_corpus_check_with_runner(VerifierGate::Rules, "c1", 0, "PROMPT", &runner).await;
        assert!(matches!(out, CorpusCheckSession::Errored { .. }));
    }

    #[tokio::test]
    async fn session_error_fails_closed() {
        let out =
            run_corpus_check_with_runner(VerifierGate::Rules, "c1", 0, "PROMPT", &ErrorRunner).await;
        assert!(matches!(out, CorpusCheckSession::Errored { .. }));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn fail_closed_diagnostics_carry_the_gate_label() {
        let runner = CannedRunner { submission: None };
        let _ =
            run_corpus_check_with_runner(VerifierGate::Rules, "c1", 0, "PROMPT", &runner).await;
        assert!(
            logs_contain("[verifier:rules]"),
            "the fail-closed WARN must carry the gate identifier"
        );
    }
}
