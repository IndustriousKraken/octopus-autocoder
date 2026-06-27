use super::*;

/// Startup logging + jitter wait. Returns true if cancellation arrived
/// during the jitter sleep (the caller then returns). Extracted from
/// `run_with_hooks` (a68 function-size split).
pub(crate) async fn log_startup_and_jitter(
    repo: &ArcSwap<RepositoryConfig>,
    paths: &DaemonPaths,
    startup_jitter_max_secs: u64,
    cancel: &CancellationToken,
) -> bool {
    {
        let initial = repo.load();
        let workspace = workspace::resolve_path(paths, initial.as_ref());
        tracing::info!(
            url = initial.url.as_str(),
            workspace = %workspace.display(),
            poll_interval_sec = initial.poll_interval_sec,
            "starting polling loop"
        );
    }

    // Startup jitter: each task waits a uniformly-random duration in
    // `[0, startup_jitter_max_secs]` before its first iteration. Without
    // this, N concurrent polling tasks all fire `git fetch` at process
    // start within the same millisecond, which an IDS can flag as a
    // port-scan / scraping signature. Cancellation is honoured during
    // the wait, matching the inter-iteration sleep's contract.
    let startup_jitter_secs = pick_startup_jitter_secs(startup_jitter_max_secs);
    {
        let initial = repo.load();
        tracing::info!(
            url = initial.url.as_str(),
            startup_jitter_secs,
            "polling task for {} will wait {startup_jitter_secs}s before first iteration",
            initial.url
        );
    }
    if startup_jitter_secs > 0 {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                tracing::info!(url = %repo.load().url, "polling loop exiting");
                return true;
            }
            () = sleep(Duration::from_secs(startup_jitter_secs)) => {}
        }
    }
    false
}

/// Workspace-cache bookkeeping + LRU eviction (a65), run on the blocking
/// pool. Best-effort. Extracted from `run_with_hooks` (a68 split).
pub(crate) async fn run_workspace_cache_eviction(
    paths: &Arc<DaemonPaths>,
    workspace: &Path,
    cache_holder: &CacheHolder,
) {
    // Workspace-cache bookkeeping + LRU eviction (a65). This repo is
    // about to do work, so (1) record its workspace as used-now (the
    // freshest timestamp, so it is never the oldest candidate) and
    // (2) if `cache.workspaces_max_gb` is set AND the cache is over
    // budget, evict least-recently-used IDLE workspaces to stay under
    // the cap. The current workspace AND any busy-marked workspace are
    // never evicted; eviction is best-effort and never blocks work.
    // The cap is read from the hot-swappable holder here so a reload's
    // new value takes effect from this iteration onward.
    //
    // The pass can still do heavy synchronous filesystem work:
    // `enforce_cap` measures THIS repo's workspace fresh (an idle
    // workspace's size is reused from the `<state>/workspace-sizes/`
    // cache, so the pass does not re-walk every workspace every tick),
    // and an over-cap eviction's `remove_dir_all` can touch hundreds
    // of thousands of files for a large workspace (a Rust `target/`, a
    // JS `node_modules/`) and block for seconds. Run it on the
    // blocking thread pool via `spawn_blocking` so it never stalls the
    // tokio worker thread — and with it the other repos' polling
    // tasks, the control socket, and the chatops listener sharing the
    // runtime. Awaiting keeps the original "before any work" ordering;
    // the worker thread is parked at the await, not blocked.
    if let Some(current_basename) = workspace.file_name().and_then(|n| n.to_str()) {
        let cap_gb = cache_holder.load().workspaces_max_gb;
        let paths_for_cache = paths.clone();
        let basename = current_basename.to_string();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            crate::workspace_cache::record_last_used(&paths_for_cache, &basename);
            crate::workspace_cache::enforce_cap(&paths_for_cache, cap_gb, &basename);
        })
        .await
        {
            // A join error means the blocking closure panicked; the
            // pass is best-effort, so log and continue the iteration.
            tracing::warn!(
                "workspace-cache: eviction pass did not complete (iteration continues): {e}"
            );
        }
    }
}

