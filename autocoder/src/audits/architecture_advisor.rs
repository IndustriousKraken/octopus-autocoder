//! Architecture advisory audit. Replaces the retired `architecture_brightline`
//! (pure-metric, high-volume, frequently-wrong) AND `architecture_consultative`
//! (judgment but no action) audits with a single judgment-based advisor.
//!
//! The audit uses a cheap, language-agnostic SELECTOR — whole-file line count
//! ([`crate::audits::code_metrics::select_candidate_files`]) — to pick a
//! bounded set of the longest files over a pain threshold. The line count is
//! a selector ONLY: it decides which files the judgment pass examines AND is
//! NEVER emitted as a finding. There is no function-length, duplicate-
//! signature, or duplicate-body metric, AND no `.brightline-ignore` file.
//!
//! For the selected candidates the audit invokes the wrapped agent CLI with a
//! read-only sandbox (`Read`, `Glob`, `Grep`, `Bash`) plus the
//! `submit_findings` MCP tool (a57) and the `architecture-advisor` prompt. The
//! agent reads each candidate (and the surrounding context needed to judge
//! cohesion AND placement) and returns up to 5 ranked, anchored
//! recommendations — what is wrong, why it matters, AND the concrete action —
//! by calling `submit_findings`. The daemon consumes the stored submission and
//! returns `AuditOutcome::Reported`. A run that examines its candidates AND
//! finds none worth refactoring returns `Reported(vec![])`; a run with no
//! candidates over the threshold returns `Reported(vec![])` without invoking
//! the CLI. The audit-run log records the candidates examined either way, so
//! the run carries evidence it looked.
//!
//! `requires_head_change = true` — re-judging the same SHA wastes CLI
//! invocations. `WritePolicy::None` — strictly advisory; the operator decides
//! which recommendations (if any) become work via `@<bot> send it`, which
//! routes a behavior-preserving refactor to the issues lane by default.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Audit, AuditContext, AuditLogWriter, AuditOutcome, Finding, Severity, WritePolicy,
    code_metrics, workspace_is_valid, workspace_unavailable_outcome,
};
use crate::config::{AuditSettings, ExecutorConfig, ResolvedSandbox};
use crate::prompts::{PromptId, PromptLoader};

/// Tools the advisor agent may call. Excludes `Write` and `Edit` so the
/// sandbox blocks workspace modifications outright; the audit-run log
/// captures the agent's stdout for forensic review.
const ALLOWED_TOOLS: &[&str] = &["Read", "Glob", "Grep", "Bash"];

/// Maximum number of recommendations the audit will accept. More than this
/// indicates the agent ignored its cap; the schema rejects the submission as
/// a correctable tool error rather than truncating.
const MAX_FINDINGS: usize = 5;

/// Maximum number of characters of stderr to embed in an error message. The
/// full stderr always lands in the audit-run log.
const STDERR_EXCERPT_CHARS: usize = 400;

/// `audits.settings.architecture_advisor.extra` key for the selector's
/// whole-file line-count pain threshold.
const SETTINGS_KEY_SELECTOR_THRESHOLD: &str = "selector_threshold";
/// `audits.settings.architecture_advisor.extra` key for the candidate cap.
const SETTINGS_KEY_CANDIDATE_CAP: &str = "candidate_cap";

pub struct ArchitectureAdvisorAudit {
    settings: AuditSettings,
    executor_command: String,
    executor_timeout_secs: u64,
    sandbox: ResolvedSandbox,
    /// Selector pain threshold: a file must exceed this line count to be a
    /// candidate. Resolved from settings; defaults to
    /// [`code_metrics::DEFAULT_SELECTOR_THRESHOLD`].
    selector_threshold: u64,
    /// Cap on the number of candidate files examined per run. Resolved from
    /// settings; defaults to [`code_metrics::DEFAULT_CANDIDATE_CAP`].
    candidate_cap: usize,
    /// Override for the directory the per-invocation sandbox settings file is
    /// written to. `None` (production) means `std::env::temp_dir()`. Tests
    /// pass a per-test TempDir.
    settings_dir: Option<PathBuf>,
    /// Test-only injected `submit_findings` submission (a57).
    #[cfg(test)]
    test_submission: Option<Option<serde_json::Value>>,
}

