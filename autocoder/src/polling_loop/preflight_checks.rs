use super::*;

/// Run the spec-delta archivability pre-flight (a17) against `change`.
/// On clean result: returns `Ok(None)` and the caller proceeds to the
/// executor. On any violation: writes the `.needs-spec-revision.json`
/// marker with `unarchivable_deltas` populated, posts the existing
/// `AlertCategory::SpecNeedsRevision` chatops alert (subject to the 24h
/// throttle), and returns `Ok(Some(QueueStep::SpecRevisionMarked))` so
/// the caller short-circuits without invoking the executor.
pub(crate) async fn handle_archivability_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
) -> Result<Option<QueueStep>> {
    let violations =
        crate::preflight::spec_archivability::check_spec_deltas_archivable(workspace, change)
            .with_context(|| format!("spec-delta archivability check for `{change}`"))?;
    if violations.is_empty() {
        return Ok(None);
    }
    let suggestion = build_unarchivable_revision_suggestion(change, &violations);
    tracing::warn!(
        url = %repo.url,
        change = %change,
        violations = violations.len(),
        "spec-delta archivability pre-flight FAILED; skipping executor and writing marker"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: violations.clone(),
        canon_editing_tasks: Vec::new(),
        revision_suggestion: suggestion.clone(),
        gate_error: None,
        contradictions: Vec::new(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to write spec-needs-revision marker (pre-flight): {e:#}"
        );
    }
    maybe_post_unarchivable_deltas_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        &violations,
        &suggestion,
    )
    .await;
    Ok(Some(QueueStep::SpecRevisionMarked))
}

/// Compose the auto-generated `revision_suggestion` text written into
/// the marker file when the pre-flight catches one or more unarchivable
/// deltas. Names each violation and points the operator at the spec
/// file to edit + the recovery verb.
fn build_unarchivable_revision_suggestion(
    change: &str,
    violations: &[crate::preflight::spec_archivability::UnarchivableDelta],
) -> String {
    let mut out = format!(
        "Pre-flight check found {} unarchivable spec delta{}:\n",
        violations.len(),
        if violations.len() == 1 { "" } else { "s" }
    );
    for v in violations {
        out.push_str(&format!(
            "- capability={cap} kind={kind} header=\"{hdr}\" reason=\"{reason}\"\n",
            cap = v.capability,
            kind = v.kind.as_str(),
            hdr = v.header,
            reason = v.reason,
        ));
    }
    out.push_str(&format!(
        "\nEdit openspec/changes/{change}/specs/<capability>/spec.md to use the\n\
         exact canonical header. After fixing, push the spec change AND clear\n\
         this marker via @<bot> clear-revision <repo> <change>.\n"
    ));
    out
}

