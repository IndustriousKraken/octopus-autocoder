//! Shared helper for the "spec-writing" audits — those that invoke the
//! wrapped agent CLI with a prompt, expect zero or more new planning-lane
//! units to appear, validate the spec-lane ones via
//! `openspec validate <name> --strict`, drop ones over the cap, commit
//! the validated set on the agent branch, and return
//! `AuditOutcome::SpecsWritten(validated_names)` so the same iteration's
//! lane walkers pick them up.
//!
//! Two write policies share this harness, selected by
//! [`SpecsWritingAuditParams::planning_lanes`]: `canon_consolidation_audit`
//! runs spec-lane-only (`OpenSpecOnly`) and produces only
//! `openspec/changes/<name>/` directories, while the two bug/gap audits
//! (`missing_tests_audit`, `security_bug_audit`) run under `PlanningLanes`
//! (a01) and route each finding to the spec lane (`openspec/changes/`) OR
//! the issues lane (`openspec/issues/`) by canon judgment.
//!
//! The audits differ only in their prompt, their per-run cap source, their
//! human-readable commit subject, and whether they choose a lane —
//! everything else (sandbox shape, snapshot diff, validation, over-cap
//! pruning, commit) is identical. They all delegate to
//! [`run_specs_writing_audit`] so the algorithm lives in one place and
//! cannot drift across audits.

use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use super::{
    AuditContext, AuditFailureCause, AuditLogWriter, AuditOutcome, WritePolicy, build_validation_addendum,
    post_proposal_created_notification, post_validation_exhausted_notification,
    read_proposal_why_first_line, workspace_is_valid, workspace_unavailable_outcome,
};
use crate::config::ResolvedSandbox;

/// Default tools a spec-writing audit allows. `Write` and `Edit` are
/// needed because the agent's whole job is to create OpenSpec change
/// files; the framework's post-hoc `OpenSpecOnly` check catches writes
/// outside `openspec/changes/`. Audits with narrower needs pass their own
/// list via [`SpecsWritingAuditParams::allowed_tools`] (e.g. the
/// canon-consolidation audit drops `Bash`, which it never needs).
pub(crate) const ALLOWED_TOOLS: &[&str] =
    &["Read", "Glob", "Grep", "Bash", "Write", "Edit"];

/// Parameters for one spec-writing audit invocation. Carried as a
/// struct rather than positional args because the list grew long enough
/// that call-sites became hard to read.
pub(crate) struct SpecsWritingAuditParams<'a> {
    /// Stable audit slug. Used as the prefix for every log section name
    /// and as the "audit_type" label inside error messages.
    pub audit_type: &'static str,
    /// Fully resolved prompt body (override or embedded default, with
    /// any placeholder substitutions already applied).
    pub prompt: &'a str,
    /// Hard cap on the number of new change directories committed this
    /// run. Excess directories are deleted post-hoc.
    pub max_proposals: u32,
    /// Wrapped agent CLI binary (typically `claude`).
    pub executor_command: &'a str,
    /// Wall-clock budget for the agent invocation.
    pub executor_timeout_secs: u64,
    /// Resolved sandbox (the helper overrides `allowed_tools` per
    /// [`ALLOWED_TOOLS`] before writing the settings file).
    pub sandbox: &'a ResolvedSandbox,
    /// Override for the directory the per-invocation sandbox-settings
    /// file is written to. `None` means `std::env::temp_dir()`. Tests
    /// pass a per-test TempDir to avoid concurrent name collisions.
    pub settings_dir: Option<&'a Path>,
    /// Override for the `openspec` validation binary. `None` means
    /// `openspec`. Tests point at a shell script so the audit can be
    /// exercised without the real CLI on PATH.
    pub openspec_command: &'a str,
    /// Optional prompt-source label included in the preamble log line
    /// (e.g. the override path or "<embedded default>"). Cosmetic only.
    pub prompt_source: &'a str,
    /// Human-readable subject inserted into the commit message:
    /// `audit: <commit_subject> (N change(s))`.
    pub commit_subject: &'a str,
    /// Sandbox tool allow-list for this audit's agent invocation. Most
    /// audits pass [`ALLOWED_TOOLS`]; an audit that needs a narrower set
    /// (e.g. canon-consolidation, which never shells out, drops `Bash`)
    /// passes its own slice.
    pub allowed_tools: &'a [&'a str],
    /// When `true`, the agent is invoked through the MCP-enabled CLI path
    /// so the autocoder MCP tools (`query_canonical_specs` / `ask_user` /
    /// `outcome_*`) are reachable — the canon-consolidation audit (a76)
    /// uses this to retrieve nearest canonical requirements via
    /// `query_canonical_specs` when a21's RAG is enabled. `false` keeps the
    /// no-MCP capture path the missing-tests / security-bug audits use.
    pub include_autocoder_tools: bool,
    /// audit-model-selection: the resolved model this audit routes to (the
    /// audit runner selects the CLI strategy for its provider AND passes
    /// `--model <provider>/<model>`), or `None` to keep the default `claude`
    /// strategy with no model override.
    pub model: Option<&'a crate::agentic_run::ResolvedModel>,
    /// a01: when `true`, this audit chooses its output lane per finding —
    /// the harness snapshots, validates, stages, and commits BOTH planning
    /// lanes (`openspec/changes/` AND the issues lane), and its commit
    /// subject counts produced UNITS (`audit: <subject> (N unit(s))`). The
    /// two bug/gap audits (`security_bug_audit`, `missing_tests_audit`) set
    /// this. When `false` the harness is spec-lane-only and counts CHANGES
    /// (`audit: <subject> (N change(s))`) — the unchanged
    /// `canon_consolidation_audit` behavior.
    pub planning_lanes: bool,
    /// a01: the resolved `features.issues` flag for the repository this run
    /// targets. Only meaningful when `planning_lanes` is `true`: it gates
    /// whether the issue lane is OFFERED to the agent (when `false`, the
    /// daemon's lane-availability addendum tells the agent only the spec
    /// lane is available, preserving pre-a01 behavior). The harness still
    /// permits issue-lane writes structurally (the `PlanningLanes`
    /// `WritePolicy`) — the flag governs the prompt, not the post-hoc check.
    pub issues_lane_enabled: bool,
    /// a02: the authoring-time gate checker run against each just-written unit
    /// AFTER it passes `openspec validate --strict` (spec lane) OR is produced
    /// (issue lane). Production audits pass [`ScopedGateChecker`], which reads
    /// the verifier-gate task-local contexts (`[in]`/`[canon]`) so an enabled
    /// gate runs at authoring time (self-healing) AND at implement time (the
    /// unchanged backstop); a gate that is disabled (no scoped context) runs at
    /// neither. Tests inject a scripted checker to drive the retry / re-route /
    /// fail-closed paths deterministically without spawning a CLI.
    pub gate_checker: &'a dyn AuthoringGateChecker,
}

/// Result of running the enabled authoring-time gate checks against ONE
/// just-written unit (a02). Collapses the per-gate `[in]`/`[canon]` /
/// issue-contract-change outcomes into the disposition the write-loop acts on.
#[derive(Debug)]
pub(crate) enum GateCheckOutcome {
    /// Every enabled gate ran AND returned no finding (or no gate is enabled).
    /// The unit is clean by the gates AND proceeds to commit.
    Clean,
    /// An enabled gate found a contradiction (`[in]`/`[canon]`, spec lane) OR a
    /// required contract change (issue lane). Carries a human narrative for the
    /// retry addendum so the rewrite is directed.
    Found(String),
    /// An enabled gate could NOT run (transport / parse / no-submission). The
    /// unit is held — NOT treated as clean (the gates' fail-closed posture).
    CouldNotRun(String),
}

/// Runs the enabled authoring-time verifier-gate checks against a single
/// just-written unit (a02; task 1.1). One method per lane: spec-lane changes
/// run the `[in]` + `[canon]` contradiction checks; issue-lane units run the
/// contract-change check. The production implementation ([`ScopedGateChecker`])
/// reuses the verifier framework's existing checks, prompts, AND
/// `submit_contradictions` / `submit_canon_contradictions` MCP tools unchanged.
#[async_trait::async_trait]
pub(crate) trait AuthoringGateChecker: Send + Sync {
    /// Run the enabled `[in]` and `[canon]` checks against the spec-lane change
    /// `slug` (under `openspec/changes/<slug>/`). Returns `Clean` when every
    /// enabled gate is clean (or none is enabled).
    async fn check_spec_change(&self, workspace: &Path, slug: &str) -> GateCheckOutcome;
    /// Run the issue contract-change check against the issue-lane unit `slug`
    /// (under the issues lane), when the `[canon]` gate is enabled. Returns
    /// `Clean` when the `[canon]` gate is disabled (the check does not run) OR
    /// the issue is honest.
    async fn check_issue_unit(&self, workspace: &Path, slug: &str) -> GateCheckOutcome;
}

