//! Change-vs-canonical contradiction pre-flight check — the `[canon]` gate
//! of the verifier framework (a61; realized by a62).
//!
//! The `[in]` gate (a59) catches a change that contradicts *itself*. It
//! cannot catch a change that is internally coherent but contradicts an
//! *already-canonical* requirement — a delta that re-specifies a behavior the
//! project has locked elsewhere, or asserts a default a canonical requirement
//! forbids. The `[canon]` gate closes that gap with a pre-executor check of
//! the change's deltas against the EXISTING canonical specs.
//!
//! It is the natural sibling of the `[in]` gate: same lifecycle position
//! (pre-executor), same opt-in + fail-CLOSED posture, same agentic transport
//! (a56 [`crate::agentic_run`] + a `submit_*` tool). The check runs a
//! CLI-wrapped agentic session in a read-only sandbox (`Read`, `Glob`, `Grep`
//! — NO `Bash`/`Write`/`Edit`) with `ORCH_MCP_ROLE = canon_contradiction_check`
//! AND the `submit_canon_contradictions` MCP tool. The agent reads the
//! change's spec-delta files AND the canonical specs on demand — directly via
//! `Read` of `openspec/specs/*/spec.md`, OR via the common `query_canonical_specs`
//! MCP tool when a21's RAG is enabled — AND returns its findings by calling
//! `submit_canon_contradictions`.
//!
//! The check **fails CLOSED** (gatekeepers-fail-closed standard): a session
//! error (spawn, timeout, a resolved CLI strategy that is not registered yet),
//! a schema-rejected submission the agent never corrects, OR a session that
//! ends with no submission all log a WARN (carrying the `[verifier:canon]`
//! label) AND HOLD the change (an `Errored` outcome — the change was NOT
//! evaluated), never waved through as "no contradictions found". An empty
//! submission is a clean pass. The shared session/retry/fail-closed machinery
//! lives in [`crate::preflight::corpus_check`]; this module instantiates it
//! with the canonical-spec corpus (the `[rules]` gate instantiates the same
//! core with the global rule corpus).

use crate::agentic_run::ResolvedModel;
use crate::preflight::corpus_check::{
    CliCorpusCheckSessionRunner, CorpusCheckSession, CorpusCheckSessionRunner, run_corpus_check_with_runner,
};
use crate::verifier_gate::VerifierGate;
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The MCP role AND submission routing key the canon-contradiction check
/// uses. The per-execution MCP child advertises `submit_canon_contradictions`
/// ONLY when `ORCH_MCP_ROLE` equals this value; the daemon-side schema
/// validator is registered under the same key (a56/a62).
pub const CANON_CONTRADICTION_CHECK_ROLE: &str = "canon_contradiction_check";

/// The full `--allowedTools` list the canon-contradiction-check sandbox
/// grants: the read-only file tools PLUS the qualified
/// `submit_canon_contradictions` MCP tool. Notably absent: `Bash`, `Write`,
/// `Edit`. The common `query_canonical_specs` tool is added separately by the
/// agentic-run layer (it is part of the daemon's MCP tool contract, available
/// when a21's RAG is configured). Delegates to the shared corpus-check core so
/// the `[canon]` AND `[rules]` gates derive their surface the same way. Exposed
/// so tests can assert the surface.
pub fn agentic_canon_contradiction_allowed_tools() -> Vec<String> {
    crate::preflight::corpus_check::allowed_tools_for_role(CANON_CONTRADICTION_CHECK_ROLE)
}

/// Runtime context for the canon-contradiction-check pre-flight.
///
/// Holds the agentic-transport pieces (parallel to the `[in]` gate's
/// `ContradictionCheckCtx`). The `model` tuple (a56) is translated into the
/// wrapped CLI's model-selection mechanism by the resolved
/// [`crate::agentic_run::CliStrategy`]; its `provider` also selects which CLI
/// strategy runs. `command` is the wrapped CLI binary (`executor.command`).
/// `prompt_template` is the resolved prompt body — either the embedded default
/// OR the override file's contents.
///
/// Constructed once at daemon startup when the check is enabled. The polling
/// loop reads it on every iteration via [`current`].
pub struct CanonContradictionCheckCtx {
    /// Wrapped CLI binary the agentic session spawns (`executor.command`).
    pub command: String,
    /// Resolved `(provider, model, api_base_url, api_key)` tuple (a56). The
    /// `claude` strategy translates it into `ANTHROPIC_*`; its `provider`
    /// selects the CLI strategy.
    pub model: ResolvedModel,
    /// Resolved prompt body (embedded default OR override file contents).
    pub prompt_template: String,
    /// Redaction-safe `<provider>/<model>` attribution (a49) for the
    /// configured canon-check model. Surfaced as
    /// `*Canon-contradiction-check: <provider>/<model>*` on the operator-facing
    /// findings alert. `None` only for test contexts built without a resolved
    /// config block.
    pub attribution: Option<String>,
    /// Bounded retry of the agentic session on a no-submission outcome
    /// (`executor.verifier_gate_retries`). Counts ADDITIONAL attempts; `0`
    /// is the historical single-attempt behavior. Only the flaky
    /// no-submission case retries — the gate still fails closed after the
    /// bound is exhausted (gatekeepers-fail-closed standard).
    pub retries: u32,
    /// Wall-clock cap for one agentic session, resolved from the SINGLE
    /// `executor.agentic_session_timeout_secs` (shared with the other gates,
    /// the reviewer, AND the revision sessions). Set once at daemon startup.
    pub timeout: Duration,
    /// Resolved daemon paths, used to persist the gate session's full captured
    /// output to a discoverable per-session log under `gates/`
    /// (verifier-gates-persist-session-log). `None` only for test/standalone
    /// contexts that opt out; production wires it from the daemon's
    /// `DaemonPaths`. The session log is written for EVERY outcome.
    pub paths: Option<Arc<crate::paths::DaemonPaths>>,
    /// Test-only injected `submit_canon_contradictions` submission, bypassing
    /// the CLI subprocess AND the control socket. `Some(Some(p))` stands in
    /// for a recorded payload; `Some(None)` simulates "agent never submitted";
    /// `None` (default/production) uses the real CLI + `consume_submission`
    /// path.
    #[cfg(test)]
    pub test_submission: Option<Option<serde_json::Value>>,
}

