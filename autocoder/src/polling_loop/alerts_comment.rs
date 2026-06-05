use super::*;

/// Per-canonical-spec character cap on a threaded notification body before
/// the per-comment "failed" notifications switch to the threaded API AND
/// truncate. Mirrors the audit-findings threading threshold; see
/// `audits::AUDIT_THREAD_BODY_CHAR_CAP`. (a68: the two byte-identical
/// `35_000` revise/code-review caps collapsed into this single constant.)
const FAILED_REASON_THREAD_CAP: usize = 35_000;

/// Render `duration` using the same human-format shape the chatops
/// `status` reply uses for "started Nm ago" — delegates to
/// `busy_marker::format_age_human` so the two stay in lockstep.
fn format_revise_duration(duration: std::time::Duration) -> String {
    busy_marker::format_age_human(duration.as_secs())
}

/// Compose the canonical `change_list_summary` segment for a
/// revise-lifecycle notification: `` `<first_change>` +N more `` (the
/// `+0 more` suffix is omitted; `+1 more` AND higher are included). The
/// caller wraps the result in `(...)` when embedding.
pub(crate) fn format_revise_change_list_summary(change_list: &[String]) -> String {
    if change_list.is_empty() {
        return "(unknown change)".to_string();
    }
    let first = &change_list[0];
    let extras = change_list.len().saturating_sub(1);
    if extras == 0 {
        format!("`{first}`")
    } else {
        format!("`{first}` +{extras} more")
    }
}

/// Truncate `operator_comment` to at most `max_chars` characters,
/// appending `…` when truncated. Used by the picked-up dispatch site to
/// fit the operator's revise text into the 80-char quote slot.
pub(crate) fn truncate_operator_comment(operator_comment: &str, max_chars: usize) -> String {
    let count = operator_comment.chars().count();
    if count <= max_chars {
        return operator_comment.to_string();
    }
    let mut out: String = operator_comment.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Dedup key for a per-comment lifecycle notification. Unifies the two
/// previously copy-pasted families (revise + code-review) so the 9-step
/// load/dedup/post/record/save skeleton lives in exactly one place
/// ([`post_comment_dedup_alert`]). (a68 §3.1 collapse.)
#[derive(Clone, Copy)]
enum CommentNotifKey {
    Revise(crate::alert_state::ReviseNotificationKind),
    CodeReview(crate::alert_state::CodeReviewNotificationKind),
}

impl CommentNotifKey {
    fn already_posted(&self, state: &AlertState, comment_id: &str) -> bool {
        match self {
            CommentNotifKey::Revise(k) => state.revise_notification_already_posted(comment_id, *k),
            CommentNotifKey::CodeReview(k) => {
                state.code_review_notification_already_posted(comment_id, *k)
            }
        }
    }

    fn record(&self, state: &mut AlertState, comment_id: &str) {
        match self {
            CommentNotifKey::Revise(k) => {
                state.record_revise_notification(comment_id, *k, Utc::now())
            }
            CommentNotifKey::CodeReview(k) => {
                state.record_code_review_notification(comment_id, *k, Utc::now())
            }
        }
    }

    /// Stable diagnostic label used in the post-failure / save-failure WARN
    /// log lines. Matches the per-variant wording the pre-collapse helpers
    /// emitted (operator-log diagnostics, not a shipped message).
    fn label(&self) -> &'static str {
        use crate::alert_state::{CodeReviewNotificationKind as C, ReviseNotificationKind as R};
        match self {
            CommentNotifKey::Revise(R::PickedUp) => "revise-picked-up",
            CommentNotifKey::Revise(R::Succeeded) => "revise-succeeded",
            CommentNotifKey::Revise(R::Failed) => "revise-failed",
            CommentNotifKey::CodeReview(C::Triggered) => "code-review-triggered",
            CommentNotifKey::CodeReview(C::Complete) => "code-review-complete",
            CommentNotifKey::CodeReview(C::Failed) => "code-review-failed",
        }
    }
}

/// Rendered notification body: a single inline message, or a top-line +
/// threaded reply (the "failed" variants switch to this when the reason
/// exceeds [`FAILED_REASON_THREAD_CAP`]).
enum CommentBody {
    Inline(String),
    Threaded {
        top_line: String,
        thread_body: String,
    },
}