/// Production gate checker: resolves the `[in]`/`[canon]` verifier-gate
/// contexts from the task-locals the polling task scoped (so an enabled gate
/// runs at authoring time AND implement time, a disabled one at neither — the
/// SAME opt-in flags govern both points), AND dispatches to the verifier
/// framework's existing checks.
pub(crate) struct ScopedGateChecker;

/// The scoped `[in]` gate context, gated on the registry installing the
/// change-internal contradiction check — mirrors `run_in_gate`'s enable test
/// so the authoring-time check fires under exactly the same condition as the
/// implement-time gate.
fn scoped_in_gate_ctx()
-> Option<std::sync::Arc<crate::preflight::change_contradiction::ContradictionCheckCtx>> {
    crate::preflight::change_contradiction::current().filter(|_| {
        matches!(
            crate::verifier_gate::GateRegistry::standard()
                .resolve(crate::verifier_gate::VerifierGate::In),
            Some(crate::verifier_gate::GateImpl::ContradictionCheck)
        )
    })
}

/// The scoped `[canon]` gate context, gated on the registry installing the
/// change-vs-canonical check. Governs BOTH the spec-lane `[canon]` check AND
/// the issue-lane contract-change check (per a02: the issue check runs whenever
/// the `[canon]` gate is enabled).
fn scoped_canon_gate_ctx()
-> Option<std::sync::Arc<crate::preflight::canon_contradiction::CanonContradictionCheckCtx>> {
    crate::preflight::canon_contradiction::current().filter(|_| {
        matches!(
            crate::verifier_gate::GateRegistry::standard()
                .resolve(crate::verifier_gate::VerifierGate::Canon),
            Some(crate::verifier_gate::GateImpl::CanonContradictionCheck)
        )
    })
}

#[async_trait::async_trait]
impl AuthoringGateChecker for ScopedGateChecker {
    async fn check_spec_change(&self, workspace: &Path, slug: &str) -> GateCheckOutcome {
        use crate::preflight::canon_contradiction::CanonContradictionCheckOutcome;
        use crate::preflight::change_contradiction::ContradictionCheckOutcome;
        // [in] first (cheaper: change-internal), then [canon]. Short-circuit on
        // the first non-clean result — a single finding already drives the
        // retry, AND stopping early respects the per-run cost (design D7).
        if let Some(cc) = scoped_in_gate_ctx() {
            match crate::preflight::change_contradiction::run_agentic_contradiction_check(
                cc.as_ref(),
                workspace,
                slug,
            )
            .await
            {
                ContradictionCheckOutcome::Clean => {}
                ContradictionCheckOutcome::Found(f) => {
                    return GateCheckOutcome::Found(render_in_findings(&f));
                }
                ContradictionCheckOutcome::Errored { cause } => {
                    return GateCheckOutcome::CouldNotRun(format!("[verifier:in] {cause}"));
                }
            }
        }
        if let Some(canon) = scoped_canon_gate_ctx() {
            match crate::preflight::canon_contradiction::run_agentic_canon_contradiction_check(
                canon.as_ref(),
                workspace,
                slug,
            )
            .await
            {
                CanonContradictionCheckOutcome::Clean => {}
                CanonContradictionCheckOutcome::Found(f) => {
                    return GateCheckOutcome::Found(render_canon_findings(&f));
                }
                CanonContradictionCheckOutcome::Errored { cause } => {
                    return GateCheckOutcome::CouldNotRun(format!("[verifier:canon] {cause}"));
                }
            }
        }
        GateCheckOutcome::Clean
    }

    async fn check_issue_unit(&self, workspace: &Path, slug: &str) -> GateCheckOutcome {
        use crate::preflight::canon_contradiction::CanonContradictionCheckOutcome;
        // The issue contract-change check runs only when the [canon] gate is
        // enabled (a02). Disabled → Clean (the issue commits on its structural
        // validity; the implement-time issue kick-back is the backstop).
        let Some(canon) = scoped_canon_gate_ctx() else {
            return GateCheckOutcome::Clean;
        };
        match crate::preflight::canon_contradiction::run_agentic_issue_contract_change_check(
            canon.as_ref(),
            workspace,
            slug,
        )
        .await
        {
            CanonContradictionCheckOutcome::Clean => GateCheckOutcome::Clean,
            CanonContradictionCheckOutcome::Found(f) => {
                GateCheckOutcome::Found(render_issue_contract_findings(&f))
            }
            CanonContradictionCheckOutcome::Errored { cause } => {
                GateCheckOutcome::CouldNotRun(format!("[verifier:canon] {cause}"))
            }
        }
    }
}

