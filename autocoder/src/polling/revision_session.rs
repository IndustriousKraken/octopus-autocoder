//! Interactive spec-revision thread sessions (a03). Two roles, both drained
//! one-per-iteration from the polling loop:
//!
//! 1. [`process_pending_revision_advise`] — the **advisor**. A non-`send it`
//!    `@<bot>` reply in a revision thread runs a READ-ONLY agentic session
//!    reconstructed from the flagged change's spec deltas, the relevant canon,
//!    the marker's contradiction narrative, AND the thread transcript so far,
//!    then replies in the thread. It writes nothing AND holds no session
//!    between replies — each round rebuilds context from the (now longer)
//!    transcript (design D2).
//!
//! 2. [`process_pending_revision_execute`] — the **executor**. `@<bot> send it`
//!    in a revision thread runs a WRITE-scoped session that edits the change's
//!    spec deltas along the discussed direction, re-runs the `[in]` AND
//!    `[canon]` gates (a02's invocation) against the revised change, AND opens
//!    a PR on a clean re-gate (reporting the PR link in the thread). On a
//!    still-failing re-gate it opens NO PR and reports the remaining
//!    contradiction (design D3/D5). It never commits to the base branch
//!    outside the PR AND never edits `tasks.md` (design D4).
//!
//! Both sessions source their model + wrapped-CLI command from the `[in]`
//! contradiction gate's context ([`change_contradiction::current`]): a03
//! reuses a02's gate invocation, so when that gate is not configured the
//! sessions degrade with an explanatory thread reply rather than acting
//! blind. The session boundaries (advisor run, edit run, re-gate, PR open) are
//! injected behind traits so the orchestration is unit-testable without
//! spawning a CLI or touching GitHub.

use crate::busy_marker;
use crate::chatops::ThreadMessage;
use crate::config::{GithubConfig, RepositoryConfig};
use crate::control_socket::{RevisionAdviseRequest, RevisionExecuteRequest};
use crate::paths::DaemonPaths;
use crate::polling_loop::ChatOpsContext;
use crate::preflight::canon_contradiction::{self, CanonContradictionCheckOutcome};
use crate::preflight::change_contradiction::{self, ContradictionCheckOutcome};
use crate::revision_thread::{self, RevisionThreadState, RevisionThreadStatus};
use crate::{git, github};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::path::Path;
use std::time::Duration;

// =====================================================================
// Shared helpers
// =====================================================================

/// Workspace-relative path of the change's contradiction marker file.
fn marker_rel_path(change_slug: &str) -> String {
    format!("openspec/changes/{change_slug}/.needs-spec-revision.json")
}

/// Workspace-relative path of the change's directory (the revision scope).
fn change_dir_rel(change_slug: &str) -> String {
    format!("openspec/changes/{change_slug}/")
}

/// Enumerate the change's spec-delta file paths (workspace-relative), sorted
/// by capability. The advisor + executor point the agent at these so it reads
/// them on demand (mirrors the contradiction check's path-listing prompt).
fn spec_delta_paths(workspace: &Path, change_slug: &str) -> Vec<String> {
    let specs_dir = workspace
        .join("openspec/changes")
        .join(change_slug)
        .join("specs");
    let Ok(read) = std::fs::read_dir(&specs_dir) else {
        return Vec::new();
    };
    let mut caps: Vec<String> = Vec::new();
    for entry in read.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if entry.path().is_dir()
            && entry.path().join("spec.md").is_file()
        {
            caps.push(name);
        }
    }
    caps.sort();
    caps.into_iter()
        .map(|cap| format!("openspec/changes/{change_slug}/specs/{cap}/spec.md"))
        .collect()
}

/// Render the thread transcript as a conversation history block for the
/// advisor's prompt (task 3.2). Each message is labelled by author so the
/// agent can follow both sides of the discussion. The bot's own messages
/// (the alert body, prior advisor answers) are included — that is what makes
/// multi-round discussion work without a held session.
pub(crate) fn render_transcript(messages: &[ThreadMessage]) -> String {
    if messages.is_empty() {
        return "(no prior messages in this thread)".to_string();
    }
    let mut out = String::new();
    for m in messages {
        let who = if m.from_bot { "autocoder" } else { "operator" };
        let text = m.text.trim();
        if text.is_empty() {
            continue;
        }
        out.push_str(&format!("{who}: {text}\n"));
    }
    if out.is_empty() {
        return "(no prior messages in this thread)".to_string();
    }
    out
}

/// Fetches a thread transcript. Production reads it from the chatops backend;
/// tests inject a fetcher that fails a scripted number of times so the
/// bounded-retry + fail-closed behavior is exercised without a real backend.
#[async_trait]
trait TranscriptFetcher: Send + Sync {
    async fn fetch(&self, channel: &str, thread_ts: &str) -> Result<Vec<ThreadMessage>>;
}

/// Production transcript fetcher: reads the thread from the chatops backend.
struct ChatOpsTranscriptFetcher<'a> {
    ctx: &'a ChatOpsContext,
}

#[async_trait]
impl TranscriptFetcher for ChatOpsTranscriptFetcher<'_> {
    async fn fetch(&self, channel: &str, thread_ts: &str) -> Result<Vec<ThreadMessage>> {
        self.ctx.chatops.fetch_thread_transcript(channel, thread_ts).await
    }
}

