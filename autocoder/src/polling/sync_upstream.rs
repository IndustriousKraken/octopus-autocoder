//! OSS-fork support (a26): polling-iteration handler for
//! `@<bot> sync-upstream <repo>`. Fetches the configured upstream
//! remote, rebases the workspace's base branch on top, AND posts
//! a chatops thread reply summarizing the result OR naming
//! conflicting files when the rebase aborts.
//!
//! Best-effort: failures NEVER propagate to the surrounding
//! iteration. The handler also NEVER pushes — the operator decides
//! when to push the rebased base branch to their fork.

use crate::config::RepositoryConfig;
use crate::control_socket::SyncUpstreamRequest;
use crate::git;
use crate::polling_loop::ChatOpsContext;
use anyhow::Result;
use std::path::Path;

/// Process one drained `SyncUpstreamRequest`. Always returns
/// `Ok(())` (chatops failures are logged at WARN; rebase failures
/// produce a conflict reply rather than an error).
pub async fn process_pending_sync_upstream(
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    request: &SyncUpstreamRequest,
) -> Result<()> {
    let reply = run_sync_upstream(workspace, repo);
    post_reply(chatops_ctx, request, &reply).await;
    Ok(())
}

/// Pure result enum: the handler decides between misconfiguration,
/// fetch failure, conflict, and happy-path outcomes; the chatops
/// layer renders each into a one-line thread reply.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncUpstreamReply {
    /// `upstream` block not set on the repo.
    NoUpstream,
    /// `git fetch <remote>` returned non-zero.
    FetchFailed { reason: String },
    /// Checkout of the base branch failed.
    CheckoutFailed { branch: String, reason: String },
    /// Rebase aborted due to conflicts; lists conflicted files.
    Conflict { files: Vec<String> },
    /// Rebase failed for some other reason; the rebase was aborted.
    RebaseFailed { reason: String },
    /// Rebase succeeded with `<pulled>` newly-incorporated commits;
    /// the workspace is `<ahead>` commits ahead of upstream after.
    Success { pulled: usize, ahead: usize, remote: String, branch: String },
}

impl SyncUpstreamReply {
    pub fn render(&self) -> String {
        match self {
            Self::NoUpstream => {
                "✗ sync-upstream: no upstream configured for this repo. \
                 Set the upstream block in config.yaml."
                    .to_string()
            }
            Self::FetchFailed { reason } => {
                format!("✗ sync-upstream: fetch failed: {reason}.")
            }
            Self::CheckoutFailed { branch, reason } => {
                format!("✗ sync-upstream: checkout {branch} failed: {reason}.")
            }
            Self::Conflict { files } => {
                let list = if files.is_empty() {
                    "(unknown — git did not report conflicting paths)".to_string()
                } else {
                    files.join(", ")
                };
                format!(
                    "✗ sync-upstream: rebase conflict on {list}. Aborted. \
                     Resolve manually in the workspace AND re-run, OR merge manually."
                )
            }
            Self::RebaseFailed { reason } => {
                format!(
                    "✗ sync-upstream: rebase failed: {reason}. Workspace restored."
                )
            }
            Self::Success {
                pulled,
                ahead,
                remote,
                branch,
            } => {
                format!(
                    "✓ sync-upstream: pulled {pulled} commit(s) from {remote}/{branch}. \
                     Base branch is {ahead} commit(s) ahead of upstream."
                )
            }
        }
    }
}

