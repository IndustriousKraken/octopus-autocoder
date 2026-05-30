//! Brownfield-batch polling handler (a29). Two roles:
//!
//! 1. [`process_pending_brownfield_batch`] — runs ONCE per
//!    `send it`-in-survey-thread action. Loads the referenced
//!    survey, validates the concurrent-batch rule (one InProgress
//!    per workspace), flips status to `InProgress`, AND posts the
//!    queue-confirmation reply.
//!
//! 2. [`drain_next_brownfield_batch_item`] — runs EVERY iteration. If
//!    a survey is `InProgress`, finds the first `Pending` item AND
//!    runs the single-capability brownfield-generation flow (per
//!    `a23`) against it. On terminal item status, posts the per-item
//!    status reply. When ALL items are terminal, flips the survey to
//!    `Completed` AND posts the batch-complete summary.
//!
//! The one-item-per-iteration discipline gives each brownfield run its
//! own fresh executor invocation, eliminating mid-batch context
//! compression as a failure mode.

use crate::config::{GithubConfig, RepositoryConfig};
use crate::executor::Executor;
use crate::polling_loop::ChatOpsContext;
use crate::spec_root::SpecRoot;
use crate::state::brownfield_request::{
    BrownfieldRequestState, BrownfieldRequestStatus,
};
use crate::state::brownfield_survey::{
    self, BrownfieldSurveyState, ItemStatus, SurveyStatus,
};
use anyhow::Result;
use std::path::Path;

/// Process the one drained brownfield-batch action. Flips the
/// referenced survey to `InProgress` AND posts the queue
/// confirmation. Subsequent iterations drive the actual item drain.
pub async fn process_pending_brownfield_batch(
    workspace: &Path,
    _repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &crate::control_socket::BrownfieldBatchRequest,
) -> Result<()> {
    let mut state = match brownfield_survey::read_state(workspace, &request.survey_request_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: survey state {} not found (was it cleared?). Re-run brownfield-survey for a fresh list.",
                    request.survey_request_id
                ),
            )
            .await;
            return Ok(());
        }
        Err(e) => {
            tracing::warn!(
                survey_request_id = %request.survey_request_id,
                "brownfield-batch: state read failed: {e:#}"
            );
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: could not read survey state {}: {e}",
                    request.survey_request_id
                ),
            )
            .await;
            return Ok(());
        }
    };

    match state.status {
        SurveyStatus::InProgress => {
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: a brownfield batch is already in progress for survey {}.",
                    state.request_id
                ),
            )
            .await;
            return Ok(());
        }
        SurveyStatus::Completed => {
            post_reply(
                chatops_ctx,
                &request.channel,
                &request.thread_ts,
                &format!(
                    "✗ send it: the brownfield batch for survey {} has already completed.",
                    state.request_id
                ),
            )
            .await;
            return Ok(());
        }
        SurveyStatus::Pending => {}
    }

    // Concurrent-batch handling: only ONE survey can be InProgress
    // per workspace.
    if let Some(other) = find_in_progress_other_than(workspace, &state.request_id) {
        post_reply(
            chatops_ctx,
            &request.channel,
            &request.thread_ts,
            &format!(
                "✗ send it: a brownfield batch is already in progress for this workspace (survey {prior}). Wait for it to finish OR run @<bot> clear-survey <repo> to abort.",
                prior = other
            ),
        )
        .await;
        return Ok(());
    }

    state.status = SurveyStatus::InProgress;
    if let Err(e) = brownfield_survey::write_state(workspace, &state) {
        tracing::warn!(
            survey_request_id = %state.request_id,
            "brownfield-batch: write_state InProgress failed: {e:#}"
        );
        return Ok(());
    }
    post_reply(
        chatops_ctx,
        &state.channel,
        &state.thread_ts,
        &format!(
            "✓ Queued {n} capability spec generations. The first will start on the next iteration.",
            n = state.items.len()
        ),
    )
    .await;
    Ok(())
}