/// Fetch the thread transcript with a bounded retry (`retries` ADDITIONAL
/// attempts beyond the first, short backoff between attempts). The transcript
/// carries the operator's chosen DIRECTION, so a revision must never run
/// against an empty discussion — a transient failure is absorbed here; only a
/// PERSISTENT failure (every attempt failed) surfaces as `Err` to the caller,
/// which decides whether to fail closed (executor) or degrade-but-answer
/// (advisor). Keys ONLY on fetch success/failure — never on provider-specific
/// error text.
async fn fetch_transcript_bounded(
    fetcher: &dyn TranscriptFetcher,
    channel: &str,
    thread_ts: &str,
    retries: u32,
) -> Result<Vec<ThreadMessage>> {
    let attempts = retries.saturating_add(1);
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..attempts {
        match fetcher.fetch(channel, thread_ts).await {
            Ok(t) => return Ok(t),
            Err(e) => {
                last_err = Some(e);
                // Short backoff between attempts; none after the final one.
                if attempt + 1 < attempts {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("transcript fetch failed")))
}

/// Best-effort threaded reply. Logs + swallows the error so a failed reply
/// never aborts the surrounding iteration.
async fn post_reply(
    chatops_ctx: Option<&ChatOpsContext>,
    channel: &str,
    thread_ts: &str,
    body: &str,
) {
    let Some(ctx) = chatops_ctx else { return };
    if let Err(e) = ctx.chatops.post_threaded_reply(channel, thread_ts, body).await {
        tracing::warn!("revision-session: thread reply failed: {e:#}");
    }
}

// =====================================================================
// Advisor (read-only)
// =====================================================================

/// Build the advisor session prompt: the marching orders, the on-disk
/// artifacts to read (change deltas, the marker's contradiction, the canon),
/// the transcript so far, AND the operator's current question. The agent
/// reads files on demand via the read-only sandbox; their contents are NOT
/// inlined (mirrors the contradiction check).
pub(crate) fn build_advisor_prompt(
    workspace: &Path,
    change_slug: &str,
    current_reply: &str,
    transcript: &[ThreadMessage],
) -> String {
    let paths = spec_delta_paths(workspace, change_slug);
    let mut out = String::new();
    out.push_str(
        "You are autocoder's spec-revision ADVISOR. A change tripped the `[in]` \
         or `[canon]` contradiction gate at implement time, and an operator is \
         discussing the revision with you in a chat thread. You are READ-ONLY: \
         read the artifacts below and answer the operator's question. Do NOT \
         write, edit, or create any file — this is a discussion, not the \
         revision itself (the operator triggers the rewrite separately with \
         `send it`).\n\n",
    );
    out.push_str(&format!("# The flagged change: `{change_slug}`\n\n"));
    out.push_str(&format!(
        "Read its contradiction narrative: `{}`\n",
        marker_rel_path(change_slug)
    ));
    if paths.is_empty() {
        out.push_str(
            "This change has no per-capability spec deltas under \
             openspec/changes/<change>/specs/.\n",
        );
    } else {
        out.push_str("Read its spec deltas:\n");
        for p in &paths {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out.push_str(
        "\nThe relevant canonical specs live under `openspec/specs/<capability>/spec.md`; \
         Read the ones the change touches so you can weigh align-the-change-to-canon \
         versus MODIFY-the-canonical-requirement.\n\n",
    );
    out.push_str("# The discussion so far\n\n");
    out.push_str(&render_transcript(transcript));
    out.push_str("\n# The operator's current message\n\n");
    out.push_str(current_reply.trim());
    out.push_str(
        "\n\nAnswer concisely. Typically the choice is: (a) align the change's \
         vocabulary to canon's existing term, or (b) MODIFY the contradicted \
         canonical requirement — say which you recommend and why, and sketch \
         the concrete edit. Do not write any file.",
    );
    out
}

/// Runs ONE read-only advisor session AND returns the agent's text answer.
/// Production is [`CliAdvisorRunner`]; tests inject a canned answer.
#[async_trait]
trait AdvisorSessionRunner: Send + Sync {
    async fn advise(&self, workspace: &Path, prompt: &str) -> Result<String>;
}

/// Production advisor runner: a read-only agentic session (`Read`/`Glob`/`Grep`,
/// `deny_writes: true`) whose captured stdout is the answer.
struct CliAdvisorRunner<'a> {
    command: String,
    model: &'a crate::agentic_run::ResolvedModel,
    /// Wall-clock cap for the session, resolved from the SINGLE
    /// `executor.agentic_session_timeout_secs` (shared with the verifier gates
    /// AND the reviewer). Carried by the `[in]` gate ctx the revision sessions
    /// reuse for their model + command.
    timeout: Duration,
}

#[async_trait]
impl AdvisorSessionRunner for CliAdvisorRunner<'_> {
    async fn advise(&self, workspace: &Path, prompt: &str) -> Result<String> {
        let strategy = crate::agentic_run::strategy_for_provider(
            self.model.provider,
            self.command.clone(),
            Vec::new(),
        )
        .context("resolving CLI strategy for the revision advisor")?;
        let outcome = crate::agentic_run::agentic_run_with_session(
            crate::agentic_run::AgenticRunOpts {
                workspace,
                change: "revision_advise",
                strategy: strategy.as_ref(),
                prompt,
                sandbox: crate::agentic_run::SandboxConfig {
                    allowed_tools: vec![
                        "Read".to_string(),
                        "Glob".to_string(),
                        "Grep".to_string(),
                    ],
                    disallowed_bash_patterns: Vec::new(),
                    disallowed_read_paths: Vec::new(),
                    deny_writes: true,
                },
                model: Some(self.model),
                output_mode: crate::agentic_run::OutputMode::Capture,
                timeout: self.timeout,
                paths: None,
                settings_dir: None,
                include_autocoder_tools: false,
                emit_stream_json_in_capture: false,
                resume_session_id: None,
                track_subprocess_marker: false,
                etxtbsy_retry_spawn: true,
                os_sandbox: crate::sandbox::current_run_sandbox(
                    crate::config::default_cli_for(self.model.provider),
                    false,
                ),
            },
            true,
            None,
        )
        .await
        .context("spawning the revision-advisor session")?;
        if outcome.timed_out {
            return Err(anyhow!(
                "revision-advisor session timed out after {}s",
                self.timeout.as_secs()
            ));
        }
        Ok(outcome.stdout.trim().to_string())
    }
}

/// Drain ONE advisor request: reconstruct context (transcript + on-disk
/// artifacts) AND reply read-only. Stateless — nothing is persisted; the next
/// reply rebuilds from the longer transcript.
pub async fn process_pending_revision_advise(
    workspace: &Path,
    _repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &RevisionAdviseRequest,
) -> Result<()> {
    let Some(ctx) = chatops_ctx else {
        // No chat backend → nowhere to reply. Nothing to do.
        return Ok(());
    };
    // The advisor reuses the `[in]` gate's model + command (a02's invocation).
    let cc_ctx = match change_contradiction::current() {
        Some(c) => c,
        None => {
            post_reply(
                Some(ctx),
                &request.channel,
                &request.thread_ts,
                "✗ The spec-revision advisor needs the `[in]` contradiction gate configured (it supplies the model). Discuss the change directly, or enable the gate.",
            )
            .await;
            return Ok(());
        }
    };
    // Re-fetch the transcript each reply so multi-round discussion works
    // without a held session (task 3.2), with the SAME bounded retry the
    // executor uses. The advisor WRITES NOTHING, so a persistent failure does
    // NOT abort it (unlike the executor): it answers from the degraded thread
    // AND surfaces that it did (task 5.3).
    let fetcher = ChatOpsTranscriptFetcher { ctx };
    let (transcript, degraded) = match fetch_transcript_bounded(
        &fetcher,
        &request.channel,
        &request.thread_ts,
        cc_ctx.revision_transcript_fetch_retries,
    )
    .await
    {
        Ok(t) => (t, false),
        Err(e) => {
            tracing::warn!(
                change = %request.change_slug,
                "revision-advise: transcript fetch failed after bounded retry (degrading to single-turn; advisor still answers): {e:#}"
            );
            (Vec::new(), true)
        }
    };
    let runner = CliAdvisorRunner {
        command: cc_ctx.command.clone(),
        model: &cc_ctx.model,
        timeout: cc_ctx.timeout,
    };
    advise_with_runner(workspace, ctx, request, &transcript, degraded, &runner).await
}

/// Orchestration shared by production AND tests: build the prompt, run the
/// (injected) advisor, AND post the answer. Posts a fallback line when the
/// session yields no text OR errors — never silently drops the operator's
/// question.
async fn advise_with_runner(
    workspace: &Path,
    ctx: &ChatOpsContext,
    request: &RevisionAdviseRequest,
    transcript: &[ThreadMessage],
    degraded: bool,
    runner: &dyn AdvisorSessionRunner,
) -> Result<()> {
    let prompt =
        build_advisor_prompt(workspace, &request.change_slug, &request.reply_text, transcript);
    let answer = match runner.advise(workspace, &prompt).await {
        Ok(a) if !a.trim().is_empty() => a.trim().to_string(),
        Ok(_) => {
            "🤔 The revision advisor produced no answer this round. Try rephrasing, or `send it` to attempt the revision.".to_string()
        }
        Err(e) => {
            tracing::warn!(
                change = %request.change_slug,
                "revision-advise: session failed: {e:#}"
            );
            format!("✗ The revision advisor session failed: {e}")
        }
    };
    // The advisor writes nothing, so a degraded thread is acceptable — but it
    // SHALL surface that it answered from a partial thread (task 5.3) so the
    // operator can weigh the answer accordingly.
    let answer = if degraded {
        format!(
            "⚠️ I could not load the full discussion thread, so this answer is from a partial view of it.\n\n{answer}"
        )
    } else {
        answer
    };
    post_reply(Some(ctx), &request.channel, &request.thread_ts, &answer).await;
    Ok(())
}

// =====================================================================
// Executor (write + re-gate + PR)
// =====================================================================

/// The branch the executor pushes the spec revision on. Distinct per change so
/// concurrent revisions never collide. The revision is reviewed via this
/// branch's PR — never committed to the base branch (design D4).
pub(crate) fn revision_branch_name(agent_branch: &str, change_slug: &str) -> String {
    format!("{agent_branch}-spec-revision-{change_slug}")
}

/// Render the marker's CURRENT structured findings as the explicit set the
/// revision MUST resolve (task 4): "resolve EACH of these contradictions: …",
/// each naming the conflicting requirement identity (and, for `[canon]`, the
/// canonical capability) plus the why-summary AND the gate's suggested fix when
/// present. Empty when the marker records no structured findings (an older
/// marker, or a non-contradiction hold) — the prompt then falls back to the
/// narrative + transcript.
fn render_findings_to_resolve(
    findings: &[crate::spec_revision::ContradictionFindingRecord],
) -> String {
    use crate::spec_revision::ContradictionGate;
    if findings.is_empty() {
        return String::new();
    }
    let n = findings.len();
    let mut out = format!(
        "\n# The contradictions this revision MUST resolve ({n})\n\n\
         These are the change's CURRENT contradictions, recorded in the marker \
         above. Resolve EVERY one — not only the first:\n\n"
    );
    for (i, f) in findings.iter().enumerate() {
        match f.gate {
            ContradictionGate::In => {
                out.push_str(&format!(
                    "{idx}. [in] within-change conflict:\n   \
                     Requirement A: {a}\n   Requirement B: {b}\n",
                    idx = i + 1,
                    a = f.requirement_a,
                    b = f.requirement_b,
                ));
            }
            ContradictionGate::Canon => {
                out.push_str(&format!(
                    "{idx}. [canon] change-vs-canon conflict:\n   \
                     Change requirement: {a}\n   \
                     Conflicting canonical requirement: {b} (capability: {cap})\n",
                    idx = i + 1,
                    a = f.requirement_a,
                    b = f.requirement_b,
                    cap = f.canonical_capability,
                ));
            }
        }
        if !f.summary.trim().is_empty() {
            out.push_str(&format!("   Why: {}\n", f.summary));
        }
        if !f.suggested_fix.trim().is_empty() {
            out.push_str(&format!("   Suggested fix: {}\n", f.suggested_fix));
        }
        out.push('\n');
    }
    out
}

/// Render the specific requirement(s) a revision is NOT clearing (the
/// escalation report, task 6.3): each surviving finding named by its conflicting
/// requirement identity (and capability for `[canon]`), so a persistent
/// non-convergence is legible rather than an opaque repeating failure.
fn render_surviving_findings(
    survivors: &[&crate::spec_revision::ContradictionFindingRecord],
) -> String {
    use crate::spec_revision::ContradictionGate;
    let mut out = String::new();
    for (i, f) in survivors.iter().enumerate() {
        match f.gate {
            ContradictionGate::In => {
                out.push_str(&format!(
                    "{idx}. [in] still conflicting: `{a}` vs `{b}`\n",
                    idx = i + 1,
                    a = f.requirement_a,
                    b = f.requirement_b,
                ));
            }
            ContradictionGate::Canon => {
                out.push_str(&format!(
                    "{idx}. [canon] still conflicting: change requirement `{a}` vs canonical `{b}` (capability: {cap})\n",
                    idx = i + 1,
                    a = f.requirement_a,
                    b = f.requirement_b,
                    cap = f.canonical_capability,
                ));
            }
        }
    }
    out
}

/// Compose the budget-exhausted failure reply for a `send it` that exhausted
/// its bounded converge loop with a contradiction remaining. ALWAYS names the
/// remaining contradiction — the surviving finding identities when the SAME
/// identity persisted across the attempts (escalation), else the gate summary
/// `text`. When `consecutive_count` has reached `threshold` consecutive failed
/// rounds (default 3), the reply ADDITIONALLY recommends DECOMPOSING the change
/// into smaller changes — a change failing repeated revision rounds is likely
/// too large or interconnected to converge via `send it`; the operator MAY
/// still `send it` again, but decomposition is the recommended path. Below the
/// threshold the reply is the pre-existing form (names the contradiction,
/// invites another `send it`) with NO decomposition nudge. `threshold == 0`
/// disables the nudge entirely.
fn build_budget_exhausted_reply(
    survivors: &[&crate::spec_revision::ContradictionFindingRecord],
    text: &str,
    max_iterations: u32,
    consecutive_count: u32,
    threshold: u32,
) -> String {
    // The contradiction-naming head (unchanged behavior).
    let mut body = if !survivors.is_empty() {
        format!(
            "✗ send it: the revision is NOT clearing {} contradiction(s) that survived {} attempt(s) — no PR opened.\n{}",
            survivors.len(),
            max_iterations,
            render_surviving_findings(survivors),
        )
    } else {
        format!(
            "✗ send it: the revision still fails the gates after {} attempt(s) — no PR opened.\n{text}",
            max_iterations,
        )
    };
    if threshold > 0 && consecutive_count >= threshold {
        // Threshold reached: recommend decomposition (additive — the operator
        // may still `send it`, but this is the recommended path).
        body.push_str(&format!(
            "\n\n⚠️ This change has now failed {consecutive_count} consecutive `send it` rounds. A change that fails repeated revision rounds is usually too large or too interconnected to converge this way — the recommended fix is to DECOMPOSE it into smaller, independent changes that each gate cleanly, rather than another `send it`. You MAY still `send it` again, but decomposition is the better path here."
        ));
    } else if !survivors.is_empty() {
        body.push_str(
            "\nThis is a persistent non-convergence; discuss the direction further, then `send it` again.",
        );
    } else {
        body.push_str("\nReply to discuss further, then `send it` again.");
    }
    body
}

/// Build the executor session prompt: the marching orders (edit the change's
/// spec deltas along the discussed direction), the artifacts to read, the
/// discussion so far, the CURRENT contradiction set the revision MUST resolve
/// (read from the durable marker), AND the hard guardrails (scope, no
/// tasks.md). The transcript supplies the operator's chosen DIRECTION; the
/// marker supplies WHAT must be resolved (task 4).
pub(crate) fn build_executor_prompt(
    workspace: &Path,
    change_slug: &str,
    transcript: &[ThreadMessage],
) -> String {
    let paths = spec_delta_paths(workspace, change_slug);
    // The marker is the durable source of truth for what currently contradicts.
    // Read it best-effort; an unreadable/absent marker leaves the prompt
    // grounded in the narrative + transcript (degraded but still actionable).
    let marker_findings = crate::spec_revision::read_marker(workspace, change_slug)
        .ok()
        .flatten()
        .map(|m| m.contradictions)
        .unwrap_or_default();
    let mut out = String::new();
    out.push_str(
        "You are autocoder's spec-revision EXECUTOR. An operator discussed a \
         contradiction in a change and has now triggered the revision. Edit the \
         change's spec deltas (proposal.md and the per-capability spec deltas) \
         to resolve the contradiction along the direction the discussion \
         converged on.\n\n",
    );
    out.push_str(&format!("# The change to revise: `{change_slug}`\n\n"));
    out.push_str(&format!(
        "Read its contradiction narrative first: `{}`\n",
        marker_rel_path(change_slug)
    ));
    out.push_str(&render_findings_to_resolve(&marker_findings));
    if paths.is_empty() {
        out.push_str(
            "This change has no per-capability spec deltas; edit \
             openspec/changes/<change>/proposal.md as needed.\n",
        );
    } else {
        out.push_str("Its spec deltas:\n");
        for p in &paths {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out.push_str(
        "\nRead the relevant canonical specs under `openspec/specs/<capability>/spec.md` \
         so your revision is consistent with them.\n\n",
    );
    out.push_str("# The discussion that directs this revision\n\n");
    out.push_str(&render_transcript(transcript));
    out.push_str("\n\n# Hard rules\n\n");
    out.push_str(&format!(
        "- Edit ONLY files under `{}` — nothing outside the change directory.\n",
        change_dir_rel(change_slug)
    ));
    out.push_str(
        "- Do NOT edit tasks.md. The revision resolves the spec contradiction; \
         it does not touch the task list.\n",
    );
    out.push_str(
        "- Do NOT delete the `.needs-spec-revision.json` marker.\n\
         - Make the smallest edit that resolves the contradiction the operator described.\n",
    );
    out
}

/// Outcome of re-running the `[in]` + `[canon]` gates against the revised
/// change. The executor opens a PR only on [`ReGateOutcome::Clean`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReGateOutcome {
    /// Both gates ran AND found no contradiction. Open the PR.
    Clean,
    /// A gate found a remaining contradiction. Open NO PR. Carries the
    /// re-gate's CURRENT structured findings (the durable source of truth the
    /// marker is refreshed with, AND the identity the escalation tracks) AND a
    /// pre-formatted human-readable summary for the thread reply.
    Contradiction {
        findings: Vec<crate::spec_revision::ContradictionFindingRecord>,
        text: String,
    },
    /// A gate could not be evaluated (disabled / errored). Fail closed: open
    /// NO PR; report this cause (the operator fixes the gate AND retries).
    CouldNotRun(String),
}

/// Runs the re-gate against the revised change. Production combines the two
/// real gate invocations; tests inject a canned outcome.
#[async_trait]
trait ReGateRunner: Send + Sync {
    async fn regate(&self, workspace: &Path, change_slug: &str) -> ReGateOutcome;
}

/// Production re-gate: re-runs a02's `[in]` AND `[canon]` invocations against
/// the revised change AND combines them. A `Found` from either is a remaining
/// contradiction (no PR); an `Errored` from either fails closed (no PR); only
/// both-`Clean` opens the PR.
struct GatesReGateRunner {
    in_ctx: Option<std::sync::Arc<change_contradiction::ContradictionCheckCtx>>,
    canon_ctx: Option<std::sync::Arc<canon_contradiction::CanonContradictionCheckCtx>>,
}

#[async_trait]
impl ReGateRunner for GatesReGateRunner {
    async fn regate(&self, workspace: &Path, change_slug: &str) -> ReGateOutcome {
        use crate::spec_revision::ContradictionFindingRecord;
        // Accumulate the CURRENT findings of BOTH gates so the executor can
        // resolve every remaining contradiction in one converge iteration AND
        // the refreshed marker carries the full present set. An `Errored` from
        // either gate fails closed (the revision was NOT verified).
        let mut records: Vec<ContradictionFindingRecord> = Vec::new();
        let mut summaries: Vec<String> = Vec::new();

        // `[in]` gate.
        if let Some(ctx) = &self.in_ctx {
            match change_contradiction::run_agentic_contradiction_check(ctx, workspace, change_slug)
                .await
            {
                ContradictionCheckOutcome::Clean => {}
                ContradictionCheckOutcome::Found(findings) => {
                    summaries.push(format!(
                        "[in] gate still finds {} change-internal contradiction(s): {}",
                        findings.len(),
                        findings
                            .iter()
                            .map(|f| f.summary.clone())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                    records.extend(findings.iter().map(ContradictionFindingRecord::from));
                }
                ContradictionCheckOutcome::Errored { cause } => {
                    return ReGateOutcome::CouldNotRun(format!("[in] gate could not run: {cause}"));
                }
            }
        } else {
            return ReGateOutcome::CouldNotRun(
                "[in] contradiction gate is not configured; cannot verify the revision".to_string(),
            );
        }
        // `[canon]` gate.
        if let Some(ctx) = &self.canon_ctx {
            match canon_contradiction::run_agentic_canon_contradiction_check(
                ctx,
                workspace,
                change_slug,
            )
            .await
            {
                CanonContradictionCheckOutcome::Clean => {}
                CanonContradictionCheckOutcome::Found(findings) => {
                    summaries.push(format!(
                        "[canon] gate still finds {} change-vs-canon contradiction(s): {}",
                        findings.len(),
                        findings
                            .iter()
                            .map(|f| f.summary.clone())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                    records.extend(findings.iter().map(ContradictionFindingRecord::from));
                }
                CanonContradictionCheckOutcome::Errored { cause } => {
                    return ReGateOutcome::CouldNotRun(format!(
                        "[canon] gate could not run: {cause}"
                    ));
                }
            }
        }
        // No `[canon]` ctx → only the `[in]` gate ran. a03 reuses a02's
        // invocation: if `[canon]` is disabled there is nothing more to check.
        if records.is_empty() {
            ReGateOutcome::Clean
        } else {
            ReGateOutcome::Contradiction {
                findings: records,
                text: summaries.join("\n"),
            }
        }
    }
}

/// Runs ONE write-scoped edit session that revises the change's spec deltas.
/// Production is a write sandbox; tests perform a canned edit.
#[async_trait]
trait EditSessionRunner: Send + Sync {
    async fn revise(&self, workspace: &Path, prompt: &str) -> Result<()>;
}

/// Production edit runner: a write-scoped agentic session (`Read`/`Glob`/`Grep`/
/// `Edit`/`Write`, `deny_writes: false`, workspace-writable OS sandbox).
struct CliEditRunner<'a> {
    command: String,
    model: &'a crate::agentic_run::ResolvedModel,
    /// Wall-clock cap for the session, resolved from the SINGLE
    /// `executor.agentic_session_timeout_secs` (shared with the verifier gates
    /// AND the reviewer). Carried by the `[in]` gate ctx the revision sessions
    /// reuse for their model + command.
    timeout: Duration,
}

#[async_trait]
impl EditSessionRunner for CliEditRunner<'_> {
    async fn revise(&self, workspace: &Path, prompt: &str) -> Result<()> {
        let strategy = crate::agentic_run::strategy_for_provider(
            self.model.provider,
            self.command.clone(),
            Vec::new(),
        )
        .context("resolving CLI strategy for the revision executor")?;
        let outcome = crate::agentic_run::agentic_run_with_session(
            crate::agentic_run::AgenticRunOpts {
                workspace,
                change: "revision_execute",
                strategy: strategy.as_ref(),
                prompt,
                sandbox: crate::agentic_run::SandboxConfig {
                    allowed_tools: vec![
                        "Read".to_string(),
                        "Glob".to_string(),
                        "Grep".to_string(),
                        "Edit".to_string(),
                        "Write".to_string(),
                    ],
                    disallowed_bash_patterns: Vec::new(),
                    disallowed_read_paths: Vec::new(),
                    deny_writes: false,
                },
                model: Some(self.model),
                output_mode: crate::agentic_run::OutputMode::Capture,
                timeout: self.timeout,
                paths: None,
                settings_dir: None,
                include_autocoder_tools: false,
                emit_stream_json_in_capture: false,
                resume_session_id: None,
                track_subprocess_marker: false,
                etxtbsy_retry_spawn: true,
                os_sandbox: crate::sandbox::current_run_sandbox(
                    crate::config::default_cli_for(self.model.provider),
                    true,
                ),
            },
            true,
            None,
        )
        .await
        .context("spawning the revision-executor session")?;
        if outcome.timed_out {
            return Err(anyhow!(
                "revision-executor session timed out after {}s",
                self.timeout.as_secs()
            ));
        }
        Ok(())
    }
}

/// Opens the PR carrying the spec revision. Production pushes the branch AND
/// creates the PR; tests return a canned URL.
#[async_trait]
trait RevisionPrOpener: Send + Sync {
    async fn open_pr(
        &self,
        workspace: &Path,
        repo: &RepositoryConfig,
        github_cfg: &GithubConfig,
        head_branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String>;
}

/// Production PR opener: pushes the spec-revision branch (force-with-lease) AND
/// creates the PR via the existing helpers. Never commits to the base branch.
struct GithubPrOpener;

#[async_trait]
impl RevisionPrOpener for GithubPrOpener {
    async fn open_pr(
        &self,
        workspace: &Path,
        repo: &RepositoryConfig,
        github_cfg: &GithubConfig,
        head_branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String> {
        let push_remote = if github_cfg.fork_owner.is_some() {
            "fork"
        } else {
            "origin"
        };
        git::push_force_with_lease(workspace, head_branch, push_remote)
            .with_context(|| format!("pushing spec-revision branch `{head_branch}`"))?;
        let (owner, name) =
            github::parse_repo_url(&repo.url).context("parsing repo URL for the revision PR")?;
        let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
        let head = if let Some(fork_owner) = github_cfg.fork_owner.as_deref() {
            format!("{fork_owner}:{head_branch}")
        } else {
            head_branch.to_string()
        };
        let pr = github::create_pull_request(
            &owner,
            &name,
            &head,
            &repo.base_branch,
            title,
            body,
            &token,
            None,
            false,
        )
        .await?;
        Ok(pr.html_url)
    }
}

/// Injected executor boundaries (edit session, re-gate, PR open).
struct ExecutorDeps<'a> {
    edit: &'a dyn EditSessionRunner,
    regate: &'a dyn ReGateRunner,
    pr: &'a dyn RevisionPrOpener,
}

/// Classify the workspace's post-edit git status against the revision scope.
/// Returns the list of paths OUTSIDE the change directory (scope leaks) AND
/// whether `tasks.md` was edited (a guardrail violation, design D4). The
/// pre-existing marker file is ignored — it is daemon bookkeeping, not an
/// edit-session product.
pub(crate) fn classify_revision_changes(
    entries: &[git::StatusEntry],
    change_slug: &str,
) -> (Vec<String>, bool) {
    let dir = change_dir_rel(change_slug);
    let marker = marker_rel_path(change_slug);
    let tasks = format!("openspec/changes/{change_slug}/tasks.md");
    let mut leaked = Vec::new();
    let mut tasks_edited = false;
    for e in entries {
        let p = &e.path;
        if p.is_empty() || *p == marker {
            continue;
        }
        if *p == tasks {
            tasks_edited = true;
            continue;
        }
        if !p.starts_with(&dir) {
            leaked.push(p.clone());
        }
    }
    (leaked, tasks_edited)
}

/// `true` when at least one in-scope spec-delta path (under the change dir,
/// excluding the marker AND tasks.md) was modified — i.e. the edit session
/// actually revised something.
pub(crate) fn has_in_scope_edit(entries: &[git::StatusEntry], change_slug: &str) -> bool {
    let dir = change_dir_rel(change_slug);
    let marker = marker_rel_path(change_slug);
    let tasks = format!("openspec/changes/{change_slug}/tasks.md");
    entries
        .iter()
        .any(|e| e.path.starts_with(&dir) && e.path != marker && e.path != tasks)
}

/// Drain ONE executor request: revise the change's spec deltas, re-gate, AND
/// open a PR on a clean re-gate (else report back). Builds the production
/// boundaries from the `[in]` gate's context; degrades when that gate is not
/// configured.
pub async fn process_pending_revision_execute(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &RevisionExecuteRequest,
    stuck_threshold_secs: u64,
) -> Result<()> {
    let state_root = revision_thread::default_state_root(paths);
    // The executor reuses the `[in]` gate's model + command (a02's invocation),
    // AND its re-gate IS that gate. When it is not configured the executor
    // cannot safely revise + verify, so it degrades with a thread reply.
    let in_ctx = match change_contradiction::current() {
        Some(c) => c,
        None => {
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                "✗ The spec-revision executor needs the `[in]` contradiction gate configured (it supplies the model AND the re-gate). Enable the gate, then `send it` again.",
            )
            .await;
            return Ok(());
        }
    };
    let canon_ctx = canon_contradiction::current();
    // The discussed direction lives in the thread; re-fetch it with a bounded
    // retry so a transient failure does not derail the revision. NEVER REVISE
    // BLIND (task 5): on a PERSISTENT failure the executor opens NO PR and
    // reports it could not read the discussion, instead of silently revising
    // against an empty thread. A revision without a chat backend at all is also
    // refused — there is no direction to ground it.
    let Some(ctx) = chatops_ctx else {
        // No chat backend → no thread to read, AND nowhere to report. Refuse to
        // revise blind rather than running against an empty discussion.
        tracing::warn!(
            change = %request.change_slug,
            "revision-execute: no chat backend; cannot read the discussion thread — not revising blind"
        );
        return Ok(());
    };
    let fetcher = ChatOpsTranscriptFetcher { ctx };
    let edit = CliEditRunner {
        command: in_ctx.command.clone(),
        model: &in_ctx.model,
        timeout: in_ctx.timeout,
    };
    let regate = GatesReGateRunner {
        in_ctx: Some(in_ctx.clone()),
        canon_ctx,
    };
    let pr = GithubPrOpener;
    let deps = ExecutorDeps {
        edit: &edit,
        regate: &regate,
        pr: &pr,
    };
    execute_with_deps(
        paths,
        &deps,
        &fetcher,
        workspace,
        repo,
        github_cfg,
        ctx,
        request,
        in_ctx.revision_transcript_fetch_retries,
        in_ctx.revision_converge_attempts,
        in_ctx.revision_nonconvergence_threshold,
        stuck_threshold_secs,
        &state_root,
    )
    .await
}

/// Fetch the thread transcript with a bounded retry, then run the revision
/// (orchestration shared by production AND tests). NEVER REVISES BLIND (task
/// 5): on a PERSISTENT fetch failure the executor opens NO PR AND reports it
/// could not read the discussion, instead of degrading to an empty thread.
/// Extracted so the transcript-retry + fail-closed behavior is exercised with
/// an injected [`TranscriptFetcher`] + [`ExecutorDeps`] without a real chatops
/// backend, a CLI subprocess, or GitHub.
#[allow(clippy::too_many_arguments)]
async fn execute_with_deps(
    paths: &DaemonPaths,
    deps: &ExecutorDeps<'_>,
    fetcher: &dyn TranscriptFetcher,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    ctx: &ChatOpsContext,
    request: &RevisionExecuteRequest,
    transcript_fetch_retries: u32,
    converge_attempts: u32,
    nonconvergence_threshold: u32,
    stuck_threshold_secs: u64,
    state_root: &Path,
) -> Result<()> {
    // Hold the per-repo busy marker for the DURATION of the revision, recording
    // the change slug being revised. Acquired BEFORE the transcript fetch so
    // EVERY terminal path below — transcript-unreadable refusal, scope/edit
    // violation, gate could-not-run, budget exhausted, clean PR, or error —
    // releases it via the guard's Drop (the same RAII mechanism the normal pass
    // uses, so the existing stale-detection/recovery + the status `currently:`
    // branching apply unchanged). Effects: a `status` issued mid-revision reads
    // `working on <change>` instead of `idle`, AND a normal pass cannot run on
    // this workspace concurrently (per-repo concurrency).
    let _busy_guard = match busy_marker::try_acquire(
        paths,
        workspace,
        &repo.url,
        stuck_threshold_secs,
    ) {
        Ok(busy_marker::AcquireOutcome::Acquired(g)) => {
            // Record the change under revision so the `currently:` line renders
            // the change-non-empty branch (`working on <change>`).
            busy_marker::update_change(paths, workspace, &request.change_slug);
            g
        }
        Ok(busy_marker::AcquireOutcome::SkipFreshInProgress(details)) => {
            tracing::info!(
                url = %repo.url,
                change = %request.change_slug,
                pid = details.marker.pid,
                age = %busy_marker::format_age_human(details.age_secs),
                "revision-execute: per-repo busy marker held by another pass; not running a revision concurrently"
            );
            post_reply(
                Some(ctx),
                &request.channel,
                &request.thread_ts,
                "⏳ send it: the workspace is busy with another pass right now — `send it` again in a moment.",
            )
            .await;
            return Ok(());
        }
        Ok(busy_marker::AcquireOutcome::SkipAmbiguous(m)) => {
            tracing::error!(
                url = %repo.url,
                change = %request.change_slug,
                pid = m.pid,
                "revision-execute: ambiguous busy-marker state; not running a revision"
            );
            post_reply(
                Some(ctx),
                &request.channel,
                &request.thread_ts,
                "✗ send it: the workspace busy marker is in an ambiguous state (possible stuck pass) — investigate, then `send it` again.",
            )
            .await;
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let transcript = match fetch_transcript_bounded(
        fetcher,
        &request.channel,
        &request.thread_ts,
        transcript_fetch_retries,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                change = %request.change_slug,
                "revision-execute: transcript fetch failed after bounded retry — not revising blind (no PR): {e:#}"
            );
            post_reply(
                Some(ctx),
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: could not read the discussion thread — not revising \
                     blind. No PR opened.\nReason: {e:#}\n• If this mentions \
                     `missing_scope` / `not_in_channel`, the Slack bot lacks the \
                     channel-history scope — add `channels:history` (public) or \
                     `groups:history` (private), reinstall the app, and invite the \
                     bot to this channel.\n• If it mentions `ratelimited` / `http \
                     429`, `send it` again in a moment."
                ),
            )
            .await;
            return Ok(());
        }
    };
    run_revision_execute(
        deps,
        workspace,
        repo,
        github_cfg,
        Some(ctx),
        request,
        &transcript,
        converge_attempts,
        nonconvergence_threshold,
        state_root,
    )
    .await
}

/// Orchestration shared by production AND tests. Recreates the revision
/// branch, runs the edit session, enforces the scope guardrails, re-gates,
/// AND either opens a PR (clean) or reports back (contradiction / could-not-
/// run). May converge across a bounded number of edit→re-gate attempts within
/// one `send it` (`converge_attempts` ADDITIONAL passes beyond the first),
/// accumulating fixes on the same revision branch; refreshes the durable
/// marker with each re-gate's current findings; AND escalates by naming a
/// finding identity that survives the bound. Always restores the base-branch
/// checkout on exit.
#[allow(clippy::too_many_arguments)]
async fn run_revision_execute(
    deps: &ExecutorDeps<'_>,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &RevisionExecuteRequest,
    transcript: &[ThreadMessage],
    converge_attempts: u32,
    nonconvergence_threshold: u32,
    state_root: &Path,
) -> Result<()> {
    let change_slug = &request.change_slug;
    // Defensive idempotency: a thread already acted on (a prior `send it`
    // opened a PR) does not re-run (the dispatcher also guards this).
    if let Ok(Some(state)) = revision_thread::read_state(state_root, &request.thread_ts)
        && state.status == RevisionThreadStatus::Acted
    {
        post_reply(
            chatops_ctx,
            &request.channel,
            &request.thread_ts,
            &format!("✓ A PR has already been opened for the revision of `{change_slug}`. Review/merge it, or reply to discuss further."),
        )
        .await;
        return Ok(());
    }

    // Work on a dedicated revision branch off base — never the base branch.
    let branch = revision_branch_name(&repo.agent_branch, change_slug);
    if let Err(e) = git::checkout(workspace, &repo.base_branch) {
        tracing::warn!("revision-execute: checkout of base branch failed: {e:#}");
    }
    if let Err(e) = git::recreate_branch(workspace, &branch) {
        post_reply(
            chatops_ctx,
            &request.channel,
            &request.thread_ts,
            &format!("✗ send it: could not prepare the revision branch `{branch}`: {e}"),
        )
        .await;
        return Ok(());
    }

    // Bounded converge loop within one `send it` (task 6): up to
    // `converge_attempts` ADDITIONAL edit→scope-check→re-gate passes beyond the
    // first, accumulating fixes on the SAME revision branch so a change with
    // multiple contradictions is resolved in one trigger. Each edit re-reads
    // the change's spec deltas as they now stand, AND its prompt enumerates the
    // marker's CURRENT findings (refreshed below). The loop ends on a clean
    // re-gate (→ PR), or on a terminal condition (edit failure, scope
    // violation, gate could-not-run, OR the budget exhausted with a
    // contradiction remaining).
    let max_iterations = converge_attempts.saturating_add(1);
    // Identities seen on the PRIOR re-gate, used to detect a finding that
    // SURVIVES an attempt (escalation, task 6.3) — keyed on structured
    // identity, never on message text.
    let mut prev_identities: Vec<crate::spec_revision::ContradictionIdentity> = Vec::new();
    for iteration in 0..max_iterations {
        let remaining = max_iterations - iteration - 1;

        // Edit the change's spec deltas along the discussed direction (the
        // prompt also enumerates the marker's current findings to resolve).
        let prompt = build_executor_prompt(workspace, change_slug, transcript);
        if let Err(e) = deps.edit.revise(workspace, &prompt).await {
            restore_base(workspace, repo);
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!("✗ send it: the revision session failed: {e}"),
            )
            .await;
            return Ok(());
        }

        // Scope + guardrail enforcement.
        let entries = match git::status_entries(workspace) {
            Ok(e) => e,
            Err(e) => {
                restore_base(workspace, repo);
                post_reply(
                    chatops_ctx,
                    &request.channel,
                    &request.thread_ts,
                    &format!("✗ send it: could not read the revision's git status: {e}"),
                )
                .await;
                return Ok(());
            }
        };
        let (leaked, tasks_edited) = classify_revision_changes(&entries, change_slug);
        if !leaked.is_empty() || tasks_edited {
            restore_base(workspace, repo);
            let mut why = Vec::new();
            if !leaked.is_empty() {
                why.push(format!("wrote outside the change directory: {}", leaked.join(", ")));
            }
            if tasks_edited {
                why.push("edited tasks.md (not allowed — the revision touches spec deltas only)".to_string());
            }
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: the revision session violated its scope and was discarded ({}). No PR opened.",
                    why.join("; ")
                ),
            )
            .await;
            return Ok(());
        }
        if !has_in_scope_edit(&entries, change_slug) {
            restore_base(workspace, repo);
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!("✗ send it: the revision session made no spec-delta edits for `{change_slug}`. Reply to discuss the direction, then `send it` again."),
            )
            .await;
            return Ok(());
        }

        // Re-gate BEFORE opening the PR (design D5): an operator-directed
        // revision cannot itself ship a new contradiction.
        match deps.regate.regate(workspace, change_slug).await {
            ReGateOutcome::Clean => break,
            ReGateOutcome::Contradiction { findings, text } => {
                // Refresh the durable marker with the CURRENT findings,
                // replacing the prior set, so the next edit (this loop) AND a
                // future `send it` are grounded in the present contradiction
                // (task 3). Best-effort: a write failure is logged AND never
                // changes the revision outcome. The marker is gitignored
                // untracked state and survives `restore_base`, so the refresh
                // persists for the next `send it`.
                if let Err(e) = crate::spec_revision::refresh_marker_contradictions(
                    workspace,
                    change_slug,
                    &findings,
                ) {
                    tracing::warn!(
                        change = %change_slug,
                        "revision-execute: refreshing the marker's contradiction set failed (continuing): {e:#}"
                    );
                }

                // Identities that SURVIVED this attempt (present now AND on the
                // prior re-gate) — the escalation set (task 6.3).
                let current_identities: Vec<crate::spec_revision::ContradictionIdentity> =
                    findings.iter().map(|f| f.identity()).collect();
                let survivors: Vec<&crate::spec_revision::ContradictionFindingRecord> = findings
                    .iter()
                    .filter(|f| prev_identities.contains(&f.identity()))
                    .collect();

                if remaining == 0 {
                    // Budget exhausted with a contradiction remaining → restore
                    // base AND report. This is a FAILED revision round: bump the
                    // per-change consecutive-failure counter so repeated
                    // non-convergence can nudge the operator toward decomposition
                    // (best-effort — a marker-write failure falls back to a count
                    // of 0, i.e. no nudge, so the reply still posts). When a
                    // finding identity survived the bounded attempts the reply
                    // NAMES it specifically (escalation); at the threshold it
                    // ALSO recommends decomposing the change.
                    restore_base(workspace, repo);
                    let consecutive =
                        match crate::spec_revision::record_failed_round(workspace, change_slug) {
                            Ok(n) => n,
                            Err(e) => {
                                tracing::warn!(
                                    change = %change_slug,
                                    "revision-execute: recording the consecutive-failed-round counter failed (continuing without the decomposition nudge): {e:#}"
                                );
                                0
                            }
                        };
                    let body = build_budget_exhausted_reply(
                        &survivors,
                        &text,
                        max_iterations,
                        consecutive,
                        nonconvergence_threshold,
                    );
                    post_reply(chatops_ctx, &request.channel, &request.thread_ts, &body).await;
                    return Ok(());
                }

                // Budget remains → re-edit + re-gate again on the SAME branch
                // (the refreshed marker now grounds the next edit).
                prev_identities = current_identities;
                continue;
            }
            ReGateOutcome::CouldNotRun(cause) => {
                // A gate that could not run is TERMINAL (the revision was not
                // verified) — no converge retry would change that.
                restore_base(workspace, repo);
                post_reply(
                    chatops_ctx,
                    &request.channel,
                    &request.thread_ts,
                    &format!(
                        "✗ send it: could not verify the revision (gate held) — no PR opened.\n{cause}"
                    ),
                )
                .await;
                return Ok(());
            }
        }
    }

    // Clean re-gate → stage the change directory's revised deltas, then
    // unstage the daemon's `.needs-spec-revision.json` marker so it never
    // rides into the PR (it is untracked bookkeeping, not a spec delta).
    // tasks.md is already guaranteed unmodified by the guardrail above.
    let change_dir = change_dir_rel(change_slug);
    let marker = marker_rel_path(change_slug);
    let _ = std::process::Command::new("git")
        .args(["add", "--", &change_dir])
        .current_dir(workspace)
        .status();
    let _ = std::process::Command::new("git")
        .args(["reset", "-q", "--", &marker])
        .current_dir(workspace)
        .status();
    let subject = format!("spec-revision: resolve contradiction in `{change_slug}`");
    if let Err(e) = git::commit(workspace, &subject) {
        restore_base(workspace, repo);
        post_reply(
            chatops_ctx,
            &request.channel,
            &request.thread_ts,
            &format!("✗ send it: re-gate passed but the commit failed: {e}. No PR opened."),
        )
        .await;
        return Ok(());
    }
    let title = format!("spec-revision: `{change_slug}`");
    let body = build_pr_body(change_slug, &repo.url);
    let pr_url = match deps
        .pr
        .open_pr(workspace, repo, github_cfg, &branch, &title, &body)
        .await
    {
        Ok(u) => u,
        Err(e) => {
            restore_base(workspace, repo);
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!("✗ send it: re-gate passed but opening the PR failed: {e}."),
            )
            .await;
            return Ok(());
        }
    };

    // The change has cleared (a clean re-gate opened a PR): reset the per-change
    // consecutive-failure counter so a future first failure does not start at
    // the decomposition-nudge threshold. The marker is gitignored untracked
    // state that survives `restore_base`, so this reset persists. Best-effort:
    // a write failure is logged AND never changes the revision outcome.
    if let Err(e) = crate::spec_revision::reset_consecutive_failures(workspace, change_slug) {
        tracing::warn!(
            change = %change_slug,
            "revision-execute: resetting the consecutive-failure counter after a clean re-gate failed (continuing): {e:#}"
        );
    }

    // Flip the thread state to Acted so a repeat `send it` is handled
    // gracefully (task 4.3). Reconstruct a fresh state when none was stored
    // (degraded alert) so the acted flag still lands.
    let acted = match revision_thread::read_state(state_root, &request.thread_ts) {
        Ok(Some(mut s)) => {
            s.status = RevisionThreadStatus::Acted;
            s
        }
        _ => RevisionThreadState {
            thread_ts: request.thread_ts.clone(),
            channel: request.channel.clone(),
            repo_url: request.repo_url.clone(),
            change_slug: change_slug.clone(),
            status: RevisionThreadStatus::Acted,
            posted_at: chrono::Utc::now(),
        },
    };
    if let Err(e) = revision_thread::write_state(state_root, &acted) {
        tracing::warn!(
            change = %change_slug,
            "revision-execute: flipping thread state to Acted failed: {e:#}"
        );
    }

    restore_base(workspace, repo);
    post_reply(
        chatops_ctx,
        &request.channel,
        &request.thread_ts,
        &format!("✅ Revision PR opened for `{change_slug}`: {pr_url}\nReview + merge it to apply the revision."),
    )
    .await;
    Ok(())
}

