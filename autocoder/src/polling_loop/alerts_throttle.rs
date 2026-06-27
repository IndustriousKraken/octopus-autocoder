use super::*;

const PERMA_STUCK_ALERT_THROTTLE_HOURS: i64 = 24;
pub(crate) const PERMA_STUCK_REASON_EXCERPT_MAX: usize = 200;

/// Best-effort chatops alert for stuck busy-marker states. Posts a
/// notification via `post_notification` if a chatops backend is
/// configured; otherwise the ERROR log line is the operator's only
/// signal. Returns immediately on any post failure (logged at WARN).
pub(crate) async fn post_stuck_alert(
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    marker: &busy_marker::BusyMarker,
    ambiguous: bool,
) {
    let ctx = match chatops_ctx {
        Some(c) => c,
        None => return,
    };
    let kind = if ambiguous {
        "stuck (ambiguous — investigate)"
    } else {
        "recovered from stuck state"
    };
    let text = format!(
        ":rotating_light: autocoder {kind}\nrepo: `{}`\npid: {} (recorded comm: `{}`)\nstage: `{}`\nstarted: {}",
        repo.url,
        marker.pid,
        marker.comm,
        marker.stage.as_str(),
        marker.started_at,
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            "busy_marker: failed to post stuck-state chatops alert: {e:#}"
        );
    }
}

/// Post an advisory chatops heads-up when the `[out]` gate's verdict reports
/// gaps (a63). Best-effort AND gated on `failure_alerts_enabled`: a post
/// failure is logged at WARN but never propagated, AND the gate never blocks
/// PR creation. No revision is opened — the operator decides what to do.
pub(crate) async fn post_spec_verification_gaps_alert(
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    verification: &crate::code_implements_spec::SpecVerification,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let text = format!(
        "⚠ `{repo_url}`: code-implements-spec verification found {n} gap(s) — see the PR's `## Spec Verification` section (advisory: no revision opened, PR not blocked)",
        repo_url = repo.url,
        n = verification.gaps.len(),
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            "spec-verification gaps chatops notification post failed: {e:#}"
        );
    }
}

/// Which per-change throttle map in the alert-state file a throttled alert
/// reads/writes. (a68 §3.2: the five throttle-family helpers share one
/// 24h-per-change throttle skeleton, [`post_throttled_change_alert`], and
/// differ only in this map, the body text, and the stored excerpt.)
#[derive(Clone, Copy)]
enum ThrottleMap {
    SpecRevision,
    PermaStuck,
}

impl ThrottleMap {
    /// True when no prior alert exists for `change`, or the last one is older
    /// than the 24h window. Byte-identical to the per-helper checks it
    /// replaces.
    fn should_alert(&self, state: &AlertState, change: &str, now: chrono::DateTime<Utc>) -> bool {
        let last = match self {
            ThrottleMap::SpecRevision => state.spec_revision_alerts.get(change),
            ThrottleMap::PermaStuck => state.perma_stuck_alerts.get(change),
        };
        last.map(|entry| {
            now - entry.last_alerted_at >= ChronoDuration::hours(PERMA_STUCK_ALERT_THROTTLE_HOURS)
        })
        .unwrap_or(true)
    }

    fn insert(&self, state: &mut AlertState, change: &str, entry: AlertEntry) {
        match self {
            ThrottleMap::SpecRevision => {
                state.spec_revision_alerts.insert(change.to_string(), entry);
            }
            ThrottleMap::PermaStuck => {
                state.perma_stuck_alerts.insert(change.to_string(), entry);
            }
        }
    }
}

/// The shared 24h-per-change throttle skeleton: gate on chatops presence +
/// `failure_alerts_enabled`, load alert-state, short-circuit if the change
/// was alerted within the window, build the body (only when not throttled),
/// post it, then (only on success) record + persist the throttle entry. On
/// post failure the throttle entry is NOT written so a later iteration can
/// retry. `label` is a stable diagnostic used only in the WARN log lines.
#[allow(clippy::too_many_arguments)]
async fn post_throttled_change_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    map: ThrottleMap,
    excerpt: String,
    label: &str,
    build_text: impl FnOnce(&Path) -> String,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let workspace = workspace::resolve_path(paths, repo);
    let mut state = AlertState::load_or_default(paths, &workspace);
    let now = Utc::now();
    if !map.should_alert(&state, change, now) {
        return;
    }
    let text = build_text(&workspace);
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "{label} chatops alert post failed: {e:#}"
        );
        return;
    }
    map.insert(
        &mut state,
        change,
        AlertEntry {
            last_alerted_at: now,
            last_error_excerpt: excerpt,
        },
    );
    if let Err(e) = state.save(paths, &workspace) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to persist {label} alert state: {e:#}"
        );
    }
}

