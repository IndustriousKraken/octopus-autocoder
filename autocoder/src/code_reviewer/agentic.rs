//! Agentic reviewer transport (a58).
//!
//! The self-contained agentic-review path: the MCP role/tooling consts, the
//! `submit_review` payload shapes and their mapping to a [`ReviewResult`], the
//! prompt renderer, the [`ReviewSessionRunner`] abstraction and its production
//! CLI implementation, the bundled / per-change orchestration, the
//! startup-kind resolution, and the submission-schema registration. Extracted
//! from the parent `code_reviewer` module so the reviewer file keeps a single
//! responsibility; the parent re-exports the items external callers reference.
//!
//! The parent module's types (`ReviewResult`, `ReviewContext`, `ReviewConcern`,
//! …) and helpers (`split_per_change_contexts`, `concerns_flag_security_critical`,
//! …) are reached via the `use super::*` glob below.
use super::*;

// =====================================================================
// Agentic reviewer transport (a58)
// =====================================================================

/// The MCP role AND submission routing key the agentic reviewer uses. The
/// per-execution MCP child advertises `submit_review` ONLY when
/// `ORCH_MCP_ROLE` equals this value; the daemon-side schema validator is
/// registered under the same key.
pub const REVIEWER_ROLE: &str = "reviewer";

/// Read-only CLI tool permissions for the agentic reviewer sandbox. NO
/// `Bash`, NO `Write`, NO `Edit` — the reviewer reads files on demand AND
/// returns its verdict through the `submit_review` MCP tool.
pub const AGENTIC_REVIEW_ALLOWED_TOOLS: &[&str] = &["Read", "Glob", "Grep"];