/// Restore the base-branch checkout (discarding any in-flight revision tree)
/// so the next iteration phase starts clean. Best-effort.
fn restore_base(workspace: &Path, repo: &RepositoryConfig) {
    let _ = git::reset_hard_head(workspace);
    let _ = git::clean_force(workspace);
    let _ = git::checkout(workspace, &repo.base_branch);
}

/// Body for the spec-revision PR. Names the change AND the human-review gate.
pub(crate) fn build_pr_body(change_slug: &str, repo_url: &str) -> String {
    format!(
        "This PR revises the spec deltas of change `{change_slug}` in `{repo_url}` to resolve \
         a `[in]` / `[canon]` contradiction flagged at implement time.\n\n\
         The revision was directed by an operator in the change's spec-revision thread AND \
         re-verified against the `[in]` and `[canon]` gates before this PR opened. Review the \
         spec deltas; merging applies the revision. No code or tasks.md changes are included.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn msg(from_bot: bool, text: &str) -> ThreadMessage {
        ThreadMessage {
            from_bot,
            user_id: if from_bot { "Ubot".into() } else { "Uop".into() },
            text: text.into(),
            ts: "1.0".into(),
        }
    }

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    // ---------- pure helpers ----------

    #[test]
    fn render_transcript_labels_both_sides() {
        let t = vec![
            msg(true, "Contradiction: A says X, canon says Y."),
            msg(false, "Should we align to canon?"),
        ];
        let r = render_transcript(&t);
        assert!(r.contains("autocoder: Contradiction"), "{r}");
        assert!(r.contains("operator: Should we align"), "{r}");
    }

    #[test]
    fn render_transcript_empty_is_explicit() {
        assert_eq!(render_transcript(&[]), "(no prior messages in this thread)");
    }

    #[test]
    fn advisor_prompt_includes_transcript_change_and_marker() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write(
            &ws.join("openspec/changes/c1/specs/cap/spec.md"),
            "## ADDED Requirements\n\n### Requirement: A\nBody.\n",
        );
        let t = vec![msg(true, "alert body text"), msg(false, "first question")];
        let p = build_advisor_prompt(ws, "c1", "is canon wrong here?", &t);
        // read-only framing
        assert!(p.contains("READ-ONLY"), "{p}");
        assert!(p.contains("Do NOT write"), "{p}");
        // change + marker + spec delta path
        assert!(p.contains("`c1`"), "{p}");
        assert!(p.contains("openspec/changes/c1/.needs-spec-revision.json"), "{p}");
        assert!(p.contains("openspec/changes/c1/specs/cap/spec.md"), "{p}");
        // transcript + current reply
        assert!(p.contains("alert body text"), "{p}");
        assert!(p.contains("first question"), "{p}");
        assert!(p.contains("is canon wrong here?"), "{p}");
    }

    #[test]
    fn executor_prompt_forbids_tasks_and_scopes_to_change_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write(
            &ws.join("openspec/changes/c1/specs/cap/spec.md"),
            "## ADDED Requirements\n\n### Requirement: A\nBody.\n",
        );
        let p = build_executor_prompt(ws, "c1", &[msg(false, "align to canon's term")]);
        assert!(p.contains("EXECUTOR"), "{p}");
        assert!(p.contains("Do NOT edit tasks.md"), "{p}");
        assert!(p.contains("openspec/changes/c1/"), "{p}");
        assert!(p.contains("align to canon's term"), "{p}");
    }

    #[test]
    fn revision_branch_name_is_per_change() {
        assert_eq!(
            revision_branch_name("agent-q", "a03-spec-revision-thread"),
            "agent-q-spec-revision-a03-spec-revision-thread"
        );
    }

    #[test]
    fn classify_changes_flags_leaks_and_tasks_ignores_marker() {
        let entry = |p: &str| git::StatusEntry {
            staged: 'M',
            worktree: ' ',
            path: p.into(),
            orig_path: None,
        };
        let entries = vec![
            entry("openspec/changes/c1/proposal.md"),
            entry("openspec/changes/c1/specs/cap/spec.md"),
            entry("openspec/changes/c1/.needs-spec-revision.json"), // ignored
            entry("openspec/changes/c1/tasks.md"),                  // flagged
            entry("src/main.rs"),                                   // leak
        ];
        let (leaked, tasks_edited) = classify_revision_changes(&entries, "c1");
        assert_eq!(leaked, vec!["src/main.rs".to_string()]);
        assert!(tasks_edited);
    }

    #[test]
    fn has_in_scope_edit_true_only_for_spec_deltas() {
        let entry = |p: &str| git::StatusEntry {
            staged: 'M',
            worktree: ' ',
            path: p.into(),
            orig_path: None,
        };
        // only the marker → no real edit
        assert!(!has_in_scope_edit(
            &[entry("openspec/changes/c1/.needs-spec-revision.json")],
            "c1"
        ));
        // a spec delta → yes
        assert!(has_in_scope_edit(
            &[entry("openspec/changes/c1/specs/cap/spec.md")],
            "c1"
        ));
        // only tasks.md → not an in-scope spec edit
        assert!(!has_in_scope_edit(&[entry("openspec/changes/c1/tasks.md")], "c1"));
    }

    // ---------- advisor orchestration ----------

    struct CapturingAdvisor {
        answer: String,
        seen_prompt: Mutex<Option<String>>,
    }
    #[async_trait]
    impl AdvisorSessionRunner for CapturingAdvisor {
        async fn advise(&self, _ws: &Path, prompt: &str) -> Result<String> {
            *self.seen_prompt.lock().unwrap() = Some(prompt.to_string());
            Ok(self.answer.clone())
        }
    }

    struct RecordingChat {
        replies: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl crate::chatops::ChatOpsBackend for RecordingChat {
        fn provider_name(&self) -> &'static str {
            "recording"
        }
        fn is_experimental(&self) -> bool {
            true
        }
        async fn post_question(&self, _: &str, _: &str, _: &str) -> Result<String> {
            unreachable!()
        }
        async fn poll_thread_for_human_reply(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<crate::chatops::HumanReply>> {
            Ok(None)
        }
        async fn post_notification(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn post_threaded_reply(&self, _: &str, _: &str, text: &str) -> Result<()> {
            self.replies.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    fn ctx_for(chat: &std::sync::Arc<RecordingChat>) -> ChatOpsContext {
        ChatOpsContext {
            chatops: chat.clone(),
            channel: "C1".into(),
            start_work_enabled: false,
            failure_alerts_enabled: false,
            pr_opened_enabled: false,
        }
    }

    #[tokio::test]
    async fn advisor_posts_answer_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write(
            &ws.join("openspec/changes/c1/specs/cap/spec.md"),
            "## ADDED Requirements\n\n### Requirement: A\nBody.\n",
        );
        let before = std::fs::read_to_string(ws.join("openspec/changes/c1/specs/cap/spec.md")).unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let req = RevisionAdviseRequest {
            repo_url: "git@github.com:o/r.git".into(),
            change_slug: "c1".into(),
            channel: "C1".into(),
            thread_ts: "9.9".into(),
            reply_text: "align to canon?".into(),
            submitted_at: chrono::Utc::now(),
        };
        let runner = CapturingAdvisor {
            answer: "I recommend aligning to canon's term.".into(),
            seen_prompt: Mutex::new(None),
        };
        advise_with_runner(ws, &ctx, &req, &[], false, &runner).await.unwrap();
        let replies = chat.replies.lock().unwrap();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].contains("aligning to canon"), "{}", replies[0]);
        // wrote nothing
        let after = std::fs::read_to_string(ws.join("openspec/changes/c1/specs/cap/spec.md")).unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn advisor_second_reply_includes_first_exchange_via_transcript() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let req = RevisionAdviseRequest {
            repo_url: "git@github.com:o/r.git".into(),
            change_slug: "c1".into(),
            channel: "C1".into(),
            thread_ts: "9.9".into(),
            reply_text: "and what about the default?".into(),
            submitted_at: chrono::Utc::now(),
        };
        // The transcript carries the FIRST exchange.
        let transcript = vec![
            msg(false, "should we align to canon?"),
            msg(true, "yes, align to canon's existing term"),
        ];
        let runner = CapturingAdvisor {
            answer: "second answer".into(),
            seen_prompt: Mutex::new(None),
        };
        advise_with_runner(ws, &ctx, &req, &transcript, false, &runner)
            .await
            .unwrap();
        let seen = runner.seen_prompt.lock().unwrap().clone().unwrap();
        assert!(seen.contains("should we align to canon?"), "{seen}");
        assert!(seen.contains("align to canon's existing term"), "{seen}");
        assert!(seen.contains("and what about the default?"), "{seen}");
    }

    // ---------- executor orchestration (real git, injected edit/regate/PR) ----------

    fn run_git(path: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A git repo with a committed change dir (proposal + spec delta + marker).
    fn fixture_repo_with_change(change_slug: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        run_git(&ws, &["init", "-q", "-b", "main"]);
        run_git(&ws, &["config", "user.email", "t@e.com"]);
        run_git(&ws, &["config", "user.name", "t"]);
        write(
            &ws.join(format!("openspec/changes/{change_slug}/proposal.md")),
            "# Why\n\noriginal\n",
        );
        write(
            &ws.join(format!("openspec/changes/{change_slug}/tasks.md")),
            "- [ ] 1.1 do it\n",
        );
        write(
            &ws.join(format!("openspec/changes/{change_slug}/specs/cap/spec.md")),
            "## ADDED Requirements\n\n### Requirement: A\nOriginal body.\n",
        );
        run_git(&ws, &["add", "-A"]);
        run_git(&ws, &["commit", "-q", "-m", "seed change"]);
        // Mirror production: the marker is registered in .git/info/exclude at
        // workspace init, so `git clean -fd` (in restore_base) does NOT delete
        // it — the refresh persists for the next `send it`.
        crate::workspace::ensure_git_info_excluded(&ws, ".needs-spec-revision.json").unwrap();
        // The contradiction marker is written post-commit (untracked).
        write(
            &ws.join(format!(
                "openspec/changes/{change_slug}/.needs-spec-revision.json"
            )),
            "{\"change\":\"x\"}",
        );
        (dir, ws)
    }

    struct EditSpec {
        body: String,
    }
    #[async_trait]
    impl EditSessionRunner for EditSpec {
        async fn revise(&self, ws: &Path, _prompt: &str) -> Result<()> {
            // Edit a spec delta in-scope.
            std::fs::write(
                ws.join("openspec/changes/c1/specs/cap/spec.md"),
                &self.body,
            )
            .unwrap();
            Ok(())
        }
    }
    struct NoopEdit;
    #[async_trait]
    impl EditSessionRunner for NoopEdit {
        async fn revise(&self, _ws: &Path, _prompt: &str) -> Result<()> {
            Ok(())
        }
    }
    struct EditTasks;
    #[async_trait]
    impl EditSessionRunner for EditTasks {
        async fn revise(&self, ws: &Path, _prompt: &str) -> Result<()> {
            std::fs::write(ws.join("openspec/changes/c1/tasks.md"), "- [x] 1.1 dodged\n")
                .unwrap();
            Ok(())
        }
    }

    struct CannedReGate(ReGateOutcome);
    #[async_trait]
    impl ReGateRunner for CannedReGate {
        async fn regate(&self, _ws: &Path, _slug: &str) -> ReGateOutcome {
            self.0.clone()
        }
    }

    struct FakePr {
        url: String,
        calls: std::sync::Arc<Mutex<usize>>,
    }
    #[async_trait]
    impl RevisionPrOpener for FakePr {
        async fn open_pr(
            &self,
            _ws: &Path,
            _repo: &RepositoryConfig,
            _gh: &GithubConfig,
            _head: &str,
            _title: &str,
            _body: &str,
        ) -> Result<String> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.url.clone())
        }
    }

    /// Edit runner that performs a real in-scope spec-delta edit each call,
    /// records every prompt it receives, AND counts invocations. Lets the
    /// converge + resolve-all tests assert the data flow (how many edits ran,
    /// what the edit was grounded in) without a CLI.
    struct RecordingEdit {
        prompts: std::sync::Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl EditSessionRunner for RecordingEdit {
        async fn revise(&self, ws: &Path, prompt: &str) -> Result<()> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            // A real in-scope edit so the scope/has-edit guards pass; the body
            // varies per call so successive edits are distinct git states.
            let n = self.prompts.lock().unwrap().len();
            std::fs::write(
                ws.join("openspec/changes/c1/specs/cap/spec.md"),
                format!("## ADDED Requirements\n\n### Requirement: A\nRevision pass {n}.\n"),
            )
            .unwrap();
            Ok(())
        }
    }

    /// Re-gate runner that plays back a SCRIPTED sequence of outcomes (one per
    /// call); the last entry repeats once the script is exhausted. Drives the
    /// converge-loop + escalation tests.
    struct ScriptedReGate {
        script: Vec<ReGateOutcome>,
        calls: std::sync::Arc<Mutex<usize>>,
    }
    #[async_trait]
    impl ReGateRunner for ScriptedReGate {
        async fn regate(&self, _ws: &Path, _slug: &str) -> ReGateOutcome {
            let mut g = self.calls.lock().unwrap();
            let idx = (*g).min(self.script.len() - 1);
            *g += 1;
            self.script[idx].clone()
        }
    }

    /// Transcript fetcher that fails its first `fail_count` calls, then returns
    /// `messages`; counts calls. `fail_count == u32::MAX` fails every attempt.
    struct ScriptedTranscriptFetcher {
        fail_count: u32,
        messages: Vec<ThreadMessage>,
        calls: std::sync::Arc<Mutex<u32>>,
    }
    #[async_trait]
    impl TranscriptFetcher for ScriptedTranscriptFetcher {
        async fn fetch(&self, _channel: &str, _thread_ts: &str) -> Result<Vec<ThreadMessage>> {
            let mut g = self.calls.lock().unwrap();
            let n = *g;
            *g += 1;
            if n < self.fail_count {
                Err(anyhow!("simulated transcript fetch failure (attempt {n})"))
            } else {
                Ok(self.messages.clone())
            }
        }
    }

    /// A `[canon]` finding record for the converge/escalation tests.
    fn canon_record(a: &str, b: &str, cap: &str) -> crate::spec_revision::ContradictionFindingRecord {
        crate::spec_revision::ContradictionFindingRecord {
            gate: crate::spec_revision::ContradictionGate::Canon,
            requirement_a: a.into(),
            requirement_b: b.into(),
            canonical_capability: cap.into(),
            summary: format!("{a} conflicts with {b}"),
            suggested_fix: String::new(),
        }
    }

    fn test_repo(ws: &Path) -> RepositoryConfig {
        RepositoryConfig {
            forge: None,
            url: "git@github.com:o/r.git".into(),
            local_path: Some(ws.to_path_buf()),
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

    fn test_github() -> GithubConfig {
        GithubConfig {
            token_env: "X".into(),
            token: None,
            owner_tokens: None,
            fork_owner: None,
            recreate_fork_on_reinit: false,
            command_authorization: Default::default(),
        }
    }

    fn execute_request(thread_ts: &str) -> RevisionExecuteRequest {
        RevisionExecuteRequest {
            repo_url: "git@github.com:o/r.git".into(),
            change_slug: "c1".into(),
            channel: "C1".into(),
            thread_ts: thread_ts.into(),
            submitted_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn executor_clean_regate_opens_pr_and_flips_status() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        // Seed an Open thread state.
        revision_thread::write_state(
            state_dir.path(),
            &RevisionThreadState {
                thread_ts: "9.9".into(),
                channel: "C1".into(),
                repo_url: repo.url.clone(),
                change_slug: "c1".into(),
                status: RevisionThreadStatus::Open,
                posted_at: chrono::Utc::now(),
            },
        )
        .unwrap();
        let calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &EditSpec {
                body: "## ADDED Requirements\n\n### Requirement: A\nRevised body.\n".into(),
            },
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr {
                url: "https://example/pr/1".into(),
                calls: calls.clone(),
            },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
             0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        // PR opened + link reported.
        assert_eq!(*calls.lock().unwrap(), 1, "PR opener called once");
        let replies = chat.replies.lock().unwrap();
        assert!(replies.iter().any(|r| r.contains("https://example/pr/1")), "{replies:?}");
        // Status flipped to Acted.
        let st = revision_thread::read_state(state_dir.path(), "9.9").unwrap().unwrap();
        assert_eq!(st.status, RevisionThreadStatus::Acted);
        // The revision was committed to the branch, NOT to base: base HEAD is
        // unchanged (the working tree is restored to base).
        run_git(&ws, &["rev-parse", "--verify", "main"]);
        // tasks.md unchanged on the branch (only spec delta + proposal staged).
        let tasks = std::fs::read_to_string(ws.join("openspec/changes/c1/tasks.md")).unwrap();
        assert_eq!(tasks, "- [ ] 1.1 do it\n");
    }

    #[tokio::test]
    async fn executor_contradiction_regate_opens_no_pr() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &EditSpec {
                body: "## ADDED Requirements\n\n### Requirement: A\nStill conflicting.\n".into(),
            },
            regate: &CannedReGate(ReGateOutcome::Contradiction {
                findings: vec![crate::spec_revision::ContradictionFindingRecord {
                    gate: crate::spec_revision::ContradictionGate::Canon,
                    requirement_a: "A".into(),
                    requirement_b: "canon B".into(),
                    canonical_capability: "cap".into(),
                    summary: "A still conflicts with canon B".into(),
                    suggested_fix: String::new(),
                }],
                text: "A still conflicts with canon B".into(),
            }),
            pr: &FakePr {
                url: "https://example/pr/x".into(),
                calls: calls.clone(),
            },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
             0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 0, "no PR on a still-failing re-gate");
        let replies = chat.replies.lock().unwrap();
        assert!(
            replies.iter().any(|r| r.contains("still fails the gates") && r.contains("A still conflicts")),
            "{replies:?}"
        );
        // No revision-thread state created (none seeded) → still nothing acted.
        assert!(revision_thread::read_state(state_dir.path(), "9.9").unwrap().is_none());
    }

    #[tokio::test]
    async fn executor_tasks_edit_is_rejected_no_pr() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &EditTasks,
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr {
                url: "https://example/pr/x".into(),
                calls: calls.clone(),
            },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
             0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 0, "editing tasks.md must not open a PR");
        let replies = chat.replies.lock().unwrap();
        assert!(replies.iter().any(|r| r.contains("tasks.md")), "{replies:?}");
    }

    #[tokio::test]
    async fn executor_no_edit_reports_and_no_pr() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &NoopEdit,
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr {
                url: "https://example/pr/x".into(),
                calls: calls.clone(),
            },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
             0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 0);
        let replies = chat.replies.lock().unwrap();
        assert!(replies.iter().any(|r| r.contains("no spec-delta edits")), "{replies:?}");
    }

    #[tokio::test]
    async fn executor_already_acted_short_circuits() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        revision_thread::write_state(
            state_dir.path(),
            &RevisionThreadState {
                thread_ts: "9.9".into(),
                channel: "C1".into(),
                repo_url: "git@github.com:o/r.git".into(),
                change_slug: "c1".into(),
                status: RevisionThreadStatus::Acted,
                posted_at: chrono::Utc::now(),
            },
        )
        .unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &NoopEdit,
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr {
                url: "x".into(),
                calls: calls.clone(),
            },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
             0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 0, "an acted thread does not re-run");
        let replies = chat.replies.lock().unwrap();
        assert!(replies.iter().any(|r| r.contains("already been opened")), "{replies:?}");
    }

    // ---------- fail-closed transcript (task 5) ----------

    /// A transcript fetch that fails EVERY attempt → the edit session is NOT
    /// invoked, NO PR is opened, AND a thread reply is posted (fail-closed; the
    /// executor never revises blind).
    #[tokio::test]
    async fn execute_fail_closed_transcript_skips_edit_and_pr() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        let fetcher = ScriptedTranscriptFetcher {
            fail_count: u32::MAX, // fail every attempt
            messages: Vec::new(),
            calls: std::sync::Arc::new(Mutex::new(0)),
        };
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        execute_with_deps(
            &paths,
            &deps,
            &fetcher,
            &ws,
            &repo,
            &gh,
            &ctx,
            &execute_request("9.9"),
            2, // transcript_fetch_retries
            0,
            3, // nonconvergence_threshold
            1800, // stuck_threshold_secs
            state_dir.path(),
        )
        .await
        .unwrap();
        assert!(
            edit_prompts.lock().unwrap().is_empty(),
            "the edit session must NOT run when the thread cannot be read"
        );
        assert_eq!(*pr_calls.lock().unwrap(), 0, "no PR when revising blind is refused");
        let replies = chat.replies.lock().unwrap();
        assert!(
            replies.iter().any(|r| r.contains("could not read the discussion")),
            "must report it could not read the thread: {replies:?}"
        );
        // The marker acquired before the transcript fetch is released on the
        // unreadable-thread refusal path (task 1.2) — no lingering marker.
        assert!(
            busy_marker::current(&paths, &ws, 1800).is_none(),
            "the busy marker must be released after the transcript-unreadable refusal"
        );
    }

    /// A transcript fetch that FAILS then SUCCEEDS within the retry budget → the
    /// revision proceeds (the edit session runs, the clean re-gate opens a PR).
    #[tokio::test]
    async fn execute_transcript_fail_then_succeed_proceeds() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr { url: "https://example/pr/ok".into(), calls: pr_calls.clone() },
        };
        let fetcher = ScriptedTranscriptFetcher {
            fail_count: 1, // fail attempt 0, succeed attempt 1
            messages: vec![msg(false, "align to canon")],
            calls: std::sync::Arc::new(Mutex::new(0)),
        };
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        execute_with_deps(
            &paths,
            &deps,
            &fetcher,
            &ws,
            &repo,
            &gh,
            &ctx,
            &execute_request("9.9"),
            2, // transcript_fetch_retries (>= the one failure)
            0,
            3, // nonconvergence_threshold
            1800, // stuck_threshold_secs
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            edit_prompts.lock().unwrap().len(),
            1,
            "the revision proceeds once the transcript loads on retry"
        );
        assert_eq!(*pr_calls.lock().unwrap(), 1, "a clean re-gate opens the PR");
        // Clean PR path released the marker (task 1.2 / 2.2).
        assert!(
            busy_marker::current(&paths, &ws, 1800).is_none(),
            "the busy marker must be released after a clean re-gate opens the PR"
        );
    }

    // ---------- resolve-all grounding (task 4) ----------

    /// A marker recording TWO structured findings grounds the edit session in
    /// BOTH (the prompt enumerates each finding identity to resolve). Asserts
    /// the data flow — both identities reach the session — not prose wording.
    #[tokio::test]
    async fn executor_grounds_edit_in_all_marker_findings() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        // Overwrite the fixture marker with TWO structured contradictions.
        crate::spec_revision::refresh_marker_contradictions(
            &ws,
            "c1",
            &[
                canon_record("change-req-ONE", "canon-req-ONE", "alpha"),
                canon_record("change-req-TWO", "canon-req-TWO", "beta"),
            ],
        )
        .unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr { url: "https://example/pr/2".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        let prompts = edit_prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1, "one edit pass");
        // BOTH finding identities were threaded to the edit session.
        assert!(prompts[0].contains("change-req-ONE"), "first finding must reach the session: {}", prompts[0]);
        assert!(prompts[0].contains("canon-req-ONE"), "{}", prompts[0]);
        assert!(prompts[0].contains("change-req-TWO"), "second finding must reach the session: {}", prompts[0]);
        assert!(prompts[0].contains("canon-req-TWO"), "{}", prompts[0]);
    }

    // ---------- converge loop + marker refresh (tasks 3 + 6) ----------

    /// A re-gate that returns Contradiction THEN Clean opens a PR within ONE
    /// `send it`: the edit runner is called twice, the PR is opened once, with
    /// no second operator trigger. `revision_converge_attempts >= 1`.
    #[tokio::test]
    async fn converge_contradiction_then_clean_opens_pr_in_one_send_it() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let regate_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &ScriptedReGate {
                script: vec![
                    ReGateOutcome::Contradiction {
                        findings: vec![canon_record("A", "canon-A", "cap")],
                        text: "still conflicting".into(),
                    },
                    ReGateOutcome::Clean,
                ],
                calls: regate_calls.clone(),
            },
            pr: &FakePr { url: "https://example/pr/converge".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            2, // converge_attempts → up to 3 passes
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(edit_prompts.lock().unwrap().len(), 2, "edit ran twice (re-edit on contradiction)");
        assert_eq!(*regate_calls.lock().unwrap(), 2, "re-gated twice");
        assert_eq!(*pr_calls.lock().unwrap(), 1, "the clean re-gate opens the PR in one send it");
        let replies = chat.replies.lock().unwrap();
        assert!(
            replies.iter().any(|r| r.contains("https://example/pr/converge")),
            "the PR link is reported: {replies:?}"
        );
    }

    /// With `revision_converge_attempts: 0` (single-pass), the first re-gate
    /// contradiction reports back as today — no re-edit, no PR.
    #[tokio::test]
    async fn converge_zero_reports_first_contradiction() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let regate_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &ScriptedReGate {
                script: vec![ReGateOutcome::Contradiction {
                    findings: vec![canon_record("A", "canon-A", "cap")],
                    text: "still conflicting".into(),
                }],
                calls: regate_calls.clone(),
            },
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0, // single-pass
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(edit_prompts.lock().unwrap().len(), 1, "single-pass: edit ran once");
        assert_eq!(*regate_calls.lock().unwrap(), 1, "single-pass: re-gated once");
        assert_eq!(*pr_calls.lock().unwrap(), 0, "no PR on the first contradiction");
        let replies = chat.replies.lock().unwrap();
        assert!(replies.iter().any(|r| r.contains("no PR opened")), "{replies:?}");
    }

    /// A re-gate Contradiction refreshes the durable marker with the CURRENT
    /// findings, REPLACING the prior set (task 3). The fixture marker carries a
    /// stale prior set; after a single-pass contradiction the marker records the
    /// NEW finding. Asserts the marker's structured findings, not wording.
    #[tokio::test]
    async fn regate_contradiction_refreshes_marker_with_current_findings() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        // Seed the marker with a STALE prior contradiction.
        crate::spec_revision::refresh_marker_contradictions(
            &ws,
            "c1",
            &[canon_record("STALE-change", "STALE-canon", "old-cap")],
        )
        .unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Contradiction {
                findings: vec![canon_record("NEW-change", "NEW-canon", "new-cap")],
                text: "the current contradiction".into(),
            }),
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0,
            3,
            state_dir.path(),
        )
        .await
        .unwrap();
        let marker = crate::spec_revision::read_marker(&ws, "c1").unwrap().unwrap();
        assert_eq!(marker.contradictions.len(), 1, "the set is replaced, not appended");
        assert_eq!(marker.contradictions[0].requirement_a, "NEW-change");
        assert_eq!(marker.contradictions[0].requirement_b, "NEW-canon");
    }

    // ---------- escalation (task 6.3) ----------

    /// The SAME finding identity surviving the bounded attempts produces an
    /// exhaustion report that NAMES that finding's identity (the conflicting
    /// requirements), not an identical generic "still fails" line. Asserts the
    /// reported identity, not prose wording.
    #[tokio::test]
    async fn escalation_names_surviving_finding_identity() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let regate_calls = std::sync::Arc::new(Mutex::new(0));
        // The SAME finding identity every re-gate (only the summary phrasing
        // differs), so it survives the bounded attempts.
        let mut stuck_then = canon_record("STUCK-change", "STUCK-canon", "stuck-cap");
        stuck_then.summary = "phrasing changes but identity is the same".into();
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &ScriptedReGate {
                script: vec![
                    ReGateOutcome::Contradiction {
                        findings: vec![canon_record("STUCK-change", "STUCK-canon", "stuck-cap")],
                        text: "still fails".into(),
                    },
                    ReGateOutcome::Contradiction {
                        findings: vec![stuck_then],
                        text: "still fails".into(),
                    },
                ],
                calls: regate_calls.clone(),
            },
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            1, // one additional attempt → 2 passes, both contradict the same identity
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*pr_calls.lock().unwrap(), 0, "no PR when the contradiction persists");
        let replies = chat.replies.lock().unwrap();
        // The escalation NAMES the surviving requirement identity.
        assert!(
            replies.iter().any(|r| r.contains("STUCK-change") && r.contains("STUCK-canon")),
            "the escalation must name the stuck requirement identity: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| r.contains("not clearing") || r.contains("persistent")),
            "the escalation must say the revision is not clearing it: {replies:?}"
        );
    }

    // ---------- non-convergence nudges decomposition (this change) ----------

    /// Pure reply builder: at the threshold the reply names the contradiction
    /// (`no PR opened`) AND recommends decomposition (task 3.1).
    #[test]
    fn budget_reply_recommends_decomposition_at_threshold() {
        let body = build_budget_exhausted_reply(&[], "gate summary", 3, 3, 3);
        assert!(body.contains("no PR opened"), "{body}");
        assert!(body.to_lowercase().contains("decompose"), "{body}");
        assert!(body.to_lowercase().contains("too large"), "{body}");
    }

    /// Below the threshold the reply is the existing "names the contradiction,
    /// invites another `send it`" form with NO decomposition nudge (task 3.2).
    #[test]
    fn budget_reply_unchanged_below_threshold() {
        let body = build_budget_exhausted_reply(&[], "gate summary", 3, 2, 3);
        assert!(body.contains("no PR opened"), "{body}");
        assert!(body.contains("send it"), "{body}");
        assert!(!body.to_lowercase().contains("decompose"), "{body}");
    }

    /// A threshold of zero disables the nudge entirely (defensive: a count is
    /// always ≥1 at a failure, so `>= 0` must NOT mean always-nudge).
    #[test]
    fn budget_reply_threshold_zero_disables_nudge() {
        let body = build_budget_exhausted_reply(&[], "gate summary", 3, 9, 0);
        assert!(!body.to_lowercase().contains("decompose"), "{body}");
    }

    /// End-to-end (task 3.1): with two prior failed rounds recorded, a third
    /// budget-exhausted `send it` reaches the default threshold of 3 — the reply
    /// recommends decomposition while still naming the failed revision, AND the
    /// per-change counter advanced to the threshold.
    #[tokio::test]
    async fn nudge_at_threshold_recommends_decomposition() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        // Pre-seed two prior failed rounds; THIS run is the third.
        crate::spec_revision::record_failed_round(&ws, "c1").unwrap();
        crate::spec_revision::record_failed_round(&ws, "c1").unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Contradiction {
                findings: vec![canon_record("A", "canon-A", "cap")],
                text: "still conflicting".into(),
            }),
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0, // single-pass → budget exhausted on the first contradiction
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*pr_calls.lock().unwrap(), 0, "no PR when the contradiction persists");
        let replies = chat.replies.lock().unwrap();
        assert!(
            replies.iter().any(|r| r.to_lowercase().contains("decompose")),
            "at the threshold the reply must recommend decomposition: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| r.contains("no PR opened")),
            "the reply still names the failed revision: {replies:?}"
        );
        assert_eq!(
            crate::spec_revision::read_marker(&ws, "c1")
                .unwrap()
                .unwrap()
                .consecutive_failed_rounds,
            3,
            "the failed round advanced the counter to the threshold"
        );
    }

    /// End-to-end (task 3.2): the FIRST budget-exhausted `send it` (count 1 <
    /// threshold 3) gets the existing reply — names the contradiction, invites
    /// another `send it` — with no decomposition nudge.
    #[tokio::test]
    async fn below_threshold_no_decomposition_nudge() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Contradiction {
                findings: vec![canon_record("A", "canon-A", "cap")],
                text: "still conflicting".into(),
            }),
            pr: &FakePr { url: "x".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0,
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        let replies = chat.replies.lock().unwrap();
        assert!(
            replies.iter().any(|r| r.contains("no PR opened") && r.contains("send it")),
            "below the threshold the reply names the contradiction and invites another send it: {replies:?}"
        );
        assert!(
            replies.iter().all(|r| !r.to_lowercase().contains("decompose")),
            "below the threshold the reply must NOT recommend decomposition: {replies:?}"
        );
    }

    /// End-to-end (task 3.3): a clean re-gate that opens a PR resets the
    /// per-change consecutive-failure counter to zero, so a later first failure
    /// does not immediately trigger the decomposition nudge.
    #[tokio::test]
    async fn clean_regate_resets_consecutive_failure_count() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        // Two prior failed rounds recorded in the marker.
        crate::spec_revision::record_failed_round(&ws, "c1").unwrap();
        crate::spec_revision::record_failed_round(&ws, "c1").unwrap();
        assert_eq!(
            crate::spec_revision::read_marker(&ws, "c1")
                .unwrap()
                .unwrap()
                .consecutive_failed_rounds,
            2
        );
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let edit_prompts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit: &RecordingEdit { prompts: edit_prompts.clone() },
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr { url: "https://example/pr/clean".into(), calls: pr_calls.clone() },
        };
        run_revision_execute(
            &deps,
            &ws,
            &repo,
            &gh,
            Some(&ctx),
            &execute_request("9.9"),
            &[],
            0,
            3, // nonconvergence_threshold
            state_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(*pr_calls.lock().unwrap(), 1, "a clean re-gate opens the PR");
        assert_eq!(
            crate::spec_revision::read_marker(&ws, "c1")
                .unwrap()
                .unwrap()
                .consecutive_failed_rounds,
            0,
            "a clean re-gate (PR opened) resets the consecutive-failure counter"
        );
    }

    // ---------- busy marker held across the revision (revision-holds-busy-marker) ----------

    /// Edit runner that peeks the per-repo busy marker WHILE the revision is in
    /// flight (recording the rendered `currently:` line AND whether a concurrent
    /// acquire is blocked), then performs a real in-scope spec-delta edit so the
    /// scope/has-edit guards pass. Lets the in-flight assertions observe the
    /// marker the converge loop is holding.
    struct MarkerPeekEdit {
        paths: std::sync::Arc<crate::paths::DaemonPaths>,
        repo_url: String,
        currently: std::sync::Arc<Mutex<Option<String>>>,
        concurrent_acquire_blocked: std::sync::Arc<Mutex<Option<bool>>>,
    }
    #[async_trait]
    impl EditSessionRunner for MarkerPeekEdit {
        async fn revise(&self, ws: &Path, _prompt: &str) -> Result<()> {
            // (2.1) Peek the marker exactly as the `status` verb does, and
            // render the `currently:` line from it.
            let summary = busy_marker::current(&self.paths, ws, 1800);
            *self.currently.lock().unwrap() = Some(
                crate::chatops::operator_commands::format_currently_line(summary.as_ref()),
            );
            // (2.3) A normal pass acquiring the SAME repo now must be blocked
            // (per-repo concurrency). `try_acquire` finds the held marker (our
            // own live pid, fresh) and yields a skip — never `Acquired`.
            let blocked = !matches!(
                busy_marker::try_acquire(&self.paths, ws, &self.repo_url, 1800),
                Ok(busy_marker::AcquireOutcome::Acquired(_))
            );
            *self.concurrent_acquire_blocked.lock().unwrap() = Some(blocked);
            // Real in-scope edit so the revision proceeds past the scope guards.
            std::fs::write(
                ws.join("openspec/changes/c1/specs/cap/spec.md"),
                "## ADDED Requirements\n\n### Requirement: A\nRevised in-flight.\n",
            )
            .unwrap();
            Ok(())
        }
    }

    /// (2.1 + 2.3) While a revision is editing, the per-repo busy marker is
    /// stamped with the change slug so the `currently:` line reads `working on
    /// <slug>` and NOT `idle`; AND a normal pass cannot acquire the marker for
    /// that repo. After the revision completes the marker is released (2.2).
    #[tokio::test]
    async fn revision_in_flight_marker_is_stamped_and_blocks_a_pass() {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        let paths = std::sync::Arc::new(paths);
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let currently = std::sync::Arc::new(Mutex::new(None));
        let blocked = std::sync::Arc::new(Mutex::new(None));
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let peek = MarkerPeekEdit {
            paths: paths.clone(),
            repo_url: repo.url.clone(),
            currently: currently.clone(),
            concurrent_acquire_blocked: blocked.clone(),
        };
        let deps = ExecutorDeps {
            edit: &peek,
            regate: &CannedReGate(ReGateOutcome::Clean),
            pr: &FakePr {
                url: "https://example/pr/inflight".into(),
                calls: pr_calls.clone(),
            },
        };
        let fetcher = ScriptedTranscriptFetcher {
            fail_count: 0,
            messages: vec![msg(false, "align to canon")],
            calls: std::sync::Arc::new(Mutex::new(0)),
        };
        execute_with_deps(
            &paths,
            &deps,
            &fetcher,
            &ws,
            &repo,
            &gh,
            &ctx,
            &execute_request("9.9"),
            0,
            0,
            3, // nonconvergence_threshold
            1800,
            state_dir.path(),
        )
        .await
        .unwrap();
        // (2.1) Mid-revision the `currently:` line surfaced the change slug,
        // never `idle`.
        let line = currently
            .lock()
            .unwrap()
            .clone()
            .expect("the edit ran while the marker was held");
        assert!(line.contains("working on c1"), "currently in flight: {line}");
        assert!(!line.contains("idle"), "must not be idle while revising: {line}");
        // (2.3) A concurrent pass acquire for the same repo was blocked.
        assert_eq!(
            blocked.lock().unwrap().clone(),
            Some(true),
            "a normal pass must not acquire the marker while a revision holds it"
        );
        // (2.2) After completion the marker is released — no lingering marker.
        assert!(
            busy_marker::current(&paths, &ws, 1800).is_none(),
            "the busy marker is released once the revision completes"
        );
    }

    /// (2.2) Run one revision terminal path and assert the per-repo busy marker
    /// is released afterward. The transcript always loads; `edit`/`regate`/
    /// `converge_attempts` select the path under test.
    async fn assert_marker_released_after(
        edit: &dyn EditSessionRunner,
        regate: ReGateOutcome,
        converge_attempts: u32,
    ) {
        let (_d, ws) = fixture_repo_with_change("c1");
        let state_dir = TempDir::new().unwrap();
        let (_td_paths, paths) = crate::testing::test_daemon_paths();
        let chat = std::sync::Arc::new(RecordingChat {
            replies: Mutex::new(Vec::new()),
        });
        let ctx = ctx_for(&chat);
        let repo = test_repo(&ws);
        let gh = test_github();
        let pr_calls = std::sync::Arc::new(Mutex::new(0));
        let deps = ExecutorDeps {
            edit,
            regate: &CannedReGate(regate),
            pr: &FakePr {
                url: "x".into(),
                calls: pr_calls.clone(),
            },
        };
        let fetcher = ScriptedTranscriptFetcher {
            fail_count: 0,
            messages: vec![msg(false, "align to canon")],
            calls: std::sync::Arc::new(Mutex::new(0)),
        };
        execute_with_deps(
            &paths,
            &deps,
            &fetcher,
            &ws,
            &repo,
            &gh,
            &ctx,
            &execute_request("9.9"),
            0,
            converge_attempts,
            3, // nonconvergence_threshold
            1800,
            state_dir.path(),
        )
        .await
        .unwrap();
        assert!(
            busy_marker::current(&paths, &ws, 1800).is_none(),
            "the busy marker must be released after the revision completes"
        );
    }

    /// (2.2) Budget exhausted with a contradiction remaining → marker released.
    #[tokio::test]
    async fn marker_released_on_budget_exhausted() {
        assert_marker_released_after(
            &EditSpec {
                body: "## ADDED Requirements\n\n### Requirement: A\nStill conflicting.\n".into(),
            },
            ReGateOutcome::Contradiction {
                findings: vec![canon_record("A", "canon-A", "cap")],
                text: "still conflicting".into(),
            },
            0,
        )
        .await;
    }

    /// (2.2) A scope/edit-guardrail violation (tasks.md) → marker released.
    #[tokio::test]
    async fn marker_released_on_scope_violation() {
        assert_marker_released_after(&EditTasks, ReGateOutcome::Clean, 0).await;
    }

    /// (2.2) A gate that could-not-run → marker released.
    #[tokio::test]
    async fn marker_released_on_gate_could_not_run() {
        assert_marker_released_after(
            &EditSpec {
                body: "## ADDED Requirements\n\n### Requirement: A\nRevised.\n".into(),
            },
            ReGateOutcome::CouldNotRun("[in] gate disabled".into()),
            0,
        )
        .await;
    }
}