/// a03: the tracked-thread variant of [`post_throttled_change_alert`] for a
/// CONTRADICTION marker (`[in]` / `[canon]` findings). Same 24h-per-change
/// throttle + `failure_alerts_enabled` gate, but it posts via the
/// thread-returning notification path (`post_notification_with_thread`),
/// captures the returned `thread_ts`, AND stamps a
/// [`crate::revision_thread::RevisionThreadState`] keyed to repo + change so a
/// later reply can be matched to the change (the dispatcher's fourth `send it`
/// context + the advisor routing).
///
/// A degraded post that returns `Ok(None)` (a backend with no native threading)
/// still records the throttle entry — the alert landed — but writes NO
/// `RevisionThreadState`: the alert is simply not reply-matchable (graceful
/// degradation, never an error). On post failure the throttle entry is NOT
/// written so a later iteration can retry.
///
/// Only the contradiction posters call this; the gate-error AND
/// unarchivable-deltas markers keep the untracked [`post_throttled_change_alert`]
/// path (a03 D1: those markers are NOT tracked as revision threads).
#[allow(clippy::too_many_arguments)]
async fn post_tracked_revision_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    excerpt: String,
    label: &str,
    top_line: impl FnOnce(&Path) -> String,
    // `threaded` = this backend returns an addressable `thread_ts`, so the post
    // is reply-matchable AND the body may advertise the interactive thread
    // (`@<bot> send it`). A non-threading backend gets `false` so the advert —
    // which would be orphaned in a single-message degraded post — is omitted in
    // place (send-it-explains-manual-fix-markers: a degraded contradiction
    // post's body "does not advertise `@<bot> send it` as an actionable path").
    thread_body: impl FnOnce(&Path, bool) -> String,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let workspace = workspace::resolve_path(paths, repo);
    let mut state = AlertState::load_or_default(paths, &workspace);
    let now = Utc::now();
    if !ThrottleMap::SpecRevision.should_alert(&state, change, now) {
        return;
    }
    let top = top_line(&workspace);
    let body = thread_body(&workspace, ctx.chatops.supports_threading());
    let posted = ctx
        .chatops
        .post_notification_with_thread(&ctx.channel, &top, &body)
        .await;
    let maybe_thread_ts = match posted {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "{label} chatops alert post failed: {e:#}"
            );
            return;
        }
    };
    // The alert landed — record the throttle entry so we don't re-alert within
    // the window, regardless of whether the backend gave us a trackable thread.
    ThrottleMap::SpecRevision.insert(
        &mut state,
        change,
        AlertEntry {
            last_alerted_at: now,
            last_error_excerpt: excerpt,
        },
    );
    if let Err(e) = state.save(paths, &workspace) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to persist {label} alert state: {e:#}"
        );
    }
    // a03 task 1.1: only a thread-returning post yields a trackable thread.
    let Some(thread_ts) = maybe_thread_ts else {
        // Degraded post (no native threading): the alert is not reply-matchable.
        tracing::debug!(
            url = %repo.url,
            change = %change,
            "{label} contradiction alert posted without a thread_ts; not recording a RevisionThreadState"
        );
        return;
    };
    let revision_state = crate::revision_thread::RevisionThreadState {
        thread_ts: thread_ts.clone(),
        channel: ctx.channel.clone(),
        repo_url: repo.url.clone(),
        change_slug: change.to_string(),
        status: crate::revision_thread::RevisionThreadStatus::Open,
        posted_at: now,
    };
    let root = crate::revision_thread::default_state_root(paths);
    if let Err(e) = crate::revision_thread::write_state(&root, &revision_state) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            thread_ts = %thread_ts,
            "{label} failed to stamp revision-thread state (`send it` / advisor will not resolve this thread): {e:#}"
        );
    }
}

/// The trailing block appended to every CONTRADICTION alert's thread body (a03
/// task 1.3): the alert IS a revision thread, so it advertises that the
/// operator may reply to discuss OR `@<bot> send it` to revise + open a PR.
fn revision_thread_advert() -> &'static str {
    "This alert is an interactive revision thread:\n  • Reply in this thread to discuss the revision with autocoder (read-only — nothing is written).\n  • Post `@<bot> send it` in this thread to have autocoder revise the change's spec deltas, re-run the gates, AND open a PR for review."
}