/// Run the canon-editing-tasks pre-flight against `change`. A sibling of
/// [`handle_archivability_preflight`] — same point in the pipeline, same
/// marker, same halt semantics — but it scans `tasks.md` CONTENT for a task
/// that directs a direct edit to the canonical specs (`openspec/specs/`).
///
/// The implementer implements code and tests only; a change's spec delta is
/// folded into canon by `openspec archive`. A task that instead applies the
/// delta to `openspec/specs/` makes the implementer pre-fold canon, after which
/// `openspec archive` aborts on a duplicate requirement and the change goes
/// perma-stuck — so the defect is caught here, before any executor or
/// verifier-gate run is spent on it.
///
/// On a clean result: returns `Ok(None)` and the caller proceeds. On a flag:
/// writes the `.needs-spec-revision.json` marker with `canon_editing_tasks`
/// populated, posts the `AlertCategory::SpecNeedsRevision` chatops alert
/// (24h-throttled), and returns `Ok(Some(QueueStep::SpecRevisionMarked))` so the
/// caller short-circuits without invoking the executor OR the `[in]`/`[canon]`
/// verifier gates.
pub(crate) async fn handle_canon_editing_tasks_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
) -> Result<Option<QueueStep>> {
    let offending =
        crate::preflight::canon_editing_tasks::check_tasks_edit_canon(workspace, change);
    if offending.is_empty() {
        return Ok(None);
    }
    let suggestion = build_canon_editing_revision_suggestion(change, &offending);
    tracing::warn!(
        url = %repo.url,
        change = %change,
        offending = offending.len(),
        "canon-editing-tasks pre-flight FAILED; skipping executor and writing marker"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: offending.clone(),
        revision_suggestion: suggestion.clone(),
        gate_error: None,
        contradictions: Vec::new(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to write spec-needs-revision marker (canon-editing pre-flight): {e:#}"
        );
    }
    maybe_post_canon_editing_tasks_alert(paths, chatops_ctx, repo, change, &offending, &suggestion)
        .await;
    Ok(Some(QueueStep::SpecRevisionMarked))
}

/// Compose the `revision_suggestion` written into the marker when the
/// canon-editing-tasks pre-flight flags a change. Names each offending task AND
/// states the rule: the implementer implements code and tests only; the spec
/// delta is folded into canon by `openspec archive`, so no task may apply it to
/// `openspec/specs/`.
fn build_canon_editing_revision_suggestion(change: &str, offending: &[String]) -> String {
    let mut out = format!(
        "Pre-flight found {} task{} directing an edit to the canonical specs:\n",
        offending.len(),
        if offending.len() == 1 { "" } else { "s" }
    );
    for task in offending {
        out.push_str(&format!("- {task}\n"));
    }
    out.push_str(&format!(
        "\nThe implementer implements CODE and TESTS only. A change's spec delta\n\
         lives in openspec/changes/{change}/specs/<capability>/spec.md and is folded\n\
         into the canonical openspec/specs/ by `openspec archive` automatically — a\n\
         task must NOT apply it to openspec/specs/ (doing so makes archive abort on a\n\
         duplicate requirement). Remove the offending task(s) from\n\
         openspec/changes/{change}/tasks.md, push, AND clear this marker via\n\
         @<bot> clear-revision <repo> <change> (or delete the marker file).\n"
    ));
    out
}

/// Run the change-internal contradiction pre-flight — the `[in]` gate — against
/// `change`, returning the gate's [`GateVerdict`] (verifier-gates-fail-closed
/// §5). The verdict is what the default-deny ledger records:
///   - `Clean` → [`GateVerdict::Pass`] (logs the positive "gate passed" line,
///     proceed);
///   - `Found` → [`GateVerdict::Fail`] (writes the findings marker + alert);
///   - `Errored` → [`GateVerdict::FailedToRun`] (calls [`handle_gate_error`] for
///     the `gate_error` hold marker + the distinct "gate FAILED TO RUN" alert).
///
/// ALL existing side effects (markers, alerts, logs) are preserved; only the
/// return type changed from `Option<QueueStep>` to `GateVerdict` so the
/// structural fail-closed ledger — not per-arm branching — decides the hold.
pub(crate) async fn handle_contradiction_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    cc_ctx: &crate::preflight::change_contradiction::ContradictionCheckCtx,
) -> Result<crate::gate_ledger::GateVerdict> {
    use crate::gate_ledger::GateVerdict;
    use crate::preflight::change_contradiction::ContradictionCheckOutcome;
    let findings = match crate::preflight::change_contradiction::run_agentic_contradiction_check(
        cc_ctx, workspace, change,
    )
    .await
    {
        ContradictionCheckOutcome::Clean => {
            // Positive signal: a clean pass is logged, so an operator can verify
            // the gate RAN and PASSED rather than inferring it from silence.
            let label = crate::verifier_gate::VerifierGate::In.label();
            tracing::info!(
                url = %repo.url,
                change = %change,
                "{label} gate passed: no change-internal contradictions; proceeding to executor"
            );
            return Ok(GateVerdict::Pass);
        }
        ContradictionCheckOutcome::Errored { cause } => {
            // FailedToRun: the gate could not evaluate the change. The marker +
            // alert are written by `handle_gate_error`; the verdict holds.
            handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                crate::verifier_gate::VerifierGate::In,
                &cause,
                cc_ctx.attribution.as_deref(),
            )
            .await?;
            return Ok(GateVerdict::FailedToRun);
        }
        ContradictionCheckOutcome::Found(findings) => findings,
    };
    let suggestion = build_contradiction_revision_suggestion(&findings);
    // a61: this is the `[in]` verifier gate; its diagnostics carry the label.
    let label = crate::verifier_gate::VerifierGate::In.label();
    tracing::warn!(
        url = %repo.url,
        change = %change,
        findings = findings.len(),
        "{label} change-contradiction pre-flight FAILED; skipping executor and writing marker"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: Vec::new(),
        revision_suggestion: suggestion.clone(),
        gate_error: None,
        // The durable marker carries the structured findings (additive to the
        // prose) so a later `send it` — even one that cannot read the chat
        // thread — is grounded in the present contradiction set.
        contradictions: findings
            .iter()
            .map(crate::spec_revision::ContradictionFindingRecord::from)
            .collect(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "{label} failed to write spec-needs-revision marker (contradiction pre-flight): {e:#}"
        );
    }
    maybe_post_contradiction_findings_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        &findings,
        &suggestion,
        cc_ctx.attribution.as_deref(),
    )
    .await;
    Ok(GateVerdict::Fail)
}