/// Per-iteration state-file housekeeping: prune stale audit-thread,
/// proposal-request, and changelog-request entries (>7 days). Best-effort.
/// Extracted from `run_with_hooks` (a68 split).
pub(crate) fn run_state_housekeeping(paths: &DaemonPaths) {
    // Audit-thread state housekeeping runs first: prune any audit-
    // thread state files older than 7 days regardless of status, so
    // the audit-threads directory stays bounded. Best-effort; a
    // failure is logged and the iteration continues.
    let audit_state_root = crate::audits::threads::default_state_root(paths);
    match crate::audits::threads::prune_stale_entries(&audit_state_root, chrono::Duration::days(7))
    {
        Ok(0) => {}
        Ok(n) => tracing::debug!(
            count = n,
            "audit-threads prune removed {n} stale entry(ies)"
        ),
        Err(e) => tracing::warn!("audit-threads prune failed (iteration continues): {e:#}"),
    }

    // Same housekeeping for proposal-request state files (per
    // `chat-request-triage`). Stale entries (>7 days) are removed
    // regardless of status so the directory stays bounded.
    let proposal_state_root = crate::proposal_requests::default_state_root(paths);
    match crate::proposal_requests::prune_stale_entries(
        &proposal_state_root,
        chrono::Duration::days(7),
    ) {
        Ok(0) => {}
        Ok(n) => tracing::debug!(
            count = n,
            "proposal-requests prune removed {n} stale entry(ies)"
        ),
        Err(e) => tracing::warn!("proposal-requests prune failed (iteration continues): {e:#}"),
    }

    // Same housekeeping for changelog-request state files (per
    // `a06-chat-driven-changelog`). Stale entries (>7 days) are
    // removed regardless of status so the directory stays bounded.
    let changelog_state_root = crate::changelog_requests::default_state_root(paths);
    match crate::changelog_requests::prune_stale_entries(
        &changelog_state_root,
        chrono::Duration::days(7),
    ) {
        Ok(0) => {}
        Ok(n) => tracing::debug!(
            count = n,
            "changelog-requests prune removed {n} stale entry(ies)"
        ),
        Err(e) => tracing::warn!("changelog-requests prune failed (iteration continues): {e:#}"),
    }
}

