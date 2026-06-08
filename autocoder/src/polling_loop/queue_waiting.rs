use super::*;

/// Invoke the periodic-audit scheduler at the post-queue-walk position.
/// Audit failures inside the scheduler are logged and never abort the
/// iteration — the caller continues to the push+PR step regardless.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_due_audits_after_queue(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    audit_registry: &AuditRegistry,
    audits_cfg: Option<&AuditsConfig>,
    audit_settings: &HashMap<String, AuditSettings>,
    chatops_ctx: Option<&ChatOpsContext>,
    queued_audit_types: &std::collections::HashSet<String>,
) {
    if let Err(e) = run_due_audits(
        paths,
        audit_registry,
        workspace,
        repo,
        audits_cfg,
        audit_settings,
        chatops_ctx,
        queued_audit_types,
    )
    .await
    {
        tracing::error!(
            url = %repo.url,
            "audit scheduler errored (iteration continues): {e:#}"
        );
    }
}

/// Iterate over the workspace's `list_waiting` changes. For each:
///   1. Read `.question.json` to recover the resume handle + thread coords.
///   2. Poll Slack for the first human reply.
///   3. If a reply has arrived: write `.answer.json`, delete
///      `.question.json`, call `executor.resume(handle, &reply.text)`,
///      classify the new outcome the same way `walk_queue` would.
///
/// Returns the list of changes that resumed-to-completed (i.e. were
/// archived this iteration). Failures during processing are logged and the
/// iteration moves to the next waiting change — they do NOT abort the
/// pass.
pub(crate) async fn process_waiting_changes(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsContext>,
    perma_stuck_threshold: u32,
    max_changes_per_pr: u32,
) -> Result<Vec<String>> {
    let ctx = match chatops_ctx {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };
    let waiting = queue::list_waiting(workspace)?;
    // Pre-flight archive-collision filter: a change with a dated archive
    // entry already on disk would fail at resume-archive time. Exclude
    // it, alert once (subject to 24h throttle), and proceed with the
    // rest. Same helper as the pending-side filter so behavior is
    // identical at both call sites.
    let waiting =
        apply_archive_collision_preflight(paths, workspace, repo, chatops_ctx, waiting).await;
    let mut resumed_archived: Vec<String> = Vec::new();

    for change in waiting {
        match process_one_waiting(
            paths,
            workspace,
            repo,
            executor,
            ctx,
            &change,
            perma_stuck_threshold,
        )
        .await
        {
            Ok(Some(archived)) => {
                resumed_archived.push(archived);
                if resumed_archived.len() as u32 >= max_changes_per_pr {
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(
                    url = repo.url.as_str(),
                    "waiting-change processing failed for `{change}`: {e:#}"
                );
            }
        }
    }
    Ok(resumed_archived)
}

/// Process a single waiting change. Returns `Ok(Some(name))` when the
/// change was resumed-to-completed-with-diff and archived (so the caller
/// adds it to the pass's processed list); `Ok(None)` for every other
/// outcome (still waiting, resumed-to-failed, resumed-to-AskUser again,
/// resumed-to-completed-no-diff).
async fn process_one_waiting(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    ctx: &ChatOpsContext,
    change: &str,
    perma_stuck_threshold: u32,
) -> Result<Option<String>> {
    let question = chatops::read_question_file(workspace, change)
        .with_context(|| format!("reading .question.json for `{change}`"))?;
    let reply = ctx
        .chatops
        .poll_thread_for_human_reply(&question.channel, &question.thread_ts)
        .await
        .with_context(|| format!("polling Slack thread for `{change}`"))?;
    let reply = match reply {
        Some(r) => r,
        None => return Ok(None),
    };

    // Persist the answer BEFORE removing the question, in the order
    // mandated by orchestrator-cli/spec.md "Resuming a change after an
    // answer arrives": write answer → delete question → call resume.
    let answer = AnswerPayload {
        answer: reply.text.clone(),
        answered_at: chrono::Utc::now(),
        answerer_user_id: reply.user_id.clone(),
    };
    chatops::write_answer_file(workspace, change, &answer)?;
    chatops::delete_question_file(workspace, change)?;

    let handle = ResumeHandle(question.resume_handle.clone());
    // Record the resumed change in the busy marker so chatops `status`
    // reflects this iteration's active work.
    busy_marker::update_change(paths, workspace, change);
    tracing::info!(
        url = %repo.url,
        change = %change,
        "starting work on change (resume)"
    );
    let outcome = executor.resume(handle, &reply.text).await;

    // After resume returns (any outcome), delete .answer.json so the
    // change reverts to a clean state regardless of the outcome.
    let _ = chatops::delete_answer_file(workspace, change);

    let (result, failure_reason): (ResumeDisposition, Option<String>) = match outcome {
        Err(e) => {
            tracing::error!("executor.resume errored on `{change}`: {e:#}");
            // A resume-side task error is closer to infrastructure than an
            // agent decision. Per spec, transient daemon-side errors do
            // NOT increment the counter; we treat resume errors the same.
            (ResumeDisposition::Errored, None)
        }
        Ok(ExecutorOutcome::Completed { .. }) => resume_completed(workspace, repo, change)?,
        Ok(ExecutorOutcome::AskUser {
            question: q2,
            resume_handle: rh2,
        }) => {
            // Agent asked another question. Post it and rotate the
            // question file. The change stays in the waiting set.
            escalate_to_chatops(paths, workspace, repo, ctx, change, &q2, rh2.0).await?;
            (ResumeDisposition::EscalatedAgain, None)
        }
        Ok(ExecutorOutcome::Failed { reason }) => {
            tracing::error!("resume of `{change}` returned Failed: {reason}");
            // .answer.json already deleted above. .question.json was
            // deleted before the resume call. The change reverts cleanly
            // to pending state for the next iteration.
            (ResumeDisposition::Failed, Some(reason))
        }
        Ok(ExecutorOutcome::PreconditionUnmet { reason }) => {
            // a74: surfaced only on the revise path today; the resume path is
            // out of scope. Treat it as a Failed-equivalent so the operator
            // sees the unhandled case rather than silent loss.
            tracing::error!(
                "resume of `{change}` returned PreconditionUnmet: {reason}"
            );
            (ResumeDisposition::Failed, Some(reason))
        }
        Ok(ExecutorOutcome::SpecNeedsRevision {
            unimplementable_tasks,
            revision_suggestion,
        }) => {
            resume_spec_needs_revision(
                paths,
                workspace,
                repo,
                ctx,
                change,
                unimplementable_tasks,
                revision_suggestion,
            )
            .await
        }
        Ok(ExecutorOutcome::IterationRequested { .. }) => {
            // a27a1: resume returning IterationRequested is unusual but
            // possible (e.g. the operator's answer pointed the agent at
            // additional work it can complete in another iteration).
            // Today's resume path doesn't have the WIP-commit + push
            // plumbing the pending arm has, AND the iteration cap is
            // enforced at the classifier which already produced this
            // variant. Treat it as a Failed-equivalent so the operator
            // sees the unhandled case rather than silent loss; the next
            // polling iteration will re-enter the change normally.
            tracing::warn!(
                url = %repo.url,
                change = %change,
                "resume returned IterationRequested; treating as Failed (resume-side iteration sequences not yet supported)"
            );
            (
                ResumeDisposition::Failed,
                Some(
                    "resume returned IterationRequested (unsupported on the resume path)"
                        .to_string(),
                ),
            )
        }
        Ok(ExecutorOutcome::Aborted { reason }) => {
            // a39: the resume's subprocess was killed by the daemon's
            // own SIGTERM cascade. The .question.json was deleted
            // before the resume call (above), so we cannot restore the
            // pre-resume waiting-on-answer state. The change is back
            // in pending state for the next iteration to retry from
            // the agent-q tip. We do NOT increment the failure counter
            // (operator initiated the shutdown) AND do NOT post a
            // chatops alert.
            tracing::info!(
                url = %repo.url,
                change = %change,
                "resume aborted by daemon shutdown: {reason}"
            );
            (ResumeDisposition::Aborted, None)
        }
    };

    // Counter book-keeping mirrors the pending path:
    //   - Archived → clear
    //   - Failed / CompletedNoDiff (transformed-to-Failed) → record + maybe perma-stuck
    //   - Errored / EscalatedAgain → leave the counter alone
    match (&result, failure_reason) {
        (ResumeDisposition::Archived, _) => {
            if let Err(e) = failure_state::clear(paths, workspace, change) {
                tracing::warn!(
                    url = %repo.url,
                    change = %change,
                    "failed to clear failure-state entry after resume archive: {e:#}"
                );
            }
        }
        (ResumeDisposition::Failed, Some(reason))
        | (ResumeDisposition::CompletedNoDiff, Some(reason)) => {
            handle_failure_counter(
                paths,
                workspace,
                repo,
                Some(ctx),
                change,
                &reason,
                perma_stuck_threshold,
            )
            .await;
        }
        _ => {}
    }

    tracing::info!(
        url = %repo.url,
        change = %change,
        outcome = result.label(),
        "change finished (resume)"
    );

    Ok(match result {
        ResumeDisposition::Archived => Some(change.to_string()),
        _ => None,
    })
}

/// Resume-path `SpecNeedsRevision` handling: write the marker, drop the
/// iteration-pending marker, and alert the operator. Extracted from
/// `process_one_waiting` (a68 function-size split).
async fn resume_spec_needs_revision(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    ctx: &ChatOpsContext,
    change: &str,
    unimplementable_tasks: Vec<UnimplementableTask>,
    revision_suggestion: String,
) -> (ResumeDisposition, Option<String>) {
    // Even on the resume path, the agent may decide a task is
    // unimplementable (e.g. the operator's answer revealed a
    // requirement outside the sandbox). Same treatment as the
    // pending path: write the marker, alert the operator, halt.
    // Question/answer files were already cleared above; the
    // marker is the new operator-action gate.
    tracing::warn!(
        url = %repo.url,
        change = %change,
        flagged = unimplementable_tasks.len(),
        "resume returned SpecNeedsRevision; writing marker and alerting operator"
    );
    let detail = SpecNeedsRevisionDetail {
        unimplementable_tasks: unimplementable_tasks.clone(),
        unarchivable_deltas: Vec::new(),
        revision_suggestion: revision_suggestion.clone(),
    };
    if let Err(e) = spec_revision::write_marker(workspace, change, &detail) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to write spec-needs-revision marker (resume): {e:#}"
        );
    }
    // a27a1: same lifecycle as the pending path — SpecNeedsRevision
    // terminates the iteration sequence; drop the marker.
    let basename_for_marker = workspace
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    if let Err(e) = crate::iteration_pending::remove_marker(paths, basename_for_marker, change) {
        tracing::warn!(
            url = %repo.url,
            change = %change,
            "failed to remove iteration-pending marker on SpecNeedsRevision (resume): {e:#}"
        );
    }
    maybe_post_spec_revision_alert(
        paths,
        Some(ctx),
        repo,
        change,
        &unimplementable_tasks,
        &revision_suggestion,
    )
    .await;
    (ResumeDisposition::SpecRevisionMarked, None)
}

