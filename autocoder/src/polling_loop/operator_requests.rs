//! Task-local handles to the operator-chatops-request queues, used by the
//! queue walk to YIELD a batch early when an operator request is waiting
//! (a71).
//!
//! The `send it` audit-triage, the `propose` chat-request, AND the
//! `changelog` request are drained at the TOP of each polling iteration
//! (before the change walk; see `loop_drive::drain_chat_and_triage_queues`).
//! But the walk processes a whole batch of pending changes — each a full
//! executor run — before the iteration ends, so a request that arrives
//! mid-batch would otherwise wait for the entire batch to complete. To bound
//! that latency to at most one in-flight change, the walk PEEKS these queues
//! between changes; when any is pending it ends the batch and returns, so the
//! next iteration's drain consumes the operator request within one
//! change-cycle. The walk NEVER drains — the iteration-top drain remains the
//! sole consumer.
//!
//! Rather than thread three `Arc<Mutex<…>>` handles through the deep
//! polling-loop call chain (`run_iteration_work` → `execute_one_pass` →
//! `run_pass_through_commits` → `walk_queue`), this follows the established
//! task-local pattern (`crate::lanes::gate`, `crate::preflight::…`): the
//! polling loop binds the context ONCE per iteration around its work future;
//! the walk reads it via [`current`]. A task that never called [`scope`] —
//! every test that does not opt in — sees `None`, so the walk never yields
//! (the pre-a71 full-batch behavior).

use crate::control_socket::{ChangelogRequest, ProposalRequest};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// The three operator-chatops-request queues the iteration-top drains
/// consume. Cloning is cheap (three `Arc` clones). The walk PEEKS these
/// via [`any_pending`](OperatorRequestQueues::any_pending) — a length check
/// only; it NEVER drains them.
#[derive(Clone)]
pub struct OperatorRequestQueues {
    /// `send it` audit-triage queue (`RepoTaskHandle::pending_triages`).
    pub triages: Arc<Mutex<Vec<String>>>,
    /// `propose` chat-request queue
    /// (`RepoTaskHandle::pending_proposal_requests`).
    pub proposal_requests: Arc<Mutex<Vec<ProposalRequest>>>,
    /// `changelog` request queue
    /// (`RepoTaskHandle::pending_changelog_requests`).
    pub changelog_requests: Arc<Mutex<Vec<ChangelogRequest>>>,
}

impl OperatorRequestQueues {
    /// True when any operator chatops request (`send it` triage, `propose`,
    /// OR `changelog`) is pending.
    ///
    /// Each queue's lock is held ONLY for the duration of its `.is_empty()`
    /// check — the temporary `MutexGuard` from `.lock().unwrap()` is dropped
    /// at the end of each `let` statement, before the next lock is taken — so
    /// the chatops listener thread (which enqueues under these same locks) is
    /// never blocked across other work. At most one lock is held at any
    /// instant.
    pub fn any_pending(&self) -> bool {
        let triages_pending = !self.triages.lock().unwrap().is_empty();
        if triages_pending {
            return true;
        }
        let proposals_pending = !self.proposal_requests.lock().unwrap().is_empty();
        if proposals_pending {
            return true;
        }
        !self.changelog_requests.lock().unwrap().is_empty()
    }
}

tokio::task_local! {
    /// Per-task operator-request-queue handles. `None` → the walk never
    /// yields on operator requests (the pre-a71 batch behavior, AND the
    /// default for any test that does not opt in via [`scope`]).
    static CTX: Option<OperatorRequestQueues>;
}

/// Run `fut` with the operator-request-queue handles bound for its
/// duration. The polling loop wraps each iteration's work future once.
pub fn scope<F>(ctx: Option<OperatorRequestQueues>, fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    CTX.scope(ctx, fut)
}

/// Snapshot the current task's operator-request-queue handles. `None` when
/// the surrounding task did not call [`scope`] (every non-opted-in test).
/// Cheap clone of three `Arc`s.
pub fn current() -> Option<OperatorRequestQueues> {
    CTX.try_with(|c| c.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_queues() -> OperatorRequestQueues {
        OperatorRequestQueues {
            triages: Arc::new(Mutex::new(Vec::new())),
            proposal_requests: Arc::new(Mutex::new(Vec::new())),
            changelog_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[tokio::test]
    async fn current_is_none_without_scope() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_is_none_when_scoped_none() {
        let seen = scope(None, async { current() }).await;
        assert!(seen.is_none(), "scoped None must read back as None");
    }

    #[tokio::test]
    async fn current_returns_scoped_queues() {
        let q = empty_queues();
        let seen = scope(Some(q.clone()), async { current() }).await;
        assert!(seen.is_some(), "scoped Some must read back as Some");
    }

    #[test]
    fn any_pending_false_for_empty_queues() {
        assert!(!empty_queues().any_pending());
    }

    #[test]
    fn any_pending_true_when_triage_queued() {
        let q = empty_queues();
        q.triages.lock().unwrap().push("T-thread".into());
        assert!(q.any_pending());
    }

    #[test]
    fn any_pending_true_when_proposal_queued() {
        let q = empty_queues();
        q.proposal_requests.lock().unwrap().push(ProposalRequest {
            request_id: "req-1".into(),
            channel: "C".into(),
            thread_ts: "T".into(),
            operator_user: "U".into(),
            request_text: "do thing".into(),
            submitted_at: chrono::Utc::now(),
        });
        assert!(q.any_pending());
    }

    #[test]
    fn any_pending_true_when_changelog_queued() {
        let q = empty_queues();
        q.changelog_requests.lock().unwrap().push(ChangelogRequest {
            request_id: "req-1".into(),
            repo_url: "git@github.com:o/r.git".into(),
            raw_args: "".into(),
            channel: "C".into(),
            lifecycle_thread_ts: "T".into(),
            submitted_at: chrono::Utc::now(),
        });
        assert!(q.any_pending());
    }
}
