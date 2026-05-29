//! Spec-it polling handler (a25). One pass per iteration: drain ONE
//! pending `SpecItRequest` from the per-repo queue, resolve the
//! referenced scout-run state, look up the chosen item, compute
//! staleness, AND submit a `ProposeRequest` (reusing the standard
//! propose machinery) by pushing onto the proposal-request queue.
//!
//! Status updates from the resulting propose lifecycle continue to
//! post on the scout's thread (the propose machinery uses the
//! `thread_ts` we wrote onto the `ProposalRequestState` file).

use crate::config::RepositoryConfig;
use crate::polling_loop::ChatOpsContext;
use crate::proposal_requests::{
    self, ProposalRequestState, ProposalRequestStatus,
};
use crate::state::scout_run::{self, ScoutItem, ScoutRunState};
use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::path::Path;
use std::sync::Arc;

/// Process the one drained spec-it request. Returns `Ok(())` on every
/// path (including refusal); irrecoverable errors propagate as `Err`.
pub async fn process_pending_spec_it(
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    pending_proposal_requests: Arc<
        std::sync::Mutex<Vec<crate::control_socket::ProposalRequest>>,
    >,
    request: &crate::control_socket::SpecItRequest,
) -> Result<()> {
    let scout_state = match scout_run::read_state(workspace, &request.scout_request_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            post_thread(
                chatops_ctx,
                request,
                &format!(
                    "✗ spec-it: scout state for request `{}` not found (was it cleared?). Re-run `@<bot> scout <repo>` to refresh the list.",
                    request.scout_request_id
                ),
            )
            .await;
            return Ok(());
        }
        Err(e) => {
            post_thread(
                chatops_ctx,
                request,
                &format!(
                    "✗ spec-it: could not read scout state `{}`: {e}",
                    request.scout_request_id
                ),
            )
            .await;
            return Ok(());
        }
    };

    let item = match scout_state.items.iter().find(|i| i.id == request.item_id) {
        Some(i) => i.clone(),
        None => {
            post_thread(
                chatops_ctx,
                request,
                &format!(
                    "✗ spec-it: item #{} not present in scout state. The list may have changed; run `@<bot> scout <repo>` for a fresh list.",
                    request.item_id
                ),
            )
            .await;
            return Ok(());
        }
    };

    // Staleness handling: warn but proceed.
    let scout_cfg = crate::config::ScoutFeatureConfig::default();
    if let Some(msg) = compute_staleness_message(workspace, &scout_state, scout_cfg.staleness_warn_days) {
        post_thread(chatops_ctx, request, &msg).await;
    }

    // Build the propose-request text.
    let request_text = build_propose_text(&item, request.guidance.as_deref());

    // Create a fresh request_id for the proposal.
    let request_id = uuid::Uuid::new_v4().to_string();

    // Persist the proposal-request state under the daemon's
    // `state_dir` so the standard propose handler finds it on the
    // next iteration. The lifecycle thread is the scout's thread, so
    // status updates from the propose flow land in the scout thread.
    let state = ProposalRequestState {
        request_id: request_id.clone(),
        repo_url: request.repo_url.clone(),
        channel: request.channel.clone(),
        thread_ts: request.thread_ts.clone(),
        ack_message_ts: request.thread_ts.clone(),
        operator_user: format!("spec-it:{}", request.scout_request_id),
        request_text,
        submitted_at: Utc::now(),
        status: ProposalRequestStatus::Pending,
        reason: None,
    };
    let state_root = proposal_requests::default_state_root();
    if let Err(e) = proposal_requests::write_state(&state_root, &state) {
        tracing::warn!(
            scout_request_id = %request.scout_request_id,
            item_id = request.item_id,
            "spec-it: write proposal-request state failed: {e:#}"
        );
        post_thread(
            chatops_ctx,
            request,
            &format!("✗ spec-it: could not persist proposal-request state: {e}"),
        )
        .await;
        return Ok(());
    }

    // Push onto the in-memory queue so the polling iteration runs it.
    {
        let mut g = pending_proposal_requests.lock().unwrap();
        if !g.iter().any(|r| r.request_id == request_id) {
            g.push(crate::control_socket::ProposalRequest {
                request_id: state.request_id.clone(),
                channel: state.channel.clone(),
                thread_ts: state.thread_ts.clone(),
                operator_user: state.operator_user.clone(),
                request_text: state.request_text.clone(),
                submitted_at: state.submitted_at,
            });
        }
    }

    let _ = repo; // repo will become useful when spec-it grows per-repo behavior

    post_thread(
        chatops_ctx,
        request,
        &format!(
            "✓ spec-it: scoped item #{} (`{}`). Queued for triage; the next polling iteration will run it.",
            item.id, item.title
        ),
    )
    .await;
    Ok(())
}