/// Run the rebase pipeline against `workspace`. Pure shell+git
/// operations; the chatops post happens in the caller. Surface
/// every failure mode as a `SyncUpstreamReply` so the caller
/// formats a single thread message.
fn run_sync_upstream(workspace: &Path, repo: &RepositoryConfig) -> SyncUpstreamReply {
    let upstream = match repo.upstream.as_ref() {
        Some(u) => u.clone(),
        None => return SyncUpstreamReply::NoUpstream,
    };
    // Ensure the remote is up-to-date with config (idempotent).
    if let Err(e) = git::ensure_remote(workspace, &upstream.remote, &upstream.url) {
        return SyncUpstreamReply::FetchFailed {
            reason: format!("ensure_remote: {e}"),
        };
    }
    // Fetch with a 60s timeout per spec.
    if let Err(e) =
        git::fetch_remote_with_timeout(workspace, &upstream.remote, 60)
    {
        return SyncUpstreamReply::FetchFailed { reason: e.to_string() };
    }
    // Capture HEAD before rebase so we can count incorporated commits.
    let pre_head =
        git::rev_parse(workspace, "HEAD").unwrap_or_else(|_| "HEAD".to_string());
    if let Err(e) = git::checkout(workspace, &repo.base_branch) {
        return SyncUpstreamReply::CheckoutFailed {
            branch: repo.base_branch.clone(),
            reason: e.to_string(),
        };
    }
    let upstream_ref = format!("{}/{}", upstream.remote, upstream.branch);
    match git::rebase(workspace, &upstream_ref) {
        Ok(()) => {
            // Count incorporated commits: how many commits are in
            // pre_head..HEAD that weren't there before. A clean
            // fast-forward gives the commit count directly.
            let pulled = git::rev_list_count(
                workspace,
                &format!("{pre_head}..HEAD"),
            )
            .unwrap_or(0);
            let ahead = git::rev_list_count(
                workspace,
                &format!("{upstream_ref}..HEAD"),
            )
            .unwrap_or(0);
            SyncUpstreamReply::Success {
                pulled,
                ahead,
                remote: upstream.remote,
                branch: upstream.branch,
            }
        }
        Err(e) => {
            let conflicts =
                git::conflicted_files(workspace).unwrap_or_default();
            // Always attempt to restore by aborting any in-progress
            // rebase. `rebase_abort` already tolerates the "no rebase
            // in progress" case.
            let _ = git::rebase_abort(workspace);
            if !conflicts.is_empty() {
                SyncUpstreamReply::Conflict { files: conflicts }
            } else {
                SyncUpstreamReply::RebaseFailed {
                    reason: e.to_string(),
                }
            }
        }
    }
}