/// Render `[in]` (change-internal) findings into a retry-addendum narrative.
fn render_in_findings(
    findings: &[crate::preflight::change_contradiction::ContradictionFinding],
) -> String {
    let body = findings
        .iter()
        .map(|f| {
            format!(
                "- `{}` vs `{}`: {}",
                f.requirement_a, f.requirement_b, f.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "The change passed `openspec validate --strict` but the [verifier:in] gate found \
         requirement(s) within it that cannot all hold at once:\n{body}"
    )
}

/// Render `[canon]` (change-vs-canonical) findings into a retry-addendum
/// narrative.
fn render_canon_findings(
    findings: &[crate::preflight::canon_contradiction::CanonContradictionFinding],
) -> String {
    let body = findings
        .iter()
        .map(|f| {
            format!(
                "- this change's `{}` conflicts with the canonical `{}` (capability `{}`): {}",
                f.change_requirement, f.canonical_requirement, f.canonical_capability, f.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "The change passed `openspec validate --strict` but the [verifier:canon] gate found \
         it contradicts an ALREADY-canonical requirement:\n{body}"
    )
}

/// Render an issue contract-change finding into a retry-addendum narrative.
fn render_issue_contract_findings(
    findings: &[crate::preflight::canon_contradiction::CanonContradictionFinding],
) -> String {
    let body = findings
        .iter()
        .map(|f| {
            format!(
                "- implementing this issue would require changing the canonical `{}` \
                 (capability `{}`): {}",
                f.canonical_requirement, f.canonical_capability, f.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "This issue claims (by carrying no spec delta) to change no contract, but implementing \
         it would require a canonical contract change:\n{body}"
    )
}

/// Execute one spec-writing audit run. Returns the outcome the framework
/// dispatches on; never panics on agent misbehavior.
///
/// Validation retry semantics: when EVERY new change directory the LLM
/// produced fails `openspec validate <name> --strict`, the LLM is
/// re-invoked with the validation errors appended to the prompt
/// (per [`build_validation_addendum`]). This repeats until either a
/// valid change dir lands OR the
/// [`AuditContext::max_validation_retries`] budget is exhausted. If
/// the LLM produces a mix of valid and invalid change dirs, the
/// existing per-change drop behavior wins (invalid dirs deleted, valid
/// dirs kept, no retry). When the LLM produces zero change dirs the
/// run is a successful "no findings" with `retries_used: 0` (zero
/// proposals is a legitimate outcome, not a validation failure).
pub(crate) async fn run_specs_writing_audit(
    params: SpecsWritingAuditParams<'_>,
    ctx: &mut AuditContext<'_>,
) -> Result<AuditOutcome> {
    let audit_type = params.audit_type;
    // Workspace-validity gate (see `audits-require-valid-workspace`).
    // The spec-writing helpers are the audits that would otherwise call
    // `fs::create_dir_all(<workspace>/openspec/changes/<slug>)` and
    // recreate workspace + openspec/ on a wiped workspace — the very
    // failure mode the gate exists to prevent.
    if !workspace_is_valid(ctx.workspace) {
        return Ok(workspace_unavailable_outcome(
            audit_type,
            ctx.workspace,
            &ctx.repo.url,
        ));
    }

    let max_retries = ctx.max_validation_retries;
    let total_attempts = max_retries.saturating_add(1);

    // a01: the `WritePolicy` this harness actually serves this run — the
    // two-prefix `PlanningLanes` policy for the lane-choosing bug/gap audits,
    // else the single-prefix `OpenSpecOnly` policy (canon_consolidation). The
    // CLI's writable-mount flag derives from it below; both are writable
    // today, but deriving from the real policy keeps the mount correct if the
    // two ever diverge (a read-only mount would silently yield 0 proposals).
    let write_policy = if params.planning_lanes {
        WritePolicy::PlanningLanes
    } else {
        WritePolicy::OpenSpecOnly
    };

    let mut sandbox = params.sandbox.clone();
    sandbox.allowed_tools = params.allowed_tools.iter().map(|s| (*s).to_string()).collect();

    let initial_before: HashSet<String> = snapshot_change_dirs(ctx.workspace);
    let _ = ctx.log_writer.write_section(
        &format!("{audit_type}_preamble"),
        &format!(
            "executor_command: {}\ntimeout_secs: {}\nprompt_source: {}\nmax_proposals_per_run: {}\nmax_validation_retries: {}\nallowed_tools: {}\ninclude_autocoder_tools: {}\nplanning_lanes: {}\nissues_lane_enabled: {}\npre_run_change_dirs: {}",
            params.executor_command,
            params.executor_timeout_secs,
            params.prompt_source,
            params.max_proposals,
            max_retries,
            sandbox.allowed_tools.join(","),
            params.include_autocoder_tools,
            params.planning_lanes,
            params.issues_lane_enabled,
            initial_before.len(),
        ),
    );

    // a01: for a planning-lanes audit, append the per-run lane-availability
    // addendum so the agent knows whether the issue lane is offered AND the
    // exact lane paths. Spec-lane-only audits (canon_consolidation) use the
    // prompt verbatim.
    let base_prompt: String = if params.planning_lanes {
        format!(
            "{}{}",
            params.prompt,
            compose_lane_availability(params.issues_lane_enabled)
        )
    } else {
        params.prompt.to_string()
    };

    // Per-attempt state: units created on the prior attempt that we need to
    // clear before the next LLM call (we hit a validation / gate failure and
    // are retrying). a02: a failure can land in EITHER planning lane (an
    // issue-lane unit can fail the contract-change check too), so this carries
    // the lane, not just the name.
    let mut prior_attempt_units: Vec<ProducedUnit> = Vec::new();
    // The fully-composed prompt addendum from the most recent failed attempt
    // (a `--strict` error narrative, an authoring-time gate-finding narrative
    // with the permitted resolutions, OR both), fed to the LLM on the next
    // attempt.
    let mut last_addendum_body: Option<String> = None;

    for attempt in 0..total_attempts {
        // Clean up units produced by the prior failed attempt so they do not
        // pollute this attempt's diff. a02: resolve each in its own lane (a
        // failure — and the re-route it drives — can land in either lane).
        for u in &prior_attempt_units {
            let path = unit_path(ctx.workspace, u.lane, &u.name);
            let _ = std::fs::remove_dir_all(&path);
        }
        prior_attempt_units.clear();

        let before_changes: HashSet<String> = snapshot_change_dirs(ctx.workspace);
        // a01: only a planning-lanes audit observes the issues lane; for a
        // spec-lane-only audit this stays empty so its behavior is unchanged.
        let before_issues: HashSet<String> = if params.planning_lanes {
            snapshot_issue_dirs(ctx.workspace)
        } else {
            HashSet::new()
        };

        let effective_prompt = match &last_addendum_body {
            None => base_prompt.clone(),
            // a02: `last_addendum_body` already carries the fully-composed
            // addendum (the `--strict` framing and/or the gate-finding framing
            // with permitted resolutions), so it is appended verbatim.
            Some(addendum) => format!("{base_prompt}\n\n{addendum}"),
        };

        let _ = ctx.log_writer.write_section(
            &format!("{audit_type}_prompt_attempt_{attempt}"),
            &effective_prompt,
        );

        // When the audit needs the autocoder MCP tools (e.g. a76's
        // `query_canonical_specs` for RAG-assisted overlap detection), go
        // through the MCP-enabled CLI path — which advertises the autocoder
        // MCP server in a per-run `.mcp.json` (written AND deleted around the
        // call) AND appends the provided-tool names to `--allowedTools`.
        // Otherwise use the plain capture path the missing-tests /
        // security-bug audits use. The role key is the audit type; an audit
        // with no registered `submit_*` tool (a76 writes its change to disk,
        // it does not submit) simply gets the common tools.
        let outcome = if params.include_autocoder_tools {
            super::run_audit_cli_with_submit(
                params.executor_command,
                &sandbox,
                ctx.workspace,
                &effective_prompt,
                Duration::from_secs(params.executor_timeout_secs),
                params.settings_dir,
                audit_type,
                params.model,
                // Writability derives from the WritePolicy this harness serves
                // this run (OpenSpecOnly OR PlanningLanes → writable) so the
                // agent can create the unit dir; a read-only mount would
                // silently yield 0 proposals.
                write_policy.workspace_writable(),
            )
            .await
        } else {
            super::run_audit_cli(
                params.executor_command,
                &sandbox,
                ctx.workspace,
                &effective_prompt,
                Duration::from_secs(params.executor_timeout_secs),
                params.settings_dir,
                params.model,
                // Writability derives from the WritePolicy this harness serves
                // this run (OpenSpecOnly OR PlanningLanes → writable) so the
                // agent can create the unit dir; a read-only mount would
                // silently yield 0 proposals.
                write_policy.workspace_writable(),
            )
            .await
        }
        .with_context(|| format!("spawning {audit_type} CLI subprocess"))?;

        let _ = ctx.log_writer.write_section(
            &format!("{audit_type}_stdout_attempt_{attempt}"),
            if outcome.stdout.is_empty() {
                "(empty)"
            } else {
                outcome.stdout.as_str()
            },
        );
        let _ = ctx.log_writer.write_section(
            &format!("{audit_type}_stderr_attempt_{attempt}"),
            if outcome.stderr.is_empty() {
                "(empty)"
            } else {
                outcome.stderr.as_str()
            },
        );

        if outcome_to_terminal_err(
            &outcome,
            &mut ctx.log_writer,
            audit_type,
            params.executor_timeout_secs,
        )
        .is_some()
        {
            // Fail closed: a terminal session error (timeout, non-zero exit,
            // OR an uncaptured exit status) is a did-not-complete outcome —
            // surfaced to chatops and cadence-preserving, never a silent
            // "0 findings". `outcome_to_terminal_err` has already logged the
            // specific cause to the run log.
            return Ok(AuditOutcome::DidNotComplete {
                audit_type: audit_type.to_string(),
                cause: AuditFailureCause::SessionErrored,
                examined_summary: None,
            });
        }

        // a01: collect new units across BOTH planning lanes. Issue-lane
        // units appear only for a planning-lanes audit whose agent actually
        // wrote there; a spec-lane-only audit sees the changes lane alone
        // (before_issues is empty, so the issues branch adds nothing).
        let after_changes = snapshot_change_dirs(ctx.workspace);
        let mut new_units: Vec<ProducedUnit> = after_changes
            .difference(&before_changes)
            .map(|name| ProducedUnit {
                lane: Lane::Changes,
                name: name.clone(),
            })
            .collect();
        if params.planning_lanes {
            let after_issues = snapshot_issue_dirs(ctx.workspace);
            new_units.extend(
                after_issues
                    .difference(&before_issues)
                    .map(|name| ProducedUnit {
                        lane: Lane::Issues,
                        name: name.clone(),
                    }),
            );
        }
        // Deterministic order across lanes: by name, then lane (Changes <
        // Issues) so the per-run cap drops the same units on every run.
        new_units.sort_by(|a, b| a.name.cmp(&b.name).then(a.lane.cmp(&b.lane)));

        let cap = params.max_proposals as usize;
        if new_units.len() > cap {
            let dropped: Vec<ProducedUnit> = new_units.split_off(cap);
            for u in &dropped {
                let path = unit_path(ctx.workspace, u.lane, &u.name);
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    tracing::warn!(
                        url = %ctx.repo.url,
                        audit_type = audit_type,
                        path = %path.display(),
                        "failed to remove over-cap unit dir: {e}"
                    );
                }
            }
            let _ = ctx.log_writer.write_section(
                &format!("{audit_type}_dropped_over_cap"),
                &format!(
                    "cap: {}\ndropped:\n{}",
                    params.max_proposals,
                    dropped
                        .iter()
                        .map(ProducedUnit::label)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            );
        }

        if new_units.is_empty() {
            // Zero proposals. Fail closed on a degenerate session: a clean
            // exit that produced NO real output is "did not complete", NOT a
            // silent "no findings" (the `gatekeepers-fail-closed` standard).
            // A session that produced substantive output but no change dirs is
            // an evidenced genuine no-findings run, carrying that output as the
            // examined-summary.
            match summarize_session_output(&outcome.stdout) {
                None => {
                    let _ = ctx.log_writer.write_section(
                        &format!("{audit_type}_outcome"),
                        &format!(
                            "kind: DidNotComplete\ncause: no_terminal_verdict\nretries_used: {attempt}"
                        ),
                    );
                    return Ok(AuditOutcome::DidNotComplete {
                        audit_type: audit_type.to_string(),
                        cause: AuditFailureCause::NoTerminalVerdict,
                        examined_summary: None,
                    });
                }
                examined @ Some(_) => {
                    let _ = ctx.log_writer.write_section(
                        &format!("{audit_type}_outcome"),
                        &format!("kind: SpecsWritten\nvalidated_count: 0\nretries_used: {attempt}"),
                    );
                    return Ok(AuditOutcome::SpecsWritten {
                        changes: Vec::new(),
                        retries_used: attempt,
                        examined_summary: examined,
                    });
                }
            }
        }

        // a01/a02: validate each produced unit in its lane, then run the
        // enabled authoring-time verifier gates against it.
        //
        // - SPEC lane: `openspec validate --strict` (a01), then — when enabled
        //   — the `[in]` and `[canon]` gate checks (a02). A `--strict` failure,
        //   a gate finding, OR a gate that could-not-run all hold the unit (it
        //   is not committed).
        // - ISSUE lane: carries no spec delta to `--strict`, but — when the
        //   `[canon]` gate is enabled — runs the authoring-time contract-change
        //   check (a02). A finding re-routes it to the spec lane via the retry.
        //
        // a02: a failure can now land in EITHER lane, so `failures` (and the
        // `prior_attempt_units` derived from it for cleanup) carries each unit's
        // lane — cleanup resolves it via [`unit_path`], not a fixed prefix.
        let mut validated: Vec<ProducedUnit> = Vec::new();
        let mut failures: Vec<UnitFailure> = Vec::new();
        for u in &new_units {
            let gate_outcome = match u.lane {
                Lane::Changes => {
                    match validate_change(params.openspec_command, ctx.workspace, &u.name).await {
                        Err(e) => {
                            failures.push(UnitFailure {
                                unit: u.clone(),
                                error: format!("{e:#}"),
                                kind: FailureKind::Strict,
                            });
                            continue;
                        }
                        Ok(()) => {
                            params
                                .gate_checker
                                .check_spec_change(ctx.workspace, &u.name)
                                .await
                        }
                    }
                }
                Lane::Issues => {
                    params
                        .gate_checker
                        .check_issue_unit(ctx.workspace, &u.name)
                        .await
                }
            };
            match gate_outcome {
                GateCheckOutcome::Clean => validated.push(u.clone()),
                GateCheckOutcome::Found(narrative) => failures.push(UnitFailure {
                    unit: u.clone(),
                    error: narrative,
                    kind: FailureKind::GateFinding,
                }),
                GateCheckOutcome::CouldNotRun(cause) => failures.push(UnitFailure {
                    unit: u.clone(),
                    error: cause,
                    kind: FailureKind::GateCouldNotRun,
                }),
            }
        }

        // Log every per-unit failure so operators can audit exactly what the
        // LLM produced and why. `--strict` rejections keep their historical
        // section name + WARN; gate failures (findings / could-not-run) carry
        // the fail-closed framing.
        for f in &failures {
            match f.kind {
                FailureKind::Strict => {
                    let _ = ctx.log_writer.write_section(
                        &format!(
                            "{audit_type}_validation_failure_{}_attempt_{attempt}",
                            f.unit.name
                        ),
                        &format!(
                            "change: {}\nattempt: {attempt}\nerror: {}",
                            f.unit.name, f.error
                        ),
                    );
                    tracing::warn!(
                        url = %ctx.repo.url,
                        audit_type = audit_type,
                        change = %f.unit.name,
                        attempt = attempt,
                        "rejecting agent-produced change that failed `openspec validate --strict`: {}",
                        f.error
                    );
                }
                FailureKind::GateFinding | FailureKind::GateCouldNotRun => {
                    let kind_str = if matches!(f.kind, FailureKind::GateCouldNotRun) {
                        "gate_could_not_run"
                    } else {
                        "gate_finding"
                    };
                    let _ = ctx.log_writer.write_section(
                        &format!(
                            "{audit_type}_gate_failure_{}_attempt_{attempt}",
                            f.unit.name
                        ),
                        &format!(
                            "unit: {}\nattempt: {attempt}\nkind: {kind_str}\ndetail: {}",
                            f.unit.label(),
                            f.error
                        ),
                    );
                    tracing::warn!(
                        url = %ctx.repo.url,
                        audit_type = audit_type,
                        unit = %f.unit.label(),
                        attempt = attempt,
                        kind = kind_str,
                        "holding agent-produced unit that failed an authoring-time verifier gate (fail-closed): {}",
                        f.error
                    );
                }
            }
        }

        if !validated.is_empty() {
            // Mixed run: keep valid units, drop the failed ones (in either
            // lane — a02), commit, return. A clean unit produced in the same
            // run still commits even when a sibling failed a gate.
            for f in &failures {
                let path = unit_path(ctx.workspace, f.unit.lane, &f.unit.name);
                if let Err(rm_err) = std::fs::remove_dir_all(&path) {
                    tracing::warn!(
                        url = %ctx.repo.url,
                        audit_type = audit_type,
                        path = %path.display(),
                        "failed to remove rejected unit dir: {rm_err}"
                    );
                }
            }
            // `🔍 created proposal` notification (per
            // `a02-audit-proposal-created-notification`). Fires AFTER
            // per-change validation succeeds AND BEFORE the git commit
            // that ships the proposal, so operators see the audit's
            // signal in the channel ahead of the implementer's
            // `🚀 starting work on …` message on the next iteration.
            // One notification per validated SPEC-lane unit; an issue-lane
            // unit carries no `proposal.md` `## Why`, AND the a02 signal is
            // the spec-proposal counterpart, so issue units are committed
            // silently and surface when the issues walker works them.
            for u in &validated {
                if u.lane != Lane::Changes {
                    continue;
                }
                let why_excerpt = read_proposal_why_first_line(ctx.workspace, &u.name);
                post_proposal_created_notification(
                    ctx.chatops_ctx,
                    &ctx.repo.url,
                    audit_type,
                    &u.name,
                    &why_excerpt,
                    attempt,
                    max_retries,
                )
                .await;
            }
            // a01: stage BOTH planning lanes for a planning-lanes audit;
            // a spec-lane-only audit (canon_consolidation) stages just
            // `openspec/changes/`.
            if params.planning_lanes {
                git_add_planning_lanes(ctx.workspace)
                    .with_context(|| format!("staging {audit_type}'s planning lanes for commit"))?;
            } else {
                git_add_openspec_changes(ctx.workspace).with_context(|| {
                    format!("staging {audit_type}'s openspec/changes/ for commit")
                })?;
            }
            // a01: a planning-lanes audit counts UNITS (it can span both
            // lanes); a spec-lane-only audit counts CHANGES (unchanged).
            let unit_noun = if params.planning_lanes { "unit" } else { "change" };
            let commit_msg = format!(
                "audit: {} ({} {unit_noun}(s))",
                params.commit_subject,
                validated.len()
            );
            crate::git::commit(ctx.workspace, &commit_msg).with_context(|| {
                format!("committing {audit_type}'s {} {unit_noun}(s)", validated.len())
            })?;

            // `SpecsWritten` carries unit directory NAMES regardless of which
            // lane produced them, so the same iteration's lane walkers (the
            // changes walker AND the issues walker) pick them up under the
            // established `issues > changes` precedence.
            let validated_names: Vec<String> =
                validated.iter().map(|u| u.name.clone()).collect();
            let _ = ctx.log_writer.write_section(
                &format!("{audit_type}_outcome"),
                &format!(
                    "kind: SpecsWritten\nvalidated_count: {}\nretries_used: {attempt}\nunits:\n{}",
                    validated.len(),
                    validated
                        .iter()
                        .map(ProducedUnit::label)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            );

            return Ok(AuditOutcome::SpecsWritten {
                changes: validated_names,
                retries_used: attempt,
                examined_summary: summarize_session_output(&outcome.stdout),
            });
        }

        // All produced units failed (a `--strict` rejection, a gate finding, OR
        // a gate that could not run). Drop them and either retry (budget
        // remains) or fail closed.
        prior_attempt_units = failures.iter().map(|f| f.unit.clone()).collect();
        let combined_err = failures
            .iter()
            .map(|f| format!("{}: {}", f.unit.label(), f.error))
            .collect::<Vec<_>>()
            .join("\n");

        if attempt + 1 < total_attempts {
            // Retry. Compose the addendum for the next LLM call — the `--strict`
            // framing AND/OR the gate-finding framing with the permitted
            // resolutions (a02 task 2.1). The dirs get deleted at the top of
            // the next iteration.
            last_addendum_body =
                Some(build_retry_addendum(&failures, params.issues_lane_enabled));
            continue;
        }

        // Exhausted budget. Clean up first (each unit in its own lane — a02).
        for u in &prior_attempt_units {
            let path = unit_path(ctx.workspace, u.lane, &u.name);
            let _ = std::fs::remove_dir_all(&path);
        }
        prior_attempt_units.clear();

        // a02: a contradiction / required-contract-change / gate-could-not-run
        // that survived to budget exhaustion fails CLOSED — the offending unit
        // is NOT committed AND the audit resolves to `DidNotComplete` (the
        // found-but-could-not-persist disposition). The scheduler surfaces it
        // via the audit-failure path (the `🚫` did-not-complete notification),
        // so — unlike the `ValidationExhausted` path below — this branch does
        // NOT post chatops itself. A pure `--strict` exhaustion keeps the
        // historical `ValidationExhausted` + `❌`-notification behavior.
        if failures.iter().any(|f| f.kind != FailureKind::Strict) {
            let _ = ctx.log_writer.write_section(
                &format!("{audit_type}_outcome"),
                &format!(
                    "kind: DidNotComplete\ncause: found_but_could_not_persist\nretries_attempted: {attempt}\nfinal_error:\n{combined_err}"
                ),
            );
            return Ok(AuditOutcome::DidNotComplete {
                audit_type: audit_type.to_string(),
                cause: AuditFailureCause::FoundButCouldNotPersist,
                examined_summary: summarize_session_output(&outcome.stdout),
            });
        }

        let _ = ctx.log_writer.write_section(
            &format!("{audit_type}_outcome"),
            &format!(
                "kind: ValidationExhausted\nretries_attempted: {attempt}\nfinal_error:\n{combined_err}"
            ),
        );

        // Post the chatops `❌` notification directly so the helper's
        // single-slug directory cleanup does not race with our multi-
        // dir cleanup above. Multi-line / long errors route through the
        // threaded notification path; short single-line errors continue
        // to use the inline single-message form.
        if let Some(chat_ctx) = ctx.chatops_ctx
            && let Err(e) = post_validation_exhausted_notification(
                chat_ctx,
                &ctx.repo.url,
                audit_type,
                attempt,
                &combined_err,
            )
            .await
        {
            tracing::warn!(
                url = %ctx.repo.url,
                audit_type = audit_type,
                "validation-exhausted chatops post failed: {e:#}"
            );
        }

        return Ok(AuditOutcome::ValidationExhausted {
            audit_type: audit_type.to_string(),
            retries_attempted: attempt,
            final_error: combined_err,
        });
    }

    // Loop always returns inside; this is unreachable in practice but
    // makes the function total without a panic.
    unreachable!(
        "specs-writing retry loop must return from inside; max_retries was {max_retries}"
    )
}


/// Enumerate the immediate child directory names under
/// `<workspace>/openspec/changes/`. Returns an empty set if the
/// directory is absent (fresh repo with no changes yet). The `archive/`
/// subdirectory is filtered out so archived changes never count as
/// newly created.
pub(crate) fn snapshot_change_dirs(workspace: &Path) -> HashSet<String> {
    let changes = workspace.join("openspec/changes");
    let Ok(entries) = std::fs::read_dir(&changes) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if name == "archive" {
                continue;
            }
            out.insert(name.to_string());
        }
    }
    out
}

/// Enumerate the immediate child directory names under the issues lane
/// (`<workspace>/openspec/issues/`, the path the issues walker reads via
/// [`crate::lanes::issues::ISSUES_SUBDIR`]). Mirrors [`snapshot_change_dirs`]:
/// returns an empty set when the directory is absent, AND filters the
/// `archive/` subdirectory so archived issues never count as newly created.
pub(crate) fn snapshot_issue_dirs(workspace: &Path) -> HashSet<String> {
    let issues = workspace.join(crate::lanes::issues::ISSUES_SUBDIR);
    let Ok(entries) = std::fs::read_dir(&issues) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if name == "archive" {
                continue;
            }
            out.insert(name.to_string());
        }
    }
    out
}

/// Which planning lane a produced unit landed in. Ordered so `Changes <
/// Issues` — the tie-break the per-run cap uses for deterministic dropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Lane {
    Changes,
    Issues,
}

/// A unit the agent produced this run, tagged with the lane it landed in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProducedUnit {
    lane: Lane,
    name: String,
}

impl ProducedUnit {
    /// `<lane>/<name>` label used in the run-log's dropped/units sections.
    fn label(&self) -> String {
        let lane = match self.lane {
            Lane::Changes => "openspec/changes",
            Lane::Issues => crate::lanes::issues::ISSUES_SUBDIR,
        };
        format!("{lane}/{}", self.name)
    }
}

/// Absolute path to a produced unit's directory, in its lane.
fn unit_path(workspace: &Path, lane: Lane, name: &str) -> std::path::PathBuf {
    match lane {
        Lane::Changes => workspace.join("openspec/changes").join(name),
        Lane::Issues => workspace
            .join(crate::lanes::issues::ISSUES_SUBDIR)
            .join(name),
    }
}

/// Why a produced unit failed the per-attempt checks (a02). Distinguishes a
/// `--strict` rejection (keeps the historical `ValidationExhausted` exhaustion
/// outcome) from an authoring-time verifier-gate failure (a contradiction /
/// required contract change, OR a gate that could-not-run — both fail CLOSED to
/// `DidNotComplete` on exhaustion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    /// `openspec validate --strict` rejected the spec-lane change.
    Strict,
    /// An enabled `[in]`/`[canon]` gate found a contradiction (spec lane) OR
    /// the issue contract-change check found a required contract change.
    GateFinding,
    /// An enabled authoring-time gate could not run (transport / parse /
    /// no-submission). Held — never treated as clean (fail-closed posture).
    GateCouldNotRun,
}