/// Build the propose-request text shape documented in the proposal:
///   `[scout-item #N] <title>`
///   `<body>`
///   `Source: ...`
///   `Category: ...`
///   `Tractability: ...`
///   `<operator guidance, if any>`
pub fn build_propose_text(item: &ScoutItem, guidance: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!("[scout-item #{}] {}\n\n", item.id, item.title));
    out.push_str(item.body.trim_end());
    out.push_str("\n\n");
    out.push_str(&format!("Source: {}\n", item.source));
    out.push_str(&format!("Category: {}\n", item.category));
    out.push_str(&format!("Tractability: {}", item.tractability));
    if let Some(g) = guidance {
        let g = g.trim();
        if !g.is_empty() {
            out.push_str("\n\n");
            out.push_str(g);
        }
    }
    out
}

/// Returns the staleness warning message when either signal is true.
/// `None` when the scout is fresh AND HEAD is unchanged.
fn compute_staleness_message(
    workspace: &Path,
    scout_state: &ScoutRunState,
    threshold_days: u64,
) -> Option<String> {
    let now = Utc::now();
    let age = now - scout_state.completed_at;
    let too_old = age > ChronoDuration::days(threshold_days as i64);

    let current_head = crate::git::rev_parse(workspace, "HEAD").ok();
    let current_short: Option<String> = current_head
        .as_deref()
        .map(|s| s.chars().take(12).collect::<String>());
    let drifted = match current_short.as_deref() {
        Some(s) => s != scout_state.head_sha_at_run,
        None => false,
    };
    if !too_old && !drifted {
        return None;
    }
    let age_clause = humanize_age(age, scout_state.completed_at);
    let head_clause = if drifted {
        let commit_count = commit_count_between(
            workspace,
            &scout_state.head_sha_at_run,
            current_short.as_deref().unwrap_or(""),
        )
        .unwrap_or(0);
        if commit_count > 0 {
            format!("HEAD has moved {commit_count} commit(s)")
        } else {
            "HEAD has moved".to_string()
        }
    } else {
        "HEAD has unchanged".to_string()
    };
    Some(format!(
        "⚠️ Scout from {age_clause} ago; {head_clause}. Proceeding with the scouted item; consider re-running scout for fresh results."
    ))
}

fn humanize_age(delta: ChronoDuration, completed_at: DateTime<Utc>) -> String {
    let _ = completed_at;
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn commit_count_between(workspace: &Path, from: &str, to: &str) -> Option<usize> {
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let range = format!("{from}..{to}");
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", &range])
        .current_dir(workspace)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    raw.parse().ok()
}

async fn post_thread(
    chatops_ctx: Option<&ChatOpsContext>,
    request: &crate::control_socket::SpecItRequest,
    body: &str,
) {
    let Some(ctx) = chatops_ctx else { return };
    if let Err(e) = ctx
        .chatops
        .post_threaded_reply(&request.channel, &request.thread_ts, body)
        .await
    {
        tracing::warn!(
            scout_request_id = %request.scout_request_id,
            item_id = request.item_id,
            "spec-it: thread reply failed: {e:#}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_item() -> ScoutItem {
        ScoutItem {
            id: 3,
            category: "error_handling".into(),
            title: "Swallowed error in middleware".into(),
            body: "The middleware swallows the auth error.\n\nA second paragraph.".into(),
            source: "src/auth.rs:99".into(),
            tractability: "small".into(),
        }
    }

    #[test]
    fn build_propose_text_includes_documented_lines() {
        let text = build_propose_text(&fixture_item(), None);
        assert!(text.starts_with("[scout-item #3]"));
        assert!(text.contains("Swallowed error in middleware"));
        assert!(text.contains("Source: src/auth.rs:99"));
        assert!(text.contains("Category: error_handling"));
        assert!(text.contains("Tractability: small"));
    }

    #[test]
    fn build_propose_text_appends_guidance_when_present() {
        let text = build_propose_text(
            &fixture_item(),
            Some("stick to the OAuth scope, ignore the rate-limit angle"),
        );
        assert!(
            text.ends_with("\n\nstick to the OAuth scope, ignore the rate-limit angle"),
            "{text}"
        );
    }

    #[test]
    fn build_propose_text_omits_guidance_when_blank() {
        let text = build_propose_text(&fixture_item(), Some("   "));
        assert!(!text.contains("\n\n\n"));
        // Last line is the tractability line — no guidance suffix.
        assert!(text.trim_end().ends_with("Tractability: small"));
    }

    #[test]
    fn humanize_age_units() {
        assert_eq!(humanize_age(ChronoDuration::seconds(5), Utc::now()), "5s");
        assert_eq!(humanize_age(ChronoDuration::seconds(120), Utc::now()), "2m");
        assert_eq!(humanize_age(ChronoDuration::hours(3), Utc::now()), "3h");
        assert_eq!(humanize_age(ChronoDuration::days(4), Utc::now()), "4d");
    }
}