/// Best-effort post the chatops reply. Failures log at WARN and
/// return cleanly so the caller's outcome is unaffected.
async fn post_reply(
    chatops_ctx: Option<&ChatOpsContext>,
    request: &SyncUpstreamRequest,
    reply: &SyncUpstreamReply,
) {
    let Some(ctx) = chatops_ctx else {
        tracing::info!(
            request_id = %request.request_id,
            "sync-upstream: no chatops context; result: {}",
            reply.render()
        );
        return;
    };
    if request.channel.is_empty() {
        tracing::warn!(
            request_id = %request.request_id,
            "sync-upstream: empty channel id; skipping chatops reply"
        );
        return;
    }
    let body = reply.render();
    let post_result = if request.thread_ts.is_empty() {
        ctx.chatops.post_notification(&request.channel, &body).await
    } else {
        ctx.chatops
            .post_threaded_reply(&request.channel, &request.thread_ts, &body)
            .await
    };
    if let Err(e) = post_result {
        tracing::warn!(
            request_id = %request.request_id,
            "sync-upstream: chatops post failed: {e:#}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UpstreamConfig;

    fn fixture_repo() -> RepositoryConfig {
        RepositoryConfig { forge: None,
            url: "git@github.com:owner/repo.git".to_string(),
            local_path: None,
            base_branch: "main".to_string(),
            agent_branch: "agent-q".to_string(),
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

    #[test]
    fn render_no_upstream_names_misconfig() {
        let body = SyncUpstreamReply::NoUpstream.render();
        assert!(body.contains("no upstream configured"));
        assert!(body.contains("config.yaml"));
    }

    #[test]
    fn render_success_includes_counts_and_remote() {
        let body = SyncUpstreamReply::Success {
            pulled: 7,
            ahead: 0,
            remote: "upstream".into(),
            branch: "main".into(),
        }
        .render();
        assert!(body.contains("pulled 7 commit(s)"));
        assert!(body.contains("upstream/main"));
        assert!(body.contains("0 commit(s) ahead"));
    }

    #[test]
    fn render_conflict_lists_files() {
        let body = SyncUpstreamReply::Conflict {
            files: vec!["src/lib.rs".into(), "tests/integration.rs".into()],
        }
        .render();
        assert!(body.contains("src/lib.rs"));
        assert!(body.contains("tests/integration.rs"));
        assert!(body.contains("Aborted"));
    }

    #[test]
    fn render_conflict_no_files_falls_back() {
        let body = SyncUpstreamReply::Conflict { files: vec![] }.render();
        assert!(body.contains("unknown"));
    }

    #[test]
    fn render_fetch_failed_includes_reason() {
        let body = SyncUpstreamReply::FetchFailed {
            reason: "network unreachable".into(),
        }
        .render();
        assert!(body.contains("network unreachable"));
        assert!(body.contains("fetch failed"));
    }

    #[test]
    fn run_sync_upstream_no_upstream_returns_no_upstream() {
        let dir = tempfile::TempDir::new().unwrap();
        // The repo doesn't need to exist for the no-upstream branch.
        let reply = run_sync_upstream(dir.path(), &fixture_repo());
        assert_eq!(reply, SyncUpstreamReply::NoUpstream);
    }

    fn init_bare(dir: &Path) {
        let st = std::process::Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir)
            .status()
            .unwrap();
        assert!(st.success(), "bare init failed");
    }

    fn clone(remote: &Path, target: &Path) {
        let st = std::process::Command::new("git")
            .args([
                "clone",
                "-q",
                remote.to_string_lossy().as_ref(),
                target.to_string_lossy().as_ref(),
            ])
            .status()
            .unwrap();
        assert!(st.success(), "clone failed");
    }

    fn commit_in(workspace: &Path, file: &str, content: &str, message: &str) {
        std::fs::write(workspace.join(file), content).unwrap();
        let st = std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["add", file])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success());
    }

    /// Happy path: workspace's base branch lags upstream by 1 commit;
    /// rebase pulls it cleanly.
    #[test]
    fn run_sync_upstream_happy_path_returns_success() {
        let dir = tempfile::TempDir::new().unwrap();
        let upstream_bare = dir.path().join("upstream.git");
        init_bare(&upstream_bare);
        // Seed upstream with one commit.
        let upstream_seed = dir.path().join("upstream-seed");
        clone(&upstream_bare, &upstream_seed);
        commit_in(&upstream_seed, "README.md", "initial\n", "initial");
        let st = std::process::Command::new("git")
            .args(["push", "-q", "origin", "main"])
            .current_dir(&upstream_seed)
            .status()
            .unwrap();
        assert!(st.success());

        // Now clone the upstream as the workspace (workspace's
        // origin == upstream for simplicity; the rebase uses the
        // separately-added `upstream` remote).
        let workspace = dir.path().join("workspace");
        clone(&upstream_bare, &workspace);

        // Add another commit on upstream that the workspace doesn't
        // have yet.
        commit_in(&upstream_seed, "FILE2.md", "second\n", "second");
        let st = std::process::Command::new("git")
            .args(["push", "-q", "origin", "main"])
            .current_dir(&upstream_seed)
            .status()
            .unwrap();
        assert!(st.success());

        let mut repo = fixture_repo();
        repo.local_path = Some(workspace.clone());
        repo.upstream = Some(UpstreamConfig {
            remote: "upstream".to_string(),
            branch: "main".to_string(),
            url: upstream_bare.to_string_lossy().to_string(),
        });

        let reply = run_sync_upstream(&workspace, &repo);
        match reply {
            SyncUpstreamReply::Success {
                pulled,
                ahead,
                remote,
                branch,
            } => {
                assert_eq!(pulled, 1, "should have pulled one commit");
                assert_eq!(ahead, 0, "no local-only commits, so 0 ahead");
                assert_eq!(remote, "upstream");
                assert_eq!(branch, "main");
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    /// Conflict path: workspace's base branch has a divergent commit
    /// touching the same file as upstream; rebase aborts.
    #[test]
    fn run_sync_upstream_conflict_path_reports_conflict() {
        let dir = tempfile::TempDir::new().unwrap();
        let upstream_bare = dir.path().join("upstream.git");
        init_bare(&upstream_bare);
        let upstream_seed = dir.path().join("upstream-seed");
        clone(&upstream_bare, &upstream_seed);
        commit_in(&upstream_seed, "README.md", "initial\n", "initial");
        let st = std::process::Command::new("git")
            .args(["push", "-q", "origin", "main"])
            .current_dir(&upstream_seed)
            .status()
            .unwrap();
        assert!(st.success());

        let workspace = dir.path().join("workspace");
        clone(&upstream_bare, &workspace);

        // Workspace edits README.md.
        commit_in(&workspace, "README.md", "workspace edit\n", "workspace");

        // Upstream also edits README.md to a different value.
        commit_in(
            &upstream_seed,
            "README.md",
            "upstream edit\n",
            "upstream",
        );
        let st = std::process::Command::new("git")
            .args(["push", "-q", "origin", "main"])
            .current_dir(&upstream_seed)
            .status()
            .unwrap();
        assert!(st.success());

        let mut repo = fixture_repo();
        repo.local_path = Some(workspace.clone());
        repo.upstream = Some(UpstreamConfig {
            remote: "upstream".to_string(),
            branch: "main".to_string(),
            url: upstream_bare.to_string_lossy().to_string(),
        });

        let reply = run_sync_upstream(&workspace, &repo);
        match reply {
            SyncUpstreamReply::Conflict { files } => {
                assert!(
                    files.iter().any(|f| f.contains("README.md")),
                    "expected README.md in conflicts, got {files:?}"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // The workspace should be left at the pre-rebase commit (the
        // workspace edit). Confirm porcelain is clean.
        let porc = git::status_porcelain(&workspace).unwrap();
        assert!(porc.is_empty(), "porcelain should be clean after abort: {porc}");
    }
}
