//! Agentic reviewer transport (a58): the read-only CLI reviewer session,
//! its `submit_review` payload mapping, prompt rendering, and the
//! orchestration that dispatches one or more sessions per
//! `ReviewerConfig::mode`. Extracted from `code_reviewer` behind its own
//! module boundary; the moved items are re-exported from `code_reviewer`
//! so existing call sites keep compiling.

use super::{
    CodeReviewer, ConcernEntry, ReviewConcern, ReviewContext, ReviewResult, ReviewTarget,
    Verdict, concerns_flag_security_critical, review_diff_artifact_rel,
    split_per_change_contexts, synthesize_agentic_per_change,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// The MCP role AND submission routing key the agentic reviewer uses. The
/// per-execution MCP child advertises `submit_review` ONLY when
/// `ORCH_MCP_ROLE` equals this value; the daemon-side schema validator is
/// registered under the same key.
pub const REVIEWER_ROLE: &str = "reviewer";

/// Read-only CLI tool permissions for the agentic reviewer sandbox. NO
/// `Bash`, NO `Write`, NO `Edit` — the reviewer reads files on demand AND
/// returns its verdict through the `submit_review` MCP tool.
pub const AGENTIC_REVIEW_ALLOWED_TOOLS: &[&str] = &["Read", "Glob", "Grep"];

/// The full `--allowedTools` list the agentic reviewer sandbox grants:
/// the read-only file tools PLUS the qualified `submit_review` MCP tool.
/// Notably absent: `Bash`, `Write`, `Edit`. Exposed so tests can assert
/// the advertised surface (task 4.2).
pub fn agentic_review_allowed_tools() -> Vec<String> {
    let mut tools: Vec<String> = AGENTIC_REVIEW_ALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if let Some(t) = crate::mcp_askuser_server::submission_tool_name_for_role(REVIEWER_ROLE) {
        tools.push(crate::mcp_askuser_server::qualified_tool_name(t));
    }
    tools
}

/// One reviewer submission concern, as it arrives in the `submit_review`
/// payload. The daemon-side validator [`payload_to_review_result`]
/// deserializes the payload into this shape; a missing required field is a
/// deserialize error surfaced to the agent as a correctable tool error.
#[derive(Debug, Clone, Deserialize)]
struct RawReviewConcern {
    title: String,
    detail: String,
    anchor: String,
    should_request_revision: bool,
    #[serde(default)]
    actionable_request: Option<String>,
    /// The reviewer's own security signal (a004): `true` when this finding
    /// is a credential/secret/key exposure or an injection vulnerability.
    /// Drives the verdict-escalation safety net the same way the oneshot
    /// `revision-requests` block's `security_critical` does. Defaults to
    /// `false` when the reviewer omits it.
    #[serde(default)]
    security_critical: bool,
}

/// The `submit_review` payload shape.
#[derive(Debug, Clone, Deserialize)]
struct RawReviewSubmission {
    verdict: String,
    summary: String,
    #[serde(default)]
    concerns: Vec<RawReviewConcern>,
}

/// Validate AND map a consumed `submit_review` payload into a
/// [`ReviewResult`] (a58). This is BOTH the daemon-side schema validator
/// (registered via [`register_reviewer_submission_schema`] with its `Ok`
/// value discarded) AND the consume-time mapper — so a payload that
/// records successfully is exactly one that maps, and the two can never
/// drift (mirrors the advisory audits' `payload_to_findings`).
///
/// Returns `Err(reason)` (a correction-suitable string) when the verdict
/// is outside `{Approve, Block}`, when a concern sets
/// `should_request_revision: true` without a non-empty `actionable_request`,
/// OR when the payload does not match the expected shape. `record_submission`
/// surfaces the reason to the agent as a correctable tool error.
///
/// On success the `raw_output` AND `markdown` are the rendered summary +
/// concerns markdown used for the PR-body `## Code Review` block;
/// `attribution` is left `None` for the caller to stamp from the reviewer's
/// configured model.
pub(crate) fn payload_to_review_result(
    payload: &Value,
) -> std::result::Result<ReviewResult, String> {
    let sub: RawReviewSubmission = serde_json::from_value(payload.clone()).map_err(|e| {
        format!("submit_review: payload does not match the expected shape: {e}")
    })?;
    let verdict = match sub.verdict.as_str() {
        "Approve" => Verdict::Approve,
        "Block" => Verdict::Block,
        other => {
            return Err(format!(
                "submit_review: verdict must be one of Approve | Block; got `{other}`"
            ));
        }
    };
    let mut concerns: Vec<ReviewConcern> = Vec::with_capacity(sub.concerns.len());
    for (idx, c) in sub.concerns.iter().enumerate() {
        if c.should_request_revision {
            let has_request = c
                .actionable_request
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_request {
                return Err(format!(
                    "submit_review: concerns[{idx}] (`{title}`) sets should_request_revision: true \
                     but has an empty actionable_request; provide the concrete revision instruction",
                    title = c.title
                ));
            }
        }
        concerns.push(ReviewConcern {
            summary: c.title.clone(),
            actionable_request: c.actionable_request.clone(),
            should_request_revision: c.should_request_revision,
            change_slug: None,
            security_critical: c.security_critical,
        });
    }
    // a004 safety net (agentic path): a payload flagging a credential/secret/
    // key exposure or injection via its own `security_critical` finding signal
    // but returning a non-`Block` verdict is escalated to `Block` before the
    // result reaches the PR-draft / auto-revise handling. Keyed on the
    // structured signal, never on the prose of the finding.
    let verdict = if verdict != Verdict::Block && concerns_flag_security_critical(&concerns) {
        Verdict::Block
    } else {
        verdict
    };
    let raw_output = render_review_submission_markdown(&sub.summary, &sub.concerns);
    let per_concern = concerns.iter().map(ConcernEntry::from).collect();
    Ok(ReviewResult {
        verdict,
        per_concern,
        raw_output: raw_output.clone(),
        markdown: raw_output,
        per_change_sections: Vec::new(),
        concerns,
        attribution: None,
    })
}

/// Render a `submit_review` payload's summary + concerns into the markdown
/// body the PR-body `## Code Review` block carries. Wording is not
/// asserted by tests (per the project-documentation requirement "Tests
/// assert behavior or derivation, never message wording").
fn render_review_submission_markdown(summary: &str, concerns: &[RawReviewConcern]) -> String {
    let mut out = String::new();
    if !summary.trim().is_empty() {
        out.push_str(summary.trim_end());
    }
    if !concerns.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("## Concerns");
        for c in concerns {
            out.push_str(&format!("\n\n- **{}**", c.title));
            if !c.anchor.trim().is_empty() {
                out.push_str(&format!(" ({})", c.anchor.trim()));
            }
            if !c.detail.trim().is_empty() {
                out.push_str(&format!("\n  {}", c.detail.trim()));
            }
            if c.should_request_revision
                && let Some(req) = c.actionable_request.as_deref()
                && !req.trim().is_empty()
            {
                out.push_str(&format!("\n  - Requested revision: {}", req.trim()));
            }
        }
    }
    if out.is_empty() {
        out.push_str("(no concerns)");
    }
    out
}