/// Compose the auto-generated `revision_suggestion` text written into
/// the marker file when the contradiction pre-flight catches one or
/// more findings. Renders EVERY finding 1..N (no cap), each with the LLM's
/// `requirement_a`, `requirement_b`, the `summary` (WHY they conflict), AND —
/// when present — the `suggested_fix` on its own labeled line (WHAT to change),
/// then closes with a single short operator-action line so the per-finding
/// fixes are the prominent content. A finding with an empty `suggested_fix`
/// renders its identity + summary only.
pub(crate) fn build_contradiction_revision_suggestion(
    findings: &[crate::preflight::change_contradiction::ContradictionFinding],
) -> String {
    let n = findings.len();
    let mut out = format!(
        "Pre-flight contradiction check found {n} issue(s) where this change's\n\
         requirements appear to contradict each other:\n\n"
    );
    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!(
            "{idx}. Requirement A: {a}\n   Requirement B: {b}\n   {summary}\n",
            idx = i + 1,
            a = f.requirement_a,
            b = f.requirement_b,
            summary = f.summary,
        ));
        if !f.suggested_fix.trim().is_empty() {
            out.push_str(&format!("   Suggested fix: {fix}\n", fix = f.suggested_fix));
        }
        out.push('\n');
    }
    out.push_str(
        "Apply the suggested fix above (or edit the conflicting requirements so they can both \
         hold), push the spec change, AND clear this marker via @<bot> clear-revision <repo> <change>.\n",
    );
    out
}

/// Hold a change because a blocking verifier gate (`[in]` / `[canon]`) could
/// NOT run — the fail-CLOSED path (gatekeepers-fail-closed standard). Writes the
/// `.needs-spec-revision.json` marker with a structured `gate_error` (so the
/// held state is distinct from a findings-based revision), posts the distinct
/// "gate FAILED TO RUN" alert, AND returns `Ok(Some(QueueStep::SpecRevisionMarked))`
/// so the caller halts the queue walk — the change was NOT evaluated, so it is
/// NOT waved through.
pub(crate) async fn handle_gate_error(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    gate: crate::verifier_gate::VerifierGate,
    cause: &str,
    attribution: Option<&str>,
) -> Result<Option<QueueStep>> {
    let label = gate.label();
    tracing::warn!(
        url = %repo.url,
        change = %change,
        "{label} gate FAILED TO RUN; holding the change (fail-closed): {cause}"
    );
    let suggestion = format!(
        "The {label} verifier gate could NOT run, so this change is HELD — it was NOT \
         evaluated (this is NOT a finding that something is wrong with the change).\n\n\
         Cause: {cause}\n\n\
         Fix the gate (install/authenticate the configured CLI, check the daemon control \
         socket), then clear this marker via `@<bot> clear-revision <repo> <change>` to retry. \
         Clearing without fixing the gate will re-hold on the next attempt.\n"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: Vec::new(),
        revision_suggestion: suggestion,
        gate_error: Some(crate::spec_revision::GateErrorRecord {
            gate: label.to_string(),
            cause: cause.to_string(),
        }),
        contradictions: Vec::new(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "{label} failed to write gate-error hold marker: {e:#}"
        );
    }
    maybe_post_gate_error_alert(paths, chatops_ctx, repo, change, gate, cause, attribution).await;
    Ok(Some(QueueStep::SpecRevisionMarked))
}

