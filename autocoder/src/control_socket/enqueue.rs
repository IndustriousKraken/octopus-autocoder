//! Chat-driven enqueue handlers for the control socket, plus the one shared
//! [`enqueue_request`] helper they are all built on (and the small
//! [`clear_state_files`] helper the two `queue_clear_*` actions share). Each
//! `handle_queue_*` / `handle_revision_*` handler supplies only the parts that
//! differ — its queue selector, its request-record constructor, its de-dup
//! predicate, AND its success ack — so the resolve-repo / look-up-handle /
//! de-dup / push / reply skeleton lives in exactly one place. The control-
//! socket response JSON is unchanged from the former per-handler bodies.
//!
//! These handlers are dispatched from the [`super::DISPATCH`] table; they are
//! re-exported at `super`'s path via `pub(crate) use enqueue::*` so the table
//! AND the sibling test module reach them at their original module path.
use super::*;

/// A pending-request queue the enqueue handlers push onto. Implemented for
/// both `Vec<T>` (the proposal / changelog queues, appended via `push`) AND
/// `VecDeque<T>` (every other request queue, appended via `push_back`) so the
/// shared [`enqueue_request`] helper drives either without caring which
/// container the live [`RepoTaskHandle`] uses. Both append in arrival order.
trait PendingQueue<T> {
    /// True when an entry already queued satisfies `pred` — the de-dup check
    /// each handler supplies.
    fn has_matching<F: Fn(&T) -> bool>(&self, pred: F) -> bool;
    /// Append `item` in arrival order.
    fn enqueue(&mut self, item: T);
}

impl<T> PendingQueue<T> for Vec<T> {
    fn has_matching<F: Fn(&T) -> bool>(&self, pred: F) -> bool {
        self.iter().any(pred)
    }
    fn enqueue(&mut self, item: T) {
        self.push(item);
    }
}

impl<T> PendingQueue<T> for std::collections::VecDeque<T> {
    fn has_matching<F: Fn(&T) -> bool>(&self, pred: F) -> bool {
        self.iter().any(pred)
    }
    fn enqueue(&mut self, item: T) {
        self.push_back(item);
    }
}

/// Shared body for the chat-driven enqueue handlers (`queue_proposal_request`,
/// `queue_changelog_request`, `queue_brownfield_request`, `queue_scout_request`,
/// `queue_spec_it_request`, `queue_sync_upstream_request`,
/// `queue_brownfield_survey_request`, `queue_brownfield_batch_request`, AND the
/// two `revision_*` actions). Each follows the same skeleton: resolve the repo,
/// build the in-memory request record (optionally loading an on-disk state
/// file), look up the repo's live polling-task handle queue, de-dup, push, AND
/// return the standard `{ok, url, ..., poll_interval_sec}` ack. The caller
/// supplies ONLY the parts that differ:
///
/// - `build` constructs the record from the resolved [`RepositoryConfig`]; it
///   returns `Err(reply)` to short-circuit with a specific error reply (e.g. a
///   missing on-disk state file) WITHOUT enqueuing.
/// - `select` picks the handle's pending queue (`|h| h.pending_x.clone()`).
/// - `is_dup` is the de-dup predicate against entries already queued.
/// - `ack` renders the success reply from the resolved repo, so each handler
///   keeps its exact response shape.
///
/// Error ordering matches the hand-written handlers it replaces — unknown repo
/// first, then `build` (state-file) errors, then the "no live polling task"
/// error — so the control-socket JSON stays byte-identical.
fn enqueue_request<Q, T>(
    state: &ControlState,
    url: &str,
    build: impl FnOnce(&RepositoryConfig) -> std::result::Result<T, Value>,
    select: impl FnOnce(&RepoTaskHandle) -> Arc<Mutex<Q>>,
    is_dup: impl Fn(&T) -> bool,
    ack: impl FnOnce(&RepositoryConfig) -> Value,
) -> Value
where
    Q: PendingQueue<T>,
{
    let repo = match find_repo(state, url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let record = match build(&repo) {
        Ok(r) => r,
        Err(reply) => return reply,
    };
    let queue = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(url).map(select)
    };
    let queue = match queue {
        Some(q) => q,
        None => {
            return json!({
                "ok": false,
                "error": format!(
                    "no live polling task for `{url}` (daemon may not have spawned it yet)"
                ),
            });
        }
    };
    {
        let mut g = queue.lock().unwrap();
        if !g.has_matching(&is_dup) {
            g.enqueue(record);
        }
    }
    ack(&repo)
}