tokio::task_local! {
    /// Per-task canon-contradiction-check context. Set ONCE by [`scope`] at
    /// the top of the polling-task future; the polling loop reads it at each
    /// per-change pre-flight via [`current`]. Tests that do not call `scope`
    /// see `None`, so there is no global-state pollution.
    static CTX: Option<Arc<CanonContradictionCheckCtx>>;
}

/// Run `fut` with the given canon-contradiction-check context bound for the
/// duration of the future. `None` represents the disabled state; the polling
/// loop's [`current`] reader returns `None` AND the check is a no-op.
/// Production callers (one per polling task) wrap the top-level future once at
/// startup.
pub fn scope<F>(
    ctx: Option<Arc<CanonContradictionCheckCtx>>,
    fut: F,
) -> impl Future<Output = F::Output>
where
    F: Future,
{
    CTX.scope(ctx, fut)
}

/// Snapshot of the current task's context. `None` when the operator did not
/// opt in OR the surrounding task did not call [`scope`]. Cheap clone of an
/// `Arc`.
pub fn current() -> Option<Arc<CanonContradictionCheckCtx>> {
    CTX.try_with(|c| c.clone()).ok().flatten()
}

/// Default prompt template embedded at compile time. Overridable via
/// `executor.change_canonical_contradiction_check_prompt_path`.
pub const EMBEDDED_PROMPT: &str = include_str!("../../../prompts/change-vs-canonical-check.md");

/// Resolve the prompt template. `None` returns the embedded default.
/// `Some(path)` reads the override file; an empty file (after `trim`) is an
/// error so the daemon does NOT feed an empty prompt to the session.
pub fn load_prompt_template(override_path: Option<&Path>) -> Result<String> {
    match override_path {
        None => Ok(EMBEDDED_PROMPT.to_string()),
        Some(path) => {
            let body = std::fs::read_to_string(path).with_context(|| {
                format!(
                    "reading change-vs-canonical-check prompt override at {}",
                    path.display()
                )
            })?;
            if body.trim().is_empty() {
                return Err(anyhow!(
                    "change-vs-canonical-check prompt override at {} is empty; refusing to feed an empty prompt to the session",
                    path.display()
                ));
            }
            Ok(body)
        }
    }
}

/// One change-vs-canonical contradiction surfaced by
/// [`run_agentic_canon_contradiction_check`]. Mirrors the
/// `submit_canon_contradictions` payload's entry shape one-for-one. Unlike the
/// `[in]` gate's within-change finding (a `requirement_a`/`requirement_b`
/// pair), each finding here names the canonical requirement (by capability AND
/// title) that the change's requirement conflicts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonContradictionFinding {
    pub change_requirement: String,
    pub canonical_capability: String,
    pub canonical_requirement: String,
    /// One-line explanation of WHY the change's requirement and the canonical
    /// requirement cannot both hold.
    pub summary: String,
    /// Concrete edit plan — WHAT to change AND HOW — distinct from the
    /// why-`summary`. Empty when the agent (or an older daemon) omitted it;
    /// the field is additive, never a parse-or-render precondition.
    pub suggested_fix: String,
}

/// One entry as it arrives in the `submit_canon_contradictions` payload.
#[derive(Debug, Deserialize)]
struct RawCanonContradiction {
    change_requirement: String,
    canonical_capability: String,
    canonical_requirement: String,
    summary: String,
    /// Optional concrete edit plan. Defaults to empty when the payload omits
    /// it so an older payload (no `suggested_fix`) still parses (back-compat).
    #[serde(default)]
    suggested_fix: String,
}

/// The `submit_canon_contradictions` payload shape.
#[derive(Debug, Deserialize)]
struct RawCanonContradictionSubmission {
    contradictions: Vec<RawCanonContradiction>,
}

const PROMPT_DELIMITER: &str = "\n\n---\n\n";

/// Validate AND map a consumed `submit_canon_contradictions` payload into
/// [`CanonContradictionFinding`]s (a62). This is BOTH the daemon-side schema
/// validator (registered via [`register_canon_contradiction_submission_schema`]
/// with its `Ok` value discarded) AND the consume-time mapper — so a payload
/// that records successfully is exactly one that maps, and the two can never
/// drift (mirrors the `[in]` gate's `payload_to_contradictions`).
///
/// Returns `Err(reason)` (a correction-suitable string) when the payload is
/// missing the `contradictions` array, when it is not an array, OR when an
/// entry is missing a required field. `record_submission` surfaces the reason
/// to the agent as a correctable tool error.
pub(crate) fn payload_to_canon_contradictions(
    payload: &serde_json::Value,
) -> std::result::Result<Vec<CanonContradictionFinding>, String> {
    let sub: RawCanonContradictionSubmission =
        serde_json::from_value(payload.clone()).map_err(|e| {
            format!(
                "submit_canon_contradictions: payload does not match the expected shape \
                 {{ contradictions: [{{ change_requirement, canonical_capability, \
                 canonical_requirement, summary }}] }}: {e}"
            )
        })?;
    Ok(sub
        .contradictions
        .into_iter()
        .map(|c| CanonContradictionFinding {
            change_requirement: c.change_requirement,
            canonical_capability: c.canonical_capability,
            canonical_requirement: c.canonical_requirement,
            summary: c.summary,
            suggested_fix: c.suggested_fix,
        })
        .collect())
}

/// Register the canon check's `submit_canon_contradictions` payload schema
/// (a62) with the daemon's submission store, under
/// [`CANON_CONTRADICTION_CHECK_ROLE`]. The validator IS
/// [`payload_to_canon_contradictions`] with its `Ok` value discarded, so a
/// payload that records successfully is exactly one that maps. Called once at
/// daemon startup alongside the `[in]` gate's schema registration.
pub fn register_canon_contradiction_submission_schema(
    store: &crate::submission_store::SubmissionStore,
) {
    store.register_schema(
        CANON_CONTRADICTION_CHECK_ROLE,
        Arc::new(|p: &serde_json::Value| payload_to_canon_contradictions(p).map(|_| ())),
    );
}