/// Render the agentic reviewer's prompt from a [`ReviewContext`] (a58):
/// the change briefs, the changed-file PATH list (NOT full contents), AND a
/// reference to the unified-diff artifact at `diff_artifact_rel` (NOT the
/// inlined diff). The agent reads whatever files AND diff it needs on demand
/// via `Read`, so the prompt stays bounded regardless of diff size,
/// `reviewer.prompt_budget_chars` is NOT consulted here, AND no
/// `## Skipped (budget exhausted)` truncation occurs.
///
/// When `ctx.target` is `Some` (a59 on-demand TARGET review) the prompt
/// carries the operator's review focus AND the target file-path list IN
/// PLACE OF a diff — there is no unified diff for a target. Everything else
/// (briefs, reads-on-demand, `submit_review`) is identical to the diff-based
/// case, so the same `ReviewResult` shape is produced either way.
pub fn render_agentic_review_prompt(
    ctx: &ReviewContext,
    preamble: &str,
    diff_artifact_rel: &str,
) -> String {
    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(preamble.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(
        "You are reviewing code for quality (security, error handling, naming, \
         style, language idioms, obvious bugs). Do NOT assess whether the code implements \
         any spec — that is a separate concern.\n\n",
    );

    out.push_str("# Change briefs\n\n");
    if ctx.archived_changes.is_empty() {
        out.push_str("(no archived-change briefs for this pass)\n\n");
    } else {
        for brief in &ctx.archived_changes {
            out.push_str(&format!("## Change: {}\n\n", brief.name));
            out.push_str(brief.proposal.trim_end());
            if let Some(design) = brief.design.as_deref() {
                out.push_str("\n\n");
                out.push_str(design.trim_end());
            }
            out.push_str("\n\n");
            out.push_str(brief.tasks.trim_end());
            out.push_str("\n\n");
        }
    }

    match &ctx.target {
        // a59: TARGET review surface — no diff. Carry the operator's focus
        // AND the target file-path list in place of a diff. For a
        // `Description` target the file list is empty; the agent locates the
        // files itself via Glob/Grep AND must name the files it reviewed.
        Some(target) => {
            out.push_str("# Review focus\n\n");
            out.push_str(target.focus_text().trim_end());
            out.push_str("\n\n");

            out.push_str("# Target files\n\n");
            match target {
                ReviewTarget::Files { paths } => {
                    out.push_str(
                        "Review the CURRENT content of these files (there is NO diff for an \
                         on-demand target review). Their contents are NOT inlined — use the \
                         `Read`, `Glob`, AND `Grep` tools to read whatever you need on \
                         demand.\n\n",
                    );
                    if paths.is_empty() {
                        out.push_str("(no target files named)\n\n");
                    } else {
                        for p in paths {
                            out.push_str(&format!("- {p}\n"));
                        }
                        out.push('\n');
                    }
                }
                ReviewTarget::Description { .. } => {
                    out.push_str(
                        "No file list was provided — LOCATE the files relevant to the review \
                         focus yourself using `Glob` AND `Grep`, then `Read` them. There is NO \
                         diff for an on-demand target review. In your `submit_review` summary, \
                         NAME the files you actually reviewed so the operator can see the scope \
                         you chose.\n\n",
                    );
                }
            }
        }
        // Diff-based review (the per-pass review, OR an on-demand PR/commit):
        // unchanged rendering.
        None => {
            out.push_str("# Changed files\n\n");
            out.push_str(
                "These files were modified by this pass. Their full contents are NOT inlined — use \
                 the `Read`, `Glob`, AND `Grep` tools to read whatever you need on demand.\n\n",
            );
            if ctx.changed_files.is_empty() {
                out.push_str("(no changed files reported)\n");
            } else {
                for f in &ctx.changed_files {
                    out.push_str(&format!("- {}\n", f.path));
                }
            }
            out.push('\n');

            out.push_str("# Unified diff\n\n");
            if ctx.diff.trim().is_empty() {
                out.push_str("(no diff produced this pass)\n\n");
            } else {
                out.push_str(&format!(
                    "The unified diff for this pass is written to `{diff_artifact_rel}` — it is NOT \
                     inlined here, so this prompt stays bounded regardless of how large the diff is. \
                     `Read` that file to see exactly what changed; for any file you need in full, \
                     `Read` it directly from the changed-file list above. Prioritize the hunks most \
                     relevant to a code-quality review.\n\n"
                ));
            }
        }
    }

    out.push_str(
        "Security-critical findings are always Block. Credential or secret leakage (a key, \
         token, or secret written where it could be committed or otherwise exposed), hardcoded \
         secrets, AND injection vulnerabilities (SQL, command, path) are stop-the-line: return \
         `Block`, never a soft verdict, AND set `security_critical: true` on that concern. The \
         daemon escalates the verdict to `Block` from the `security_critical` signal even if you \
         returned `Approve`.\n\n",
    );

    out.push_str(
        "When your analysis is complete, call the `submit_review` MCP tool exactly once with \
         your verdict (Approve | Block), a summary, AND any concerns. Each concern that should \
         drive a revision MUST set `should_request_revision: true` with a non-empty \
         `actionable_request`. Mark any credential/secret/key-exposure or injection finding with \
         `security_critical: true`. Do NOT print the verdict to stdout — the daemon reads it ONLY \
         from `submit_review`.\n",
    );
    out
}

/// Outcome of an agentic review pass (a58). `Reviewed` carries the
/// schema-validated [`ReviewResult`]; `Discarded` means a session ended
/// with no valid `submit_review` submission, so the caller writes NO
/// verdict (it does NOT default to `Approve`) AND posts the reviewer-
/// failure operator alert.
#[derive(Debug, Clone)]
pub enum AgenticReviewOutcome {
    Reviewed(ReviewResult),
    Discarded { reason: String },
}

/// One reviewer session's result (executor-outcome-legibility-and-retry): the
/// consumed `submit_review` payload (`None` on a no-submission discard) PLUS
/// the assembled, truncated diagnostic from the session's captured output —
/// the agent's final message / stderr / exit-signal, raw — surfaced in the
/// discard reason on the `None` arm so the operator can tell WHY the session
/// failed to submit (an upstream-API overload notice, prose emitted instead of
/// a tool call, a schema-rejected submission, etc.). The diagnostic is empty
/// when a submission was consumed (the `Some` arm ignores it).
pub(crate) struct ReviewSessionOutput {
    pub submission: Option<Value>,
    /// Assembled from the session's captured `final_answer`/`stdout`/`stderr`/
    /// exit-signal via [`crate::agentic_run::failure_reason`], truncated, raw.
    /// Names the persisted per-session log path when present so the full output
    /// is recoverable from disk.
    pub no_submission_diagnostic: String,
}

/// Abstracts "run ONE agentic reviewer session AND drain its submission"
/// so the orchestration ([`run_agentic_review_with_runner`]) is unit-
/// testable without spawning a CLI. Production is
/// [`CliReviewSessionRunner`]; tests inject canned submissions.
#[async_trait]
pub(crate) trait ReviewSessionRunner: Send + Sync {
    /// Run one session against `prompt` AND return its [`ReviewSessionOutput`]:
    /// the consumed `submit_review` payload (`None` when the agent recorded no
    /// valid submission) plus the no-submission diagnostic. `slug` labels the
    /// session (empty for bundled). `diff` is the session's unified diff, which
    /// the runner writes to the read-only-readable artifact the prompt
    /// references (and removes after).
    async fn run_session(&self, slug: &str, prompt: &str, diff: &str)
    -> Result<ReviewSessionOutput>;
}

/// Production session runner: writes the per-execution MCP config
/// (`ORCH_MCP_ROLE = reviewer`), runs the wrapped CLI through
/// [`crate::agentic_run::agentic_run`] in a read-only capture sandbox, AND
/// drains the stored submission via the control socket. Mirrors the
/// advisory audits' `run_audit_cli_with_submit` + `try_consume_submission`.
pub(crate) struct CliReviewSessionRunner<'a> {
    pub(crate) workspace: &'a Path,
    pub(crate) strategy: &'a dyn crate::agentic_run::CliStrategy,
    /// The reviewer's resolved CLI, so the OS sandbox admits THIS CLI's own
    /// credential store (and binds its binary) instead of masking it as a
    /// foreign CLI's. Must match `strategy`'s CLI.
    pub(crate) cli: crate::config::CliKind,
    pub(crate) settings_dir: Option<&'a Path>,
    pub(crate) timeout: Duration,
    /// The reviewer's fully-resolved model, passed to `agentic_run` so the
    /// wrapped CLI runs the operator-configured model (not the CLI's own
    /// default). `None` only on the test-only path where no config was
    /// resolved; production always carries `Some`.
    pub(crate) model: Option<&'a crate::agentic_run::ResolvedModel>,
    /// Daemon paths for the per-session reviewer log under `reviews/`
    /// (executor-outcome-legibility-and-retry). `None` on the test-only path
    /// (no `DaemonPaths` resolved) → the log is skipped; production threads it
    /// from the reviewer's stored paths.
    pub(crate) paths: Option<&'a std::sync::Arc<crate::paths::DaemonPaths>>,
}