/// Wall-clock cap for one agentic reviewer session. The oneshot path has
/// no analogous timeout (the HTTP client owns it); this bounds the wrapped
/// CLI subprocess the way the audits bound theirs.
const AGENTIC_REVIEW_TIMEOUT: Duration = Duration::from_secs(900);

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
/// the change briefs, the changed-file PATH list (NOT full contents), AND
/// the unified diff. The agent reads whatever files it needs on demand via
/// `Read`, so `reviewer.prompt_budget_chars` is NOT consulted here AND no
/// `## Skipped (budget exhausted)` truncation occurs.
pub fn render_agentic_review_prompt(ctx: &ReviewContext, preamble: &str) -> String {
    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(preamble.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(
        "You are reviewing a code change for quality (security, error handling, naming, \
         style, language idioms, obvious bugs). Do NOT assess whether the diff implements \
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
        out.push_str("```diff\n");
        out.push_str(&ctx.diff);
        if !ctx.diff.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
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

/// Abstracts "run ONE agentic reviewer session AND drain its submission"
/// so the orchestration ([`run_agentic_review_with_runner`]) is unit-
/// testable without spawning a CLI. Production is
/// [`CliReviewSessionRunner`]; tests inject canned submissions.
#[async_trait]
pub(crate) trait ReviewSessionRunner: Send + Sync {
    /// Run one session against `prompt` AND return the consumed
    /// `submit_review` payload, or `None` when the agent recorded no valid
    /// submission. `slug` labels the session (empty for bundled).
    async fn run_session(&self, slug: &str, prompt: &str) -> Result<Option<Value>>;
}

/// Production session runner: writes the per-execution MCP config
/// (`ORCH_MCP_ROLE = reviewer`), runs the wrapped CLI through
/// [`crate::agentic_run::agentic_run`] in a read-only capture sandbox, AND
/// drains the stored submission via the control socket. Mirrors the
/// advisory audits' `run_audit_cli_with_submit` + `try_consume_submission`.
struct CliReviewSessionRunner<'a> {
    workspace: &'a Path,
    strategy: &'a dyn crate::agentic_run::CliStrategy,
    /// The reviewer's resolved CLI, so the OS sandbox admits THIS CLI's own
    /// credential store (and binds its binary) instead of masking it as a
    /// foreign CLI's. Must match `strategy`'s CLI.
    cli: crate::config::CliKind,
    settings_dir: Option<&'a Path>,
    timeout: Duration,
    /// The reviewer's fully-resolved model, passed to `agentic_run` so the
    /// wrapped CLI runs the operator-configured model (not the CLI's own
    /// default). `None` only on the test-only path where no config was
    /// resolved; production always carries `Some`.
    model: Option<&'a crate::agentic_run::ResolvedModel>,
}

#[async_trait]
impl ReviewSessionRunner for CliReviewSessionRunner<'_> {
    async fn run_session(&self, _slug: &str, prompt: &str) -> Result<Option<Value>> {
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

        // Always remove the config we wrote, regardless of run outcome.
        crate::executor::claude_cli::ClaudeCliExecutor::delete_mcp_config(self.workspace);

        let outcome = result.context("spawning agentic reviewer subprocess")?;
        if outcome.timed_out {
            return Err(anyhow!(
                "agentic reviewer session timed out after {}s",
                self.timeout.as_secs()
            ));
        }
        Ok(crate::audits::try_consume_submission(self.workspace, REVIEWER_ROLE).await)
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

/// Whether `cli` resolves to an executable on the daemon host. An absolute
/// or path-qualified command (`/usr/local/bin/claude`, `./claude`) is tested
/// directly; a bare name (`claude`) is searched across the entries in `$PATH`.
/// No subprocess is spawned — the binary is located, not executed — so the
/// startup probe is fast AND has no side effects. Used by
/// [`resolve_startup_reviewer_kind`] for the a64 agentic-CLI fallback.
pub(crate) fn reviewer_binary_on_path(cli: &str) -> bool {
    let candidate = Path::new(cli);
    if candidate.is_absolute() || cli.contains('/') {
        return candidate.is_file();
    }
    match std::env::var_os("PATH") {
        Some(path_var) => std::env::split_paths(&path_var).any(|dir| dir.join(cli).is_file()),
        None => false,
    }
}

/// Pure decision behind the a64 startup CLI-availability fallback. Given the
/// configured reviewer transport, the resolved CLI name, AND whether that CLI
/// is available on the host, return the effective startup transport plus an
/// optional loud WARN message:
///
/// - `Oneshot` configured → `(Oneshot, None)`: the operator opted out of
///   agentic deliberately, so no probe AND no warning.
/// - `Agentic` configured AND CLI available → `(Agentic, None)`: agentic runs.
/// - `Agentic` configured AND CLI unavailable → `(Oneshot, Some(warn))`: the
///   reviewer degrades to the HTTP one-shot path for the boot (review is NOT
///   disabled) AND the caller logs `warn`, which names the missing CLI AND the
///   remedy. The same disposition applies whether `agentic` was the default or
///   set explicitly.
///
/// Separated from the host probe ([`resolve_startup_reviewer_kind`]) so tests
/// assert the decision without depending on what is installed on the host —
/// mirroring [`crate::config::clamp_max_code_reviews_per_pr`]'s observable
/// `Option<String>` warning return.
pub fn startup_reviewer_kind_decision(
    configured: ReviewerKind,
    cli: &str,
    cli_available: bool,
) -> (ReviewerKind, Option<String>) {
    match configured {
        ReviewerKind::Oneshot => (ReviewerKind::Oneshot, None),
        ReviewerKind::Agentic if cli_available => (ReviewerKind::Agentic, None),
        ReviewerKind::Agentic => {
            let warn = format!(
                "reviewer.kind is `agentic` but the resolved reviewer CLI `{cli}` is unavailable \
                 on the daemon host (no registered strategy, OR the binary is not on PATH); \
                 falling back to the `oneshot` HTTP review path for this boot — review is NOT \
                 disabled. Install `{cli}` to enable the agentic reviewer, OR set \
                 `reviewer.kind: oneshot` to silence this warning. A daemon restart or \
                 `autocoder reload` re-evaluates availability."
            );
            (ReviewerKind::Oneshot, Some(warn))
        }
    }
}

/// Resolve the reviewer's effective transport at startup AND on
/// `autocoder reload`, applying the a64 agentic-CLI-availability fallback.
///
/// When the configured kind is `agentic` (defaulted OR explicit) this probes
/// the host: the CLI is "available" only when its strategy is registered
/// (resolved via the a55/a56 `provider → CLI` rule) AND its binary is found on
/// PATH. An unavailable CLI degrades to `oneshot` for the boot, returning the
/// loud WARN for the caller to log exactly once. When the configured kind is
/// `oneshot` no probe runs. The daemon wires this in at the two reviewer
/// construction sites (startup in `cli::run`, reload in `control_socket`), so
/// availability is evaluated once per boot/reload — never per polling
/// iteration. This supersedes a58's "a reviewer CLI with no registered
/// strategy returns a clear error, no session" behavior for the reviewer role:
/// instead of erroring, the reviewer degrades to HTTP review.
pub fn resolve_startup_reviewer_kind(reviewer: &CodeReviewer) -> (ReviewerKind, Option<String>) {
    if reviewer.kind() != ReviewerKind::Agentic {
        return (reviewer.kind(), None);
    }
    // "Available" requires BOTH a registered strategy AND a binary on PATH.
    let cli_available =
        resolve_reviewer_strategy(reviewer).is_ok() && reviewer_binary_on_path(&reviewer.command);
    startup_reviewer_kind_decision(ReviewerKind::Agentic, &reviewer.command, cli_available)
}

/// Apply the a64 startup CLI-availability fallback to a freshly built
/// reviewer. When the effective kind is `agentic` but the resolved reviewer
/// CLI is unavailable, log ONE loud WARN (naming the missing CLI AND the
/// remedy) AND return the reviewer with its kind overridden to `oneshot` for
/// the boot — review continues over HTTP, never disabled. Otherwise the
/// reviewer is returned unchanged. Both reviewer construction sites (startup
/// in `cli::run`, reload in `control_socket::build_reviewer`) call this, so
/// availability is evaluated once per boot/reload — the live polling-loop
/// reviewer slot already carries the resolved kind, so no per-iteration probe
/// (and no re-warn) occurs.
pub fn apply_startup_cli_fallback(reviewer: CodeReviewer) -> CodeReviewer {
    let (effective, warn) = resolve_startup_reviewer_kind(&reviewer);
    if let Some(msg) = warn {
        tracing::warn!("{msg}");
    }
    reviewer.with_kind(effective)
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
        timeout: AGENTIC_REVIEW_TIMEOUT,
        // Pass the reviewer's resolved model so the wrapped CLI runs the
        // operator-configured model, mirroring the verifier gates.
        model: reviewer.resolved_model.as_ref(),
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
        let prompt = render_agentic_review_prompt(session_ctx, preamble);
        let consumed = runner
            .run_session(slug.as_deref().unwrap_or(""), &prompt)
            .await?;
        match consumed {
            None => {
                let reason = match slug {
                    Some(s) => format!(
                        "agentic reviewer session for `{s}` recorded no valid submit_review submission"
                    ),
                    None => "agentic reviewer session recorded no valid submit_review submission"
                        .to_string(),
                };
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

/// Aggregate per-change agentic [`ReviewResult`]s into one result whose
/// `per_change_sections` drives the composer to emit one
/// `## Code Review: <slug>` section per change — the same disposition the
/// one-shot per-change path produces. The aggregate verdict is `Block` when
/// ANY change blocked, else `Approve`; the flat `concerns` vec is the union
/// of each change's concerns tagged with their `change_slug`.
pub(crate) fn synthesize_agentic_per_change(
    reviews: Vec<(Option<String>, ReviewResult)>,
    attribution: Option<String>,
) -> ReviewResult {
    // a015: a synthesis from zero per-change reviews must NEVER be the
    // source of a defaulted `Approve`. The dispatch in
    // `run_agentic_review_with_runner` now falls back to a bundled session
    // before reaching here with an empty vec, so this guard is defensive —
    // it makes the "never a defaulted Approve" invariant explicit. `Block`
    // is the only fail-safe verdict: an empty synthesis can never become a
    // silent approval. (Mirrors the one-shot `synthesize_per_change_report`
    // guard.)
    if reviews.is_empty() {
        return ReviewResult {
            verdict: Verdict::Block,
            per_concern: Vec::new(),
            raw_output: String::new(),
            markdown: "No per-change reviews were performed; refusing to \
                synthesize a verdict from zero reviews."
                .to_string(),
            per_change_sections: Vec::new(),
            concerns: Vec::new(),
            attribution,
        };
    }
    let mut verdict = Verdict::Approve;
    let mut concerns: Vec<ReviewConcern> = Vec::new();
    let mut sections: Vec<PerChangeSection> = Vec::with_capacity(reviews.len());
    for (slug, result) in reviews {
        let slug = slug.unwrap_or_default();
        if matches!(result.verdict, Verdict::Block) {
            verdict = Verdict::Block;
        }
        for concern in &result.concerns {
            let mut tagged = concern.clone();
            tagged.change_slug = Some(slug.clone());
            concerns.push(tagged);
        }
        let section_body = format!(
            "VERDICT: {}\n\n{}",
            result.verdict.label(),
            result.raw_output
        );
        sections.push(PerChangeSection {
            change_slug: slug,
            markdown: section_body,
        });
    }
    let per_concern = concerns.iter().map(ConcernEntry::from).collect();
    ReviewResult {
        verdict,
        per_concern,
        raw_output: String::new(),
        markdown: String::new(),
        per_change_sections: sections,
        concerns,
        attribution,
    }
}

/// Register the reviewer's `submit_review` payload schema (a58) with the
/// daemon's submission store, under [`REVIEWER_ROLE`]. The validator IS
/// [`payload_to_review_result`] with its `Ok` value discarded, so a
/// payload that records successfully is exactly one that maps. Called once
/// at daemon startup alongside the advisory audits' schema registration.
pub fn register_reviewer_submission_schema(store: &crate::submission_store::SubmissionStore) {
    use std::sync::Arc;
    store.register_schema(
        REVIEWER_ROLE,
        Arc::new(|p: &Value| payload_to_review_result(p).map(|_| ())),
    );
}

impl ReviewResult {
    /// Convert an agentic [`ReviewResult`] into the [`ReviewReport`] the
    /// polling-loop's post-review pipeline consumes (draft decision,
    /// reviewer-revision partitioning, PR-body composition). The two-state
    /// agentic verdict maps `Approve → Pass` AND `Block → Block`.
    pub fn into_review_report(self) -> ReviewReport {
        let verdict = match self.verdict {
            Verdict::Approve => ReviewVerdict::Pass,
            Verdict::Block => ReviewVerdict::Block,
        };
        ReviewReport {
            verdict,
            markdown: self.markdown,
            concerns: self.concerns,
            per_change_sections: self.per_change_sections,
            attribution: self.attribution,
        }
    }
}