/// Drain one item from the workspace's in-progress survey, if any.
/// Runs every polling iteration. No-op when no survey is in progress
/// OR when every item has reached a terminal state.
pub async fn drain_next_brownfield_batch_item(
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
) -> Result<()> {
    let mut state = match find_in_progress_survey(workspace) {
        Some(s) => s,
        None => return Ok(()),
    };
    let total = state.items.len();
    let next_pending_idx = state.items.iter().position(|i| i.status == ItemStatus::Pending);
    let idx = match next_pending_idx {
        Some(i) => i,
        None => {
            finalize_survey_if_all_terminal(workspace, &mut state, chatops_ctx).await;
            return Ok(());
        }
    };

    let item_slug = state.items[idx].slug.clone();
    let scope_in = state.items[idx].scope_in.clone();
    let scope_out = state.items[idx].scope_out.clone();
    let source_modules = state.items[idx].source_modules.clone();

    // Pre-check: spec already exists?
    let spec_root = SpecRoot::for_repo(repo, workspace);
    let spec_path = spec_root.canonical_specs_dir().join(&item_slug).join("spec.md");
    if spec_path.is_file() {
        state.items[idx].status = ItemStatus::Skipped;
        state.items[idx].failure_reason =
            Some(format!("openspec/specs/{item_slug}/spec.md already exists"));
        let _ = brownfield_survey::write_state(workspace, &state);
        let m_done = count_terminal(&state);
        post_reply(
            chatops_ctx,
            &state.channel,
            &state.thread_ts,
            &format!(
                "⏭ Skipped `{item_slug}` ({m_done}/{total} done): spec already exists."
            ),
        )
        .await;
        finalize_survey_if_all_terminal(workspace, &mut state, chatops_ctx).await;
        return Ok(());
    }

    // Mark Generating, persist.
    state.items[idx].status = ItemStatus::Generating;
    let _ = brownfield_survey::write_state(workspace, &state);

    // Build a per-item BrownfieldRequest. Guidance appends the survey
    // item's scope context so the LLM scopes its draft accordingly.
    let guidance = build_per_item_guidance(
        state.guidance.as_deref(),
        &scope_in,
        &scope_out,
        &source_modules,
    );
    let item_request_id = format!(
        "survey-{}-item-{}-{}",
        state.request_id, state.items[idx].id, item_slug
    );

    // Write the on-disk BrownfieldRequestState the existing handler
    // reads from disk.
    let bf_state = BrownfieldRequestState {
        request_id: item_request_id.clone(),
        repo_url: state.repo_url.clone(),
        capability_name: item_slug.clone(),
        guidance: Some(guidance.clone()),
        channel: state.channel.clone(),
        thread_ts: state.thread_ts.clone(),
        submitted_at: chrono::Utc::now(),
        status: BrownfieldRequestStatus::Pending,
        reason: None,
        pr_url: None,
    };
    if let Err(e) = crate::state::brownfield_request::write_state(workspace, &bf_state) {
        tracing::warn!(
            survey_request_id = %state.request_id,
            item_slug = %item_slug,
            "brownfield-batch: writing per-item BrownfieldRequestState failed: {e:#}"
        );
        mark_item_failed(
            workspace,
            &mut state,
            idx,
            format!("could not persist per-item state: {e}"),
            chatops_ctx,
            total,
        )
        .await;
        finalize_survey_if_all_terminal(workspace, &mut state, chatops_ctx).await;
        return Ok(());
    }

    let bf_request = crate::control_socket::BrownfieldRequest {
        request_id: item_request_id.clone(),
        repo_url: state.repo_url.clone(),
        capability_name: item_slug.clone(),
        guidance: Some(guidance.clone()),
        channel: state.channel.clone(),
        thread_ts: state.thread_ts.clone(),
        submitted_at: chrono::Utc::now(),
    };

    // Invoke the existing brownfield-draft flow WITHOUT chatops_ctx so
    // its "✅ Brownfield draft PR opened" reply doesn't double up with
    // our survey-specific status message below.
    let exec_result = crate::polling::brownfield::process_pending_brownfield(
        workspace,
        repo,
        executor,
        github_cfg,
        None,
        &bf_request,
    )
    .await;
    if let Err(e) = exec_result {
        tracing::warn!(
            survey_request_id = %state.request_id,
            item_slug = %item_slug,
            "brownfield-batch: per-item brownfield handler errored: {e:#}"
        );
        mark_item_failed(
            workspace,
            &mut state,
            idx,
            format!("executor pipeline errored: {e}"),
            chatops_ctx,
            total,
        )
        .await;
        finalize_survey_if_all_terminal(workspace, &mut state, chatops_ctx).await;
        return Ok(());
    }

    // Read the final per-item state to discover the outcome.
    let final_bf = crate::state::brownfield_request::read_state(workspace, &item_request_id);
    let outcome_status = match final_bf {
        Ok(Some(s)) => Some(s),
        _ => None,
    };

    match outcome_status {
        Some(s) if s.status == BrownfieldRequestStatus::Acted => {
            state.items[idx].status = ItemStatus::Completed;
            state.items[idx].pr_url = s.pr_url.clone();
            let _ = brownfield_survey::write_state(workspace, &state);
            let m_done = count_terminal(&state);
            let pr_clause = s
                .pr_url
                .as_deref()
                .map(|u| format!(": {u}"))
                .unwrap_or_default();
            post_reply(
                chatops_ctx,
                &state.channel,
                &state.thread_ts,
                &format!(
                    "✅ Spec PR opened for `{item_slug}` ({m_done}/{total} done){pr_clause}"
                ),
            )
            .await;
        }
        Some(s) if s.status == BrownfieldRequestStatus::Aborted => {
            state.items[idx].status = ItemStatus::Skipped;
            state.items[idx].failure_reason = s.reason.clone();
            let _ = brownfield_survey::write_state(workspace, &state);
            let m_done = count_terminal(&state);
            post_reply(
                chatops_ctx,
                &state.channel,
                &state.thread_ts,
                &format!(
                    "⏭ Skipped `{item_slug}` ({m_done}/{total} done): {reason}",
                    reason = s.reason.as_deref().unwrap_or("conflict")
                ),
            )
            .await;
        }
        other => {
            let reason = other
                .and_then(|s| s.reason)
                .unwrap_or_else(|| "no failure reason recorded".to_string());
            mark_item_failed(workspace, &mut state, idx, reason, chatops_ctx, total).await;
        }
    }

    finalize_survey_if_all_terminal(workspace, &mut state, chatops_ctx).await;
    Ok(())
}