/// Resume-path `Completed` handling: detect a no-op resume vs. a real diff,
/// then commit+archive. Extracted from `process_one_waiting` (a68 split).
fn resume_completed(
    workspace: &Path,
    repo: &RepositoryConfig,
    change: &str,
) -> Result<(ResumeDisposition, Option<String>)> {
    let r = {
        // The porcelain output here will include the .question.json
        // deletion (and possibly an .answer.json transient) that
        // autocoder itself just performed above. Those are
        // bookkeeping, not executor output, so they must not count
        // as "the executor modified the workspace."
        let dirty = git::status_porcelain(workspace)?;
        if !has_executor_changes(&dirty, change) {
            tracing::warn!(
                "resume of `{change}` returned Completed without modifying the workspace; marking Failed"
            );
            // The question/answer file shuffle is left in the working
            // tree for now; the next pass's startup dirty-check will
            // either auto-recover or skip. The .in-progress lock was
            // removed when the question was first posted, so the
            // change is already in pending state for retry.
            (
                ResumeDisposition::CompletedNoDiff,
                Some("agent reported Completed without modifying the workspace (resume)".into()),
            )
        } else {
            let subject = build_commit_subject(workspace, change)?;
            git::add_all(workspace)?;
            git::commit(workspace, &subject)?;
            let spec_root = crate::spec_root::SpecRoot::for_repo(repo, workspace);
            queue::archive_at(&spec_root, change)?;
            (ResumeDisposition::Archived, None)
        }
    };
    Ok(r)
}