#[async_trait]
impl ReviewSessionRunner for CliReviewSessionRunner<'_> {
    async fn run_session(
        &self,
        slug: &str,
        prompt: &str,
        diff: &str,
    ) -> Result<ReviewSessionOutput> {
        // Write the unified diff to the artifact the prompt references, so the
        // read-only agent `Read`s it on demand instead of receiving it inlined
        // (which would overflow the model's context on a large pass). The
        // artifact lives at the workspace root (a dotfile the agent can Read)
        // and is removed after the session below, regardless of outcome.
        let diff_artifact = self
            .workspace
            .join(review_diff_artifact_rel(slug));
        if let Err(e) = std::fs::write(&diff_artifact, diff) {
            tracing::warn!(
                path = %diff_artifact.display(),
                "failed to write reviewer diff artifact; the agent will see an empty diff file: {e}"
            );
        }

        // Write the per-execution MCP config advertising `submit_review`.
        // `change == REVIEWER_ROLE` keys the submission-store entry; this
        // runner consumes the same key after exit.
        crate::executor::claude_cli::ClaudeCliExecutor::write_mcp_config(
            self.workspace,
            REVIEWER_ROLE,
            Some(REVIEWER_ROLE),
        )
        .context("writing reviewer MCP config")?;

        // a70: a single-shot role — prune the session it creates on completion.
        let result = crate::agentic_run::agentic_run_with_session(
            crate::agentic_run::AgenticRunOpts {
            workspace: self.workspace,
            change: REVIEWER_ROLE,
            strategy: self.strategy,
            prompt,
            sandbox: crate::agentic_run::SandboxConfig {
                allowed_tools: agentic_review_allowed_tools(),
                disallowed_bash_patterns: Vec::new(),
                disallowed_read_paths: Vec::new(),
                deny_writes: true,
            },
            // a006-followup: pass the reviewer's RESOLVED model so the wrapped
            // CLI runs the operator-configured model — NOT the CLI's own
            // default (the bug). The strategy handles it per-CLI: a keyless
            // `openai_compatible` reviewer (empty api_key) → NO opencode
            // provider block + verbatim `--model <m.model>`; a keyed one or
            // Ollama → provider block + `--model <provider>/<model>`; a `claude`
            // reviewer → `ANTHROPIC_MODEL`/`ANTHROPIC_BASE_URL`. `None` only on
            // the test-only no-config path (CLI falls back to its default).
            model: self.model,
            output_mode: crate::agentic_run::OutputMode::Capture,
            timeout: self.timeout,
            paths: None,
            settings_dir: self.settings_dir,
            include_autocoder_tools: true,
            emit_stream_json_in_capture: false,
            resume_session_id: None,
            track_subprocess_marker: false,
            etxtbsy_retry_spawn: true,
            // a006: the agentic reviewer is a read-only role — read-only
            // workspace. The OS sandbox MUST match the reviewer's actual CLI so
            // the role's OWN credential store is admitted (not masked as a
            // foreign CLI's) AND its binary is bound. Previously hardcoded to
            // `Claude`, which masked an `opencode`/`agy` reviewer's own store →
            // the CLI could not authenticate → "no valid submit_review submission".
            os_sandbox: crate::sandbox::current_run_sandbox(self.cli, false),
            },
            true,
            None,
        )
        .await;

        // Always remove the config AND the diff artifact we wrote, regardless
        // of run outcome — the run leaves no reviewer litter in the workspace.
        crate::executor::claude_cli::ClaudeCliExecutor::delete_mcp_config(self.workspace);
        if let Err(e) = std::fs::remove_file(&diff_artifact) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %diff_artifact.display(),
                    "failed to remove reviewer diff artifact: {e}"
                );
            }
        }

        let outcome = result.context("spawning agentic reviewer subprocess")?;

        // Persist the session's FULL captured output to a discoverable per-
        // session log under `reviews/`, mirroring the audit-log pattern,
        // REGARDLESS of outcome (a recorded submission, a no-submission discard,
        // OR a timeout) — so the full output is recoverable from disk when the
        // surfaced reason is truncated. Best-effort: a log-write failure is
        // logged, never fatal.
        let log_path = self.paths.and_then(|paths| {
            match crate::audits::persist_reviewer_session_log(paths, self.workspace, slug, &outcome)
            {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(
                        slug = %slug,
                        "failed to persist reviewer session log (continuing): {e:#}"
                    );
                    None
                }
            }
        });

        // A timed-out session keeps its distinct timeout reason rather than the
        // assembled no-submission diagnostic.
        if outcome.timed_out {
            return Err(anyhow!(
                "agentic reviewer session timed out after {}s",
                self.timeout.as_secs()
            ));
        }

        let submission = crate::audits::try_consume_submission(self.workspace, REVIEWER_ROLE).await;
        // On a no-submission outcome, assemble the captured-evidence diagnostic
        // (final message / stderr / exit-signal, raw, truncated) so the discard
        // reason surfaces WHY the session failed to submit. Name the per-session
        // log path so the full (untruncated) output is recoverable from disk.
        let no_submission_diagnostic = if submission.is_none() {
            let mut diag = crate::agentic_run::failure_reason(
                &outcome,
                crate::agentic_run::FAILURE_REASON_MAX,
            );
            if let Some(path) = &log_path {
                diag.push_str(&format!(" (full session log: {})", path.display()));
            }
            diag
        } else {
            String::new()
        };
        Ok(ReviewSessionOutput {
            submission,
            no_submission_diagnostic,
        })
    }
}