/// One produced unit that failed an attempt's checks, with its lane (for
/// cleanup), the error / finding narrative (for the retry addendum AND the
/// run-log), AND the [`FailureKind`] (which governs the exhaustion outcome).
struct UnitFailure {
    unit: ProducedUnit,
    error: String,
    kind: FailureKind,
}

/// Compose the next attempt's prompt addendum from this attempt's failures
/// (a02 task 2.1). `--strict` failures keep the historical
/// [`build_validation_addendum`] framing (so the existing retry-prompt shape is
/// unchanged); gate findings / could-not-runs get a resolutions-aware addendum
/// that presents the findings AND the permitted resolutions so the rewrite is
/// directed. When both kinds are present this attempt, both sections are
/// included.
fn build_retry_addendum(failures: &[UnitFailure], issues_lane_enabled: bool) -> String {
    let mut sections: Vec<String> = Vec::new();
    let strict: Vec<&UnitFailure> = failures
        .iter()
        .filter(|f| f.kind == FailureKind::Strict)
        .collect();
    if !strict.is_empty() {
        let combined = strict
            .iter()
            .map(|f| format!("{}: {}", f.unit.label(), f.error))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(build_validation_addendum(&combined));
    }
    let gated: Vec<&UnitFailure> = failures
        .iter()
        .filter(|f| f.kind != FailureKind::Strict)
        .collect();
    if !gated.is_empty() {
        sections.push(build_gate_finding_addendum(&gated, issues_lane_enabled));
    }
    sections.join("\n\n")
}