impl ArchitectureAdvisorAudit {
    pub const TYPE: &'static str = "architecture_advisor";

    pub fn new(
        audit_settings: &HashMap<String, AuditSettings>,
        executor: &ExecutorConfig,
    ) -> Self {
        let settings = audit_settings.get(Self::TYPE).cloned().unwrap_or_default();
        let selector_threshold = settings
            .extra
            .get(SETTINGS_KEY_SELECTOR_THRESHOLD)
            .and_then(|v| v.as_u64())
            .unwrap_or(code_metrics::DEFAULT_SELECTOR_THRESHOLD);
        let candidate_cap = settings
            .extra
            .get(SETTINGS_KEY_CANDIDATE_CAP)
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(code_metrics::DEFAULT_CANDIDATE_CAP);
        let sandbox = ResolvedSandbox::resolve(executor.sandbox.as_ref());
        Self {
            settings,
            executor_command: executor.command.clone(),
            executor_timeout_secs: executor.timeout_secs,
            sandbox,
            selector_threshold,
            candidate_cap,
            settings_dir: None,
            #[cfg(test)]
            test_submission: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_settings_dir(mut self, dir: PathBuf) -> Self {
        self.settings_dir = Some(dir);
        self
    }

    /// Test-only override standing in for the agent's `submit_findings`
    /// submission. `Some(payload)` → consumed as the result; `None` → the
    /// audit observes "no submission".
    #[cfg(test)]
    pub(crate) fn with_submission(mut self, submission: Option<serde_json::Value>) -> Self {
        self.test_submission = Some(submission);
        self
    }

    /// Drain the agent's `submit_findings` submission (a57).
    async fn consume_submission(&self, workspace: &Path) -> Option<serde_json::Value> {
        #[cfg(test)]
        if let Some(over) = &self.test_submission {
            return over.clone();
        }
        super::try_consume_submission(workspace, Self::TYPE).await
    }

    /// Resolve the advisor prompt via the uniform [`PromptLoader`].
    /// `settings.prompt_path` is the audit's nested override
    /// (`audits.settings.architecture_advisor.prompt_path`); missing/empty
    /// values fall through to the embedded default.
    fn resolve_prompt(&self, workspace: Option<&Path>) -> String {
        PromptLoader::load(
            PromptId::AuditArchitectureAdvisor,
            self.settings.prompt_path.as_deref(),
            None,
            workspace,
        )
    }
}

/// Append the selected candidate list to the base prompt so the agent knows
/// exactly which files the selector surfaced. The line counts are shown as
/// selection context — NOT as findings the agent should echo back.
fn compose_prompt_with_candidates(base: &str, candidates: &[code_metrics::CandidateFile]) -> String {
    let mut out = String::with_capacity(base.len() + 64 * candidates.len());
    out.push_str(base);
    out.push_str(
        "\n\n## Candidate files selected for this run\n\n\
         The selector picked the following files by whole-file line count (the \
         longest over the pain threshold). The line count is why each file was \
         selected for examination — it is NOT itself a finding, and you MUST NOT \
         emit \"this file is N lines\" as a recommendation. Read each file (and the \
         surrounding context needed to judge cohesion and placement) and decide, \
         by cohesion, whether it warrants refactoring:\n\n",
    );
    for c in candidates {
        out.push_str(&format!("- `{}` ({} lines)\n", c.rel_path, c.lines));
    }
    out
}

/// Render the audit-run log line describing the examined candidate set.
fn examined_summary(candidates: &[code_metrics::CandidateFile]) -> String {
    if candidates.is_empty() {
        return "(none — no files exceeded the selector threshold)".to_string();
    }
    candidates
        .iter()
        .map(|c| format!("- {} ({} lines)", c.rel_path, c.lines))
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl Audit for ArchitectureAdvisorAudit {
    fn audit_type(&self) -> &'static str {
        Self::TYPE
    }

    fn description(&self) -> &'static str {
        "advisory refactor recommendations for the worst-offending files (LLM-driven)"
    }

    fn requires_head_change(&self) -> bool {
        true
    }

    fn write_policy(&self) -> WritePolicy {
        WritePolicy::None
    }

    async fn run(&self, ctx: &mut AuditContext<'_>) -> Result<AuditOutcome> {
        // Workspace-validity gate (see `audits-require-valid-workspace`).
        if !workspace_is_valid(ctx.workspace) {
            return Ok(workspace_unavailable_outcome(
                Self::TYPE,
                ctx.workspace,
                &ctx.repo.url,
            ));
        }

        // SELECTOR: pick the longest files over the pain threshold, capped.
        // The line count chooses where judgment points; it is never a finding.
        let candidates = code_metrics::select_candidate_files(
            ctx.workspace,
            self.selector_threshold,
            self.candidate_cap,
        );

        let _ = ctx.log_writer.write_section(
            "architecture_advisor_preamble",
            &format!(
                "executor_command: {}\ntimeout_secs: {}\nselector_threshold: {}\ncandidate_cap: {}\nprompt_source: {}\nallowed_tools: {}",
                self.executor_command,
                self.executor_timeout_secs,
                self.selector_threshold,
                self.candidate_cap,
                self.settings
                    .prompt_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<embedded default>".to_string()),
                ALLOWED_TOOLS.join(","),
            ),
        );
        let _ = ctx.log_writer.write_section(
            "architecture_advisor_examined",
            &examined_summary(&candidates),
        );

        // No candidates over the threshold → nothing to judge. Return an
        // evidenced clean run WITHOUT spending a CLI invocation.
        if candidates.is_empty() {
            let _ = ctx.log_writer.write_section(
                "architecture_advisor_outcome",
                "kind: Reported\nfindings_count: 0\nconclusion: no candidate files over selector threshold; no recommendation",
            );
            return Ok(AuditOutcome::reported(vec![]));
        }

        let base_prompt = self.resolve_prompt(Some(ctx.workspace));
        let prompt = compose_prompt_with_candidates(&base_prompt, &candidates);

        let mut sandbox = self.sandbox.clone();
        sandbox.allowed_tools = ALLOWED_TOOLS.iter().map(|s| (*s).to_string()).collect();

        let _ = ctx
            .log_writer
            .write_section("architecture_advisor_prompt", &prompt);

        // a57: run WITH MCP enabled; recommendations arrive via
        // `submit_findings`, not stdout.
        let model = super::audit_resolved_model(&self.settings);
        let outcome = super::run_audit_cli_with_submit(
            &self.executor_command,
            &sandbox,
            ctx.workspace,
            &prompt,
            Duration::from_secs(self.executor_timeout_secs),
            self.settings_dir.as_deref(),
            Self::TYPE,
            model.as_ref(),
            // WritePolicy::None → read-only mount + write-deny.
            self.write_policy().workspace_writable(),
        )
        .await
        .context("spawning architecture-advisor CLI subprocess")?;

        let _ = ctx.log_writer.write_section(
            "architecture_advisor_stdout",
            if outcome.stdout.is_empty() {
                "(empty)"
            } else {
                outcome.stdout.as_str()
            },
        );
        let _ = ctx.log_writer.write_section(
            "architecture_advisor_stderr",
            if outcome.stderr.is_empty() {
                "(empty)"
            } else {
                outcome.stderr.as_str()
            },
        );

        if let Some(err) = outcome_to_terminal_err(
            &outcome,
            &mut ctx.log_writer,
            Self::TYPE,
            self.executor_timeout_secs,
        ) {
            return Err(err);
        }

        // Drain the agent's `submit_findings` submission. No stored submission
        // is an audit failure (retried next iteration).
        let Some(payload) = self.consume_submission(ctx.workspace).await else {
            let _ = ctx.log_writer.write_section(
                "architecture_advisor_outcome",
                "kind: Err\nreason: no submit_findings submission recorded",
            );
            return Err(anyhow!(
                "architecture_advisor: agent exited with no submit_findings submission; stderr excerpt: {}",
                excerpt(&outcome.stderr)
            ));
        };
        let findings = match payload_to_findings(&payload) {
            Ok(f) => f,
            Err(e) => {
                let _ = ctx.log_writer.write_section(
                    "architecture_advisor_outcome",
                    &format!("kind: Err\nreason: {e}"),
                );
                return Err(anyhow!("architecture_advisor: {e}"));
            }
        };
        let conclusion = if findings.is_empty() {
            "conclusion: examined candidates; no refactor recommended"
        } else {
            "conclusion: refactor recommendation(s) submitted"
        };
        let _ = ctx.log_writer.write_section(
            "architecture_advisor_outcome",
            &format!(
                "kind: Reported\nfindings_count: {}\n{conclusion}",
                findings.len()
            ),
        );
        // Advisory audit (`Reported`) — no proposal directory, so the
        // post-write validate/retry loop AND the `🔍 created proposal`
        // notification do NOT apply. `retries_used` is always 0.
        Ok(AuditOutcome::reported(findings))
    }
}

/// Deserialize a `submit_findings` payload (`{ "findings": [...] }`) into
/// [`Finding`]s (a57). Rejects more than `MAX_FINDINGS` entries — the
/// registered `record_submission` validator (this function with its `Ok`
/// value discarded) surfaces the rejection to the agent as a correctable
/// tool error rather than silently truncating. Returns `Err(reason)` (a
/// correction-suitable string) on any malformed payload.
pub(crate) fn payload_to_findings(
    payload: &serde_json::Value,
) -> std::result::Result<Vec<Finding>, String> {
    let arr = payload
        .get("findings")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            "architecture_advisor: submission missing top-level `findings` array".to_string()
        })?;
    if arr.len() > MAX_FINDINGS {
        return Err(format!(
            "architecture_advisor: submission has {} findings; the schema caps at {MAX_FINDINGS}",
            arr.len(),
        ));
    }
    let mut findings = Vec::with_capacity(arr.len());
    for (idx, raw) in arr.iter().enumerate() {
        let entry: RawFinding = serde_json::from_value(raw.clone()).map_err(|e| {
            format!("architecture_advisor: findings[{idx}] does not match the expected shape: {e}")
        })?;
        let severity = parse_severity(&entry.severity);
        findings.push(Finding {
            severity,
            subject: entry.subject,
            body: entry.body,
            anchor: Some(entry.anchor),
        });
    }
    Ok(findings)
}