/// Run the change-vs-canonical contradiction pre-flight — the `[canon]` gate
/// (a62) — against `change`, returning the gate's [`GateVerdict`]
/// (verifier-gates-fail-closed §5). Disposition is identical to the `[in]`
/// gate's; the gates differ only in what they read AND what each finding names:
///   - `Clean` → [`GateVerdict::Pass`] (logs the positive line, proceed);
///   - `Found` → [`GateVerdict::Fail`] (writes the findings marker + alert);
///   - `Errored` → [`GateVerdict::FailedToRun`] (calls [`handle_gate_error`]).
///
/// ALL existing side effects (markers, alerts, logs) are preserved; only the
/// return type changed from `Option<QueueStep>` to `GateVerdict`.
pub(crate) async fn handle_canon_contradiction_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    canon_ctx: &crate::preflight::canon_contradiction::CanonContradictionCheckCtx,
) -> Result<crate::gate_ledger::GateVerdict> {
    use crate::gate_ledger::GateVerdict;
    use crate::preflight::canon_contradiction::CanonContradictionCheckOutcome;
    let findings = match crate::preflight::canon_contradiction::run_agentic_canon_contradiction_check(
        canon_ctx, workspace, change,
    )
    .await
    {
        CanonContradictionCheckOutcome::Clean => {
            // Positive signal: a clean pass is logged, so an operator can verify
            // the gate RAN and PASSED rather than inferring it from silence.
            let label = crate::verifier_gate::VerifierGate::Canon.label();
            tracing::info!(
                url = %repo.url,
                change = %change,
                "{label} gate passed: no change-vs-canon contradictions; proceeding to executor"
            );
            return Ok(GateVerdict::Pass);
        }
        CanonContradictionCheckOutcome::Errored { cause } => {
            handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                crate::verifier_gate::VerifierGate::Canon,
                &cause,
                canon_ctx.attribution.as_deref(),
            )
            .await?;
            return Ok(GateVerdict::FailedToRun);
        }
        CanonContradictionCheckOutcome::Found(findings) => findings,
    };
    let suggestion = build_canon_contradiction_revision_suggestion(&findings);
    // a61: this is the `[canon]` verifier gate; its diagnostics carry the label.
    let label = crate::verifier_gate::VerifierGate::Canon.label();
    tracing::warn!(
        url = %repo.url,
        change = %change,
        findings = findings.len(),
        "{label} change-vs-canonical pre-flight FAILED; skipping executor and writing marker"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: Vec::new(),
        revision_suggestion: suggestion.clone(),
        gate_error: None,
        // The durable marker carries the structured findings (additive to the
        // prose) so a later `send it` — even one that cannot read the chat
        // thread — is grounded in the present contradiction set.
        contradictions: findings
            .iter()
            .map(crate::spec_revision::ContradictionFindingRecord::from)
            .collect(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "{label} failed to write spec-needs-revision marker (canon pre-flight): {e:#}"
        );
    }
    maybe_post_canon_contradiction_findings_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        &findings,
        &suggestion,
        canon_ctx.attribution.as_deref(),
    )
    .await;
    Ok(GateVerdict::Fail)
}

/// Compose the auto-generated `revision_suggestion` text written into the
/// marker file when the `[canon]` gate catches one or more findings. Renders
/// EVERY finding 1..N (no cap), each naming the change's requirement AND the
/// conflicting canonical requirement (by capability + title), the `summary`
/// (WHY they conflict), AND — when present — the `suggested_fix` on its own
/// labeled line (WHAT to change), then closes with a single short
/// operator-action line so the per-finding fixes are the prominent content. A
/// finding with an empty `suggested_fix` renders its identity + summary only.
pub(crate) fn build_canon_contradiction_revision_suggestion(
    findings: &[crate::preflight::canon_contradiction::CanonContradictionFinding],
) -> String {
    let n = findings.len();
    let mut out = format!(
        "Pre-flight change-vs-canonical check found {n} issue(s) where this change's\n\
         requirements appear to contradict the project's existing canonical specs:\n\n"
    );
    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!(
            "{idx}. Change requirement: {cr}\n   \
             Conflicting canonical requirement: {canon_req} (capability: {cap})\n   {summary}\n",
            idx = i + 1,
            cr = f.change_requirement,
            canon_req = f.canonical_requirement,
            cap = f.canonical_capability,
            summary = f.summary,
        ));
        if !f.suggested_fix.trim().is_empty() {
            out.push_str(&format!("   Suggested fix: {fix}\n", fix = f.suggested_fix));
        }
        out.push('\n');
    }
    out.push_str(
        "Apply the suggested fix above (or align the delta with canon / turn it into a coherent \
         MODIFIED delta), push the spec change, AND clear this marker via @<bot> clear-revision <repo> <change>.\n",
    );
    out
}