/// Shared body for the `queue_clear_scout` / `queue_clear_survey` actions:
/// resolve the repo, resolve its workspace, run `clear`, AND return
/// `{ok, url, cleared}` — or a `{ok:false, error}` reply naming `kind` (e.g.
/// `"clearing scout state: ..."`). The two handlers differ only in which
/// per-workspace state directory `clear` wipes.
fn clear_state_files(
    parsed: &Value,
    state: &ControlState,
    kind: &str,
    clear: impl FnOnce(&Path) -> Result<usize>,
) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    let cleared = match clear(&workspace) {
        Ok(n) => n,
        Err(e) => {
            return json!({
                "ok": false,
                "error": format!("clearing {kind} state: {e:#}"),
            });
        }
    };
    json!({
        "ok": true,
        "url": url,
        "cleared": cleared,
    })
}

/// Queue a chat-driven proposal-request for the repo's next polling
/// iteration. The request was already persisted to disk as a
/// `ProposalRequestState` file by the chatops dispatcher; this handler's
/// job is to look up the repo's live polling-task handle, load the
/// state from disk, and push a `ProposalRequest` onto the handle's
/// `pending_proposal_requests` queue so the polling loop drains it.
///
/// On success returns `{ok: true, url, request_id, poll_interval_sec}`.
/// On any failure (unknown repo, missing state file, etc.) returns
/// `{ok: false, error}` and does NOT enqueue.
pub(crate) fn handle_queue_proposal_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    enqueue_request(
        state,
        &url,
        |_repo| {
            // Load the on-disk state file the chatops dispatcher just wrote.
            let state_root = crate::proposal_requests::default_state_root(&state.paths);
            let proposal_state =
                match crate::proposal_requests::read_state(&state_root, &url, &request_id) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return Err(json!({
                            "ok": false,
                            "error": format!(
                                "no proposal-request state file found for request_id `{request_id}` under repo `{url}`"
                            ),
                        }));
                    }
                    Err(e) => {
                        return Err(json!({
                            "ok": false,
                            "error": format!("reading proposal-request state: {e:#}")
                        }));
                    }
                };
            Ok(ProposalRequest {
                request_id: proposal_state.request_id.clone(),
                channel: proposal_state.channel.clone(),
                thread_ts: proposal_state.thread_ts.clone(),
                operator_user: proposal_state.operator_user.clone(),
                request_text: proposal_state.request_text.clone(),
                submitted_at: proposal_state.submitted_at,
            })
        },
        |h| h.pending_proposal_requests.clone(),
        // De-dup: if the same request_id is somehow queued twice (e.g.
        // chatops retried), keep only one entry.
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a chat-driven changelog request for the repo's next polling
/// iteration. The request was already persisted to disk as a
/// `ChangelogRequestState` file by the chatops dispatcher; this
/// handler's job is to look up the repo's live polling-task handle,
/// load the state from disk, and push a `ChangelogRequest` onto the
/// handle's `pending_changelog_requests` queue.
pub(crate) fn handle_queue_changelog_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    enqueue_request(
        state,
        &url,
        |_repo| {
            let state_root = crate::changelog_requests::default_state_root(&state.paths);
            let changelog_state =
                match crate::changelog_requests::read_state(&state_root, &url, &request_id) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return Err(json!({
                            "ok": false,
                            "error": format!(
                                "no changelog-request state file found for request_id `{request_id}` under repo `{url}`"
                            ),
                        }));
                    }
                    Err(e) => {
                        return Err(json!({
                            "ok": false,
                            "error": format!("reading changelog-request state: {e:#}")
                        }));
                    }
                };
            Ok(ChangelogRequest {
                request_id: changelog_state.request_id.clone(),
                repo_url: changelog_state.repo_url.clone(),
                raw_args: changelog_state.raw_args.clone(),
                channel: changelog_state.channel.clone(),
                lifecycle_thread_ts: changelog_state.lifecycle_thread_ts.clone(),
                submitted_at: changelog_state.submitted_at,
            })
        },
        |h| h.pending_changelog_requests.clone(),
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a chat-driven brownfield request for the repo's next polling
/// iteration. The request was already persisted by the chatops
/// dispatcher to `<workspace>/.state/brownfield_requests/<request_id>.json`
/// AND the spec-existence preflight is the dispatcher's job — this
/// handler just loads the per-workspace state file and pushes a
/// `BrownfieldRequest` onto the handle's `pending_brownfield_requests`
/// queue.
pub(crate) fn handle_queue_brownfield_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    enqueue_request(
        state,
        &url,
        |repo| {
            let workspace = crate::workspace::resolve_path(&state.paths, repo);
            let brownfield_state = match crate::state::brownfield_request::read_state(
                &workspace,
                &request_id,
            ) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    return Err(json!({
                        "ok": false,
                        "error": format!(
                            "no brownfield-request state file found for request_id `{request_id}` under workspace `{}`",
                            workspace.display()
                        ),
                    }));
                }
                Err(e) => {
                    return Err(json!({
                        "ok": false,
                        "error": format!("reading brownfield-request state: {e:#}"),
                    }));
                }
            };
            Ok(BrownfieldRequest {
                request_id: brownfield_state.request_id.clone(),
                repo_url: brownfield_state.repo_url.clone(),
                capability_name: brownfield_state.capability_name.clone(),
                guidance: brownfield_state.guidance.clone(),
                channel: brownfield_state.channel.clone(),
                thread_ts: brownfield_state.thread_ts.clone(),
                submitted_at: brownfield_state.submitted_at,
            })
        },
        |h| h.pending_brownfield_requests.clone(),
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a chat-driven scout request for the repo's next polling
/// iteration (a25). The request was already persisted by the chatops
/// dispatcher to
/// `<workspace>/.state/scout_runs/<request_id>.json` is NOT yet on
/// disk (the scout polling handler creates it AFTER the executor
/// pass); this handler enqueues an in-memory `ScoutRequest` carrying
/// every field the polling-iteration handler needs.
pub(crate) fn handle_queue_scout_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let guidance = parsed
        .get("guidance")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(ScoutRequest {
                request_id: request_id.clone(),
                repo_url: url.clone(),
                guidance: guidance.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_scout_requests.clone(),
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a chat-driven spec-it request (a25). Enqueues the scout's
/// `request_id` AND the operator-selected `item_id` plus optional
/// guidance; the polling-iteration handler resolves the item against
/// the on-disk `ScoutRunState` AND submits a fresh `ProposalRequest`.
pub(crate) fn handle_queue_spec_it_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let scout_request_id = match require_str(parsed, "scout_request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let item_id = match parsed.get("item_id").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return json!({"ok": false, "error": "missing `item_id` field"}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let guidance = parsed
        .get("guidance")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(SpecItRequest {
                repo_url: url.clone(),
                scout_request_id: scout_request_id.clone(),
                item_id,
                guidance: guidance.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_spec_it_requests.clone(),
        // spec-it requests are not de-duplicated — each selection enqueues.
        |_r| false,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Handle the `queue_sync_upstream_request` action (a26). Enqueues
/// a `SyncUpstreamRequest` onto the matched repo's handle; the
/// polling iteration's handler drains it at iteration start.
pub(crate) fn handle_queue_sync_upstream_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = parsed
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(SyncUpstreamRequest {
                request_id: request_id.clone(),
                repo_url: url.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_sync_upstream_requests.clone(),
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Handle the `queue_clear_scout` action (a25). Synchronously deletes
/// every `ScoutRunState` file in the matched repo's workspace AND
/// returns the count cleared. The chatops dispatcher posts the reply
/// using that count.
pub(crate) fn handle_queue_clear_scout(parsed: &Value, state: &ControlState) -> Value {
    clear_state_files(parsed, state, "scout", crate::state::scout_run::clear_all)
}

/// Queue a chat-driven brownfield-survey request for the repo's next
/// polling iteration (a29). The survey state file is NOT yet on disk
/// (the survey polling handler creates it AFTER the executor pass);
/// this handler enqueues an in-memory `BrownfieldSurveyRequest`
/// carrying every field the polling-iteration handler needs.
pub(crate) fn handle_queue_brownfield_survey_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let guidance = parsed
        .get("guidance")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(BrownfieldSurveyRequest {
                request_id: request_id.clone(),
                repo_url: url.clone(),
                guidance: guidance.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_brownfield_survey_requests.clone(),
        |r| r.request_id == request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "request_id": request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a chat-driven brownfield-batch request for the repo's next
/// polling iteration (a29). The referenced survey state file MUST
/// already exist; the polling handler loads it to begin the batch
/// drain.
pub(crate) fn handle_queue_brownfield_batch_request(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let survey_request_id = match require_str(parsed, "survey_request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(BrownfieldBatchRequest {
                repo_url: url.clone(),
                survey_request_id: survey_request_id.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_brownfield_batch_requests.clone(),
        |r| r.survey_request_id == survey_request_id,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "survey_request_id": survey_request_id,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a spec-revision ADVISOR request (a03). A non-`send it` `@<bot>`
/// reply in a tracked revision thread routes here; the polling loop drains
/// it AND runs a read-only agentic session that discusses the revision in
/// the thread. De-duplicated on `(change_slug, reply_text)` so a doubly-
/// delivered mention event does not run the advisor twice for the same
/// message.
pub(crate) fn handle_revision_advise(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let reply_text = parsed
        .get("reply_text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(RevisionAdviseRequest {
                repo_url: url.clone(),
                change_slug: change.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                reply_text: reply_text.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_revision_requests.advise.clone(),
        |r| r.change_slug == change && r.reply_text == reply_text,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "change": change,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Queue a spec-revision EXECUTOR request (a03). `@<bot> send it` in a
/// tracked revision thread routes here; the polling loop drains it AND runs
/// a write-scoped session that revises the change's spec deltas, re-runs the
/// `[in]` / `[canon]` gates, AND opens a PR on a clean re-gate. De-duplicated
/// on `change_slug` so a doubly-delivered `send it` enqueues only one run.
pub(crate) fn handle_revision_execute(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let change = match require_str(parsed, "change") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let channel = match require_str(parsed, "channel") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let thread_ts = match require_str(parsed, "thread_ts") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    enqueue_request(
        state,
        &url,
        |_repo| {
            Ok(RevisionExecuteRequest {
                repo_url: url.clone(),
                change_slug: change.clone(),
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                submitted_at: chrono::Utc::now(),
            })
        },
        |h| h.pending_revision_requests.execute.clone(),
        |r| r.change_slug == change,
        |repo| {
            json!({
                "ok": true,
                "url": url,
                "change": change,
                "poll_interval_sec": repo.poll_interval_sec,
            })
        },
    )
}

/// Handle the `queue_clear_survey` action (a29). Synchronously deletes
/// every `BrownfieldSurveyState` file in the matched repo's workspace
/// AND returns the count cleared. The chatops dispatcher posts the
/// reply using that count.
pub(crate) fn handle_queue_clear_survey(parsed: &Value, state: &ControlState) -> Value {
    clear_state_files(
        parsed,
        state,
        "brownfield-survey",
        crate::state::brownfield_survey::clear_all,
    )
}