/// Append the captured-session diagnostic to a no-submission discard `base`
/// reason (executor-outcome-legibility-and-retry). Empty diagnostic (a
/// canned-test runner, or no captured output) leaves the base reason unchanged,
/// so the existing "recorded no valid submit_review submission" wording is
/// preserved; a non-empty diagnostic is appended so the operator sees WHY.
pub(crate) fn append_diagnostic(base: String, diagnostic: &str) -> String {
    if diagnostic.trim().is_empty() {
        base
    } else {
        format!("{base}: {diagnostic}")
    }
}

/// Resolve the agentic reviewer's CLI strategy from its provider via the
/// a55/a56 `provider → CLI` rule. Anthropic → the `claude` strategy;
/// non-Anthropic providers → the `opencode` strategy (a60). No session is
/// spawned at resolution time. (A future provider whose CLI has no
/// registered strategy would still return a clear error here.)
pub(crate) fn resolve_reviewer_strategy(
    reviewer: &CodeReviewer,
) -> Result<Box<dyn crate::agentic_run::CliStrategy>> {
    // `reviewer.command` defaults to the `claude` binary; a non-claude reviewer
    // (opencode/agy) must spawn its OWN binary, not claude. See
    // `resolve_cli_command`.
    let cli = crate::config::default_cli_for(reviewer.provider);
    let command = crate::config::resolve_cli_command(&reviewer.command, cli);
    crate::agentic_run::strategy_for_cli(cli, command, Vec::new())
}