/// The canon gate's production session runner is the shared
/// [`CliCorpusCheckSessionRunner`] instantiated with the canon role + tools (a62
/// machinery now lives in [`crate::preflight::corpus_check`]; the `[canon]` AND
/// `[rules]` gates instantiate it rather than fork it). Built in
/// [`run_agentic_canon_contradiction_check`].

/// Test-only session runner that stands in for the CLI + control socket:
/// returns a canned submission (`Some(payload)`) or `None` for the
/// no-submission case, with an empty stdout excerpt. Defined at module level
/// (not inside `mod tests`) so the `#[cfg(test)]` seam in
/// [`run_agentic_canon_contradiction_check`] can construct it.
#[cfg(test)]
struct CannedCanonContradictionRunner {
    submission: Option<serde_json::Value>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl CorpusCheckSessionRunner for CannedCanonContradictionRunner {
    async fn run_session(
        &self,
        _prompt: &str,
    ) -> Result<crate::preflight::corpus_check::CorpusCheckSessionOutcome> {
        Ok(crate::preflight::corpus_check::CorpusCheckSessionOutcome {
            submission: self.submission.clone(),
            stdout_excerpt: String::new(),
            log_path: None,
        })
    }
}

/// Run the canon-contradiction check for `change_slug` under `workspace_root`
/// (a62). Production entry point invoked from the polling loop's pre-flight.
///
/// Resolves the CLI strategy from the model's provider (a56); a provider whose
/// CLI has no registered strategy yet FAILS CLOSED here with a WARN AND no
/// subprocess is spawned (the change is held). Otherwise runs one agentic
/// session in the read-only sandbox, drains the `submit_canon_contradictions`
/// submission, AND maps it to findings.
///
/// HOLDS the change (`Errored`) on EVERY fail-closed path: strategy-not-
/// registered, session error (spawn/timeout), a never-corrected schema
/// rejection, OR a session that ends with no submission. WARN logs (carrying
/// the `[verifier:canon]` label) name the specific failure so operators can
/// investigate via journalctl. An empty submission is a clean pass.
/// Outcome of the `[canon]` gate. Fails CLOSED (gatekeepers-fail-closed
/// standard): an inability to run is `Errored`, NEVER `Clean`. Mirrors
/// [`crate::preflight::change_contradiction::ContradictionCheckOutcome`].
#[derive(Debug)]
pub enum CanonContradictionCheckOutcome {
    /// Ran successfully; no contradictions. Proceed.
    Clean,
    /// Ran successfully; found contradictions. Block (needs revision).
    Found(Vec<CanonContradictionFinding>),
    /// Could NOT run (CLI unavailable, session error, no submission, or a
    /// re-map failure). Hold the change — never treat as `Clean`.
    Errored { cause: String },
}

pub async fn run_agentic_canon_contradiction_check(
    ctx: &CanonContradictionCheckCtx,
    workspace_root: &Path,
    change_slug: &str,
) -> CanonContradictionCheckOutcome {
    // Test seam: an injected submission stands in for the CLI + control socket
    // so the orchestration is exercised without spawning a process.
    #[cfg(test)]
    if let Some(injected) = &ctx.test_submission {
        let runner = CannedCanonContradictionRunner {
            submission: injected.clone(),
        };
        return run_agentic_canon_contradiction_check_with_runner(
            ctx,
            workspace_root,
            change_slug,
            &runner,
        )
        .await;
    }

    let strategy = match crate::agentic_run::strategy_for_provider(
        ctx.model.provider,
        ctx.command.clone(),
        Vec::new(),
    ) {
        Ok(s) => s,
        Err(e) => {
            let label = VerifierGate::Canon.label();
            let cause = format!("CLI strategy unavailable: {e:#}");
            tracing::warn!(
                change = %change_slug,
                "{label} change-vs-canonical-check could not run ({cause}); holding the change (fail-closed)"
            );
            return CanonContradictionCheckOutcome::Errored { cause };
        }
    };
    let runner = CliCorpusCheckSessionRunner {
        workspace: workspace_root,
        role: CANON_CONTRADICTION_CHECK_ROLE,
        allowed_tools: agentic_canon_contradiction_allowed_tools(),
        strategy: strategy.as_ref(),
        model: &ctx.model,
        settings_dir: None,
        timeout: ctx.timeout,
        subject_noun: "change-vs-canonical-check",
        gate: VerifierGate::Canon,
        subject_slug: change_slug,
        paths: ctx.paths.clone(),
    };
    run_agentic_canon_contradiction_check_with_runner(ctx, workspace_root, change_slug, &runner)
        .await
}

/// Map a corpus-check session result into a [`CanonContradictionCheckOutcome`]
/// (the canon-specific finding shape). The shared [`run_corpus_check_with_runner`]
/// core handles the session/retry/fail-closed disposition; this only re-maps the
/// submitted payload: an empty mapped result is `Clean`, a non-empty one is
/// `Found`, AND a re-map failure (the payload passed `record_submission` yet
/// fails to map) is an internal invariant violation that holds (fail-closed).
fn map_canon_session(
    session: CorpusCheckSession,
    change_slug: &str,
) -> CanonContradictionCheckOutcome {
    let label = VerifierGate::Canon.label();
    match session {
        CorpusCheckSession::Errored { cause } => CanonContradictionCheckOutcome::Errored { cause },
        CorpusCheckSession::Submitted(payload) => match payload_to_canon_contradictions(&payload) {
            Ok(findings) if findings.is_empty() => CanonContradictionCheckOutcome::Clean,
            Ok(findings) => CanonContradictionCheckOutcome::Found(findings),
            Err(e) => {
                let cause = format!("submission failed re-validation: {e}");
                tracing::warn!(
                    change = %change_slug,
                    "{label} change-vs-canonical-check could not run ({cause}); holding the change (fail-closed)"
                );
                CanonContradictionCheckOutcome::Errored { cause }
            }
        },
    }
}

/// Orchestration shared by production AND tests. Builds the prompt, runs one
/// session via `runner` through the shared corpus-check core, AND maps the
/// result into the canon finding shape (a session error, a missing submission,
/// OR a submission that fails re-mapping all hold the change — fail-closed).
async fn run_agentic_canon_contradiction_check_with_runner(
    ctx: &CanonContradictionCheckCtx,
    workspace_root: &Path,
    change_slug: &str,
    runner: &dyn CorpusCheckSessionRunner,
) -> CanonContradictionCheckOutcome {
    let prompt = build_canon_contradiction_prompt(&ctx.prompt_template, workspace_root, change_slug);
    let session =
        run_corpus_check_with_runner(VerifierGate::Canon, change_slug, ctx.retries, &prompt, runner)
            .await;
    map_canon_session(session, change_slug)
}

/// Embedded prompt for the authoring-time issue contract-change check (a02).
/// Frames the same judgment the implement-time issue kick-back applies
/// (`prompts/implementer-issue.md`: "if the fix needs a behavior change it
/// belongs in the changes lane"), so authoring-time AND implement-time judge an
/// issue's hidden-contract-change question by the same criteria.
pub const ISSUE_CONTRACT_CHANGE_EMBEDDED_PROMPT: &str =
    include_str!("../../../prompts/issue-contract-change-check.md");

/// Run the authoring-time issue contract-change check for `issue_slug` (a02).
///
/// An issue carries NO spec delta, so the delta-reading `[canon]` gate does not
/// apply to it directly. This check instead reads the issue's `issue.md` AND
/// the canonical specs AND judges whether implementing the issue would require
/// changing a canonical contract — the SAME judgment the implement-time issue
/// kick-back applies ("Issue-flavored implementer prompt verifies against
/// existing canon"), pulled forward to authoring time so a unit that smuggles a
/// contract change is re-routed to the spec lane BEFORE it is committed.
///
/// It reuses the `[canon]` gate's context (model / command / retries) AND its
/// `submit_canon_contradictions` MCP tool: an EMPTY submission means "no
/// contract change required" (an honest issue → `Clean`); a non-empty
/// submission names the canonical requirement(s) the fix would force a change
/// to (`Found` → re-route to the spec lane). Mirrors
/// [`run_agentic_canon_contradiction_check`]'s fail-closed posture — any
/// could-not-run path is `Errored` (the unit is held), never `Clean`.
pub async fn run_agentic_issue_contract_change_check(
    ctx: &CanonContradictionCheckCtx,
    workspace_root: &Path,
    issue_slug: &str,
) -> CanonContradictionCheckOutcome {
    // Test seam: an injected submission stands in for the CLI + control socket.
    #[cfg(test)]
    if let Some(injected) = &ctx.test_submission {
        let runner = CannedCanonContradictionRunner {
            submission: injected.clone(),
        };
        return run_agentic_issue_contract_change_check_with_runner(
            ctx,
            workspace_root,
            issue_slug,
            &runner,
        )
        .await;
    }

    let strategy = match crate::agentic_run::strategy_for_provider(
        ctx.model.provider,
        ctx.command.clone(),
        Vec::new(),
    ) {
        Ok(s) => s,
        Err(e) => {
            let label = VerifierGate::Canon.label();
            let cause = format!("CLI strategy unavailable: {e:#}");
            tracing::warn!(
                issue = %issue_slug,
                "{label} issue contract-change check could not run ({cause}); holding the unit (fail-closed)"
            );
            return CanonContradictionCheckOutcome::Errored { cause };
        }
    };
    let runner = CliCorpusCheckSessionRunner {
        workspace: workspace_root,
        role: CANON_CONTRADICTION_CHECK_ROLE,
        allowed_tools: agentic_canon_contradiction_allowed_tools(),
        strategy: strategy.as_ref(),
        model: &ctx.model,
        settings_dir: None,
        timeout: ctx.timeout,
        subject_noun: "issue contract-change check",
        gate: VerifierGate::Canon,
        subject_slug: issue_slug,
        paths: ctx.paths.clone(),
    };
    run_agentic_issue_contract_change_check_with_runner(ctx, workspace_root, issue_slug, &runner)
        .await
}

/// Orchestration shared by production AND tests for the issue contract-change
/// check. Builds the issue-flavored prompt, runs one session via `runner`
/// through the shared corpus-check core, AND maps the result with the SAME
/// fail-closed policy as the `[canon]` gate.
async fn run_agentic_issue_contract_change_check_with_runner(
    ctx: &CanonContradictionCheckCtx,
    workspace_root: &Path,
    issue_slug: &str,
    runner: &dyn CorpusCheckSessionRunner,
) -> CanonContradictionCheckOutcome {
    let prompt = build_issue_contract_change_prompt(workspace_root, issue_slug);
    let session =
        run_corpus_check_with_runner(VerifierGate::Canon, issue_slug, ctx.retries, &prompt, runner)
            .await;
    map_canon_session(session, issue_slug)
}

/// Build the issue contract-change session prompt: the embedded issue-flavored
/// framing, the issue's `issue.md` PATH (read on demand), the canonical-spec
/// file PATHS, AND the `submit_canon_contradictions` instruction. The issue
/// path binds to [`crate::lanes::issues::ISSUES_SUBDIR`] so it tracks the lane
/// the issues walker actually reads.
fn build_issue_contract_change_prompt(workspace_root: &Path, issue_slug: &str) -> String {
    let issue_path = format!(
        "{}/{issue_slug}/issue.md",
        crate::lanes::issues::ISSUES_SUBDIR
    );
    let canon_paths = canonical_spec_paths(workspace_root);
    let mut out = String::new();
    out.push_str(ISSUE_CONTRACT_CHANGE_EMBEDDED_PROMPT.trim_end());
    out.push_str(PROMPT_DELIMITER);

    out.push_str("# This issue's report\n\n");
    out.push_str(&format!(
        "Read this file with the `Read` tool — it is the issue's report, diagnosis, AND \
         acceptance criteria (stated against the EXISTING specification):\n\n- {issue_path}\n"
    ));

    out.push_str("\n# The project's canonical specs\n\n");
    if canon_paths.is_empty() {
        out.push_str(
            "(the project has no canonical specs under openspec/specs/<capability>/spec.md — \
             there is no contract for this issue to change)\n",
        );
    } else {
        out.push_str(
            "Read the canonical specs that cover the behavior the issue touches (via `Read`, \
             or via `query_canonical_specs` when that tool is available), then judge whether \
             implementing the issue would require changing any of them:\n\n",
        );
        for p in &canon_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }

    out.push_str(
        "\nWhen your analysis is complete, call the `submit_canon_contradictions` MCP tool \
         exactly once with `{ contradictions: [{ change_requirement, canonical_capability, \
         canonical_requirement, summary }] }`. Pass an EMPTY array when the issue is honest \
         (no contract change required) — the common case. Do NOT print the result to stdout — \
         the daemon reads it ONLY from `submit_canon_contradictions`.\n",
    );
    out
}

/// Build the session prompt: the resolved template body, the change's
/// spec-delta file PATHS, the canonical-spec file PATHS (the agent reads them
/// on demand via `Read` — contents are NOT inlined), AND the
/// `submit_canon_contradictions` instruction. The canonical specs are listed
/// so the agent can `Read` them directly; when a21's RAG is enabled the agent
/// MAY instead use `query_canonical_specs` for focused retrieval.
fn build_canon_contradiction_prompt(
    template: &str,
    workspace_root: &Path,
    change_slug: &str,
) -> String {
    let delta_paths = spec_delta_paths(workspace_root, change_slug);
    let canon_paths = canonical_spec_paths(workspace_root);
    let mut out = String::new();
    out.push_str(template.trim_end());
    out.push_str(PROMPT_DELIMITER);

    out.push_str("# This change's spec-delta files\n\n");
    if delta_paths.is_empty() {
        out.push_str(
            "(this change has no spec-delta files under \
             openspec/changes/<change>/specs/ — there is nothing to check)\n",
        );
    } else {
        out.push_str(
            "Read each of these files with the `Read` tool — they are the change's \
             requirements:\n\n",
        );
        for p in &delta_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }

    out.push_str("\n# The project's canonical specs\n\n");
    if canon_paths.is_empty() {
        out.push_str(
            "(the project has no canonical specs under openspec/specs/<capability>/spec.md — \
             there is no canon for this change to contradict)\n",
        );
    } else {
        out.push_str(
            "Read the canonical specs that cover the same — or related — capabilities as the \
             change's deltas (via `Read`, or via `query_canonical_specs` when that tool is \
             available), then compare the change against canon:\n\n",
        );
        for p in &canon_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }

    out.push_str(
        "\nWhen your analysis is complete, call the `submit_canon_contradictions` MCP tool \
         exactly once with `{ contradictions: [{ change_requirement, canonical_capability, \
         canonical_requirement, summary }] }` (an empty array means \"no contradictions \
         found\"). Do NOT print the result to stdout — the daemon reads it ONLY from \
         `submit_canon_contradictions`.\n",
    );
    out
}

/// Enumerate every `openspec/changes/<change>/specs/<cap>/spec.md` path
/// (workspace-relative) for the change, sorted by capability. Delegates to the
/// shared corpus-check helper so the `[canon]` AND `[rules]` gates list a
/// change's deltas the same way.
fn spec_delta_paths(workspace_root: &Path, change_slug: &str) -> Vec<String> {
    crate::preflight::corpus_check::change_spec_delta_paths(workspace_root, change_slug)
}

/// Enumerate every canonical `openspec/specs/<cap>/spec.md` path
/// (workspace-relative), sorted by capability. Returns an empty `Vec` when the
/// project has no canonical specs yet. The agent reads them on demand via the
/// read-only sandbox (OR retrieves focused slices via `query_canonical_specs`
/// when a21's RAG is enabled). The list mirrors `documentation_audit`'s
/// canon-gather: paths only, contents read on demand.
fn canonical_spec_paths(workspace_root: &Path) -> Vec<String> {
    let specs_dir = workspace_root.join("openspec/specs");
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
            out.push(format!("openspec/specs/{cap_name}/spec.md"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmProvider;
    use crate::preflight::corpus_check::CorpusCheckSessionOutcome;
    use async_trait::async_trait;
    use tempfile::TempDir;

    /// Test runner that simulates a session error (spawn/timeout/strategy).
    struct ErrorCanonContradictionRunner;

    #[async_trait]
    impl CorpusCheckSessionRunner for ErrorCanonContradictionRunner {
        async fn run_session(&self, _prompt: &str) -> Result<CorpusCheckSessionOutcome> {
            Err(anyhow!("simulated session spawn error"))
        }
    }

    /// Test runner that plays back a SCRIPTED sequence of session submissions
    /// (one per call) AND counts invocations; the last entry repeats once the
    /// script is exhausted. Drives the shared retry-loop tests.
    struct ScriptedCanonContradictionRunner {
        script: Vec<Option<serde_json::Value>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedCanonContradictionRunner {
        fn new(script: Vec<Option<serde_json::Value>>) -> Self {
            Self {
                script,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CorpusCheckSessionRunner for ScriptedCanonContradictionRunner {
        async fn run_session(&self, _prompt: &str) -> Result<CorpusCheckSessionOutcome> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let idx = n.min(self.script.len().saturating_sub(1));
            Ok(CorpusCheckSessionOutcome {
                submission: self.script[idx].clone(),
                stdout_excerpt: String::new(),
                log_path: None,
            })
        }
    }

    fn test_model() -> ResolvedModel {
        ResolvedModel {
            provider: LlmProvider::Anthropic,
            model: "claude-test".into(),
            api_base_url: "https://example.invalid".into(),
            api_key: "sk-test".into(),
        }
    }

    fn test_ctx() -> CanonContradictionCheckCtx {
        CanonContradictionCheckCtx {
            command: "claude".into(),
            model: test_model(),
            prompt_template: "TEST_PROMPT".into(),
            attribution: None,
            // Default to no retry so the canned-runner tests below run the
            // session exactly once; the retry behavior has its own tests.
            retries: 0,
            timeout: Duration::from_secs(crate::config::default_agentic_session_timeout()),
            paths: None,
            test_submission: None,
        }
    }

    /// unified-agentic-session-timeout task 4.2 ([canon] gate): the gate ctx
    /// carries the value resolved from `executor.agentic_session_timeout_secs`,
    /// which both `[canon]` session runners (the contradiction check AND the
    /// issue contract-change check) feed to the CLI session.
    #[test]
    fn canon_gate_ctx_carries_resolved_agentic_session_timeout() {
        let exec: crate::config::ExecutorConfig =
            serde_yml::from_str("kind: claude_cli\nagentic_session_timeout_secs: 4500\n")
                .expect("executor parses");
        let ctx = CanonContradictionCheckCtx {
            command: "claude".into(),
            model: test_model(),
            prompt_template: "T".into(),
            attribution: None,
            retries: 0,
            timeout: exec.agentic_session_timeout(),
            paths: None,
            test_submission: None,
        };
        assert_eq!(ctx.timeout, exec.agentic_session_timeout());
        assert_eq!(ctx.timeout, Duration::from_secs(4500));
    }

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn write_change_spec(workspace: &Path, change: &str, capability: &str, body: &str) {
        write(
            &workspace
                .join("openspec/changes")
                .join(change)
                .join("specs")
                .join(capability)
                .join("spec.md"),
            body,
        );
    }

    fn write_canonical_spec(workspace: &Path, capability: &str, body: &str) {
        write(
            &workspace
                .join("openspec/specs")
                .join(capability)
                .join("spec.md"),
            body,
        );
    }

    // ---- payload_to_canon_contradictions (the registered validator + mapper) ----

    #[test]
    fn empty_contradictions_array_maps_to_empty_vec() {
        let payload = serde_json::json!({ "contradictions": [] });
        let out = payload_to_canon_contradictions(&payload).expect("empty array deserializes");
        assert!(out.is_empty());
    }

    #[test]
    fn single_contradiction_is_mapped() {
        let payload = serde_json::json!({
            "contradictions": [
                {
                    "change_requirement": "Secrets MAY live in config.yaml",
                    "canonical_capability": "security",
                    "canonical_requirement": "All secrets in env vars",
                    "summary": "the change re-allows what canon forbids"
                }
            ]
        });
        let out = payload_to_canon_contradictions(&payload).expect("deserializes");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].change_requirement, "Secrets MAY live in config.yaml");
        assert_eq!(out[0].canonical_capability, "security");
        assert_eq!(out[0].canonical_requirement, "All secrets in env vars");
        assert_eq!(out[0].summary, "the change re-allows what canon forbids");
    }

    /// A payload carrying `suggested_fix` maps the field through to the finding,
    /// distinct from the why-`summary`.
    #[test]
    fn suggested_fix_is_mapped_when_present() {
        let payload = serde_json::json!({
            "contradictions": [
                {
                    "change_requirement": "A",
                    "canonical_capability": "security",
                    "canonical_requirement": "B",
                    "summary": "why they conflict",
                    "suggested_fix": "turn the delta into a coherent MODIFIED of B"
                }
            ]
        });
        let out = payload_to_canon_contradictions(&payload).expect("deserializes");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].summary, "why they conflict");
        assert_eq!(out[0].suggested_fix, "turn the delta into a coherent MODIFIED of B");
    }

    /// Back-compat (task 5.3): a payload that OMITS `suggested_fix` still parses,
    /// with the field defaulting to empty — an older daemon's payload is valid.
    #[test]
    fn missing_suggested_fix_defaults_to_empty() {
        let payload = serde_json::json!({
            "contradictions": [
                {
                    "change_requirement": "A",
                    "canonical_capability": "security",
                    "canonical_requirement": "B",
                    "summary": "s"
                }
            ]
        });
        let out =
            payload_to_canon_contradictions(&payload).expect("legacy payload still parses");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].suggested_fix, "", "absent suggested_fix defaults to empty");
    }