/// The advert block spliced into a contradiction alert body, with its trailing
/// blank-line separator. Empty when the post is NOT reply-matchable (the backend
/// returns no `thread_ts`): a degraded post's body must NOT advertise `@<bot>
/// send it`, since there is no thread for the operator to act in
/// (send-it-explains-manual-fix-markers).
fn revision_advert_block(threaded: bool) -> String {
    if threaded {
        format!("{}\n\n", revision_thread_advert())
    } else {
        String::new()
    }
}

/// Sibling of `maybe_post_unarchivable_deltas_alert` for the a19
/// contradiction pre-flight path. Same throttle state, channel, and
/// gating flag as the existing alert so a single stream of
/// `AlertCategory::SpecNeedsRevision` notifications covers both code
/// paths. Body framing names "contradictions" instead of unarchivable
/// deltas.
pub(crate) async fn maybe_post_contradiction_findings_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    findings: &[crate::preflight::change_contradiction::ContradictionFinding],
    revision_suggestion: &str,
    attribution: Option<&str>,
) {
    let label = crate::verifier_gate::VerifierGate::In.label();
    // a03: the contradiction marker (empty unimplementable_tasks, empty
    // gate_error, AND empty unarchivable_deltas) is a TRACKED revision thread —
    // post via the thread-returning path AND stamp a RevisionThreadState so a
    // later reply can be matched. A non-empty unarchivable_deltas array is a
    // MANUAL-FIX hold, not a contradiction (`maybe_post_unarchivable_deltas_alert`
    // keeps the untracked path), so `send it`'s executor cannot revise it.
    let findings = findings.to_vec();
    let attribution = attribution.map(|a| a.to_string());
    post_tracked_revision_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        truncate_reason(revision_suggestion),
        &format!("{label} contradiction-findings"),
        |_workspace| {
            crate::verifier_gate::VerifierGate::In.label_line(&format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` has change-internal contradictions (pre-flight)",
                repo_url = repo.url,
            ))
        },
        |workspace, threaded| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let mut findings_block = String::new();
            for (i, f) in findings.iter().enumerate() {
                findings_block.push_str(&format!(
                    "  {n}. A: \"{a}\" vs B: \"{b}\" — {s}\n",
                    n = i + 1,
                    a = f.requirement_a,
                    b = f.requirement_b,
                    s = f.summary,
                ));
                // Surface the concrete edit plan on its own labeled line,
                // distinct from the why-summary. Empty → identity + summary
                // only (the suggested fix is additive).
                if !f.suggested_fix.trim().is_empty() {
                    findings_block
                        .push_str(&format!("     Suggested fix: {fix}\n", fix = f.suggested_fix));
                }
            }
            // a49: append the `*Contradiction-check: <provider>/<model>*`
            // attribution when the daemon knows the configured model.
            let attribution_suffix = attribution
                .map(|a| {
                    format!(
                        "\n\n{}",
                        crate::attribution::attribution_line("Contradiction-check", &a)
                    )
                })
                .unwrap_or_default();
            let advert_block = revision_advert_block(threaded);
            format!(
                "Requirements within this change cannot all hold simultaneously:\n{findings_block}\n{advert_block}Manual escape (still supported): edit openspec/changes/{change}/specs/<capability>/spec.md so the conflicting requirements can both hold (or remove one), commit + push to {base}, then `@<bot> clear-revision <repo> <change>` (or delete the marker file).\n\nmarker: {marker}{attribution_suffix}",
                findings_block = findings_block,
                advert_block = advert_block,
                change = change,
                base = repo.base_branch,
                marker = marker_path.display(),
            )
        },
    )
    .await;
}

/// Alert for a verifier gate that could NOT run (a fail-closed hold). Distinct
/// from a findings alert: the change is held because the gate was NOT evaluated,
/// NOT because a problem was found. Shares the `SpecNeedsRevision` throttle +
/// channel so all held-change notifications stream together, but the body makes
/// the failed-to-run state explicit so an operator can tell it apart from a real
/// finding. Works for either blocking gate (`[in]` / `[canon]`).
pub(crate) async fn maybe_post_gate_error_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    gate: crate::verifier_gate::VerifierGate,
    cause: &str,
    attribution: Option<&str>,
) {
    let label = gate.label();
    post_throttled_change_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        ThrottleMap::SpecRevision,
        truncate_reason(cause),
        &format!("{label} gate-failed-to-run"),
        |workspace| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let attribution_suffix = attribution
                .map(|a| {
                    format!(
                        "\n\n{}",
                        crate::attribution::attribution_line("Verifier-gate", a)
                    )
                })
                .unwrap_or_default();
            gate.label_line(&format!(
                "⚠️ `{repo_url}`: {label} gate FAILED TO RUN on `{change}` — change HELD (NOT evaluated; this is NOT a finding)\n\nThe gate could not run, so the change is held rather than waved through (gatekeepers fail closed).\nCause: {cause}\n\nThis is a MANUAL fix — `@<bot> send it` cannot revise it (it cannot fix a broken gate). Fix the gate, then clear the hold:\nOperator action:\n  1. Fix the gate — e.g. install/authenticate the configured CLI, or check the daemon control socket.\n  2. `@<bot> clear-revision <repo> <change>` to retry (clearing without fixing the gate will re-hold).\n\nmarker: {marker}{attribution_suffix}",
                repo_url = repo.url,
                label = label,
                change = change,
                cause = cause,
                marker = marker_path.display(),
            ))
        },
    )
    .await;
}

/// Sibling of `maybe_post_contradiction_findings_alert` for the a62 `[canon]`
/// gate. Same throttle state, channel, and gating flag as the existing alert
/// so a single stream of `AlertCategory::SpecNeedsRevision` notifications
/// covers all pre-flight paths. Body framing names "change-vs-canonical
/// contradictions" AND each finding names the conflicting canonical
/// requirement.
pub(crate) async fn maybe_post_canon_contradiction_findings_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    findings: &[crate::preflight::canon_contradiction::CanonContradictionFinding],
    revision_suggestion: &str,
    attribution: Option<&str>,
) {
    let label = crate::verifier_gate::VerifierGate::Canon.label();
    // a03: like the `[in]` findings alert, the `[canon]` contradiction marker is
    // a TRACKED revision thread — post threaded AND stamp a RevisionThreadState.
    let findings = findings.to_vec();
    let attribution = attribution.map(|a| a.to_string());
    post_tracked_revision_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        truncate_reason(revision_suggestion),
        &format!("{label} canon-contradiction-findings"),
        |_workspace| {
            crate::verifier_gate::VerifierGate::Canon.label_line(&format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` contradicts the existing canonical specs (pre-flight)",
                repo_url = repo.url,
            ))
        },
        |workspace, threaded| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let mut findings_block = String::new();
            for (i, f) in findings.iter().enumerate() {
                findings_block.push_str(&format!(
                    "  {n}. change: \"{cr}\" vs canonical \"{canon_req}\" ({cap}) — {s}\n",
                    n = i + 1,
                    cr = f.change_requirement,
                    canon_req = f.canonical_requirement,
                    cap = f.canonical_capability,
                    s = f.summary,
                ));
                // Surface the concrete edit plan on its own labeled line,
                // distinct from the why-summary. Empty → identity + summary
                // only (the suggested fix is additive).
                if !f.suggested_fix.trim().is_empty() {
                    findings_block
                        .push_str(&format!("     Suggested fix: {fix}\n", fix = f.suggested_fix));
                }
            }
            // a49: append the `*Canon-contradiction-check: <provider>/<model>*`
            // attribution when the daemon knows the configured model.
            let attribution_suffix = attribution
                .map(|a| {
                    format!(
                        "\n\n{}",
                        crate::attribution::attribution_line("Canon-contradiction-check", &a)
                    )
                })
                .unwrap_or_default();
            let advert_block = revision_advert_block(threaded);
            format!(
                "This change's requirements conflict with canon:\n{findings_block}\n{advert_block}Manual escape (still supported): edit openspec/changes/{change}/specs/<capability>/spec.md so the change is consistent with canon (or turn it into a coherent MODIFIED delta of the canonical requirement), commit + push to {base}, then `@<bot> clear-revision <repo> <change>` (or delete the marker file).\n\nmarker: {marker}{attribution_suffix}",
                findings_block = findings_block,
                advert_block = advert_block,
                change = change,
                base = repo.base_branch,
                marker = marker_path.display(),
            )
        },
    )
    .await;
}