/// Build the inline-or-threaded body for a "failed" per-comment
/// notification. When `reason` exceeds [`FAILED_REASON_THREAD_CAP`] the body
/// switches to the threaded API AND truncates with the canonical
/// pointer-to-daemon-log tail (a68 §3.3: the duplicated journalctl tail now
/// lives here only). `action` is the verb phrase ("revision failed" vs
/// "code review failed").
fn render_failed_comment_body(
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    reason: &str,
    action: &str,
) -> CommentBody {
    if reason.chars().count() > FAILED_REASON_THREAD_CAP {
        let top_line = format!(
            "✗ `{repo_url}`: {action} on PR #{pr_number} (full reason in thread)\n{pr_url}",
            repo_url = repo.url,
        );
        let truncated: String = reason.chars().take(FAILED_REASON_THREAD_CAP).collect();
        let thread_body = format!(
            "{truncated}\n\n… [truncated; full reason at journalctl -u autocoder | grep pr={pr_number}]"
        );
        CommentBody::Threaded {
            top_line,
            thread_body,
        }
    } else {
        CommentBody::Inline(format!(
            "✗ `{repo_url}`: {action} on PR #{pr_number}: {reason}\n{pr_url}",
            repo_url = repo.url,
        ))
    }
}

/// The shared per-comment-dedup skeleton: gate on chatops presence +
/// `failure_alerts_enabled`, load alert-state, short-circuit if the
/// notification was already posted for this comment, post the body, then
/// (only on success) record + persist. On post failure the alert-state file
/// is NOT updated so a later iteration can retry. (a68 §3.1: the six
/// near-identical revise/code-review helpers delegate here.)
async fn post_comment_dedup_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    comment_id: &str,
    key: CommentNotifKey,
    body: CommentBody,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let workspace = workspace::resolve_path(paths, repo);
    let mut state = AlertState::load_or_default(paths, &workspace);
    if key.already_posted(&state, comment_id) {
        return;
    }
    let post_result = match &body {
        CommentBody::Inline(text) => ctx.chatops.post_notification(ctx.channel, text).await,
        CommentBody::Threaded {
            top_line,
            thread_body,
        } => ctx
            .chatops
            .post_notification_with_thread(ctx.channel, top_line, thread_body)
            .await
            .map(|_| ()),
    };
    if let Err(e) = post_result {
        tracing::warn!(
            url = %repo.url,
            pr_number = pr_number,
            comment_id = %comment_id,
            "{} chatops notification post failed: {e:#}",
            key.label()
        );
        return;
    }
    key.record(&mut state, comment_id);
    if let Err(e) = state.save(paths, &workspace) {
        tracing::warn!(
            url = %repo.url,
            pr_number = pr_number,
            comment_id = %comment_id,
            "failed to persist {} notification state: {e:#}",
            key.label()
        );
    }
}

/// Post the chatops "Revise picked up" lifecycle notification (best-
/// effort, deduplicated per-comment via the alert-state file's
/// `revise_notifications` map). Returns silently when the chatops
/// backend is absent, `failure_alerts_enabled` is `false`, OR the
/// notification was already posted for this comment. On post failure,
/// the alert-state file is NOT updated so a subsequent iteration can
/// retry.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_post_revise_picked_up_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    change_list_summary: &str,
    operator_comment_quote: &str,
    comment_id: &str,
) {
    let text = format!(
        "🔧 `{repo_url}`: revising PR #{pr_number} ({change_list_summary}): \"{quote}\"\n{pr_url}",
        repo_url = repo.url,
        quote = operator_comment_quote,
    );
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::Revise(crate::alert_state::ReviseNotificationKind::PickedUp),
        CommentBody::Inline(text),
    )
    .await;
}

/// Post the chatops "Revise succeeded" lifecycle notification (mirrors
/// [`maybe_post_revise_picked_up_alert`] with the `Succeeded` kind).
/// Posted after the executor returns `Completed` (or `IterationRequested`)
/// AND the commit + force-push step succeeds.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_post_revise_succeeded_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    change_list_summary: &str,
    agent_branch: &str,
    duration: std::time::Duration,
    comment_id: &str,
) {
    let text = format!(
        "✓ `{repo_url}`: revision applied to PR #{pr_number} ({change_list_summary}) — force-pushed `{agent_branch}` (took {duration_human})\n{pr_url}",
        repo_url = repo.url,
        duration_human = format_revise_duration(duration),
    );
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::Revise(crate::alert_state::ReviseNotificationKind::Succeeded),
        CommentBody::Inline(text),
    )
    .await;
}

/// Post the chatops "Revise failed" lifecycle notification (mirrors
/// [`maybe_post_revise_picked_up_alert`] with the `Failed` kind). When
/// `reason.len() > FAILED_REASON_THREAD_CAP`, the helper switches
/// to the threaded-notification API AND truncates the body at 35,000
/// characters with a pointer-to-daemon-log tail (per the existing
/// canonical "Thread body truncates at 35,000 characters" requirement).
pub(crate) async fn maybe_post_revise_failed_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    reason: &str,
    comment_id: &str,
) {
    let body = render_failed_comment_body(repo, pr_number, pr_url, reason, "revision failed");
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::Revise(crate::alert_state::ReviseNotificationKind::Failed),
        body,
    )
    .await;
}