    #[test]
    fn missing_contradictions_key_is_correctable_error() {
        let payload = serde_json::json!({ "results": [] });
        let err =
            payload_to_canon_contradictions(&payload).expect_err("missing key must error");
        assert!(err.contains("contradictions"), "got: {err}");
    }

    #[test]
    fn non_array_contradictions_is_correctable_error() {
        let payload = serde_json::json!({ "contradictions": "not-an-array" });
        let err =
            payload_to_canon_contradictions(&payload).expect_err("non-array must error");
        assert!(err.contains("contradictions"), "got: {err}");
    }

    #[test]
    fn entry_missing_canonical_requirement_is_correctable_error() {
        let payload = serde_json::json!({
            "contradictions": [
                {
                    "change_requirement": "A",
                    "canonical_capability": "cap",
                    "summary": "missing canonical_requirement"
                }
            ]
        });
        let err = payload_to_canon_contradictions(&payload)
            .expect_err("missing required field must error");
        assert!(err.contains("submit_canon_contradictions"), "got: {err}");
    }

    // ---- orchestration (run_agentic_canon_contradiction_check_with_runner) ----

    /// A schema-valid non-empty submission is consumed into findings.
    #[tokio::test]
    async fn valid_submission_is_consumed_into_findings() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
        );
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner {
            submission: Some(serde_json::json!({
                "contradictions": [
                    {
                        "change_requirement": "A",
                        "canonical_capability": "security",
                        "canonical_requirement": "B",
                        "summary": "x"
                    }
                ]
            })),
        };
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        match out {
            CanonContradictionCheckOutcome::Found(f) => {
                assert_eq!(f.len(), 1);
                assert_eq!(f[0].change_requirement, "A");
                assert_eq!(f[0].canonical_requirement, "B");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// An empty submission is a CLEAN run (proceed-to-executor).
    #[tokio::test]
    async fn empty_submission_is_clean() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner {
            submission: Some(serde_json::json!({ "contradictions": [] })),
        };
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Clean),
            "empty submission is clean: {out:?}"
        );
    }

    /// A session that records NO submission FAILS CLOSED (Errored → held).
    #[tokio::test]
    async fn no_submission_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner { submission: None };
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "no submission must fail CLOSED (held): {out:?}"
        );
    }

    /// The fail-CLOSED diagnostics carry the `[verifier:canon]` gate identifier
    /// so the held change is attributable to the gate that could not run.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn fail_closed_diagnostics_carry_the_canon_gate_label() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner { submission: None };
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "no submission fails CLOSED (held)"
        );
        assert!(
            logs_contain("[verifier:canon]"),
            "the fail-closed WARN must carry the [verifier:canon] gate identifier"
        );
    }

    /// A session error (spawn/timeout/strategy) FAILS CLOSED (Errored).
    #[tokio::test]
    async fn session_error_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let ctx = test_ctx();
        let out = run_agentic_canon_contradiction_check_with_runner(
            &ctx,
            ws,
            "c1",
            &ErrorCanonContradictionRunner,
        )
        .await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "session error must fail CLOSED (held): {out:?}"
        );
    }

    // ---- bounded retry on the flaky no-submission case (shared seam) ----

    /// No submission on attempt 1, an empty (clean) submission on attempt 2 →
    /// the gate succeeds (Clean), not held. The flaky case is retried.
    #[tokio::test]
    async fn no_submission_then_clean_succeeds_on_retry() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let mut ctx = test_ctx();
        ctx.retries = 2;
        let runner = ScriptedCanonContradictionRunner::new(vec![
            None,
            Some(serde_json::json!({ "contradictions": [] })),
        ]);
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Clean),
            "a retry that submits an empty result is Clean: {out:?}"
        );
        assert_eq!(runner.call_count(), 2, "exactly two attempts (1 retry)");
    }

    /// No submission on EVERY attempt → after `retries` retries the gate fails
    /// closed (Errored → held), invoked exactly `retries + 1` times.
    #[tokio::test]
    async fn no_submission_every_attempt_fails_closed_after_bound() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let mut ctx = test_ctx();
        ctx.retries = 2;
        let runner = ScriptedCanonContradictionRunner::new(vec![None]);
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "exhausted retries must fail closed (held): {out:?}"
        );
        assert_eq!(runner.call_count(), 3, "retries(2) + 1 = 3 attempts");
    }

    /// `retries == 0` → exactly one attempt, fails closed on no submission
    /// (historical single-attempt behavior preserved).
    #[tokio::test]
    async fn zero_retries_is_one_attempt() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let mut ctx = test_ctx();
        ctx.retries = 0;
        let runner = ScriptedCanonContradictionRunner::new(vec![None]);
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(matches!(out, CanonContradictionCheckOutcome::Errored { .. }));
        assert_eq!(runner.call_count(), 1, "retries=0 means exactly one attempt");
    }

    /// A valid submission on attempt 1 → exactly one attempt (no needless
    /// retry), even with a non-zero retry bound.
    #[tokio::test]
    async fn valid_first_attempt_does_not_retry() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let mut ctx = test_ctx();
        ctx.retries = 2;
        let runner = ScriptedCanonContradictionRunner::new(vec![Some(serde_json::json!({
            "contradictions": [
                {
                    "change_requirement": "A",
                    "canonical_capability": "security",
                    "canonical_requirement": "B",
                    "summary": "x"
                }
            ]
        }))]);
        let out =
            run_agentic_canon_contradiction_check_with_runner(&ctx, ws, "c1", &runner).await;
        assert!(matches!(out, CanonContradictionCheckOutcome::Found(_)));
        assert_eq!(runner.call_count(), 1, "a submission on attempt 1 needs no retry");
    }

    /// A non-`claude` provider resolves to a CLI with no registered strategy,
    /// so the production entry point FAILS CLOSED with no spawn.
    #[tokio::test]
    async fn unregistered_strategy_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let mut ctx = test_ctx();
        ctx.model.provider = LlmProvider::Ollama;
        ctx.command = "definitely-not-a-registered-cli".into();
        let out = run_agentic_canon_contradiction_check(&ctx, ws, "c1").await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "unregistered strategy must fail CLOSED (held): {out:?}"
        );
    }

    // ---- issue contract-change check (a02) ----

    fn write_issue(workspace: &Path, slug: &str, body: &str) {
        write(
            &workspace
                .join(crate::lanes::issues::ISSUES_SUBDIR)
                .join(slug)
                .join("issue.md"),
            body,
        );
    }

    /// An honest issue (no contract change) → empty submission → `Clean`.
    #[tokio::test]
    async fn issue_check_empty_submission_is_clean() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_issue(ws, "fix-drift", "## Report\nReturns the wrong value.\n");
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner {
            submission: Some(serde_json::json!({ "contradictions": [] })),
        };
        let out =
            run_agentic_issue_contract_change_check_with_runner(&ctx, ws, "fix-drift", &runner)
                .await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Clean),
            "an honest issue's empty submission is clean: {out:?}"
        );
    }

    /// An issue that would require a contract change → non-empty submission →
    /// `Found` (the harness re-routes it to the spec lane).
    #[tokio::test]
    async fn issue_check_contract_change_is_found() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_issue(ws, "fix-retry", "## Report\nWants 5 retries.\n");
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner {
            submission: Some(serde_json::json!({
                "contradictions": [
                    {
                        "change_requirement": "retry 5 times",
                        "canonical_capability": "executor",
                        "canonical_requirement": "Retries are capped at 3",
                        "summary": "the fix cannot be implemented without changing the cap"
                    }
                ]
            })),
        };
        let out =
            run_agentic_issue_contract_change_check_with_runner(&ctx, ws, "fix-retry", &runner)
                .await;
        match out {
            CanonContradictionCheckOutcome::Found(f) => {
                assert_eq!(f.len(), 1);
                assert_eq!(f[0].canonical_requirement, "Retries are capped at 3");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// A session that records NO submission FAILS CLOSED (held), exactly like
    /// the `[canon]` gate.
    #[tokio::test]
    async fn issue_check_no_submission_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let ctx = test_ctx();
        let runner = CannedCanonContradictionRunner { submission: None };
        let out =
            run_agentic_issue_contract_change_check_with_runner(&ctx, ws, "fix-x", &runner).await;
        assert!(
            matches!(out, CanonContradictionCheckOutcome::Errored { .. }),
            "no submission must fail CLOSED (held): {out:?}"
        );
    }

    /// The issue prompt lists the issue's `issue.md` path (bound to
    /// `ISSUES_SUBDIR`), the canonical-spec paths, AND the submit instruction;
    /// it does NOT inline file contents.
    #[tokio::test]
    async fn issue_contract_change_prompt_lists_issue_and_canon_paths() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_issue(ws, "fix-thing", "## Report\nbody.\n");
        write_canonical_spec(
            ws,
            "executor",
            "## Requirements\n\n### Requirement: Runs changes\nBody.\n",
        );
        let prompt = build_issue_contract_change_prompt(ws, "fix-thing");
        assert!(
            prompt.contains(&format!(
                "{}/fix-thing/issue.md",
                crate::lanes::issues::ISSUES_SUBDIR
            )),
            "prompt must name the issue's issue.md path: {prompt}"
        );
        assert!(prompt.contains("openspec/specs/executor/spec.md"));
        assert!(
            prompt.contains("submit_canon_contradictions"),
            "prompt must instruct the submit tool"
        );
        // Contents are read on demand, not inlined.
        assert!(!prompt.contains("Runs changes\nBody"));
    }

    // ---- prompt construction ----

    #[tokio::test]
    async fn prompt_lists_delta_and_canonical_paths_and_submit_instruction() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "alpha",
            "## ADDED Requirements\n\n### Requirement: A1\nBody.\n",
        );
        write_canonical_spec(
            ws,
            "security",
            "## Requirements\n\n### Requirement: All secrets in env vars\nBody.\n",
        );
        write_canonical_spec(
            ws,
            "executor",
            "## Requirements\n\n### Requirement: Runs changes\nBody.\n",
        );
        let prompt = build_canon_contradiction_prompt("PROMPT_TEMPLATE", ws, "c1");
        assert!(prompt.starts_with("PROMPT_TEMPLATE"));
        // The change's deltas.
        assert!(prompt.contains("openspec/changes/c1/specs/alpha/spec.md"));
        // The canonical specs, sorted by capability.
        assert!(prompt.contains("openspec/specs/executor/spec.md"));
        assert!(prompt.contains("openspec/specs/security/spec.md"));
        assert!(
            prompt.contains("submit_canon_contradictions"),
            "prompt must instruct the agent to call submit_canon_contradictions"
        );
        assert!(
            prompt.contains("query_canonical_specs"),
            "prompt must mention the RAG retrieval option"
        );
        // The agent reads files on demand — contents are NOT inlined.
        assert!(!prompt.contains("Requirement: A1"));
        assert!(!prompt.contains("All secrets in env vars"));
    }

    #[test]
    fn spec_delta_paths_empty_when_no_specs_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("openspec/changes/c1")).unwrap();
        assert!(spec_delta_paths(ws, "c1").is_empty());
    }

    #[test]
    fn canonical_spec_paths_empty_when_no_specs_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        assert!(canonical_spec_paths(ws).is_empty());
    }

    #[test]
    fn prompt_handles_absent_canon_gracefully() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "alpha",
            "## ADDED Requirements\n\n### Requirement: A1\nBody.\n",
        );
        let prompt = build_canon_contradiction_prompt("PROMPT_TEMPLATE", ws, "c1");
        assert!(
            prompt.contains("no canonical specs"),
            "prompt must note the absence of canon; got: {prompt}"
        );
    }

    // ---- allowed-tools surface ----

    /// Task 4.5: the gate runs with OR without a21 RAG. The `query_canonical_specs`
    /// tool is part of the daemon's MCP tool contract (`PROVIDED_TOOL_NAMES`),
    /// auto-advertised via `include_autocoder_tools` — so the agent has it when
    /// canonical_rag is enabled. The gate's OWN allowed-tools surface does NOT
    /// include or require it (only the read-only file tools + the submit tool),
    /// so with RAG disabled the agent reads `openspec/specs` directly via
    /// `Read` and the gate still functions.
    #[test]
    fn rag_query_tool_is_a_common_tool_not_a_gate_dependency() {
        // Advertised as a common tool (available when RAG is configured).
        assert!(
            crate::mcp_askuser_server::PROVIDED_TOOL_NAMES
                .contains(&"query_canonical_specs"),
            "query_canonical_specs must be a common autocoder MCP tool"
        );
        // The gate's own sandbox surface is RAG-independent: read-only file
        // tools + the submit tool, NO hard dependency on query_canonical_specs.
        let tools = agentic_canon_contradiction_allowed_tools();
        assert!(
            !tools.iter().any(|t| t.contains("query_canonical_specs")),
            "the gate must not bake query_canonical_specs into its own allowed-tools \
             (so it works with RAG disabled too): {tools:?}"
        );
        assert!(
            tools.contains(&"Read".to_string()),
            "with RAG disabled the agent reads openspec/specs directly via Read"
        );
    }

    #[test]
    fn allowed_tools_are_read_only_plus_submit_canon_contradictions() {
        let tools = agentic_canon_contradiction_allowed_tools();
        assert!(tools.contains(&"Read".to_string()));
        assert!(tools.contains(&"Glob".to_string()));
        assert!(tools.contains(&"Grep".to_string()));
        assert!(
            !tools.iter().any(|t| t == "Bash" || t == "Write" || t == "Edit"),
            "sandbox must deny Bash/Write/Edit: {tools:?}"
        );
        assert!(
            tools.iter().any(|t| t.contains("submit_canon_contradictions")),
            "submit_canon_contradictions must be allowed: {tools:?}"
        );
    }

    // ---- prompt loader ----

    #[test]
    fn embedded_prompt_template_is_non_empty() {
        assert!(!EMBEDDED_PROMPT.trim().is_empty(), "embedded template must not be empty");
        assert!(EMBEDDED_PROMPT.contains("canonical"));
        assert!(EMBEDDED_PROMPT.contains("submit_canon_contradictions"));
    }

    #[test]
    fn load_prompt_template_none_returns_embedded() {
        let body = load_prompt_template(None).unwrap();
        assert_eq!(body, EMBEDDED_PROMPT);
    }

    #[test]
    fn load_prompt_template_some_reads_override_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("custom.md");
        std::fs::write(&p, "CUSTOM_TEMPLATE_BODY").unwrap();
        let body = load_prompt_template(Some(&p)).unwrap();
        assert_eq!(body, "CUSTOM_TEMPLATE_BODY");
    }

    #[test]
    fn load_prompt_template_empty_override_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty.md");
        std::fs::write(&p, "   \n\n  ").unwrap();
        let err = load_prompt_template(Some(&p)).expect_err("empty override must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains(p.display().to_string().as_str()));
        assert!(msg.contains("empty"), "error must name the empty condition; got: {msg}");
    }

    #[test]
    fn load_prompt_template_missing_override_path_errors() {
        let p = Path::new("/nonexistent/path/to/template.md");
        let err = load_prompt_template(Some(p)).expect_err("missing path must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("/nonexistent/path/to/template.md"));
    }
}