/// Sibling of [`maybe_post_canon_contradiction_findings_alert`] for the
/// global-rules-gate `[rules]` gate. Same throttle state, channel, and gating
/// flag as the existing pre-flight alerts so a single stream of
/// `AlertCategory::SpecNeedsRevision` notifications covers all pre-flight paths.
/// Body framing names "global-rule violations" AND each finding names the
/// violated rule by its stable id.
pub(crate) async fn maybe_post_rule_violations_findings_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    findings: &[crate::preflight::global_rules::RuleViolationFinding],
    revision_suggestion: &str,
    attribution: Option<&str>,
) {
    let label = crate::verifier_gate::VerifierGate::Rules.label();
    // a03: like the other findings alerts, the `[rules]` marker is a TRACKED
    // revision thread — post threaded AND stamp a RevisionThreadState.
    let findings = findings.to_vec();
    let attribution = attribution.map(|a| a.to_string());
    post_tracked_revision_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        truncate_reason(revision_suggestion),
        &format!("{label} global-rule-violations"),
        |_workspace| {
            crate::verifier_gate::VerifierGate::Rules.label_line(&format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` violates a global rule (pre-flight)",
                repo_url = repo.url,
            ))
        },
        |workspace, threaded| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let mut findings_block = String::new();
            for (i, f) in findings.iter().enumerate() {
                findings_block.push_str(&format!(
                    "  {n}. rule \"{rule}\" — {s}\n",
                    n = i + 1,
                    rule = f.rule_id,
                    s = f.summary,
                ));
            }
            let attribution_suffix = attribution
                .map(|a| {
                    format!(
                        "\n\n{}",
                        crate::attribution::attribution_line("Global-rules-check", &a)
                    )
                })
                .unwrap_or_default();
            let advert_block = revision_advert_block(threaded);
            format!(
                "This change violates one or more global rules:\n{findings_block}\n{advert_block}Manual escape (still supported): edit openspec/changes/{change}/specs/<capability>/spec.md so the change honors the named rule(s), commit + push to {base}, then `@<bot> clear-revision <repo> <change>` (or delete the marker file).\n\nmarker: {marker}{attribution_suffix}",
                findings_block = findings_block,
                advert_block = advert_block,
                change = change,
                base = repo.base_branch,
                marker = marker_path.display(),
            )
        },
    )
    .await;
}

/// Sibling of [`maybe_post_spec_revision_alert`] for the a17 pre-flight
/// path. Body framing names "unarchivable spec deltas" rather than the
/// agent-detected "unimplementable tasks", and lists each violation
/// (`capability`, `kind`, `header`, `reason`). Throttle state, channel,
/// and gating flag are identical to the existing alert so a single
/// stream of `AlertCategory::SpecNeedsRevision` notifications covers
/// both code paths.
pub(crate) async fn maybe_post_unarchivable_deltas_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    violations: &[crate::preflight::spec_archivability::UnarchivableDelta],
    revision_suggestion: &str,
) {
    post_throttled_change_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        ThrottleMap::SpecRevision,
        truncate_reason(revision_suggestion),
        "unarchivable-deltas",
        |workspace| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let mut violations_block = String::new();
            for v in violations {
                violations_block.push_str(&format!(
                    "  - {cap} / {kind}: \"{hdr}\" — {reason}\n",
                    cap = v.capability,
                    kind = v.kind.as_str(),
                    hdr = v.header,
                    reason = v.reason,
                ));
            }
            format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` has unarchivable spec deltas (pre-flight)\n\nDeltas whose preconditions don't match canonical specs (would abort `openspec archive` later):\n{violations_block}\nThis is a MANUAL spec fix — `@<bot> send it` cannot revise it (it cannot reconcile a delta header with canonical). Fix the delta header(s), then clear the hold:\nOperator action:\n  1. Edit openspec/changes/{change}/specs/<capability>/spec.md so each delta block's header matches canonical.\n  2. Commit + push to {base}.\n  3. `@<bot> clear-revision <repo> <change>` from chat (or delete the marker file).\n\nmarker: {marker}",
                repo_url = repo.url,
                change = change,
                violations_block = violations_block,
                base = repo.base_branch,
                marker = marker_path.display(),
            )
        },
    )
    .await;
}