/// Run the change-vs-global-rules pre-flight — the `[rules]` gate
/// (global-rules-gate) — against `change`, returning the gate's [`GateVerdict`]
/// (verifier-gates-fail-closed §5). Disposition is identical to the `[canon]`
/// gate's; the gates differ only in the comparison corpus AND what each finding
/// names (a violated rule id vs. a canonical requirement):
///   - `Clean` → [`GateVerdict::Pass`] (logs the positive line, proceed);
///   - `Found` → [`GateVerdict::Fail`] (writes the findings marker + alert
///     naming each violated rule by id);
///   - `Errored` → [`GateVerdict::FailedToRun`] (calls [`handle_gate_error`]).
pub(crate) async fn handle_rules_violations_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    rules_ctx: &crate::preflight::global_rules::GlobalRulesCheckCtx,
) -> Result<crate::gate_ledger::GateVerdict> {
    use crate::gate_ledger::GateVerdict;
    use crate::preflight::global_rules::GlobalRulesCheckOutcome;
    let findings = match crate::preflight::global_rules::run_agentic_global_rules_check(
        rules_ctx, workspace, change,
    )
    .await
    {
        GlobalRulesCheckOutcome::Clean => {
            // Positive signal: a clean pass is logged, so an operator can verify
            // the gate RAN and PASSED rather than inferring it from silence.
            let label = crate::verifier_gate::VerifierGate::Rules.label();
            tracing::info!(
                url = %repo.url,
                change = %change,
                "{label} gate passed: no global-rule violations; proceeding to executor"
            );
            return Ok(GateVerdict::Pass);
        }
        GlobalRulesCheckOutcome::Errored { cause } => {
            handle_gate_error(
                paths,
                workspace,
                repo,
                chatops_ctx,
                change,
                crate::verifier_gate::VerifierGate::Rules,
                &cause,
                rules_ctx.attribution.as_deref(),
            )
            .await?;
            return Ok(GateVerdict::FailedToRun);
        }
        GlobalRulesCheckOutcome::Found(findings) => findings,
    };
    let suggestion = build_rules_violation_revision_suggestion(&findings);
    // global-rules-gate: this is the `[rules]` verifier gate; its diagnostics
    // carry the label.
    let label = crate::verifier_gate::VerifierGate::Rules.label();
    tracing::warn!(
        url = %repo.url,
        change = %change,
        findings = findings.len(),
        "{label} global-rules pre-flight FAILED; skipping executor and writing marker"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: Vec::new(),
        revision_suggestion: suggestion.clone(),
        gate_error: None,
        contradictions: Vec::new(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "{label} failed to write spec-needs-revision marker (global-rules pre-flight): {e:#}"
        );
    }
    maybe_post_rule_violations_findings_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        &findings,
        &suggestion,
        rules_ctx.attribution.as_deref(),
    )
    .await;
    Ok(GateVerdict::Fail)
}

/// Compose the auto-generated `revision_suggestion` text written into the marker
/// file when the `[rules]` gate catches one or more violations. Numbers each
/// violation 1..N, naming the violated rule by its stable id AND the one-line
/// summary, AND closes with operator-action guidance.
pub(crate) fn build_rules_violation_revision_suggestion(
    findings: &[crate::preflight::global_rules::RuleViolationFinding],
) -> String {
    let n = findings.len();
    let mut out = format!(
        "Pre-flight global-rules check found {n} issue(s) where this change's\n\
         requirements appear to violate a global rule:\n\n"
    );
    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!(
            "{idx}. Violated rule: {rule}\n   {summary}\n\n",
            idx = i + 1,
            rule = f.rule_id,
            summary = f.summary,
        ));
    }
    out.push_str(
        "Revise this change so its requirements honor the named global rule(s).\n\
         Push the spec change AND clear this marker via @<bot> clear-revision <repo> <change>.\n",
    );
    out
}

