//! a26 `sync-upstream` handler: rebase the workspace's base branch onto
//! `<upstream.remote>/<upstream.branch>` AND post the result OR conflict
//! notice back to the request's thread.
//!
//! The handler runs at iteration start (drained from
//! `pending_sync_upstream_requests`) AFTER the per-iteration workspace
//! init + opportunistic upstream fetch. It SHALL NOT push the rebased
//! base branch — the operator decides when to push to their fork.

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::RepositoryConfig;
use crate::control_socket::SyncUpstreamRequest;
use crate::git;
use crate::polling_loop::ChatOpsContext;

/// Process a drained `SyncUpstreamRequest`. Posts a thread reply in
/// every outcome path (no upstream configured, fetch failure, rebase
/// conflict, OR rebase success) so the operator always hears back. The
/// returned `Result` is reserved for unexpected infrastructure errors
/// — every reply-posting failure is logged at WARN AND swallowed.
pub async fn handle_sync_upstream(
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &SyncUpstreamRequest,
) -> Result<()> {
    let Some(upstream) = repo.upstream.as_ref() else {
        post_reply(
            chatops_ctx,
            request,
            "✗ sync-upstream: no upstream configured for this repo. Set the upstream block in config.yaml.",
        )
        .await;
        return Ok(());
    };

    // Step 1: best-effort fetch with a 60-second timeout. (The
    // opportunistic iteration-start fetch is shorter; here the operator
    // is explicitly waiting for a reply so we give the network a bit
    // more room.)
    if let Err(e) = git::ensure_remote(workspace, &upstream.remote, &upstream.url) {
        let body = format!(
            "✗ sync-upstream: could not register upstream remote `{}` -> `{}`: {e}",
            upstream.remote, upstream.url,
        );
        post_reply(chatops_ctx, request, &body).await;
        return Ok(());
    }
    if let Err(e) = git::fetch_remote_with_timeout(
        workspace,
        &upstream.remote,
        std::time::Duration::from_secs(60),
    ) {
        let body = format!("✗ sync-upstream: fetch failed: {e}");
        post_reply(chatops_ctx, request, &body).await;
        return Ok(());
    }

    // Step 2: checkout the base branch so the rebase applies there.
    if let Err(e) = git::checkout(workspace, &repo.base_branch) {
        let body = format!(
            "✗ sync-upstream: could not checkout base branch `{}`: {e}",
            repo.base_branch,
        );
        post_reply(chatops_ctx, request, &body).await;
        return Ok(());
    }

    // Capture the pre-rebase HEAD so we can report the number of
    // newly-incorporated commits on the success path.
    let pre_rebase_head = git::rev_parse(workspace, "HEAD")
        .context("sync-upstream: rev-parse HEAD before rebase")?;

    let upstream_ref = format!("{}/{}", upstream.remote, upstream.branch);

    // Step 3: rebase.
    match git::rebase_onto(workspace, &upstream_ref) {
        Ok(()) => {
            // Success: count commits pulled in (= `git rev-list --count
            // <pre-rebase-head>..HEAD` if the rebase moved HEAD;
            // post-rebase HEAD lineage encodes both pulled-in upstream
            // commits AND replayed local commits). To report ONLY the
            // upstream pull count, count `pre_rebase_head..upstream_ref`
            // BEFORE the rebase moved things. We compute it after the
            // fact via `<pre-rebase-head>..<upstream_ref>` which still
            // accurately reflects what upstream contributed.
            let pulled = git::rev_list_count(
                workspace,
                &format!("{pre_rebase_head}..{upstream_ref}"),
            )
            .unwrap_or(0);
            let ahead = git::rev_list_count(
                workspace,
                &format!("{upstream_ref}..HEAD"),
            )
            .unwrap_or(0);
            let body = format!(
                "✓ sync-upstream: pulled {pulled} commit(s) from {upstream_ref}. Base branch is {ahead} commit(s) ahead of upstream.",
            );
            post_reply(chatops_ctx, request, &body).await;
        }
        Err(git::RebaseError::Conflict { conflicted_files }) => {
            // Abort the rebase to restore the workspace.
            if let Err(e) = git::rebase_abort(workspace) {
                tracing::warn!(
                    request_id = %request.request_id,
                    "sync-upstream: rebase --abort failed: {e:#}"
                );
            }
            let files = if conflicted_files.is_empty() {
                "(unknown files)".to_string()
            } else {
                conflicted_files.join(", ")
            };
            let body = format!(
                "✗ sync-upstream: rebase conflict on {files}. Aborted. Resolve manually in the workspace AND re-run, OR merge manually.",
            );
            post_reply(chatops_ctx, request, &body).await;
        }
        Err(other) => {
            // Best-effort: ensure no in-flight rebase is left behind.
            let _ = git::rebase_abort(workspace);
            let body = format!("✗ sync-upstream: rebase failed: {other}");
            post_reply(chatops_ctx, request, &body).await;
        }
    }
    Ok(())
}

/// Post `body` as a thread reply to the request's channel/thread.
/// Swallows the underlying error (logged at WARN) so the handler stays
/// best-effort with respect to chat-delivery failures.
async fn post_reply(
    chatops_ctx: Option<&ChatOpsContext>,
    request: &SyncUpstreamRequest,
    body: &str,
) {
    let Some(ctx) = chatops_ctx else {
        tracing::warn!(
            request_id = %request.request_id,
            "sync-upstream: chatops not configured; skipping reply: {body}"
        );
        return;
    };
    if let Err(e) = ctx
        .chatops
        .post_threaded_reply(&request.channel, &request.thread_ts, body)
        .await
    {
        tracing::warn!(
            request_id = %request.request_id,
            "sync-upstream: thread reply failed: {e:#}"
        );
    }
}
