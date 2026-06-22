//! The `send it` verb cascade and its audit/survey/issue-candidate/revision
//! state-machine logic, extracted from the catch-all `operator_commands`
//! module. This is a child module of `operator_commands`, so the methods here
//! reach `OperatorCommandDispatcher`'s private state fields
//! (`audit_thread_state_dir`, `revision_thread_state_dir`,
//! `brownfield_survey_enabled`, `workspace_resolver`) directly.
//!
//! The verb has FOUR valid thread contexts, consulted in the canon-mandated
//! order — audit → brownfield-survey → issue-candidate → spec-revision —
//! before the canonical untracked-thread refusal. That order is fixed by the
//! `chatops-manager` spec (`Inbound listener routes send it to ...`
//! requirements); the extraction preserves it EXACTLY.

use super::{
    ActionSubmitter, OperatorCommandDispatcher, RepoIdentity, SEND_IT_REFUSE_STALE,
    SEND_IT_REFUSE_UNTRACKED,
};

impl OperatorCommandDispatcher {
    /// Handle the `send it` verb. The verb has FOUR valid thread contexts,
    /// consulted in this order (a `thread_ts` resolves to at most one record
    /// across the four sets):
    ///
    ///   1. Audit-notification thread — `read_state` against
    ///      `<state_root>/audit-threads/<thread_ts>.json`. The
    ///      canonical four-case decision tree (untracked / stale /
    ///      already-acted / fresh-and-open) applies.
    ///   2. Brownfield-survey lifecycle thread (a29) —
    ///      [`try_send_it_on_survey`], matching by `thread_ts`. On a
    ///      fresh-and-open match the dispatcher submits
    ///      `queue_brownfield_batch_request` AND replies with the queue
    ///      confirmation.
    ///   3. Issue-candidate thread (a010) —
    ///      [`try_send_it_on_issue_candidate`], matching by `thread_ts`. On
    ///      a posted candidate the dispatcher submits
    ///      `promote_issue_candidate` AND replies with the write-and-queue
    ///      confirmation; on an already-promoted candidate it replies that
    ///      no new action was taken AND submits nothing.
    ///   4. Spec-revision thread (a03) — [`try_send_it_on_revision`].
    ///
    /// Audit lookup runs first; the survey, issue-candidate, AND revision
    /// lookups are the fallbacks in that order. If NONE matches, the
    /// dispatcher posts the canonical untracked-thread refusal, whose text
    /// names all four contexts.
    pub(super) async fn dispatch_send_it_on_audit(
        &self,
        thread_ts: &str,
        repositories: &[RepoIdentity],
        submitter: &dyn ActionSubmitter,
    ) -> String {
        use crate::audits::threads::{
            AuditThreadStatus, read_state, write_state,
        };
        let state_root = self.audit_thread_state_dir.as_path();
        let mut state = match read_state(state_root, thread_ts) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // No audit thread matched — try the survey-thread, then the
                // issue-candidate, then the revision-thread fallback before
                // refusing (canon's context order: audit → survey →
                // issue-candidate → spec-revision).
                if let Some(reply) = self
                    .try_send_it_on_survey(thread_ts, repositories, submitter)
                    .await
                {
                    return reply;
                }
                if let Some(reply) = self
                    .try_send_it_on_issue_candidate(thread_ts, repositories, submitter)
                    .await
                {
                    return reply;
                }
                if let Some(reply) = self.try_send_it_on_revision(thread_ts, submitter).await {
                    return reply;
                }
                return SEND_IT_REFUSE_UNTRACKED.to_string();
            }
            Err(e) => {
                tracing::warn!(
                    thread_ts = %thread_ts,
                    "audit-thread state read failed; treating as untracked: {e:#}"
                );
                if let Some(reply) = self
                    .try_send_it_on_survey(thread_ts, repositories, submitter)
                    .await
                {
                    return reply;
                }
                if let Some(reply) = self
                    .try_send_it_on_issue_candidate(thread_ts, repositories, submitter)
                    .await
                {
                    return reply;
                }
                if let Some(reply) = self.try_send_it_on_revision(thread_ts, submitter).await {
                    return reply;
                }
                return SEND_IT_REFUSE_UNTRACKED.to_string();
            }
        };

        let age = chrono::Utc::now() - state.posted_at;
        if age > chrono::Duration::days(7) {
            return SEND_IT_REFUSE_STALE.to_string();
        }

        match state.status {
            AuditThreadStatus::Open | AuditThreadStatus::TriageFailed => {
                // Fresh request OR a retry after a prior failed attempt.
                // Both transition into TriagePending; the polling loop
                // drains the queue on its next iteration.
                let resp = submitter
                    .submit(serde_json::json!({
                        "action": "trigger_audit_action",
                        "thread_ts": thread_ts,
                    }))
                    .await;
                if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    return format!("✗ could not schedule triage: {err}");
                }
                state.status = AuditThreadStatus::TriagePending;
                state.reason = None;
                if let Err(e) = write_state(state_root, &state) {
                    tracing::warn!(
                        thread_ts = %thread_ts,
                        "failed to flip audit-thread state to TriagePending: {e:#}"
                    );
                }
                // The polling cadence varies per repo; the response shape
                // also carries `poll_interval_sec` so we can name an
                // estimate in the reply if the daemon told us one.
                let poll_clause = resp
                    .get("poll_interval_sec")
                    .and_then(|v| v.as_u64())
                    .map(|s| format!(" (~{s}s)"))
                    .unwrap_or_default();
                format!(
                    "✓ Triage scheduled for {audit_type} on {repo_url}. The next polling iteration will run it{poll_clause}.",
                    audit_type = state.audit_type,
                    repo_url = state.repo_url,
                )
            }
            AuditThreadStatus::Acted | AuditThreadStatus::TriagePending => {
                format!(
                    "✗ This audit thread is already {status}. No new action taken.",
                    status = state.status.label(),
                )
            }
        }
        // The threads module's notes prefix is unused here — `read_state`
        // returns the on-disk truth and the dispatcher never invents one.
    }

    /// Look up the operator-replied `thread_ts` against every known
    /// repo's brownfield-survey states (a29). On a unique match
    /// AND `status == Pending`, submits the `queue_brownfield_batch_request`
    /// action AND returns `Some(reply)`. On a match where the survey
    /// is already InProgress / Completed, returns the no-op reply.
    /// Returns `None` when no survey matches — caller falls through to
    /// the canonical untracked-thread refusal.
    async fn try_send_it_on_survey(
        &self,
        thread_ts: &str,
        repositories: &[RepoIdentity],
        submitter: &dyn ActionSubmitter,
    ) -> Option<String> {
        if !self.brownfield_survey_enabled {
            return None;
        }
        let resolver = self.workspace_resolver.as_ref();
        let mut matched: Option<(String, crate::state::brownfield_survey::BrownfieldSurveyState)> =
            None;
        'outer: for repo in repositories {
            let ws = match resolver {
                Some(f) => match f(&repo.url) {
                    Some(p) => p,
                    None => continue,
                },
                None => repo.workspace_path.clone(),
            };
            let surveys = match crate::state::brownfield_survey::list_surveys_by_mtime(&ws) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for (request_id, _) in surveys {
                if let Ok(Some(state)) =
                    crate::state::brownfield_survey::read_state(&ws, &request_id)
                    && state.thread_ts == thread_ts
                {
                    matched = Some((repo.url.clone(), state));
                    break 'outer;
                }
            }
        }
        let (repo_url, survey_state) = matched?;
        match survey_state.status {
            crate::state::brownfield_survey::SurveyStatus::InProgress
            | crate::state::brownfield_survey::SurveyStatus::Completed => {
                return Some(format!(
                    "✗ send it: a brownfield batch is already {status} for survey {request_id}.",
                    status = survey_state.status.label().replace('_', " "),
                    request_id = survey_state.request_id,
                ));
            }
            crate::state::brownfield_survey::SurveyStatus::Pending => {}
        }
        let payload = serde_json::json!({
            "action": "queue_brownfield_batch_request",
            "url": repo_url,
            "survey_request_id": survey_state.request_id,
            "channel": survey_state.channel,
            "thread_ts": survey_state.thread_ts,
        });
        let resp = submitter.submit(payload).await;
        if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            let err = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("(no error message)");
            return Some(format!("✗ send it: could not enqueue batch: {err}"));
        }
        Some(format!(
            "✓ Queued {n} capability spec generations. The first will start on the next iteration.",
            n = survey_state.items.len()
        ))
    }

    /// a010: look up the operator-replied `thread_ts` against the stored
    /// issue-candidate set (the dispatcher's THIRD `send it` context). On a
    /// match whose candidate is still `Posted`, submits the
    /// `promote_issue_candidate` control-socket action carrying the candidate
    /// identity AND the originating `channel`/`thread_ts`, AND returns the
    /// write-and-queue confirmation. On a match whose candidate is already
    /// `Promoted`, returns the already-promoted reply AND submits nothing
    /// (idempotent). Returns `None` when no candidate matches — the caller
    /// falls through to `try_send_it_on_revision` and ultimately the canonical
    /// untracked-thread refusal.
    ///
    /// The candidate store is global under the daemon state root (the same
    /// root the dispatcher holds as `audit_thread_state_dir`), keyed by
    /// candidate id — so, like the revision lookup, the match is found by
    /// scanning that single store, NOT by walking the per-repo workspaces.
    /// `_repositories` is accepted for call-shape symmetry with the survey
    /// lookup; the candidate carries its own `repo_url`, so it is unused here.
    async fn try_send_it_on_issue_candidate(
        &self,
        thread_ts: &str,
        _repositories: &[RepoIdentity],
        submitter: &dyn ActionSubmitter,
    ) -> Option<String> {
        use crate::lanes::ingestion::{CandidateStatus, find_candidate_by_thread};
        let state_root = self.audit_thread_state_dir.as_path();
        let candidate = find_candidate_by_thread(state_root, thread_ts)?;
        match candidate.status {
            CandidateStatus::Promoted => Some(format!(
                "✗ send it: issue candidate `{slug}` is already promoted. No new action taken.",
                slug = candidate.slug,
            )),
            CandidateStatus::Posted => {
                // `find_candidate_by_thread` matches only candidates with
                // `thread_ts: Some`, AND `post_candidate` sets `channel: Some`
                // whenever `thread_ts` is `Some`, so a matched Posted candidate
                // normally carries both. A `None` on either is a degenerate
                // record (hand-edited / partially-written state); surface it as
                // a refusal with a warn rather than submitting an empty string
                // — the handler's `require_str` checks presence, not
                // non-emptiness, so an empty channel would otherwise pass
                // through silently.
                let (Some(channel), Some(thread_ts)) =
                    (candidate.channel.clone(), candidate.thread_ts.clone())
                else {
                    tracing::warn!(
                        candidate = %candidate.id,
                        slug = %candidate.slug,
                        channel_present = candidate.channel.is_some(),
                        thread_ts_present = candidate.thread_ts.is_some(),
                        "issue-candidate send it: matched candidate is missing its channel/thread_ts; refusing to promote with an empty identifier"
                    );
                    return Some(format!(
                        "✗ send it: issue candidate `{slug}` is missing its originating channel/thread; cannot promote. Re-trigger ingestion to repost it.",
                        slug = candidate.slug,
                    ));
                };
                let payload = serde_json::json!({
                    "action": "promote_issue_candidate",
                    "url": candidate.repo_url,
                    "candidate_id": candidate.id,
                    "channel": channel,
                    "thread_ts": thread_ts,
                });
                let resp = submitter.submit(payload).await;
                if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let err = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no error message)");
                    return Some(format!("✗ send it: could not promote candidate: {err}"));
                }
                // The handler is idempotent: a race that promoted the
                // candidate between the match AND the submit reports
                // `already_promoted`. Honour that wording when it does.
                if resp
                    .get("already_promoted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return Some(format!(
                        "✗ send it: issue candidate `{slug}` is already promoted. No new action taken.",
                        slug = candidate.slug,
                    ));
                }
                let slug = resp
                    .get("slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&candidate.slug);
                Some(format!(
                    "✓ Promoted issue candidate `{slug}` — wrote issues/{slug}/ AND queued it for the issues lane."
                ))
            }
        }
    }

    /// a03: look up the operator-replied `thread_ts` against the stored
    /// `RevisionThreadState` set (the dispatcher's FOURTH `send it` context).
    /// On a match this fires the spec-revision executor (`revision_execute`)
    /// for the change AND returns `Some(reply)`. A thread whose status is
    /// already `Acted` (a prior `send it` opened a PR) returns the
    /// already-acted reply WITHOUT re-running the executor. Returns `None`
    /// when no revision thread matches — the caller falls through to the
    /// canonical untracked-thread refusal.
    ///
    /// The state files are keyed by `thread_ts` (mirroring the audit-thread
    /// store), so the lookup is a direct read — `thread_ts` resolves to at
    /// most one record across the four contexts.
    async fn try_send_it_on_revision(
        &self,
        thread_ts: &str,
        submitter: &dyn ActionSubmitter,
    ) -> Option<String> {
        use crate::revision_thread::{RevisionThreadStatus, read_state};
        let state = match read_state(&self.revision_thread_state_dir, thread_ts) {
            Ok(Some(s)) => s,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(
                    thread_ts = %thread_ts,
                    "revision-thread state read failed; treating as untracked: {e:#}"
                );
                return None;
            }
        };
        if state.status == RevisionThreadStatus::Acted {
            return Some(format!(
                "✓ send it: a PR has already been opened for the revision of `{change}`. Review/merge that PR, or reply here to discuss further.",
                change = state.change_slug,
            ));
        }
        let payload = serde_json::json!({
            "action": "revision_execute",
            "url": state.repo_url,
            "change": state.change_slug,
            "channel": state.channel,
            "thread_ts": state.thread_ts,
        });
        let resp = submitter.submit(payload).await;
        if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            let err = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("(no error message)");
            return Some(format!("✗ send it: could not start the spec revision: {err}"));
        }
        Some(format!(
            "✓ Revising `{change}` along the discussed direction. autocoder will re-run the [in] and [canon] gates AND reply in this thread with the PR link (or the remaining contradiction).",
            change = state.change_slug,
        ))
    }
}