/// Run the agentic reviewer against `ctx` (a58). Production entry point for
/// both the polling-loop initial review AND the operator-triggered rerun
/// composer. Resolves the CLI strategy (`claude` for Anthropic, `opencode`
/// for non-Anthropic providers — a60), then dispatches one session per
/// `reviewer.mode()`. This is reached only when the reviewer's effective
/// kind is `agentic`; under a64 the startup CLI-availability check
/// ([`resolve_startup_reviewer_kind`]) has already degraded an
/// unavailable-CLI reviewer to `oneshot` for the boot, so the strategy
/// resolution here succeeds in the common case (a later availability change
/// still surfaces as `Err`, handled by the caller).
pub async fn run_agentic_review(
    reviewer: &CodeReviewer,
    ctx: &ReviewContext,
    workspace: &Path,
) -> Result<AgenticReviewOutcome> {
    let strategy = resolve_reviewer_strategy(reviewer)?;
    let runner = CliReviewSessionRunner {
        workspace,
        strategy: strategy.as_ref(),
        // Match the sandbox to the reviewer's actual CLI (provider → CLI), so an
        // opencode/agy reviewer's own store is admitted, not masked as foreign.
        cli: crate::config::default_cli_for(reviewer.provider),
        settings_dir: None,
        timeout: reviewer.agentic_session_timeout,
        // Pass the reviewer's resolved model so the wrapped CLI runs the
        // operator-configured model, mirroring the verifier gates.
        model: reviewer.resolved_model.as_ref(),
        // Daemon paths for the per-session `reviews/` log (None on the
        // test-only no-config path → the log is skipped).
        paths: reviewer.paths.as_ref(),
    };
    run_agentic_review_with_runner(reviewer, ctx, &runner).await
}