/// Build the gate-finding retry addendum: the findings followed by the
/// permitted resolutions (align to canon, legible `MODIFIED`, convert to an
/// issue, OR re-route an issue to the spec lane). The convert-to-issue option
/// is offered only when the issues lane is available for this run.
fn build_gate_finding_addendum(gated: &[&UnitFailure], issues_lane_enabled: bool) -> String {
    let findings = gated
        .iter()
        .map(|f| format!("• ({})\n{}", f.unit.label(), f.error))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut resolutions = String::from(
        "Rewrite the offending unit(s) to resolve the finding(s). Permitted resolutions:\n\
         - ALIGN TO CANON: reword the change to reuse the canonical vocabulary so it no longer \
         conflicts with the existing requirement (no canonical requirement is modified).\n\
         - LEGIBLE `MODIFIED` DELTA: when the only correct fix genuinely changes a canonical \
         contract, write a `## MODIFIED Requirements` delta of the contradicted requirement AND \
         state the contract change plainly in the proposal's `## Why` rationale. Do NOT make the \
         finding vanish by quietly bending the requirement to match the original change.\n",
    );
    if issues_lane_enabled {
        resolutions.push_str(
            "- CONVERT TO AN ISSUE: when the correct fix is behavior-preserving (the code drifted \
             from an already-correct spec), delete the spec-lane change and write the unit as an \
             `openspec/issues/<slug>/` issue instead (`issue.md` + `tasks.md`, NO `specs/`).\n",
        );
    }
    resolutions.push_str(
        "- RE-ROUTE AN ISSUE TO THE SPEC LANE: an issue flagged as requiring a contract change \
         must be rewritten as a spec-lane change under `openspec/changes/<slug>/` carrying the \
         legible `MODIFIED`/`ADDED` delta — delete the issue-lane unit.\n",
    );
    format!(
        "Your previous response produced unit(s) that failed an authoring-time verifier gate:\n\n\
         {findings}\n\n{resolutions}\nReply with the full revised unit(s)."
    )
}

/// a01: the per-run lane-availability addendum the daemon appends to a
/// planning-lanes audit's prompt. It states whether the issue lane is
/// offered (resolved `features.issues`) AND names the exact lane paths so
/// the agent writes where the walkers actually read. This is operational
/// guidance the daemon resolves at run time — distinct from the durable
/// lane-choice rule baked into the prompt template, which (per the project's
/// `Tests assert behavior or derivation, never message wording` standard) is
/// NOT pinned by a unit test; a01's behavior is verified through the lane the
/// audit selects and the artifacts it produces.
fn compose_lane_availability(issues_enabled: bool) -> String {
    let issues_subdir = crate::lanes::issues::ISSUES_SUBDIR;
    if issues_enabled {
        format!(
            "\n\n---\n\n## Output-lane availability for this run (set by the daemon)\n\n\
             `features.issues` is ENABLED for this repository: BOTH planning lanes are \
             available. Apply the lane-choice rule per finding — never default to the spec \
             lane.\n\
             - Spec-lane unit (the fix changes an observable contract) → write \
             `openspec/changes/<slug>/`.\n\
             - Issue-lane unit (a behavior-preserving fix to already-correctly-specified \
             code) → write `{issues_subdir}/<slug>/`, containing `issue.md` (acceptance \
             stated against the EXISTING specification) AND `tasks.md`, with NO `specs/` \
             directory.\n"
        )
    } else {
        format!(
            "\n\n---\n\n## Output-lane availability for this run (set by the daemon)\n\n\
             `features.issues` is DISABLED for this repository: ONLY the spec lane is \
             available. Write every unit as `openspec/changes/<slug>/`; do NOT write to the \
             issues lane (`{issues_subdir}/`) this run.\n"
        )
    }
}