#[derive(Debug, Deserialize)]
struct RawFinding {
    subject: String,
    body: String,
    anchor: String,
    severity: String,
}

/// Parse an advisor severity. Recommendations carry `low`, `medium`, OR
/// `high` priority. An unknown / out-of-range value downgrades to `Low` with
/// a warn log so the audit succeeds rather than failing on a stylistic
/// difference.
fn parse_severity(raw: &str) -> Severity {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        other => {
            // no-url: pure severity parser, no AuditContext in scope
            tracing::warn!(
                severity = other,
                "architecture_advisor: unexpected severity `{other}`; defaulting to Low"
            );
            Severity::Low
        }
    }
}

fn excerpt(s: &str) -> String {
    let mut out: String = s.chars().take(STDERR_EXCERPT_CHARS).collect();
    if s.chars().count() > STDERR_EXCERPT_CHARS {
        out.push('…');
    }
    out
}

/// Pure transformation: given an [`crate::agentic_run::AgenticRunOutcome`],
/// return `Some(error)` if the outcome is terminal (timed out OR non-zero
/// exit). Returns `None` when the caller should continue processing.
/// Extracted from `run()` so tests can exercise the timeout/exit error
/// shapes by constructing synthetic outcome values directly.
fn outcome_to_terminal_err(
    outcome: &crate::agentic_run::AgenticRunOutcome,
    log_writer: &mut AuditLogWriter,
    audit_type: &str,
    timeout_secs: u64,
) -> Option<anyhow::Error> {
    if outcome.timed_out {
        let _ = log_writer
            .write_section(&format!("{audit_type}_outcome"), "kind: Err\nreason: timeout");
        return Some(anyhow!(
            "{audit_type}: CLI exceeded the {timeout_secs}s timeout"
        ));
    }
    if let Some(status) = outcome.exit_status
        && !status.success()
    {
        let _ = log_writer.write_section(
            &format!("{audit_type}_outcome"),
            &format!("kind: Err\nreason: exit {status}"),
        );
        return Some(anyhow!(
            "{audit_type}: CLI exited {status}; stderr excerpt: {}",
            excerpt(&outcome.stderr)
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audits::AuditLogWriter;
    use crate::config::{ExecutorKind, RepositoryConfig};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn executor_cfg(command: &str) -> ExecutorConfig {
        ExecutorConfig {
            kind: ExecutorKind::ClaudeCli,
            implementer_cli: None,
            command: command.to_string(),
            timeout_secs: 30,
            agentic_session_timeout_secs: crate::config::default_agentic_session_timeout(),
            sandbox: None,
            agent_env: None,
            implementer_prompt_path: None,
            changelog_stylist_prompt_path: None,
            perma_stuck_after_failures: None,
            max_changes_per_pr: None,
            startup_jitter_max_secs: None,
            inter_iteration_jitter_pct: None,
            max_auto_revisions_per_pr: 5,
            max_revise_triggers_per_pr: 10,
            wipe_drain_timeout_secs: crate::config::default_wipe_drain_timeout_secs(),
            output_format: crate::config::default_output_format(),
            log_retention_days: crate::config::default_log_retention_days(),
            busy_marker_stale_threshold_secs: None,
            change_internal_contradiction_check: crate::config::ContradictionCheckMode::Disabled,
            change_internal_contradiction_check_prompt_path: None,
            change_internal_contradiction_check_llm: None,
            change_canonical_contradiction_check: crate::config::ContradictionCheckMode::Disabled,
            change_canonical_contradiction_check_prompt_path: None,
            change_canonical_contradiction_check_llm: None,
            global_rules_check: crate::config::ContradictionCheckMode::Disabled,
            global_rules_check_prompt_path: None,
            global_rules_check_llm: None,
            global_rules: crate::config::GlobalRulesConfig::default(),
            code_implements_spec_check: crate::config::ContradictionCheckMode::Disabled,
            code_implements_spec_check_prompt_path: None,
            code_implements_spec_check_llm: None,
            verifier_gate_retries: crate::config::default_verifier_gate_retries(),
            revision_transcript_fetch_retries: crate::config::default_revision_transcript_fetch_retries(),
            revision_converge_attempts: crate::config::default_revision_converge_attempts(),
            session_retries: crate::config::default_executor_session_retries(),
            implementer: None,
            changelog_stylist: None,
            implementer_revision: None,
            audit_triage: None,
            chat_request_triage: None,
        }
    }

    fn fixture_repo() -> RepositoryConfig {
        RepositoryConfig {
            forge: None,
            url: "git@github.com:test/repo.git".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
            octopus_guide: None,
            sandbox: None,
        }
    }

    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn make_log_writer(workspace: &std::path::Path) -> AuditLogWriter {
        let (td, paths) = crate::testing::test_daemon_paths();
        std::mem::forget(td);
        AuditLogWriter::open(&paths, workspace, ArchitectureAdvisorAudit::TYPE)
            .expect("log writer opens")
    }

    fn settings_with(threshold: u64, cap: u64) -> HashMap<String, AuditSettings> {
        let mut extra = HashMap::new();
        extra.insert(
            SETTINGS_KEY_SELECTOR_THRESHOLD.to_string(),
            serde_yml::Value::Number(serde_yml::Number::from(threshold)),
        );
        extra.insert(
            SETTINGS_KEY_CANDIDATE_CAP.to_string(),
            serde_yml::Value::Number(serde_yml::Number::from(cap)),
        );
        let mut s = HashMap::new();
        s.insert(
            ArchitectureAdvisorAudit::TYPE.to_string(),
            AuditSettings {
                prompt_path: None,
                notify_on_clean: false,
                extra,
                ..Default::default()
            },
        );
        s
    }

    #[test]
    fn payload_round_trips_to_findings() {
        let payload = serde_json::json!({
            "findings": [
                {
                    "subject": "Split the polling loop's god-file",
                    "body": "polling_loop/mod.rs mixes queue walking, PR construction, and audit triage.",
                    "anchor": "src/polling_loop/mod.rs:1-1200",
                    "severity": "high"
                },
                {
                    "subject": "Extract the config validator",
                    "body": "config.rs carries both parsing and validation; the validator is a cohesive unit.",
                    "anchor": "src/config.rs:2800-3000",
                    "severity": "medium"
                }
            ]
        });
        let findings = payload_to_findings(&payload).expect("deserializes");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].anchor.as_deref(), Some("src/polling_loop/mod.rs:1-1200"));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn empty_findings_array_deserializes_to_no_findings() {
        let payload = serde_json::json!({"findings": []});
        let findings = payload_to_findings(&payload).expect("deserializes empty array");
        assert!(findings.is_empty());
    }

    /// Findings are capped at 5; a 6-entry payload is rejected with a
    /// correction-suitable reason, a 5-entry one accepted. (Tests task 7.2.)
    #[test]
    fn six_findings_rejected_then_five_accepted() {
        let mk = |n: usize| {
            let arr: Vec<serde_json::Value> = (0..n)
                .map(|i| serde_json::json!({"subject":format!("s{i}"),"body":"b","anchor":"a:1","severity":"low"}))
                .collect();
            serde_json::json!({ "findings": arr })
        };
        let err = payload_to_findings(&mk(6)).expect_err("six findings must error");
        assert!(err.contains("caps at 5"), "got: {err}");
        let findings = payload_to_findings(&mk(5)).expect("five findings accepted");
        assert_eq!(findings.len(), 5);
        // Every finding carries an anchor (task 7.2).
        assert!(findings.iter().all(|f| f.anchor.is_some()));
    }

    #[test]
    fn finding_missing_required_field_returns_err() {
        let payload = serde_json::json!({
            "findings": [{"subject": "s", "body": "b", "severity": "low"}]
        });
        let err = payload_to_findings(&payload).expect_err("missing anchor must error");
        assert!(err.contains("findings[0]"), "got: {err}");
    }

    #[test]
    fn missing_top_level_findings_key_returns_err() {
        let payload = serde_json::json!({"results": []});
        let err = payload_to_findings(&payload).expect_err("missing key must error");
        assert!(err.contains("findings"), "got: {err}");
    }

    #[test]
    fn severity_parser_accepts_canonical_strings() {
        assert_eq!(parse_severity("low"), Severity::Low);
        assert_eq!(parse_severity("MEDIUM"), Severity::Medium);
        assert_eq!(parse_severity("high"), Severity::High);
        assert_eq!(parse_severity("bogus"), Severity::Low);
    }

    #[test]
    fn audit_type_and_policy_are_fixed() {
        let cfg = executor_cfg("/bin/true");
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg);
        assert_eq!(audit.audit_type(), "architecture_advisor");
        assert!(audit.requires_head_change());
        assert!(matches!(audit.write_policy(), WritePolicy::None));
    }

    #[test]
    fn new_reads_selector_settings() {
        let cfg = executor_cfg("/bin/true");
        let audit = ArchitectureAdvisorAudit::new(&settings_with(1234, 3), &cfg);
        assert_eq!(audit.selector_threshold, 1234);
        assert_eq!(audit.candidate_cap, 3);
    }

    #[test]
    fn new_falls_back_to_defaults_when_settings_absent() {
        let cfg = executor_cfg("claude");
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg);
        assert_eq!(audit.selector_threshold, code_metrics::DEFAULT_SELECTOR_THRESHOLD);
        assert_eq!(audit.candidate_cap, code_metrics::DEFAULT_CANDIDATE_CAP);
    }

    /// The embedded prompt is recommendation-shaped (not a question) and
    /// forbids snark / generic lecturing — anti-prompt-drift assertions.
    #[test]
    fn embedded_prompt_is_recommendation_shaped_and_grounded() {
        let cfg = executor_cfg("/bin/true");
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg);
        let prompt = audit.resolve_prompt(None);
        let lower = prompt.to_lowercase();
        assert!(lower.contains("recommend"), "prompt must frame recommendations: {prompt}");
        assert!(lower.contains("snark"), "prompt must forbid snark: {prompt}");
        assert!(
            lower.contains("anchor"),
            "prompt must require an anchor per recommendation: {prompt}"
        );
    }

    /// The candidate list is appended to the prompt with an explicit "the
    /// line count is NOT a finding" instruction.
    #[test]
    fn compose_prompt_lists_candidates_without_minting_findings() {
        let base = "BASE PROMPT";
        let cands = vec![
            code_metrics::CandidateFile { rel_path: "src/big.rs".into(), lines: 1200 },
            code_metrics::CandidateFile { rel_path: "src/med.rs".into(), lines: 800 },
        ];
        let composed = compose_prompt_with_candidates(base, &cands);
        assert!(composed.starts_with("BASE PROMPT"));
        assert!(composed.contains("src/big.rs` (1200 lines)"));
        assert!(composed.contains("src/med.rs` (800 lines)"));
        assert!(
            composed.to_lowercase().contains("not itself a finding")
                || composed.to_lowercase().contains("not a finding"),
            "must instruct that the count is not a finding: {composed}"
        );
    }

    /// No candidate file over the threshold → an evidenced clean run that
    /// returns `Reported(vec![])` WITHOUT invoking the CLI, with the examined
    /// set (none) logged. (Tests task 7.2 / D3.)
    #[tokio::test]
    async fn no_candidates_returns_reported_empty_without_cli() {
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/small.rs"), "fn a() {}\n").unwrap();
        // A command that would FAIL if spawned — proving the CLI is skipped.
        let cfg = executor_cfg("/nonexistent/should-not-run");
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg);
        let repo = fixture_repo();
        let mut ctx = AuditContext {
            workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer: make_log_writer(workspace),
            max_validation_retries: 0,
        };
        let log_path = ctx.log_writer.path().to_path_buf();
        let outcome = audit.run(&mut ctx).await.expect("clean run succeeds");
        match outcome {
            AuditOutcome::Reported { findings, .. } => assert!(findings.is_empty()),
            other => panic!("expected Reported(empty), got {other:?}"),
        }
        let log = std::fs::read_to_string(&log_path).expect("log readable");
        assert!(log.contains("architecture_advisor_examined"), "examined section: {log}");
        assert!(log.contains("no files exceeded"), "logs no-candidate conclusion: {log}");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    /// A run with candidates present + an empty injected submission returns
    /// `Reported(vec![])` AND logs the examined candidate set. (Tests task 7.2.)
    #[tokio::test]
    async fn clean_run_with_candidates_logs_examined_set() {
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        let big: String = (0..900).map(|i| format!("// line {i}\n")).collect();
        std::fs::write(workspace.join("src/big.rs"), &big).unwrap();
        let script = write_script(ws_dir.path(), "clean.sh", "#!/bin/sh\nexit 0\n");
        let cfg = executor_cfg(&script.to_string_lossy());
        let settings_dir = TempDir::new().unwrap();
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg)
            .with_settings_dir(settings_dir.path().to_path_buf())
            .with_submission(Some(serde_json::json!({"findings": []})));
        let repo = fixture_repo();
        let mut ctx = AuditContext {
            workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer: make_log_writer(workspace),
            max_validation_retries: 0,
        };
        let log_path = ctx.log_writer.path().to_path_buf();
        let outcome = audit.run(&mut ctx).await.expect("clean run succeeds");
        match outcome {
            AuditOutcome::Reported { findings, .. } => assert!(findings.is_empty()),
            other => panic!("expected Reported(empty), got {other:?}"),
        }
        let log = std::fs::read_to_string(&log_path).expect("log readable");
        assert!(log.contains("src/big.rs"), "examined set must name the candidate: {log}");
        assert!(log.contains("no refactor recommended"), "logs clean conclusion: {log}");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    /// No stored submission (when candidates exist) is an audit failure.
    #[tokio::test]
    async fn run_returns_err_when_no_submission() {
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        let big: String = (0..900).map(|i| format!("// line {i}\n")).collect();
        std::fs::write(workspace.join("src/big.rs"), &big).unwrap();
        let script = write_script(ws_dir.path(), "silent.sh", "#!/bin/sh\nexit 0\n");
        let cfg = executor_cfg(&script.to_string_lossy());
        let settings_dir = TempDir::new().unwrap();
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg)
            .with_settings_dir(settings_dir.path().to_path_buf())
            .with_submission(None);
        let repo = fixture_repo();
        let mut ctx = AuditContext {
            workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer: make_log_writer(workspace),
            max_validation_retries: 0,
        };
        let log_path = ctx.log_writer.path().to_path_buf();
        let err = audit.run(&mut ctx).await.expect_err("no submission must error");
        assert!(
            format!("{err:#}").contains("no submit_findings submission"),
            "error must name the missing submission: {err:#}"
        );
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    /// Synthesized `timed_out` outcome produces an error naming
    /// `architecture_advisor` AND `timeout`, with the log recording it.
    /// (Tests the subprocess-timeout requirement, task 5.1.)
    #[test]
    fn outcome_to_terminal_err_translates_timeout() {
        let ws_dir = TempDir::new().unwrap();
        let mut log_writer = make_log_writer(ws_dir.path());
        let log_path = log_writer.path().to_path_buf();
        let outcome = crate::agentic_run::AgenticRunOutcome {
            timed_out: true,
            exit_status: None,
            stdout: String::new(),
            stderr: "timeout".into(),
            ..Default::default()
        };
        let err = outcome_to_terminal_err(&outcome, &mut log_writer, "architecture_advisor", 1)
            .expect("timed_out must produce Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("architecture_advisor"), "names the audit: {msg}");
        assert!(msg.contains("timeout"), "mentions timeout: {msg}");
        let log = std::fs::read_to_string(&log_path).expect("log readable");
        assert!(log.contains("reason: timeout"), "log records timeout: {log}");
    }

    /// Workspace-validity gate: a nonexistent workspace returns
    /// `WorkspaceUnavailable` without creating the path.
    #[tokio::test]
    async fn workspace_unavailable_when_path_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("never-existed");
        let cfg = executor_cfg("/bin/true");
        let audit = ArchitectureAdvisorAudit::new(&HashMap::new(), &cfg);
        let repo = fixture_repo();
        let mut ctx = AuditContext {
            workspace: &workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer: make_log_writer(tmp.path()),
            max_validation_retries: 0,
        };
        let log_path = ctx.log_writer.path().to_path_buf();
        let outcome = audit.run(&mut ctx).await.expect("gate returns Ok");
        match outcome {
            AuditOutcome::WorkspaceUnavailable { audit_type, reason, .. } => {
                assert_eq!(audit_type, ArchitectureAdvisorAudit::TYPE);
                assert_eq!(reason, "workspace directory does not exist");
            }
            other => panic!("expected WorkspaceUnavailable, got {other:?}"),
        }
        assert!(!workspace.exists(), "must not create the workspace path");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }
}