/// Post the chatops "Code review triggered" lifecycle notification (a33)
/// (best-effort, deduplicated per-comment via the alert-state file's
/// `code_review_notifications` map). Returns silently when the chatops
/// backend is absent, `failure_alerts_enabled` is `false`, OR the
/// notification was already posted for this comment.
pub(crate) async fn maybe_post_code_review_triggered_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    operator_login: &str,
    comment_id: &str,
) {
    let text = format!(
        "🔍 `{repo_url}`: code review triggered on PR #{pr_number} by @{operator_login}\n{pr_url}",
        repo_url = repo.url,
    );
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::CodeReview(crate::alert_state::CodeReviewNotificationKind::Triggered),
        CommentBody::Inline(text),
    )
    .await;
}

/// Post the chatops "Code review complete" lifecycle notification (a33).
pub(crate) async fn maybe_post_code_review_complete_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    verdict_label: &str,
    comment_id: &str,
) {
    let text = format!(
        "✓ `{repo_url}`: code review complete on PR #{pr_number} — verdict: {verdict_label}\n{pr_url}",
        repo_url = repo.url,
    );
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::CodeReview(crate::alert_state::CodeReviewNotificationKind::Complete),
        CommentBody::Inline(text),
    )
    .await;
}

/// Post the reviewer-failure operator alert for the pre-PR INITIAL agentic
/// review (a58). Unlike [`maybe_post_code_review_failed_alert`] (which is
/// PR-scoped, for the operator-triggered rerun), the initial review runs
/// BEFORE the PR exists, so this best-effort notification names only the
/// repo. Gated on `failure_alerts_enabled`; a missing chatops backend OR a
/// post error degrades to the ERROR log line the caller already emitted.
pub(crate) async fn post_reviewer_discarded_alert(
    chatops_ctx: Option<&ChatOpsContext>,
    repo: &RepositoryConfig,
    reason: &str,
) {
    let Some(ctx) = chatops_ctx else { return };
    if !ctx.failure_alerts_enabled {
        return;
    }
    let text = format!(
        "✗ `{repo_url}`: code review discarded (no verdict written): {reason}",
        repo_url = repo.url,
    );
    if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            "reviewer-discarded chatops notification post failed: {e:#}"
        );
    }
}

/// Post the chatops "Code review failed" lifecycle notification (a33).
/// When `reason.len() > FAILED_REASON_THREAD_CAP`, switches
/// to the threaded-notification path AND truncates per the canonical
/// 35,000-char rule.
pub(crate) async fn maybe_post_code_review_failed_alert(
    paths: &DaemonPaths,
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    reason: &str,
    comment_id: &str,
) {
    let body = render_failed_comment_body(repo, pr_number, pr_url, reason, "code review failed");
    post_comment_dedup_alert(
        paths,
        chatops_ctx,
        repo,
        pr_number,
        comment_id,
        CommentNotifKey::CodeReview(crate::alert_state::CodeReviewNotificationKind::Failed),
        body,
    )
    .await;
}

/// Post the chatops re-review suggestion (a33). Fires after a revision
/// iteration when the cumulative-since-original-review diff overlap
/// exceeds the operator-configured threshold. Best-effort,
/// `failure_alerts_enabled`-gated, AND deduplicated per-PR per
/// `revisions_count` via the per-PR state file's
/// `last_suggested_rereview_at_revisions_count` field (caller updates
/// the field after a successful post).
pub(crate) async fn maybe_post_rereview_suggestion_alert(
    chatops_ctx: Option<&crate::revisions::ChatOpsCtx<'_>>,
    repo: &RepositoryConfig,
    pr_number: u64,
    pr_url: &str,
    overlap_percent: u32,
    revisions_count: u32,
) -> bool {
    let Some(ctx) = chatops_ctx else { return false };
    if !ctx.failure_alerts_enabled {
        return false;
    }
    let text = format!(
        "💡 `{repo_url}`: PR #{pr_number} has been substantially revised (~{overlap_percent}% of original diff changed across {revisions_count} revisions). Consider `@<bot> code-review` to re-evaluate.\n{pr_url}",
        repo_url = repo.url,
    );
    if let Err(e) = ctx.chatops.post_notification(ctx.channel, &text).await {
        tracing::warn!(
            url = %repo.url,
            pr_number = pr_number,
            "re-review suggestion chatops notification post failed: {e:#}"
        );
        return false;
    }
    true
}