/// Sibling of [`maybe_post_unarchivable_deltas_alert`] for the canon-editing-
/// tasks pre-flight path. Body framing names "tasks directing a canon edit" and
/// lists each offending task line. Throttle state, channel, and gating flag are
/// identical to the existing pre-flight alerts so a single stream of
/// `AlertCategory::SpecNeedsRevision` notifications covers all reject paths.
/// Untracked (like the unarchivable-deltas alert): a mechanical reject, not a
/// contradiction revision thread.
pub(crate) async fn maybe_post_canon_editing_tasks_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    offending: &[String],
    revision_suggestion: &str,
) {
    post_throttled_change_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        ThrottleMap::SpecRevision,
        truncate_reason(revision_suggestion),
        "canon-editing-tasks",
        |workspace| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let mut tasks_block = String::new();
            for task in offending {
                tasks_block.push_str(&format!("  - {task}\n"));
            }
            format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` has a task directing a canon edit (pre-flight)\n\nThe implementer implements code and tests only; a change's spec delta is folded into openspec/specs/ by `openspec archive` automatically. These task(s) instead apply it to canon, which would abort the archive on a duplicate requirement:\n{tasks_block}\nOperator action:\n  1. Remove the offending task(s) from openspec/changes/{change}/tasks.md.\n  2. Commit + push to {base}.\n  3. `@<bot> clear-revision <repo> <change>` from chat (or delete the marker file).\n\nmarker: {marker}",
                repo_url = repo.url,
                change = change,
                tasks_block = tasks_block,
                base = repo.base_branch,
                marker = marker_path.display(),
            )
        },
    )
    .await;
}

/// Post the chatops perma-stuck alert (best-effort, 24h-throttled per
/// change). The state for this throttle lives in the daemon's
/// alert-state file (`<state_dir>/alert-state/<basename>.json`) under
/// its `perma_stuck_alerts` map.
pub(crate) async fn post_perma_stuck_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    reason: &str,
    count: u32,
) {
    let excerpt = truncate_reason(reason);
    post_throttled_change_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        ThrottleMap::PermaStuck,
        excerpt.clone(),
        "perma-stuck",
        |workspace| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".perma-stuck.json");
            // Tied to the Claude CLI executor's log convention; refactor to an
            // Executor trait method if a second executor backend with a
            // different log layout is added.
            let log_path = crate::executor::claude_cli::run_log_path(paths, workspace, change);
            format!(
                ":no_entry: autocoder: change perma-stuck\nrepo: {}\nchange: {}\nconsecutive_failures: {count}\nlast_reason: {excerpt}\nrun_log: {}\n\nThis change has failed {count} iterations in a row. autocoder will not retry until an operator removes {}.",
                repo.url,
                change,
                log_path.display(),
                marker_path.display(),
            )
        },
    )
    .await;
}

/// Post the chatops spec-needs-revision alert (best-effort, 24h-throttled
/// per change). State for this throttle lives in the daemon's
/// alert-state file (`<state_dir>/alert-state/<basename>.json`) under
/// its `spec_revision_alerts` map. Mirrors `post_perma_stuck_alert` —
/// both announce operator-action states with the same throttle window.
pub(crate) async fn maybe_post_spec_revision_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    change: &str,
    flagged_tasks: &[UnimplementableTask],
    revision_suggestion: &str,
) {
    post_throttled_change_alert(
        paths,
        chatops_ctx,
        repo,
        change,
        ThrottleMap::SpecRevision,
        truncate_reason(revision_suggestion),
        "spec-needs-revision",
        |workspace| {
            let marker_path = workspace
                .join("openspec/changes")
                .join(change)
                .join(".needs-spec-revision.json");
            let log_path = crate::executor::claude_cli::run_log_path(paths, workspace, change);
            let mut tasks_block = String::new();
            for task in flagged_tasks {
                tasks_block.push_str(&format!(
                    "  - {}: {} ({})\n",
                    task.task_id, task.task_text, task.reason
                ));
            }
            format!(
                "⚠️ `{repo_url}`: spec needs revision — `{change}` has unimplementable tasks\n\nTasks the agent flagged as outside its sandbox:\n{tasks_block}\nSuggested revision:\n  {suggestion}\n\nOperator action:\n  1. Edit openspec/changes/{change}/tasks.md to remove or revise the flagged tasks.\n  2. Commit + push to {base}.\n  3. Delete openspec/changes/{change}/.needs-spec-revision.json — the next iteration will retry the change.\n\nmarker: {marker}\nlog:    {log}",
                repo_url = repo.url,
                change = change,
                tasks_block = tasks_block,
                suggestion = revision_suggestion,
                base = repo.base_branch,
                marker = marker_path.display(),
                log = log_path.display(),
            )
        },
    )
    .await;
}

pub(crate) fn truncate_reason(reason: &str) -> String {
    let count = reason.chars().count();
    if count <= PERMA_STUCK_REASON_EXCERPT_MAX {
        reason.to_string()
    } else {
        let mut out: String = reason
            .chars()
            .take(PERMA_STUCK_REASON_EXCERPT_MAX)
            .collect();
        out.push('…');
        out
    }
}