async fn mark_item_failed(
    workspace: &Path,
    state: &mut BrownfieldSurveyState,
    idx: usize,
    reason: String,
    chatops_ctx: Option<&ChatOpsContext>,
    total: usize,
) {
    let slug = state.items[idx].slug.clone();
    state.items[idx].status = ItemStatus::Failed;
    state.items[idx].failure_reason = Some(reason.clone());
    let _ = brownfield_survey::write_state(workspace, state);
    let m_done = count_terminal(state);
    post_reply(
        chatops_ctx,
        &state.channel,
        &state.thread_ts,
        &format!(
            "✗ Spec for `{slug}` failed ({m_done}/{total} done): {reason} (continuing with next)"
        ),
    )
    .await;
}

async fn finalize_survey_if_all_terminal(
    workspace: &Path,
    state: &mut BrownfieldSurveyState,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    if state.status != SurveyStatus::InProgress {
        return;
    }
    if !state.items.iter().all(|i| i.status.is_terminal()) {
        return;
    }
    let (succeeded, skipped, failed) = state.items.iter().fold((0, 0, 0), |acc, item| {
        let (a, b, c) = acc;
        match item.status {
            ItemStatus::Completed => (a + 1, b, c),
            ItemStatus::Skipped => (a, b + 1, c),
            ItemStatus::Failed => (a, b, c + 1),
            _ => acc,
        }
    });
    state.status = SurveyStatus::Completed;
    let _ = brownfield_survey::write_state(workspace, state);
    post_reply(
        chatops_ctx,
        &state.channel,
        &state.thread_ts,
        &format!(
            "✅ Brownfield batch complete. {succeeded} succeeded, {skipped} skipped (already specced), {failed} failed. See the survey thread for individual PR links AND failure reasons."
        ),
    )
    .await;
}