async fn validate_change(
    openspec_command: &str,
    workspace: &Path,
    change_name: &str,
) -> Result<()> {
    let output = Command::new(openspec_command)
        .arg("validate")
        .arg(change_name)
        .arg("--strict")
        .current_dir(workspace)
        .output()
        .await
        .with_context(|| {
            format!("spawning `{openspec_command} validate {change_name} --strict`")
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr_tail: String = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(400)
        .collect();
    Err(anyhow!(
        "`{openspec_command} validate {change_name} --strict` exited {status}; stderr: {stderr_tail}",
        status = output.status,
    ))
}

fn git_add_openspec_changes(workspace: &Path) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("add")
        .arg("openspec/changes/")
        .current_dir(workspace)
        .status()
        .context("spawning `git add openspec/changes/`")?;
    if !status.success() {
        return Err(anyhow!("`git add openspec/changes/` exited {status}"));
    }
    Ok(())
}

/// a01: stage BOTH planning lanes for a planning-lanes audit's commit —
/// `openspec/changes/` AND the issues lane (`openspec/issues/`). Each lane
/// dir is staged only when it exists on disk (`git add <missing>` errors
/// with "pathspec did not match any files"); the caller only commits when
/// at least one lane produced a validated unit, so this always stages at
/// least one path. A relative lane dir that is absent is simply skipped.
fn git_add_planning_lanes(workspace: &Path) -> Result<()> {
    let lanes = ["openspec/changes", crate::lanes::issues::ISSUES_SUBDIR];
    for lane in lanes {
        if !workspace.join(lane).is_dir() {
            continue;
        }
        let pathspec = format!("{lane}/");
        let status = std::process::Command::new("git")
            .arg("add")
            .arg(&pathspec)
            .current_dir(workspace)
            .status()
            .with_context(|| format!("spawning `git add {pathspec}`"))?;
        if !status.success() {
            return Err(anyhow!("`git add {pathspec}` exited {status}"));
        }
    }
    Ok(())
}

/// Length cap for an audit's `examined_summary` (the agent's account of what
/// it looked at, distilled from the session transcript). Keeps the on-demand
/// chatops completion notification bounded.
const EXAMINED_SUMMARY_CAP: usize = 1500;

/// Distill an `examined_summary` from a session's captured output. Returns
/// `None` for an empty/whitespace-only transcript — the fail-closed signal
/// that a clean exit did NO real work, distinct from a substantive run that
/// genuinely found nothing. A non-empty transcript is trimmed AND tail-capped
/// (the agent's conclusion sits at the end) to `EXAMINED_SUMMARY_CAP` chars.
fn summarize_session_output(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let count = trimmed.chars().count();
    if count <= EXAMINED_SUMMARY_CAP {
        Some(trimmed.to_string())
    } else {
        let tail: String = trimmed.chars().skip(count - EXAMINED_SUMMARY_CAP).collect();
        Some(format!("…{tail}"))
    }
}

/// Pure transformation: given an [`crate::agentic_run::AgenticRunOutcome`],
/// return Some(error) if the outcome is terminal (timed out, non-zero exit,
/// OR an uncaptured exit status). Returns None when the caller should continue
/// processing. Mirrors the same-named helpers in the `architecture_advisor`
/// and `drift` audit modules.
fn outcome_to_terminal_err(
    outcome: &crate::agentic_run::AgenticRunOutcome,
    log_writer: &mut AuditLogWriter,
    audit_type: &str,
    timeout_secs: u64,
) -> Option<anyhow::Error> {
    if outcome.timed_out {
        let _ = log_writer.write_section(
            &format!("{audit_type}_outcome"),
            "kind: Err\nreason: timeout",
        );
        return Some(anyhow!(
            "{audit_type}: CLI exceeded the {timeout_secs}s timeout"
        ));
    }
    match outcome.exit_status {
        Some(status) if status.success() => None,
        Some(status) => {
            let _ = log_writer.write_section(
                &format!("{audit_type}_outcome"),
                &format!("kind: Err\nreason: exit {status}"),
            );
            Some(anyhow!("{audit_type}: CLI exited {status}"))
        }
        None => {
            // Fail closed: an uncaptured exit status (e.g. a signal kill) must
            // NOT fall through as a clean exit. Without this, a killed session
            // reaches the disk-diff and reports a silent "0 findings".
            let _ = log_writer.write_section(
                &format!("{audit_type}_outcome"),
                "kind: Err\nreason: exit status not captured",
            );
            Some(anyhow!(
                "{audit_type}: CLI exit status was not captured (likely signal-killed)"
            ))
        }
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;
    use crate::audits::AuditLogWriter;
    use tempfile::TempDir;

    fn make_log_writer(workspace: &std::path::Path) -> AuditLogWriter {
        let (td, paths) = crate::testing::test_daemon_paths();
        std::mem::forget(td);
        AuditLogWriter::open(&paths, workspace, "test_audit").expect("log writer opens")
    }

    /// Pure-data test: feed a synthesized `AgenticRunOutcome` with
    /// `timed_out: true` directly into `outcome_to_terminal_err` and
    /// assert the resulting error + log entries. No subprocess, no
    /// timer, no race — verifies the audit framework's translation
    /// logic, which is what we actually care about. Replaces the
    /// per-audit "spawn a real subprocess and time it out" tests
    /// that were race-prone across platforms.
    #[test]
    fn outcome_to_terminal_err_translates_timed_out_to_error() {
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        let mut log_writer = make_log_writer(workspace);
        let log_path = log_writer.path().to_path_buf();
        let outcome = crate::agentic_run::AgenticRunOutcome {
            timed_out: true,
            exit_status: None,
            stdout: String::new(),
            stderr: "timeout".into(),
            ..Default::default()
        };
        let err = outcome_to_terminal_err(&outcome, &mut log_writer, "missing_tests_audit", 1)
            .expect("timed_out outcome must produce Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("missing_tests_audit"));
        assert!(msg.contains("timeout"));
        let log = std::fs::read_to_string(&log_path).expect("log readable");
        assert!(log.contains("kind: Err"));
        assert!(log.contains("reason: timeout"));
    }

    #[test]
    fn outcome_to_terminal_err_translates_nonzero_exit_to_error() {
        use std::os::unix::process::ExitStatusExt;
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        let mut log_writer = make_log_writer(workspace);
        let outcome = crate::agentic_run::AgenticRunOutcome {
            timed_out: false,
            exit_status: Some(std::process::ExitStatus::from_raw(7 << 8)),
            stdout: String::new(),
            stderr: "boom".into(),
            ..Default::default()
        };
        let err = outcome_to_terminal_err(&outcome, &mut log_writer, "missing_tests_audit", 30)
            .expect("nonzero exit must produce Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("exit"));
    }

    #[test]
    fn outcome_to_terminal_err_returns_none_for_clean_outcome() {
        use std::os::unix::process::ExitStatusExt;
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        let mut log_writer = make_log_writer(workspace);
        let outcome = crate::agentic_run::AgenticRunOutcome {
            timed_out: false,
            exit_status: Some(std::process::ExitStatus::from_raw(0)),
            stdout: String::new(),
            stderr: String::new(),
            ..Default::default()
        };
        assert!(
            outcome_to_terminal_err(&outcome, &mut log_writer, "missing_tests_audit", 30).is_none()
        );
    }

    #[test]
    fn outcome_to_terminal_err_treats_uncaptured_exit_status_as_failure() {
        // Fail closed: a session whose exit status was never captured (e.g. a
        // signal kill) must be a terminal error, NOT a clean fall-through that
        // the disk-diff would report as a silent "0 findings".
        let ws_dir = TempDir::new().unwrap();
        let workspace = ws_dir.path();
        let mut log_writer = make_log_writer(workspace);
        let log_path = log_writer.path().to_path_buf();
        let outcome = crate::agentic_run::AgenticRunOutcome {
            timed_out: false,
            exit_status: None,
            stdout: "partial work".into(),
            stderr: String::new(),
            ..Default::default()
        };
        let err = outcome_to_terminal_err(&outcome, &mut log_writer, "security_bug_audit", 30)
            .expect("uncaptured exit status must produce Err");
        assert!(format!("{err:#}").contains("not captured"));
        let log = std::fs::read_to_string(&log_path).expect("log readable");
        assert!(log.contains("reason: exit status not captured"));
    }
}

/// a02: authoring-time gate-check + self-heal behavior, driven through
/// `run_specs_writing_audit` with a SCRIPTED [`AuthoringGateChecker`] so the
/// retry / re-route / fail-closed paths are exercised deterministically — no
/// CLI gate session, no task-local scope. The fake `claude` script writes the
/// units; the scripted checker decides each unit's gate verdict.
#[cfg(test)]
mod a02_gate_tests {
    use super::*;
    use crate::audits::{AuditContext, AuditLogWriter};
    use crate::config::{RepositoryConfig, ResolvedSandbox};
    use std::collections::VecDeque;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// One scripted gate verdict. `Copy` so a `VecDeque` is trivial to drain.
    #[derive(Clone, Copy)]
    enum Step {
        Clean,
        Found,
        CouldNotRun,
    }

    fn to_outcome(s: Step) -> GateCheckOutcome {
        match s {
            Step::Clean => GateCheckOutcome::Clean,
            Step::Found => GateCheckOutcome::Found("seeded contradiction with canon".into()),
            Step::CouldNotRun => {
                GateCheckOutcome::CouldNotRun("[verifier:canon] session failed".into())
            }
        }
    }

    /// Scripted checker: drains a per-lane queue of verdicts (empty → `Clean`)
    /// AND counts calls so tests can assert a gate did / did not run.
    struct ScriptedGateChecker {
        spec: Mutex<VecDeque<Step>>,
        issue: Mutex<VecDeque<Step>>,
        spec_calls: AtomicUsize,
        issue_calls: AtomicUsize,
    }

    impl ScriptedGateChecker {
        fn new(spec: Vec<Step>, issue: Vec<Step>) -> Self {
            Self {
                spec: Mutex::new(spec.into()),
                issue: Mutex::new(issue.into()),
                spec_calls: AtomicUsize::new(0),
                issue_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl AuthoringGateChecker for ScriptedGateChecker {
        async fn check_spec_change(&self, _ws: &Path, _slug: &str) -> GateCheckOutcome {
            self.spec_calls.fetch_add(1, Ordering::SeqCst);
            let step = self.spec.lock().unwrap().pop_front().unwrap_or(Step::Clean);
            to_outcome(step)
        }
        async fn check_issue_unit(&self, _ws: &Path, _slug: &str) -> GateCheckOutcome {
            self.issue_calls.fetch_add(1, Ordering::SeqCst);
            let step = self.issue.lock().unwrap().pop_front().unwrap_or(Step::Clean);
            to_outcome(step)
        }
    }

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn init_workspace() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        let runs = |args: &[&str]| {
            assert!(
                StdCommand::new("git")
                    .args(args)
                    .current_dir(&ws)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        runs(&["init", "-q", "-b", "main"]);
        runs(&["config", "user.email", "t@e.com"]);
        runs(&["config", "user.name", "t"]);
        std::fs::write(ws.join("README.md"), "hi\n").unwrap();
        runs(&["add", "README.md"]);
        runs(&["commit", "-q", "-m", "init"]);
        (dir, ws)
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
            sandbox: None,
        }
    }

    fn make_log_writer(workspace: &Path) -> AuditLogWriter {
        let (td, paths) = crate::testing::test_daemon_paths();
        std::mem::forget(td);
        AuditLogWriter::open(&paths, workspace, "a02_test").expect("log writer opens")
    }

    fn ok_validator(ws: &Path) -> PathBuf {
        write_script(ws, "ok.sh", "#!/bin/sh\nexit 0\n")
    }

    /// Drive `run_specs_writing_audit` with a fake `claude` script + scripted
    /// gate checker. Returns the outcome.
    #[allow(clippy::too_many_arguments)]
    async fn run_audit(
        ws: &Path,
        script: &Path,
        openspec_cmd: &Path,
        max_validation_retries: u32,
        checker: &dyn AuthoringGateChecker,
        issues_lane_enabled: bool,
        settings_dir: &Path,
    ) -> AuditOutcome {
        let sandbox = ResolvedSandbox::resolve(None);
        let repo = fixture_repo();
        let mut ctx = AuditContext {
            workspace: ws,
            repo: &repo,
            chatops_ctx: None,
            log_writer: make_log_writer(ws),
            max_validation_retries,
        };
        run_specs_writing_audit(
            SpecsWritingAuditParams {
                audit_type: "a02_test_audit",
                prompt: "survey and write units",
                max_proposals: 4,
                executor_command: &script.to_string_lossy(),
                executor_timeout_secs: 30,
                sandbox: &sandbox,
                settings_dir: Some(settings_dir),
                openspec_command: &openspec_cmd.to_string_lossy(),
                prompt_source: "<test>",
                commit_subject: "a02 test proposals",
                allowed_tools: ALLOWED_TOOLS,
                include_autocoder_tools: false,
                model: None,
                planning_lanes: true,
                issues_lane_enabled,
                gate_checker: checker,
            },
            &mut ctx,
        )
        .await
        .expect("run succeeds")
    }

    fn head(ws: &Path) -> String {
        let out = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(ws)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn cleanup_log(ws: &Path) {
        // Drop the per-test audit-log tree (best-effort).
        let lw = make_log_writer(ws);
        if let Some(parent) = lw.path().parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    /// 5.1: a seeded `[canon]` contradiction drives a retry with the finding in
    /// the addendum; the rewrite that aligns to canon passes the gate AND
    /// commits. Asserted via the gate result (Found → Clean across attempts) +
    /// the committed unit, NOT prompt wording.
    #[tokio::test]
    async fn seeded_contradiction_retries_then_commits_on_clean_rewrite() {
        let (_t, ws) = init_workspace();
        let dir = ws.join("openspec/changes/fix-thing").display().to_string();
        // Writes the same spec-lane change on every attempt (the gate verdict,
        // not the content, is what changes).
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!("#!/bin/sh\nmkdir -p '{dir}'\necho '# proposal' > '{dir}/proposal.md'\nexit 0\n"),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        // Attempt 0 → Found, attempt 1 → Clean.
        let checker = ScriptedGateChecker::new(vec![Step::Found, Step::Clean], vec![]);
        let outcome = run_audit(
            &ws,
            &script,
            &validator,
            1,
            &checker,
            true,
            settings.path(),
        )
        .await;
        match outcome {
            AuditOutcome::SpecsWritten {
                changes,
                retries_used,
                ..
            } => {
                assert_eq!(changes, vec!["fix-thing".to_string()]);
                assert_eq!(retries_used, 1, "the contradiction consumed one retry");
            }
            other => panic!("expected SpecsWritten after self-heal, got {other:?}"),
        }
        assert_eq!(checker.spec_calls.load(Ordering::SeqCst), 2, "checked each attempt");
        // The committed unit cleared the gate (final check was Clean).
        let tracked = StdCommand::new("git")
            .args(["ls-files", "openspec/changes/fix-thing"])
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(!tracked.stdout.is_empty(), "the self-healed change is committed");
        cleanup_log(&ws);
    }

    /// 5.2 (part 1): budget exhaustion on an unresolved contradiction yields
    /// `DidNotComplete` (found-but-could-not-persist) AND no commit.
    #[tokio::test]
    async fn unresolved_contradiction_at_budget_exhaustion_fails_closed_no_commit() {
        let (_t, ws) = init_workspace();
        let dir = ws.join("openspec/changes/fix-bad").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!("#!/bin/sh\nmkdir -p '{dir}'\necho '# p' > '{dir}/proposal.md'\nexit 0\n"),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        let head_before = head(&ws);
        // Found on every attempt (max_retries 1 → 2 attempts → 2 Founds).
        let checker = ScriptedGateChecker::new(vec![Step::Found, Step::Found], vec![]);
        let outcome = run_audit(&ws, &script, &validator, 1, &checker, true, settings.path()).await;
        assert!(
            matches!(
                outcome,
                AuditOutcome::DidNotComplete {
                    cause: AuditFailureCause::FoundButCouldNotPersist,
                    ..
                }
            ),
            "an unresolved contradiction must fail closed to DidNotComplete, got {outcome:?}"
        );
        assert_eq!(head_before, head(&ws), "no commit on the fail-closed path");
        assert!(
            !ws.join("openspec/changes/fix-bad").exists(),
            "the offending unit is dropped, not left in the tree"
        );
        cleanup_log(&ws);
    }

    /// 5.2 (part 2): a clean sibling unit produced in the same run still
    /// commits even when a sibling fails its gate (the mixed-run path drops the
    /// failed unit AND commits the clean one).
    #[tokio::test]
    async fn clean_sibling_commits_when_other_unit_fails_gate() {
        let (_t, ws) = init_workspace();
        let good = ws.join("openspec/changes/fix-good").display().to_string();
        let bad = ws.join("openspec/changes/fix-bad").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!(
                "#!/bin/sh\nmkdir -p '{good}' '{bad}'\necho '# g' > '{good}/proposal.md'\necho '# b' > '{bad}/proposal.md'\nexit 0\n"
            ),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        // Units sort by name: fix-bad (Found), fix-good (Clean).
        let checker = ScriptedGateChecker::new(vec![Step::Found, Step::Clean], vec![]);
        let outcome = run_audit(&ws, &script, &validator, 0, &checker, true, settings.path()).await;
        match outcome {
            AuditOutcome::SpecsWritten { changes, .. } => {
                assert_eq!(changes, vec!["fix-good".to_string()], "only the clean sibling commits");
            }
            other => panic!("expected SpecsWritten with the clean sibling, got {other:?}"),
        }
        assert!(ws.join("openspec/changes/fix-good").exists());
        assert!(
            !ws.join("openspec/changes/fix-bad").exists(),
            "the gate-failing sibling is dropped"
        );
        cleanup_log(&ws);
    }

    /// 5.3: with the gate disabled (the production `ScopedGateChecker` and NO
    /// scoped verifier-gate context), neither the spec-lane nor the issue
    /// check runs at authoring time — the unit commits on its a01 structural
    /// validity.
    #[tokio::test]
    async fn disabled_gate_commits_on_structural_rules() {
        let (_t, ws) = init_workspace();
        let dir = ws.join("openspec/changes/fix-structural").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!("#!/bin/sh\nmkdir -p '{dir}'\necho '# p' > '{dir}/proposal.md'\nexit 0\n"),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        // The production checker with no task-local scope → both checks Clean.
        let checker = ScopedGateChecker;
        let outcome = run_audit(&ws, &script, &validator, 0, &checker, true, settings.path()).await;
        match outcome {
            AuditOutcome::SpecsWritten { changes, .. } => {
                assert_eq!(changes, vec!["fix-structural".to_string()]);
            }
            other => panic!("expected SpecsWritten (gate disabled), got {other:?}"),
        }
        cleanup_log(&ws);
    }

    /// 5.3 (unit): the production `ScopedGateChecker` returns `Clean` for both
    /// lanes when no verifier-gate context is scoped — i.e. the authoring-time
    /// checks do NOT run when the gates are disabled.
    #[tokio::test]
    async fn scoped_checker_is_clean_when_no_gate_scoped() {
        let tmp = TempDir::new().unwrap();
        let checker = ScopedGateChecker;
        assert!(matches!(
            checker.check_spec_change(tmp.path(), "x").await,
            GateCheckOutcome::Clean
        ));
        assert!(matches!(
            checker.check_issue_unit(tmp.path(), "x").await,
            GateCheckOutcome::Clean
        ));
    }

    /// 5.4: an issue whose fix would require a contract change is re-routed to
    /// the spec lane (the committed unit is `openspec/changes/<slug>/`, not
    /// `openspec/issues/<slug>/`); an honest issue commits as an issue.
    #[tokio::test]
    async fn issue_requiring_contract_change_is_rerouted_to_spec_lane() {
        let (_t, ws) = init_workspace();
        let issue = ws.join("openspec/issues/fix-route").display().to_string();
        let change = ws.join("openspec/changes/fix-route").display().to_string();
        let toggle = ws.join(".attempt-toggle").display().to_string();
        // Attempt 0: write an issue. Attempt 1: write a spec-lane change
        // (the agent re-routed per the addendum).
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!(
                "#!/bin/sh\nif [ ! -f '{toggle}' ]; then\n  touch '{toggle}'\n  mkdir -p '{issue}'\n  printf '## Report\\nbug\\n' > '{issue}/issue.md'\n  printf '## 1\\n- [ ] 1.1 fix\\n' > '{issue}/tasks.md'\nelse\n  mkdir -p '{change}'\n  echo '# proposal' > '{change}/proposal.md'\nfi\nexit 0\n"
            ),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        // Issue check Found (attempt 0); spec check Clean (attempt 1).
        let checker = ScriptedGateChecker::new(vec![Step::Clean], vec![Step::Found]);
        let outcome = run_audit(&ws, &script, &validator, 1, &checker, true, settings.path()).await;
        match outcome {
            AuditOutcome::SpecsWritten {
                changes,
                retries_used,
                ..
            } => {
                assert_eq!(changes, vec!["fix-route".to_string()]);
                assert_eq!(retries_used, 1, "the re-route consumed one retry");
            }
            other => panic!("expected SpecsWritten after re-route, got {other:?}"),
        }
        // The committed unit is a spec-lane CHANGE, not an issue.
        let tracked = |path: &str| {
            !StdCommand::new("git")
                .args(["ls-files", path])
                .current_dir(&ws)
                .output()
                .unwrap()
                .stdout
                .is_empty()
        };
        assert!(tracked("openspec/changes/fix-route"), "the re-routed unit is a change");
        assert!(
            !tracked("openspec/issues/fix-route"),
            "the issue-lane unit must not be committed"
        );
        assert!(
            !ws.join("openspec/issues/fix-route").exists(),
            "the original issue dir is cleaned up on re-route"
        );
        cleanup_log(&ws);
    }

    /// 5.4 (honest issue): an issue whose contract-change check is clean commits
    /// as an issue (no re-route).
    #[tokio::test]
    async fn honest_issue_commits_as_issue() {
        let (_t, ws) = init_workspace();
        let issue = ws.join("openspec/issues/fix-honest").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!(
                "#!/bin/sh\nmkdir -p '{issue}'\nprintf '## Report\\nbug\\n' > '{issue}/issue.md'\nprintf '## 1\\n- [ ] 1.1 fix\\n' > '{issue}/tasks.md'\nexit 0\n"
            ),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        let checker = ScriptedGateChecker::new(vec![], vec![Step::Clean]);
        let outcome = run_audit(&ws, &script, &validator, 0, &checker, true, settings.path()).await;
        match outcome {
            AuditOutcome::SpecsWritten { changes, .. } => {
                assert_eq!(changes, vec!["fix-honest".to_string()]);
            }
            other => panic!("expected SpecsWritten, got {other:?}"),
        }
        assert!(ws.join("openspec/issues/fix-honest/issue.md").is_file());
        assert!(
            !ws.join("openspec/changes/fix-honest").exists(),
            "an honest issue is NOT routed to the spec lane"
        );
        cleanup_log(&ws);
    }

    /// 5.5: a gate that fails to run (could-not-run) does NOT commit the unit
    /// AND surfaces the failure (fail-closed) — `DidNotComplete`, distinct from
    /// a clean empty `SpecsWritten`.
    #[tokio::test]
    async fn gate_could_not_run_fails_closed_no_commit() {
        let (_t, ws) = init_workspace();
        let dir = ws.join("openspec/changes/fix-held").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!("#!/bin/sh\nmkdir -p '{dir}'\necho '# p' > '{dir}/proposal.md'\nexit 0\n"),
        );
        let validator = ok_validator(&ws);
        let settings = TempDir::new().unwrap();
        let head_before = head(&ws);
        let checker = ScriptedGateChecker::new(vec![Step::CouldNotRun], vec![]);
        let outcome = run_audit(&ws, &script, &validator, 0, &checker, true, settings.path()).await;
        assert!(
            matches!(
                outcome,
                AuditOutcome::DidNotComplete {
                    cause: AuditFailureCause::FoundButCouldNotPersist,
                    ..
                }
            ),
            "a gate that could-not-run must fail closed (not a clean SpecsWritten), got {outcome:?}"
        );
        assert_eq!(head_before, head(&ws), "no commit when the gate could not run");
        assert!(!ws.join("openspec/changes/fix-held").exists());
        cleanup_log(&ws);
    }

    /// A pure `--strict` exhaustion still resolves to `ValidationExhausted`
    /// (NOT `DidNotComplete`) — the a02 fail-closed path is reserved for gate
    /// failures, leaving the historical strict-validation behavior intact.
    #[tokio::test]
    async fn strict_only_exhaustion_stays_validation_exhausted() {
        let (_t, ws) = init_workspace();
        let dir = ws.join("openspec/changes/fix-invalid").display().to_string();
        let script = write_script(
            &ws,
            "fake-claude.sh",
            &format!("#!/bin/sh\nmkdir -p '{dir}'\necho '# p' > '{dir}/proposal.md'\nexit 0\n"),
        );
        let bad_validator =
            write_script(&ws, "bad.sh", "#!/bin/sh\necho 'missing SHALL' >&2\nexit 2\n");
        let settings = TempDir::new().unwrap();
        // The gate never gets to run (the unit fails --strict first), so the
        // checker stays Clean/unused.
        let checker = ScriptedGateChecker::new(vec![], vec![]);
        let outcome =
            run_audit(&ws, &script, &bad_validator, 0, &checker, true, settings.path()).await;
        assert!(
            matches!(outcome, AuditOutcome::ValidationExhausted { .. }),
            "a pure --strict exhaustion stays ValidationExhausted, got {outcome:?}"
        );
        assert_eq!(
            checker.spec_calls.load(Ordering::SeqCst),
            0,
            "the gate is never reached when --strict fails first"
        );
        cleanup_log(&ws);
    }
}

