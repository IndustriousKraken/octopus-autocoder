//! Fork-pending polling state
//! (`startup-fork-setup-retries-transient-failures`).
//!
//! A repository whose startup fork setup failed TRANSIENTLY spawns its
//! polling task in the fork-pending state instead of being skipped for the
//! process lifetime. Each fork-pending iteration re-attempts the full fork
//! setup (probe → create-if-missing → identity check → reachability) and does
//! NO other work for the repository until setup succeeds. On success the task
//! resumes normal polling with no operator action; a re-attempt that fails
//! with a permanent-classified cause flips to the permanent path (alert with
//! the remedy hint, then exit the polling set as if skipped at startup).

use super::*;
use crate::alert_state::AlertCategory;
use crate::alerts::handle_classified_recovery_failure;
use crate::cli::run::{ForkOps, ForkSetupFailure, ensure_forks_exist_with, fork_setup_failure_alert_message};

/// Outcome of one fork-pending re-attempt, driving the polling loop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ForkPendingStep {
    /// Fork setup succeeded: resume normal polling from this iteration on.
    Ready,
    /// Transient failure: WARN + throttled alert already emitted; the loop
    /// sleeps and re-attempts on the next tick.
    RetryTransient,
    /// Permanent failure (e.g. the upstream was renamed while pending): the
    /// remedy alert was already emitted; the task exits the polling set.
    Permanent,
}

/// Run one fork-pending re-attempt for `repo`. Reuses the SAME per-repo
/// fork-setup driver the startup path uses (`ensure_forks_exist_with` over a
/// one-element slice), so the probe/create/identity/reachability behavior —
/// and its transient/permanent classification — is identical.
///
/// This function owns all fork-pending logging + alerting so it is unit-
/// testable with a scripted [`ForkOps`]:
/// - success → INFO recovery notice, returns [`ForkPendingStep::Ready`];
/// - transient failure → WARN naming the cause + a throttled chatops alert
///   naming the repo as fork-pending, returns [`ForkPendingStep::RetryTransient`];
/// - permanent failure → a one-shot chatops alert carrying the restart-or-
///   reload remedy (the same message the startup skip path emits), returns
///   [`ForkPendingStep::Permanent`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_fork_pending_iteration(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    ops: &dyn ForkOps,
    reachability_timeout: Duration,
    poll_interval: Duration,
) -> ForkPendingStep {
    let mut failures = ensure_forks_exist_with(
        github,
        std::slice::from_ref(repo),
        ops,
        reachability_timeout,
        poll_interval,
    )
    .await;

    let Some(failure) = failures.pop() else {
        tracing::info!(
            url = %repo.url,
            "fork-pending: fork setup succeeded; resuming normal polling"
        );
        return ForkPendingStep::Ready;
    };

    match failure.class {
        RecoveryFailureClass::Permanent => {
            tracing::error!(
                url = %repo.url,
                "fork-pending: re-attempt failed with a permanent cause; skipping this \
                 repository until an operator restart or reload: {}",
                failure.cause
            );
            post_permanent_alert(chatops_ctx, &failure).await;
            ForkPendingStep::Permanent
        }
        RecoveryFailureClass::Transient => {
            tracing::warn!(
                url = %repo.url,
                "fork-pending: re-attempt failed (transient; retrying next iteration): {}",
                failure.cause
            );
            post_throttled_pending_alert(paths, workspace, repo, chatops_ctx, &failure).await;
            ForkPendingStep::RetryTransient
        }
    }
}

/// One-shot permanent alert: the SAME message the startup skip path emits
/// (`fork_setup_failure_alert_message`), posted directly through the pass's
/// chatops context. Independent of `notifications.*` flags — a sidelined
/// repository is a daemon-lifecycle event. Best-effort: a delivery failure
/// logs at WARN and never blocks the task's exit.
async fn post_permanent_alert(chatops_ctx: Option<&ChatOpsContext>, failure: &ForkSetupFailure) {
    let msg = fork_setup_failure_alert_message(failure);
    match chatops_ctx {
        Some(ctx) => {
            if let Err(e) = ctx.chatops.post_notification(&ctx.channel, &msg).await {
                tracing::warn!(
                    url = %failure.upstream_url,
                    "fork-pending permanent alert failed to deliver: {e}"
                );
            }
        }
        None => {
            tracing::warn!(
                url = %failure.upstream_url,
                "fork-pending re-attempt hit a permanent cause and no chatops backend is \
                 configured to alert: {}",
                failure.cause
            );
        }
    }
}