fn count_terminal(state: &BrownfieldSurveyState) -> usize {
    state.items.iter().filter(|i| i.status.is_terminal()).count()
}

fn find_in_progress_survey(workspace: &Path) -> Option<BrownfieldSurveyState> {
    let surveys = brownfield_survey::list_surveys_by_mtime(workspace).ok()?;
    for (request_id, _) in surveys {
        if let Ok(Some(state)) = brownfield_survey::read_state(workspace, &request_id)
            && state.status == SurveyStatus::InProgress
        {
            return Some(state);
        }
    }
    None
}

fn find_in_progress_other_than(workspace: &Path, exclude_id: &str) -> Option<String> {
    let surveys = brownfield_survey::list_surveys_by_mtime(workspace).ok()?;
    for (request_id, _) in surveys {
        if request_id == exclude_id {
            continue;
        }
        if let Ok(Some(state)) = brownfield_survey::read_state(workspace, &request_id)
            && state.status == SurveyStatus::InProgress
        {
            return Some(state.request_id);
        }
    }
    None
}

fn build_per_item_guidance(
    survey_guidance: Option<&str>,
    scope_in: &str,
    scope_out: &str,
    source_modules: &[String],
) -> String {
    let prefix = match survey_guidance {
        Some(g) if !g.trim().is_empty() => format!("{g}\n\n"),
        _ => String::new(),
    };
    format!(
        "{prefix}## Survey context\n\n**Scope-in:** {scope_in}\n\n**Scope-out:** {scope_out}\n\n**Source modules:** {sources}\n",
        sources = source_modules.join(", ")
    )
}