/// Pre-flight archive-collision check. For each entry in `candidates`,
/// call `queue::would_collide_on_archive`. Colliding entries are dropped
/// from the returned list, a WARN-level structured log fires (so
/// journalctl tailing surfaces the diagnosis even with chatops disabled),
/// and a chatops alert is posted under `AlertCategory::ArchiveCollision`
/// (subject to the existing 24h per-category throttle). The executor is
/// never invoked for an excluded change — the caller must use the
/// returned (non-colliding) list to drive its queue walk.
///
/// Centralizes the check so both the pending side (`walk_queue` call) and
/// the waiting side (`process_waiting_changes`) share one implementation.
pub(crate) async fn apply_archive_collision_preflight(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    candidates: Vec<String>,
) -> Vec<String> {
    let mut kept = Vec::with_capacity(candidates.len());
    for change in candidates {
        if !queue::would_collide_on_archive(workspace, &change) {
            kept.push(change);
            continue;
        }
        let archive_path = queue::archive_collision_path(workspace, &change);
        // WARN-level structured log: emits per iteration regardless of
        // whether the chatops alert is throttled, so operators tailing
        // journalctl see the diagnosis at least once per occurrence.
        tracing::warn!(
            url = %repo.url,
            change = %change,
            archive_path = %archive_path.display(),
            iteration_skipped = true,
            "archive collision detected for `{change}`: openspec/changes/{change}/ would archive to {} but that path already exists; excluding from this iteration",
            archive_path.display(),
        );
        // Body shape per proposal: concrete paths + the fix workflow so
        // the operator's chatops alert is actionable rather than
        // "something's wrong." `handle_predictable_failure` truncates the
        // excerpt at 200 chars when formatting; the long-form body is
        // also captured in the WARN log above so no diagnosis is lost.
        let err = anyhow!(
            "archive collision for `{change}`: openspec/changes/{change}/ would archive to {} but that path already exists. This usually means the change was archived earlier (via a merged PR) and re-added to the active path without removing the prior archive entry. The change is excluded from this iteration's queue walk to avoid burning agent tokens on a run that will fail at archive time. To resolve, on the base branch: (a) if the prior implementation is final: `git rm -r openspec/changes/{change}` and push; (b) if the prior implementation should be reverted and re-done: `git revert -m 1 <merge-sha>` (the merge that landed the prior PR), keeping the revised spec via `git checkout --ours` on the conflicting spec files, then push. Iteration continues with `{change}` excluded.",
            archive_path.display(),
        );
        handle_predictable_failure(
            paths,
            workspace,
            &repo.url,
            chatops_ctx,
            chatops_ctx
                .map(|c| c.failure_alerts_enabled)
                .unwrap_or(false),
            AlertCategory::ArchiveCollision,
            &err,
        )
        .await;
    }
    kept
}

/// Increment the per-change failure counter, and on threshold transition
/// write the perma-stuck marker + post the chatops alert. Best-effort: any
/// I/O or transport failure here is logged at WARN and does not propagate.
pub(crate) async fn handle_failure_counter(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    change: &str,
    reason: &str,
    threshold: u32,
) {
    let count = match failure_state::record_failure(paths, workspace, change, reason) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "failed to record consecutive-failure state: {e:#}"
            );
            return;
        }
    };
    if count < threshold {
        return;
    }
    let entry = failure_state::FailureEntry {
        count,
        last_reason: reason.to_string(),
        last_failed_at: Utc::now(),
    };
    if let Err(e) = perma_stuck::write_marker(workspace, change, &entry) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to write perma-stuck marker: {e:#}"
        );
        // Continue to alert — the operator should still know.
    }
    let marker_path = workspace
        .join("openspec/changes")
        .join(change)
        .join(".perma-stuck.json");
    tracing::error!(
        url = %repo.url,
        change = %change,
        marker = %marker_path.display(),
        consecutive_failures = count,
        "change marked perma-stuck after {count} consecutive failures; daemon will not retry until {} is removed",
        marker_path.display()
    );
    post_perma_stuck_alert(paths, chatops_ctx, repo, change, reason, count).await;
}