/// Throttled fork-pending alert while transient attempts continue. Reuses the
/// existing predictable-failure throttle machinery
/// (`handle_classified_recovery_failure` + the 24h per-(repo, category)
/// throttle) under the [`AlertCategory::ForkSetupPending`] category, so the
/// operator gets ONE alert per throttle window rather than one per failed
/// re-attempt. Rendered as
/// `⚠️ <repo>: fork setup pending (transient; retrying). Latest: <cause>`.
/// Passed `notifications_enabled = true` because fork-setup degradation is a
/// daemon-lifecycle signal, independent of the operator's `notifications.*`
/// preferences.
async fn post_throttled_pending_alert(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    failure: &ForkSetupFailure,
) {
    let err = anyhow!("{}", failure.cause);
    handle_classified_recovery_failure(
        paths,
        workspace,
        &repo.url,
        chatops_ctx,
        true,
        AlertCategory::ForkSetupPending,
        &err,
        RecoveryFailureClass::Transient,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert_state::AlertState;
    use crate::chatops::ChatOpsBackend;
    use crate::config::SecretSource;
    use std::sync::Mutex;

    /// Scripted [`ForkOps`] whose reachability + creation outcomes are driven
    /// by test state, so the fork-pending re-attempt is exercised without real
    /// network. Mirrors the run.rs `FakeForkOps` shape.
    struct ScriptedForkOps {
        reachable: Mutex<std::collections::HashSet<String>>,
        create_fails_with: Option<String>,
        make_reachable_on_create: Option<String>,
        identity_on_create: Option<String>,
    }

    #[async_trait::async_trait]
    impl ForkOps for ScriptedForkOps {
        fn fork_reachable(&self, fork_url: &str) -> bool {
            self.reachable.lock().unwrap().contains(fork_url)
        }
        async fn create_fork(
            &self,
            _upstream_owner: &str,
            _upstream_repo: &str,
            _token: &str,
        ) -> anyhow::Result<Option<String>> {
            if let Some(msg) = &self.create_fails_with {
                return Err(anyhow!("{msg}"));
            }
            if let Some(fork) = &self.make_reachable_on_create {
                self.reachable.lock().unwrap().insert(fork.clone());
            }
            Ok(self.identity_on_create.clone())
        }
    }

    struct RecordingChatOps {
        posts: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl ChatOpsBackend for RecordingChatOps {
        fn provider_name(&self) -> &'static str {
            "recording"
        }
        fn is_experimental(&self) -> bool {
            true
        }
        async fn post_question(
            &self,
            _channel: &str,
            _change: &str,
            _question: &str,
        ) -> anyhow::Result<String> {
            unreachable!()
        }
        async fn poll_thread_for_human_reply(
            &self,
            _channel: &str,
            _handle: &str,
        ) -> anyhow::Result<Option<crate::chatops::HumanReply>> {
            unreachable!()
        }
        async fn post_notification(&self, channel: &str, text: &str) -> anyhow::Result<()> {
            self.posts
                .lock()
                .unwrap()
                .push((channel.to_string(), text.to_string()));
            Ok(())
        }
    }

    fn fork_github() -> GithubConfig {
        GithubConfig {
            token_env: "AUTOCODER_FORK_PENDING_TEST_UNSET".into(),
            token: Some(SecretSource::Inline {
                value: "inline-fork-pat".into(),
            }),
            owner_tokens: None,
            fork_owner: Some("mu".into()),
            recreate_fork_on_reinit: false,
            command_authorization: Default::default(),
        }
    }

    fn repo() -> RepositoryConfig {
        RepositoryConfig {
            forge: None,
            url: "git@github.com:orgA/a.git".into(),
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
            octopus_guide: None,
            sandbox: None,
        }
    }

    fn ctx(backend: Arc<RecordingChatOps>) -> ChatOpsContext {
        ChatOpsContext {
            chatops: backend,
            channel: "C_FORK".into(),
            start_work_enabled: true,
            failure_alerts_enabled: true,
            pr_opened_enabled: true,
        }
    }

    /// 3.2: a transient re-attempt that succeeds returns `Ready` (the caller
    /// resumes normal polling) and emits no alert.
    #[tokio::test]
    async fn transient_reattempt_success_returns_ready() {
        let (_t, paths) = crate::testing::test_daemon_paths();
        let ws = paths.cache.join("workspaces").join("ws");
        let backend = Arc::new(RecordingChatOps {
            posts: Mutex::new(Vec::new()),
        });
        // The fork is already reachable now (the earlier startup blip cleared).
        let ops = ScriptedForkOps {
            reachable: Mutex::new(
                ["git@github.com:mu/a.git".to_string()].into_iter().collect(),
            ),
            create_fails_with: None,
            make_reachable_on_create: None,
            identity_on_create: None,
        };
        let step = run_fork_pending_iteration(
            &paths,
            &ws,
            &repo(),
            &fork_github(),
            Some(&ctx(backend.clone())),
            &ops,
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(step, ForkPendingStep::Ready);
        assert!(
            backend.posts.lock().unwrap().is_empty(),
            "success must not alert"
        );
    }

    /// 3.2 / 3.3: a still-transient re-attempt returns `RetryTransient` and
    /// posts exactly one throttled alert naming the repo as fork-pending; a
    /// second call within the throttle window stays silent (no per-iteration
    /// spam).
    #[tokio::test]
    async fn transient_reattempt_failure_alerts_once_then_throttles() {
        let (_t, paths) = crate::testing::test_daemon_paths();
        let ws = paths.cache.join("workspaces").join("ws");
        let backend = Arc::new(RecordingChatOps {
            posts: Mutex::new(Vec::new()),
        });
        let make_ops = || ScriptedForkOps {
            reachable: Mutex::new(std::collections::HashSet::new()),
            // Unknown POST error → classifier defaults to transient.
            create_fails_with: Some("simulated transient blip".into()),
            make_reachable_on_create: None,
            identity_on_create: None,
        };
        let c = ctx(backend.clone());
        for _ in 0..2 {
            let step = run_fork_pending_iteration(
                &paths,
                &ws,
                &repo(),
                &fork_github(),
                Some(&c),
                &make_ops(),
                Duration::from_millis(50),
                Duration::from_millis(1),
            )
            .await;
            assert_eq!(step, ForkPendingStep::RetryTransient);
        }
        let posts = backend.posts.lock().unwrap();
        assert_eq!(
            posts.len(),
            1,
            "fork-pending alert must throttle: one post across two failed attempts"
        );
        assert_eq!(posts[0].0, "C_FORK");
        assert!(
            posts[0].1.contains("fork setup pending"),
            "alert names the repo as fork-pending: {}",
            posts[0].1
        );
        // The throttle timestamp is persisted under the ForkSetupPending
        // category so the next iteration stays silent.
        let state = AlertState::load_or_default(&paths, &ws);
        assert!(state.alerts.contains_key(&AlertCategory::ForkSetupPending));
    }

    /// 3.2: a re-attempt that hits a PERMANENT cause (the upstream was renamed
    /// while pending → identity mismatch) flips to the permanent path: returns
    /// `Permanent` and posts the one-shot restart-or-reload remedy alert.
    #[tokio::test]
    async fn pending_reattempt_permanent_cause_flips_to_permanent() {
        let (_t, paths) = crate::testing::test_daemon_paths();
        let ws = paths.cache.join("workspaces").join("ws");
        let backend = Arc::new(RecordingChatOps {
            posts: Mutex::new(Vec::new()),
        });
        let ops = ScriptedForkOps {
            reachable: Mutex::new(std::collections::HashSet::new()),
            create_fails_with: None,
            make_reachable_on_create: None,
            // GitHub returns the pre-rename fork name → identity mismatch.
            identity_on_create: Some("mu/old-a".into()),
        };
        let step = run_fork_pending_iteration(
            &paths,
            &ws,
            &repo(),
            &fork_github(),
            Some(&ctx(backend.clone())),
            &ops,
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(step, ForkPendingStep::Permanent);
        let posts = backend.posts.lock().unwrap();
        assert_eq!(posts.len(), 1, "permanent flip posts one remedy alert");
        let lc = posts[0].1.to_lowercase();
        assert!(
            lc.contains("restart autocoder") && lc.contains("autocoder reload"),
            "permanent alert carries the restart-or-reload remedy: {}",
            posts[0].1
        );
    }
}