async fn post_reply(
    chatops_ctx: Option<&ChatOpsContext>,
    channel: &str,
    thread_ts: &str,
    body: &str,
) {
    let Some(ctx) = chatops_ctx else { return };
    if let Err(e) = ctx.chatops.post_threaded_reply(channel, thread_ts, body).await {
        tracing::warn!("brownfield-batch: thread reply failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::brownfield_survey::{ComplexityBand, SurveyItem};
    use tempfile::TempDir;

    fn fixture_item(id: usize, slug: &str, status: ItemStatus) -> SurveyItem {
        SurveyItem {
            id,
            slug: slug.into(),
            summary: "summary".into(),
            scope_in: "in".into(),
            scope_out: "out".into(),
            source_modules: vec![format!("src/{slug}/")],
            estimated_complexity: ComplexityBand::Small,
            status,
            pr_url: None,
            failure_reason: None,
        }
    }

    fn fixture_survey(request_id: &str, status: SurveyStatus, items: Vec<SurveyItem>) -> BrownfieldSurveyState {
        BrownfieldSurveyState {
            request_id: request_id.into(),
            repo_url: "git@github.com:a/b.git".into(),
            guidance: None,
            head_sha_at_survey: "abc".into(),
            completed_at: chrono::Utc::now(),
            thread_ts: "1.0".into(),
            channel: "C".into(),
            items,
            status,
        }
    }

    #[test]
    fn count_terminal_includes_completed_skipped_failed() {
        let survey = fixture_survey(
            "r",
            SurveyStatus::InProgress,
            vec![
                fixture_item(1, "a", ItemStatus::Completed),
                fixture_item(2, "b", ItemStatus::Pending),
                fixture_item(3, "c", ItemStatus::Skipped),
                fixture_item(4, "d", ItemStatus::Failed),
                fixture_item(5, "e", ItemStatus::Generating),
            ],
        );
        assert_eq!(count_terminal(&survey), 3);
    }

    #[test]
    fn find_in_progress_survey_picks_in_progress_one() {
        let tmp = TempDir::new().unwrap();
        let pending = fixture_survey(
            "pending",
            SurveyStatus::Pending,
            vec![fixture_item(1, "a", ItemStatus::Pending)],
        );
        let in_progress = fixture_survey(
            "running",
            SurveyStatus::InProgress,
            vec![fixture_item(1, "b", ItemStatus::Pending)],
        );
        brownfield_survey::write_state(tmp.path(), &pending).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        brownfield_survey::write_state(tmp.path(), &in_progress).unwrap();
        let got = find_in_progress_survey(tmp.path()).expect("in-progress found");
        assert_eq!(got.request_id, "running");
    }

    #[test]
    fn find_in_progress_other_than_skips_excluded_id() {
        let tmp = TempDir::new().unwrap();
        let in_progress = fixture_survey(
            "running",
            SurveyStatus::InProgress,
            vec![fixture_item(1, "b", ItemStatus::Pending)],
        );
        brownfield_survey::write_state(tmp.path(), &in_progress).unwrap();
        assert_eq!(
            find_in_progress_other_than(tmp.path(), "running"),
            None
        );
        assert_eq!(
            find_in_progress_other_than(tmp.path(), "different"),
            Some("running".to_string())
        );
    }

    #[test]
    fn build_per_item_guidance_includes_scope_and_sources() {
        let g = build_per_item_guidance(
            Some("primary guidance"),
            "what's in",
            "what's out",
            &["src/a/".to_string(), "src/b/".to_string()],
        );
        assert!(g.contains("primary guidance"), "{g}");
        assert!(g.contains("## Survey context"), "{g}");
        assert!(g.contains("Scope-in"), "{g}");
        assert!(g.contains("what's in"), "{g}");
        assert!(g.contains("Scope-out"), "{g}");
        assert!(g.contains("what's out"), "{g}");
        assert!(g.contains("src/a/, src/b/"), "{g}");
    }

    #[test]
    fn build_per_item_guidance_without_primary_guidance_skips_prefix() {
        let g = build_per_item_guidance(None, "in", "out", &["src/a/".into()]);
        assert!(g.starts_with("## Survey context"), "{g}");
    }

    #[tokio::test]
    async fn finalize_when_all_terminal_flips_to_completed() {
        let tmp = TempDir::new().unwrap();
        let mut s = fixture_survey(
            "r",
            SurveyStatus::InProgress,
            vec![
                fixture_item(1, "a", ItemStatus::Completed),
                fixture_item(2, "b", ItemStatus::Failed),
            ],
        );
        brownfield_survey::write_state(tmp.path(), &s).unwrap();
        finalize_survey_if_all_terminal(tmp.path(), &mut s, None).await;
        assert_eq!(s.status, SurveyStatus::Completed);
        let reloaded = brownfield_survey::read_state(tmp.path(), "r").unwrap().unwrap();
        assert_eq!(reloaded.status, SurveyStatus::Completed);
    }

    #[tokio::test]
    async fn finalize_when_not_all_terminal_leaves_status_unchanged() {
        let tmp = TempDir::new().unwrap();
        let mut s = fixture_survey(
            "r",
            SurveyStatus::InProgress,
            vec![
                fixture_item(1, "a", ItemStatus::Completed),
                fixture_item(2, "b", ItemStatus::Pending),
            ],
        );
        brownfield_survey::write_state(tmp.path(), &s).unwrap();
        finalize_survey_if_all_terminal(tmp.path(), &mut s, None).await;
        assert_eq!(s.status, SurveyStatus::InProgress);
    }
}