/// Mode-aware orchestration shared by production AND tests. Honors
/// `reviewer.mode()` identically to the one-shot path: `Bundled` → one
/// session for the whole `ReviewContext`; `PerChange` → one session per
/// archived change (split via [`split_per_change_contexts`]), synthesized
/// into a single [`ReviewResult`] with one `per_change_sections` entry per
/// change. A session that records no valid submission discards the WHOLE
/// review (returns `Discarded`) — it never defaults to `Approve`.
pub(crate) async fn run_agentic_review_with_runner(
    reviewer: &CodeReviewer,
    ctx: &ReviewContext,
    runner: &dyn ReviewSessionRunner,
) -> Result<AgenticReviewOutcome> {
    // Build the per-session work list. Bundled is always exactly one
    // session even when the pass has zero archived changes.
    //
    // a015: an empty per-change split (no archived-change briefs resolved
    // for this PR — e.g. a PR opened under one daemon build and re-reviewed
    // under another) must NEVER reach `synthesize_agentic_per_change` with
    // zero reviews: that initializer defaults to `Approve`, so the loop
    // running zero sessions would produce a blank `Approve` the reviewer
    // never performed — the exact silent-approval bug the one-shot
    // `review_pr_at_state_with` path fixes. Mirror that fix here: fall back
    // to a single BUNDLED session so the PR's diff and changed files still
    // reach the reviewer and the verdict comes from an actual invocation.
    // The `bundled` flag then also routes synthesis below through the
    // bundled arm (no empty per-change synthesis).
    let mut bundled = matches!(reviewer.mode(), crate::config::ReviewerMode::Bundled);
    let sessions: Vec<(Option<String>, ReviewContext, String)> = if bundled {
        vec![(None, ctx.clone(), String::new())]
    } else {
        let per_change = split_per_change_contexts(ctx);
        if per_change.is_empty() {
            bundled = true;
            vec![(None, ctx.clone(), String::new())]
        } else {
            per_change
                .into_iter()
                .map(|p| (Some(p.change_slug), p.context, p.cross_change_preamble))
                .collect()
        }
    };

    let mut reviews: Vec<(Option<String>, ReviewResult)> = Vec::with_capacity(sessions.len());
    for (slug, session_ctx, preamble) in &sessions {
        let session_slug = slug.as_deref().unwrap_or("");
        let artifact_rel = review_diff_artifact_rel(session_slug);
        let prompt = render_agentic_review_prompt(session_ctx, preamble, &artifact_rel);
        let session = runner
            .run_session(session_slug, &prompt, &session_ctx.diff)
            .await?;
        match session.submission {
            None => {
                let base = match slug {
                    Some(s) => format!(
                        "agentic reviewer session for `{s}` recorded no valid submit_review submission"
                    ),
                    None => "agentic reviewer session recorded no valid submit_review submission"
                        .to_string(),
                };
                // Surface the captured session output (assembled, truncated,
                // raw) so the discard is diagnosable rather than a bare "no
                // submission" — the same surface-the-evidence principle as the
                // executor failure reason.
                let reason = append_diagnostic(base, &session.no_submission_diagnostic);
                return Ok(AgenticReviewOutcome::Discarded { reason });
            }
            Some(payload) => {
                // The payload already passed `record_submission`'s validator,
                // so this re-map cannot drift; a failure here is an internal
                // invariant violation.
                let result = payload_to_review_result(&payload).map_err(|e| {
                    anyhow!("recorded submit_review payload failed re-validation: {e}")
                })?;
                reviews.push((slug.clone(), result));
            }
        }
    }

    // a015: synthesize through the SAME `bundled` flag the session list was
    // built with, so an empty-split fallback (bundled = true above) takes
    // the single-review bundled arm instead of synthesizing per-change from
    // an effectively empty set.
    let outcome = if bundled {
        let mut result = reviews
            .pop()
            .map(|(_, r)| r)
            .expect("bundled mode always runs exactly one session");
        result.attribution = reviewer.attribution.clone();
        result
    } else {
        synthesize_agentic_per_change(reviews, reviewer.attribution.clone())
    };
    Ok(AgenticReviewOutcome::Reviewed(outcome))
}