/// Drain the audit-triage, proposal-request, and changelog-request queues.
/// Each is best-effort; an error is logged and never aborts the iteration.
/// Extracted from `run_with_hooks` (a68 split).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drain_chat_and_triage_queues(
    paths: &DaemonPaths,
    workspace: &Path,
    snapshot_ref: &RepositoryConfig,
    executor: &dyn Executor,
    github_snap: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    pending_triages: &std::sync::Mutex<Vec<String>>,
    pending_proposal_requests: &std::sync::Mutex<Vec<crate::control_socket::ProposalRequest>>,
    pending_changelog_requests: &std::sync::Mutex<Vec<crate::control_socket::ChangelogRequest>>,
) {
    // Drain the per-repo triage queue (audit-reply-acts `send it`).
    // Triage runs BEFORE the rebuild check and the pending-change
    // walk so an operator's `send it` always gets attention this
    // iteration. Failures inside `process_audit_triages` are logged
    // and never abort the surrounding iteration.
    let triage_thread_tses: Vec<String> = {
        let mut g = pending_triages.lock().unwrap();
        std::mem::take(&mut *g)
    };
    if !triage_thread_tses.is_empty()
        && let Err(error) = process_audit_triages(
            paths,
            workspace,
            snapshot_ref,
            executor,
            github_snap,
            chatops_ctx,
            &triage_thread_tses,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            "audit-triage processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain the per-repo proposal-request queue (chat-request-triage
    // `propose`). Same placement contract as the audit-triage drain
    // above: runs BEFORE the rebuild check and the pending-change
    // walk so an operator's `propose` always gets attention this
    // iteration. Failures inside `process_proposal_requests` are
    // logged and never abort the surrounding iteration.
    let proposal_requests_batch: Vec<crate::control_socket::ProposalRequest> = {
        let mut g = pending_proposal_requests.lock().unwrap();
        std::mem::take(&mut *g)
    };
    if !proposal_requests_batch.is_empty()
        && let Err(error) = process_proposal_requests(
            paths,
            workspace,
            snapshot_ref,
            executor,
            github_snap,
            chatops_ctx,
            &proposal_requests_batch,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            "chat-triage processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain the per-repo changelog-request queue
    // (`a06-chat-driven-changelog`). Runs immediately after the
    // proposal-request drain AND before the pending-change walk so an
    // operator's `@<bot> changelog ...` always gets attention this
    // iteration.
    let changelog_requests_batch: Vec<crate::control_socket::ChangelogRequest> = {
        let mut g = pending_changelog_requests.lock().unwrap();
        std::mem::take(&mut *g)
    };
    if !changelog_requests_batch.is_empty()
        && let Err(error) = crate::changelog_triage::process_changelog_requests(
            paths,
            workspace,
            snapshot_ref,
            executor,
            github_snap,
            chatops_ctx,
            &changelog_requests_batch,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            "changelog-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }
}

/// Drain at most one brownfield, scout, and spec-it request per iteration.
/// Extracted from `run_with_hooks` (a68 split).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drain_oss_and_scout_queues(
    paths: &DaemonPaths,
    workspace: &Path,
    snapshot_ref: &RepositoryConfig,
    executor: &dyn Executor,
    github_snap: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    pending_brownfield_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::BrownfieldRequest>,
    >,
    pending_scout_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::ScoutRequest>,
    >,
    pending_spec_it_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::SpecItRequest>,
    >,
    pending_proposal_requests: &Arc<std::sync::Mutex<Vec<crate::control_socket::ProposalRequest>>>,
) {
    // Drain at most ONE brownfield request per iteration (per the
    // a23 spec). The handler reverts the workspace on failure so a
    // sandboxed leak doesn't bleed into the standard change-
    // processing pass that follows. Failures are logged but never
    // abort the surrounding iteration.
    let brownfield_request: Option<crate::control_socket::BrownfieldRequest> = {
        let mut g = pending_brownfield_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = brownfield_request
        && let Err(error) = crate::polling::brownfield::process_pending_brownfield(
            paths,
            workspace,
            snapshot_ref,
            executor,
            github_snap,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            request_id = req.request_id.as_str(),
            "brownfield-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE scout request per iteration (a25). The
    // handler invokes the executor in scout mode (read-only
    // sandbox) AND persists the result to disk. Failures are
    // logged but never abort the surrounding iteration.
    let scout_request: Option<crate::control_socket::ScoutRequest> = {
        let mut g = pending_scout_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = scout_request
        && let Err(error) = crate::polling::scout::process_pending_scout(
            workspace,
            snapshot_ref,
            github_snap,
            executor,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            request_id = req.request_id.as_str(),
            "scout-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE spec-it request per iteration (a25). The
    // handler translates the scouted item into a `ProposalRequest`
    // AND pushes it onto the proposal-request queue for the
    // standard propose lifecycle to consume on the next iteration.
    let spec_it_request: Option<crate::control_socket::SpecItRequest> = {
        let mut g = pending_spec_it_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = spec_it_request
        && let Err(error) = crate::polling::spec_it::process_pending_spec_it(
            paths,
            workspace,
            snapshot_ref,
            chatops_ctx,
            pending_proposal_requests.clone(),
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            scout_request_id = req.scout_request_id.as_str(),
            item_id = req.item_id,
            "spec-it-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }
}

/// Drain at most one sync-upstream, brownfield-survey, and brownfield-batch
/// request per iteration, plus one in-progress batch item. Extracted from
/// `run_with_hooks` (a68 split).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drain_sync_survey_batch_queues(
    paths: &DaemonPaths,
    workspace: &Path,
    snapshot_ref: &RepositoryConfig,
    executor: &dyn Executor,
    github_snap: &GithubConfig,
    chatops_ctx: Option<&ChatOpsContext>,
    pending_sync_upstream_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::SyncUpstreamRequest>,
    >,
    pending_brownfield_survey_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::BrownfieldSurveyRequest>,
    >,
    pending_brownfield_batch_requests: &std::sync::Mutex<
        std::collections::VecDeque<crate::control_socket::BrownfieldBatchRequest>,
    >,
    pending_revision_requests: &crate::control_socket::RevisionRequestQueues,
    stuck_threshold_secs: u64,
) {
    // OSS-fork support (a26): drain at most ONE sync-upstream
    // request per iteration. The handler fetches the configured
    // upstream remote, rebases the workspace's base branch, AND
    // posts a thread reply summarizing the result OR naming
    // conflicting files. NEVER pushes — the operator decides when
    // to push to their fork.
    let sync_upstream_request: Option<crate::control_socket::SyncUpstreamRequest> = {
        let mut g = pending_sync_upstream_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = sync_upstream_request
        && let Err(error) = crate::polling::sync_upstream::process_pending_sync_upstream(
            workspace,
            snapshot_ref,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            request_id = req.request_id.as_str(),
            "sync-upstream-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE brownfield-survey request per iteration
    // (a29). The handler invokes the executor in survey mode
    // (read-only sandbox) AND persists the result to disk.
    let brownfield_survey_request: Option<crate::control_socket::BrownfieldSurveyRequest> = {
        let mut g = pending_brownfield_survey_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = brownfield_survey_request
        && let Err(error) = crate::polling::brownfield_survey::process_pending_brownfield_survey(
            workspace,
            snapshot_ref,
            executor,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            request_id = req.request_id.as_str(),
            "brownfield-survey-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE brownfield-batch action per iteration
    // (a29). The action only flips the survey state to InProgress
    // AND posts an ack; the actual item drain happens immediately
    // afterwards in `drain_next_brownfield_batch_item` so the
    // first item starts on the next iteration AS the spec
    // promises.
    let brownfield_batch_request: Option<crate::control_socket::BrownfieldBatchRequest> = {
        let mut g = pending_brownfield_batch_requests.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = brownfield_batch_request
        && let Err(error) = crate::polling::brownfield_batch::process_pending_brownfield_batch(
            paths,
            workspace,
            snapshot_ref,
            executor,
            github_snap,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            survey_request_id = req.survey_request_id.as_str(),
            "brownfield-batch-request processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain one in-progress batch item per iteration (a29). This
    // pass runs every iteration regardless of whether an action
    // arrived — once a survey is `InProgress` it owns the per-
    // iteration item-drain slot.
    if let Err(error) = crate::polling::brownfield_batch::drain_next_brownfield_batch_item(
        paths,
        workspace,
        snapshot_ref,
        executor,
        github_snap,
        chatops_ctx,
    )
    .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            "brownfield-batch item drain errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE spec-revision ADVISOR request per iteration (a03).
    // The handler reconstructs a read-only agentic session from the change
    // deltas, the canon, the marker's contradiction, AND the thread
    // transcript, then replies in the thread — writing nothing.
    let revision_advise_request: Option<crate::control_socket::RevisionAdviseRequest> = {
        let mut g = pending_revision_requests.advise.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = revision_advise_request
        && let Err(error) = crate::polling::revision_session::process_pending_revision_advise(
            workspace,
            snapshot_ref,
            chatops_ctx,
            &req,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            change = req.change_slug.as_str(),
            "revision-advise processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }

    // Drain at most ONE spec-revision EXECUTOR request per iteration (a03).
    // The handler runs a write-scoped session that revises the change's spec
    // deltas, re-runs the `[in]` / `[canon]` gates, AND opens a PR on a clean
    // re-gate (reporting the PR link in the thread); on a still-failing
    // re-gate it opens no PR and reports the remaining contradiction.
    let revision_execute_request: Option<crate::control_socket::RevisionExecuteRequest> = {
        let mut g = pending_revision_requests.execute.lock().unwrap();
        g.pop_front()
    };
    if let Some(req) = revision_execute_request
        && let Err(error) = crate::polling::revision_session::process_pending_revision_execute(
            paths,
            workspace,
            snapshot_ref,
            github_snap,
            chatops_ctx,
            &req,
            stuck_threshold_secs,
        )
        .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            change = req.change_slug.as_str(),
            "revision-execute processing errored for {}: {error:#}",
            snapshot_ref.url
        );
    }
}

/// Run the iteration's primary work: a rebuild pass or the standard
/// pending-change pass. Extracted from `run_with_hooks` (a68 split).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_iteration_work(
    paths: &DaemonPaths,
    workspace: &Path,
    snapshot_ref: &RepositoryConfig,
    executor: &dyn Executor,
    github_snap: &GithubConfig,
    reviewer_snap: Option<&CodeReviewer>,
    chatops_ctx: Option<&ChatOpsContext>,
    want_rebuild: bool,
    queued_audit_types: &std::sync::Mutex<Vec<QueuedAudit>>,
    stuck_threshold_secs: u64,
    perma_stuck_threshold: u32,
    max_changes_per_pr: u32,
    revision_cap: u32,
    human_revise_cap: Option<u32>,
    audit_registry: &AuditRegistry,
    audits_cfg: Option<&AuditsConfig>,
    audit_settings: &HashMap<String, AuditSettings>,
) {
    if want_rebuild {
        if let Err(error) = execute_rebuild_iteration(
            paths,
            workspace,
            snapshot_ref,
            github_snap,
            chatops_ctx,
            stuck_threshold_secs,
        )
        .await
        {
            tracing::error!(
                url = snapshot_ref.url.as_str(),
                "rebuild iteration failed for {}: {error:#}",
                snapshot_ref.url
            );
        }
    } else if let Err(error) = execute_one_pass(
        paths,
        workspace,
        snapshot_ref,
        executor,
        github_snap,
        reviewer_snap,
        chatops_ctx,
        stuck_threshold_secs,
        perma_stuck_threshold,
        max_changes_per_pr,
        revision_cap,
        human_revise_cap,
        audit_registry,
        audits_cfg,
        audit_settings,
        queued_audit_types,
    )
    .await
    {
        tracing::error!(
            url = snapshot_ref.url.as_str(),
            "polling iteration failed for {}: {error:#}",
            snapshot_ref.url
        );
    }
}
