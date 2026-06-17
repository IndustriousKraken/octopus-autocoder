//! Control-socket action handlers AND the table-driven dispatcher.
//!
//! This submodule holds the accreted `queue_*` enqueue/clear handlers and the
//! spec-revision handlers (the bodies the audit flagged), plus the shared
//! [`enqueue_request`] scaffold that de-duplicates their common
//! validate-resolve-enqueue-ack shape. It also owns [`DISPATCH_TABLE`] and
//! [`dispatch`], the table-driven replacement for the old hand-maintained
//! `match action.as_str()` arm list in `dispatch_request`.
//!
//! As a child of `control_socket`, this module reaches the parent's
//! `ControlState`, `require_str`, `find_repo`, `RepoTaskHandle`, the request
//! record structs, AND the remaining (marker / outcome / async) handlers via
//! the `use super::*` glob below; only [`dispatch`] is `pub(super)`, called
//! from the parent's `dispatch_request`.
use super::*;

/// Shared scaffold for the `queue_*` enqueue handlers that key on a
/// `request_id` (proposal / changelog / brownfield / scout / sync-upstream /
/// brownfield-survey). Validates `url` + `request_id`, resolves the repo,
/// builds the request record (`build` reads any on-disk state OR validates the
/// handler's extra fields, returning an error `Value` to short-circuit the
/// response), looks up the live `RepoTaskHandle`'s pending queue, and — when
/// the handle is live — runs `enqueue` under the queue lock (the per-queue
/// de-dup + push) before returning `{ok, url, request_id, poll_interval_sec}`.
///
/// Each handler supplies only its queue selector (`select`), its record
/// constructor (`build`), AND its de-dup/push step (`enqueue`, which carries
/// the `Vec::push` vs `VecDeque::push_back` difference); the four common
/// failure responses AND the success ack live here exactly once.
fn enqueue_request<Q, T>(
    parsed: &Value,
    state: &ControlState,
    select: impl FnOnce(&RepoTaskHandle) -> Arc<Mutex<Q>>,
    build: impl FnOnce(&str, &str, &RepositoryConfig) -> std::result::Result<T, Value>,
    enqueue: impl FnOnce(&mut Q, &str, T),
) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let request_id = match require_str(parsed, "request_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let record = match build(&url, &request_id, &repo) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(select)
    };
    let queue = match queue_slot {
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
        enqueue(&mut g, &request_id, record);
    }
    json!({
        "ok": true,
        "url": url,
        "request_id": request_id,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Append `audit_type` to the named repo's `pending_audit_runs` queue
/// so the next polling iteration's audit phase runs it unconditionally
/// (bypassing cadence). De-duplicated: appending a value already in the
/// queue is a no-op (the response still reports success). The request
/// identifies the repo by `url` (chatops verb path) OR by `workspace`
/// (CLI `audit run` path — the daemon does the workspace-to-URL
/// resolution against its configured repo list). The response echoes
/// the canonical `audit_type` and resolved `url` so the chatops/CLI
/// caller can build an ack with the daemon's authoritative names;
/// `poll_interval_sec` lets the caller compute the ETA clause.
fn handle_queue_audit(parsed: &Value, state: &ControlState) -> Value {
    let audit_type = match require_str(parsed, "audit_type") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Resolve target URL: explicit `url` wins; otherwise look up by
    // `workspace` path (matched against each configured repo's
    // `workspace::resolve_path`).
    let url = if let Some(u) = parsed.get("url").and_then(|v| v.as_str()) {
        u.to_string()
    } else if let Some(ws) = parsed.get("workspace").and_then(|v| v.as_str()) {
        match find_repo_by_workspace(state, std::path::Path::new(ws)) {
            Some(u) => u,
            None => {
                return json!({
                    "ok": false,
                    "error": format!(
                        "no managed repository found for workspace path `{ws}`; the daemon is managing: {}",
                        managed_repo_list_for_error(state)
                    ),
                });
            }
        }
    } else {
        return json!({"ok": false, "error": "missing `url` or `workspace` field"});
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(|h| h.pending_audit_runs.clone())
    };
    let queue = match queue_slot {
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
    // Carry the originating chat context (when present) so the scheduler can
    // post the terminal completion notification back to the operator's thread.
    let origin = parsed
        .get("channel")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|channel| crate::polling_loop::ChatOrigin {
            channel: channel.to_string(),
            thread_ts: parsed
                .get("thread_ts")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        });
    // Resolve the repo's workspace so the mutated queue can be mirrored to
    // its durable `pending-audit-runs/<basename>.json` file the instant the
    // enqueue is acknowledged below — closing the enqueue→restart window
    // (persist-on-demand-audit-queue).
    let workspace = workspace::resolve_path(&state.paths, &repo);
    {
        let mut g = queue.lock().unwrap();
        if !g.iter().any(|a| a.audit_type == audit_type) {
            g.push(crate::polling_loop::QueuedAudit {
                audit_type: audit_type.clone(),
                origin,
            });
        }
        // Persist on every mutation. Best-effort: a write failure is logged
        // and never fails the enqueue (the in-memory queue stays
        // authoritative for the live process).
        if let Err(e) =
            crate::polling_loop::save_pending_audit_runs(&state.paths, &workspace, g.as_slice())
        {
            tracing::warn!(
                url = %url,
                "queue_audit: failed to persist pending-audit-runs queue (in-memory queue remains authoritative): {e:#}"
            );
        }
    }
    json!({
        "ok": true,
        "url": url,
        "audit_type": audit_type,
        "poll_interval_sec": repo.poll_interval_sec,
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
fn handle_queue_proposal_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_proposal_requests.clone(),
        |url, request_id, _repo| {
            // Load the on-disk state file the chatops dispatcher just wrote.
            let state_root = crate::proposal_requests::default_state_root(&state.paths);
            match crate::proposal_requests::read_state(&state_root, url, request_id) {
                Ok(Some(s)) => Ok(ProposalRequest {
                    request_id: s.request_id.clone(),
                    channel: s.channel.clone(),
                    thread_ts: s.thread_ts.clone(),
                    operator_user: s.operator_user.clone(),
                    request_text: s.request_text.clone(),
                    submitted_at: s.submitted_at,
                }),
                Ok(None) => Err(json!({
                    "ok": false,
                    "error": format!(
                        "no proposal-request state file found for request_id `{request_id}` under repo `{url}`"
                    ),
                })),
                Err(e) => Err(json!({
                    "ok": false,
                    "error": format!("reading proposal-request state: {e:#}")
                })),
            }
        },
        |g, request_id, rec| {
            // De-dup: if the same request_id is somehow queued twice (e.g.
            // chatops retried), keep only one entry.
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push(rec);
            }
        },
    )
}

/// Queue a chat-driven changelog request for the repo's next polling
/// iteration. The request was already persisted to disk as a
/// `ChangelogRequestState` file by the chatops dispatcher; this
/// handler's job is to look up the repo's live polling-task handle,
/// load the state from disk, and push a `ChangelogRequest` onto the
/// handle's `pending_changelog_requests` queue.
fn handle_queue_changelog_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_changelog_requests.clone(),
        |url, request_id, _repo| {
            let state_root = crate::changelog_requests::default_state_root(&state.paths);
            match crate::changelog_requests::read_state(&state_root, url, request_id) {
                Ok(Some(s)) => Ok(ChangelogRequest {
                    request_id: s.request_id.clone(),
                    repo_url: s.repo_url.clone(),
                    raw_args: s.raw_args.clone(),
                    channel: s.channel.clone(),
                    lifecycle_thread_ts: s.lifecycle_thread_ts.clone(),
                    submitted_at: s.submitted_at,
                }),
                Ok(None) => Err(json!({
                    "ok": false,
                    "error": format!(
                        "no changelog-request state file found for request_id `{request_id}` under repo `{url}`"
                    ),
                })),
                Err(e) => Err(json!({
                    "ok": false,
                    "error": format!("reading changelog-request state: {e:#}")
                })),
            }
        },
        |g, request_id, rec| {
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push(rec);
            }
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
fn handle_queue_brownfield_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_brownfield_requests.clone(),
        |_url, request_id, repo| {
            let workspace = crate::workspace::resolve_path(&state.paths, repo);
            match crate::state::brownfield_request::read_state(&workspace, request_id) {
                Ok(Some(s)) => Ok(BrownfieldRequest {
                    request_id: s.request_id.clone(),
                    repo_url: s.repo_url.clone(),
                    capability_name: s.capability_name.clone(),
                    guidance: s.guidance.clone(),
                    channel: s.channel.clone(),
                    thread_ts: s.thread_ts.clone(),
                    submitted_at: s.submitted_at,
                }),
                Ok(None) => Err(json!({
                    "ok": false,
                    "error": format!(
                        "no brownfield-request state file found for request_id `{request_id}` under workspace `{}`",
                        workspace.display()
                    ),
                })),
                Err(e) => Err(json!({
                    "ok": false,
                    "error": format!("reading brownfield-request state: {e:#}"),
                })),
            }
        },
        |g, request_id, rec| {
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push_back(rec);
            }
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
fn handle_queue_scout_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_scout_requests.clone(),
        |url, request_id, _repo| {
            let channel = match require_str(parsed, "channel") {
                Ok(s) => s,
                Err(e) => return Err(json!({"ok": false, "error": e})),
            };
            let thread_ts = match require_str(parsed, "thread_ts") {
                Ok(s) => s,
                Err(e) => return Err(json!({"ok": false, "error": e})),
            };
            let guidance = parsed
                .get("guidance")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Ok(ScoutRequest {
                request_id: request_id.to_string(),
                repo_url: url.to_string(),
                guidance,
                channel,
                thread_ts,
                submitted_at: chrono::Utc::now(),
            })
        },
        |g, request_id, rec| {
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push_back(rec);
            }
        },
    )
}

/// Queue a chat-driven spec-it request (a25). Enqueues the scout's
/// `request_id` AND the operator-selected `item_id` plus optional
/// guidance; the polling-iteration handler resolves the item against
/// the on-disk `ScoutRunState` AND submits a fresh `ProposalRequest`.
fn handle_queue_spec_it_request(parsed: &Value, state: &ControlState) -> Value {
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
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(|h| h.pending_spec_it_requests.clone())
    };
    let queue = match queue_slot {
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
        g.push_back(SpecItRequest {
            repo_url: url.clone(),
            scout_request_id,
            item_id,
            guidance,
            channel,
            thread_ts,
            submitted_at: chrono::Utc::now(),
        });
    }
    json!({
        "ok": true,
        "url": url,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Handle the `queue_sync_upstream_request` action (a26). Enqueues
/// a `SyncUpstreamRequest` onto the matched repo's handle; the
/// polling iteration's handler drains it at iteration start.
fn handle_queue_sync_upstream_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_sync_upstream_requests.clone(),
        |url, request_id, _repo| {
            let channel = match require_str(parsed, "channel") {
                Ok(s) => s,
                Err(e) => return Err(json!({"ok": false, "error": e})),
            };
            let thread_ts = parsed
                .get("thread_ts")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Ok(SyncUpstreamRequest {
                request_id: request_id.to_string(),
                repo_url: url.to_string(),
                channel,
                thread_ts,
                submitted_at: chrono::Utc::now(),
            })
        },
        |g, request_id, rec| {
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push_back(rec);
            }
        },
    )
}

/// Handle the `queue_clear_scout` action (a25). Synchronously deletes
/// every `ScoutRunState` file in the matched repo's workspace AND
/// returns the count cleared. The chatops dispatcher posts the reply
/// using that count.
fn handle_queue_clear_scout(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    let cleared = match crate::state::scout_run::clear_all(&workspace) {
        Ok(n) => n,
        Err(e) => {
            return json!({
                "ok": false,
                "error": format!("clearing scout state: {e:#}"),
            });
        }
    };
    json!({
        "ok": true,
        "url": url,
        "cleared": cleared,
    })
}

/// Queue a chat-driven brownfield-survey request for the repo's next
/// polling iteration (a29). The survey state file is NOT yet on disk
/// (the survey polling handler creates it AFTER the executor pass);
/// this handler enqueues an in-memory `BrownfieldSurveyRequest`
/// carrying every field the polling-iteration handler needs.
fn handle_queue_brownfield_survey_request(parsed: &Value, state: &ControlState) -> Value {
    enqueue_request(
        parsed,
        state,
        |h| h.pending_brownfield_survey_requests.clone(),
        |url, request_id, _repo| {
            let channel = match require_str(parsed, "channel") {
                Ok(s) => s,
                Err(e) => return Err(json!({"ok": false, "error": e})),
            };
            let thread_ts = match require_str(parsed, "thread_ts") {
                Ok(s) => s,
                Err(e) => return Err(json!({"ok": false, "error": e})),
            };
            let guidance = parsed
                .get("guidance")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Ok(BrownfieldSurveyRequest {
                request_id: request_id.to_string(),
                repo_url: url.to_string(),
                guidance,
                channel,
                thread_ts,
                submitted_at: chrono::Utc::now(),
            })
        },
        |g, request_id, rec| {
            if !g.iter().any(|r| r.request_id == request_id) {
                g.push_back(rec);
            }
        },
    )
}

/// Queue a chat-driven brownfield-batch request for the repo's next
/// polling iteration (a29). The referenced survey state file MUST
/// already exist; the polling handler loads it to begin the batch
/// drain.
fn handle_queue_brownfield_batch_request(parsed: &Value, state: &ControlState) -> Value {
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
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard
            .get(&url)
            .map(|h| h.pending_brownfield_batch_requests.clone())
    };
    let queue = match queue_slot {
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
        if !g.iter().any(|r| r.survey_request_id == survey_request_id) {
            g.push_back(BrownfieldBatchRequest {
                repo_url: url.clone(),
                survey_request_id: survey_request_id.clone(),
                channel,
                thread_ts,
                submitted_at: chrono::Utc::now(),
            });
        }
    }
    json!({
        "ok": true,
        "url": url,
        "survey_request_id": survey_request_id,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Queue a spec-revision ADVISOR request (a03). A non-`send it` `@<bot>`
/// reply in a tracked revision thread routes here; the polling loop drains
/// it AND runs a read-only agentic session that discusses the revision in
/// the thread. De-duplicated on `(change_slug, reply_text)` so a doubly-
/// delivered mention event does not run the advisor twice for the same
/// message.
fn handle_revision_advise(parsed: &Value, state: &ControlState) -> Value {
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
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(|h| h.pending_revision_requests.advise.clone())
    };
    let queue = match queue_slot {
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
        if !g
            .iter()
            .any(|r| r.change_slug == change && r.reply_text == reply_text)
        {
            g.push_back(RevisionAdviseRequest {
                repo_url: url.clone(),
                change_slug: change.clone(),
                channel,
                thread_ts,
                reply_text,
                submitted_at: chrono::Utc::now(),
            });
        }
    }
    json!({
        "ok": true,
        "url": url,
        "change": change,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Queue a spec-revision EXECUTOR request (a03). `@<bot> send it` in a
/// tracked revision thread routes here; the polling loop drains it AND runs
/// a write-scoped session that revises the change's spec deltas, re-runs the
/// `[in]` / `[canon]` gates, AND opens a PR on a clean re-gate. De-duplicated
/// on `change_slug` so a doubly-delivered `send it` enqueues only one run.
fn handle_revision_execute(parsed: &Value, state: &ControlState) -> Value {
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
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let queue_slot = {
        let guard = state.repo_tasks.lock().unwrap();
        guard.get(&url).map(|h| h.pending_revision_requests.execute.clone())
    };
    let queue = match queue_slot {
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
        if !g.iter().any(|r| r.change_slug == change) {
            g.push_back(RevisionExecuteRequest {
                repo_url: url.clone(),
                change_slug: change.clone(),
                channel,
                thread_ts,
                submitted_at: chrono::Utc::now(),
            });
        }
    }
    json!({
        "ok": true,
        "url": url,
        "change": change,
        "poll_interval_sec": repo.poll_interval_sec,
    })
}

/// Handle the `queue_clear_survey` action (a29). Synchronously deletes
/// every `BrownfieldSurveyState` file in the matched repo's workspace
/// AND returns the count cleared. The chatops dispatcher posts the
/// reply using that count.
fn handle_queue_clear_survey(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    let cleared = match crate::state::brownfield_survey::clear_all(&workspace) {
        Ok(n) => n,
        Err(e) => {
            return json!({
                "ok": false,
                "error": format!("clearing brownfield-survey state: {e:#}"),
            });
        }
    };
    json!({
        "ok": true,
        "url": url,
        "cleared": cleared,
    })
}

/// Handle the `promote_issue_candidate` action (a010). Synchronously
/// promotes a posted issue-lane candidate the chatops `send it` dispatcher
/// matched to a maintainer's in-thread reply: resolves the repo AND its
/// workspace, loads the candidate, AND — when it is still `Posted` — writes
/// `issues/<slug>/` (its `issue.md` + `tasks.md`, plus the quarantined
/// `report-body.md` for a public-origin candidate) AND flips the candidate's
/// status to `Promoted`. Writing the unit IS the queue (the issues-lane
/// walker picks up any ready `issues/<slug>/`). Idempotent: an already-
/// `Promoted` candidate writes nothing further AND reports `already_promoted`
/// so the dispatcher can word its reply without re-writing. Mirrors
/// [`handle_queue_clear_survey`]'s synchronous-filesystem-work shape.
///
/// `channel` AND `thread_ts` are required so the request identifies the
/// originating thread (matching the survey/audit action contract); the
/// promotion itself is a pure filesystem write that needs neither.
fn handle_promote_issue_candidate(parsed: &Value, state: &ControlState) -> Value {
    let url = match require_str(parsed, "url") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let candidate_id = match require_str(parsed, "candidate_id") {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    if let Err(e) = require_str(parsed, "channel") {
        return json!({"ok": false, "error": e});
    }
    if let Err(e) = require_str(parsed, "thread_ts") {
        return json!({"ok": false, "error": e});
    }
    let repo = match find_repo(state, &url) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let workspace = crate::workspace::resolve_path(&state.paths, &repo);
    let candidate =
        match crate::lanes::ingestion::read_candidate(&state.paths.state, &candidate_id) {
            Ok(Some(c)) => c,
            Ok(None) => {
                return json!({
                    "ok": false,
                    "error": format!("no issue candidate `{candidate_id}` recorded"),
                });
            }
            Err(e) => {
                return json!({"ok": false, "error": format!("reading issue candidate: {e:#}")});
            }
        };
    match candidate.status {
        crate::lanes::ingestion::CandidateStatus::Promoted => json!({
            "ok": true,
            "already_promoted": true,
            "slug": candidate.slug,
        }),
        crate::lanes::ingestion::CandidateStatus::Posted => {
            match crate::lanes::ingestion::promote_candidate(
                &workspace,
                &state.paths.state,
                &candidate,
            ) {
                Ok(dir) => json!({
                    "ok": true,
                    "slug": candidate.slug,
                    "path": dir.display().to_string(),
                }),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            }
        }
    }
}

/// Handle the `query_canonical_specs` action (a21). Looks up the
/// workspace's `CanonicalRagStore` in the daemon's registry; on hit,
/// runs the query and returns ranked chunks. Every error path is
/// fail-open: an `ok: true` response with an empty `hits` array and a
/// structured `error_hint`. Protocol-level violations (missing
/// `workspace_basename` or `query`) return `ok: false` per the canonical
/// request-protocol scenario.
async fn handle_query_canonical_specs(parsed: &Value, state: &ControlState) -> Value {
    let workspace_basename = match require_str(parsed, "workspace_basename") {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "error": format!("missing required field: workspace_basename ({e})")});
        }
    };
    let query = match require_str(parsed, "query") {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "error": format!("missing required field: query ({e})")});
        }
    };
    let top_k = parsed
        .get("top_k")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let cfg = state.last_config.load_full();
    if cfg
        .canonical_rag
        .as_ref()
        .map(|r| !r.is_active())
        .unwrap_or(true)
    {
        return json!({
            "ok": true,
            "hits": [],
            "error_hint": "rag disabled in config",
        });
    }

    let store = match state
        .canonical_rag_registry
        .get(&workspace_basename)
        .await
    {
        Some(s) => s,
        None => {
            // Distinguish the two empty-registry cases: a known
            // workspace whose init failed (config has the block) vs.
            // a basename the daemon doesn't manage.
            let cfg_active = cfg
                .canonical_rag
                .as_ref()
                .map(|r| r.is_active())
                .unwrap_or(false);
            let hint = if cfg_active {
                "rag init failed; see daemon log"
            } else {
                "no workspace registered for that basename"
            };
            return json!({
                "ok": true,
                "hits": [],
                "error_hint": hint,
            });
        }
    };

    match store.query(&query, top_k).await {
        Ok(hits) => json!({
            "ok": true,
            "hits": hits,
        }),
        Err(e) => {
            tracing::warn!(
                workspace_basename = %workspace_basename,
                "canonical RAG query failed: {e:#}"
            );
            json!({
                "ok": true,
                "hits": [],
                "error_hint": format!("query failed: {e}"),
            })
        }
    }
}

// =====================================================================
// Table-driven action dispatch
// =====================================================================

/// A pinned, boxed future returning a control-socket response. Lets the async
/// action handlers sit in the same dispatch table as the synchronous ones — an
/// `async fn` cannot be named by a plain `fn` pointer type, so it is wrapped.
type BoxFut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>>;

/// One dispatchable control-socket action handler: synchronous, OR async (the
/// latter awaited by [`dispatch`]).
enum Handler {
    Sync(fn(&Value, &ControlState) -> Value),
    Async(for<'a> fn(&'a Value, &'a ControlState) -> BoxFut<'a>),
}

// Thin adapters wrapping the parent module's async handlers as `BoxFut`-
// returning fn pointers so they sit in the dispatch table beside the sync ones.
fn run_reload<'a>(_p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_reload(s))
}
fn run_repo_status<'a>(p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_repo_status(p, s))
}
fn run_repo_status_all<'a>(_p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_repo_status_all(s))
}
fn run_wipe_workspace<'a>(p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_wipe_workspace(p, s))
}
fn run_rebuild_specs<'a>(p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_rebuild_specs(p, s))
}
fn run_trigger_audit_action<'a>(p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    Box::pin(super::handle_trigger_audit_action(p, s))
}
fn run_query_canonical_specs<'a>(p: &'a Value, s: &'a ControlState) -> BoxFut<'a> {
    // `handle_query_canonical_specs` moved into this submodule with the rest of
    // the queue handlers, so it is reached locally (not via `super`).
    Box::pin(handle_query_canonical_specs(p, s))
}

/// The control-socket action dispatch table: action string → handler. Adding an
/// action is a single row here, not another arm in a growing `match`. The
/// async handlers (`reload`, `repo_status`, …) live in the parent module; the
/// queue/enqueue/revision handlers are local to this module.
const DISPATCH_TABLE: &[(&str, Handler)] = &[
    ("reload", Handler::Async(run_reload)),
    ("repo_status", Handler::Async(run_repo_status)),
    ("repo_status_all", Handler::Async(run_repo_status_all)),
    ("clear_perma_stuck_marker", Handler::Sync(super::handle_clear_perma_stuck)),
    ("clear_revision_marker", Handler::Sync(super::handle_clear_revision)),
    ("ignore_for_queue_marker", Handler::Sync(super::handle_ignore_for_queue)),
    ("clear_ignore_for_queue_marker", Handler::Sync(super::handle_clear_ignore_for_queue)),
    ("wipe_workspace", Handler::Async(run_wipe_workspace)),
    ("rebuild_specs", Handler::Async(run_rebuild_specs)),
    ("trigger_audit_action", Handler::Async(run_trigger_audit_action)),
    ("queue_audit", Handler::Sync(handle_queue_audit)),
    ("queue_proposal_request", Handler::Sync(handle_queue_proposal_request)),
    ("queue_changelog_request", Handler::Sync(handle_queue_changelog_request)),
    ("queue_brownfield_request", Handler::Sync(handle_queue_brownfield_request)),
    ("queue_scout_request", Handler::Sync(handle_queue_scout_request)),
    ("queue_spec_it_request", Handler::Sync(handle_queue_spec_it_request)),
    ("queue_clear_scout", Handler::Sync(handle_queue_clear_scout)),
    ("queue_sync_upstream_request", Handler::Sync(handle_queue_sync_upstream_request)),
    ("queue_brownfield_survey_request", Handler::Sync(handle_queue_brownfield_survey_request)),
    ("queue_brownfield_batch_request", Handler::Sync(handle_queue_brownfield_batch_request)),
    ("revision_advise", Handler::Sync(handle_revision_advise)),
    ("revision_execute", Handler::Sync(handle_revision_execute)),
    ("queue_clear_survey", Handler::Sync(handle_queue_clear_survey)),
    ("promote_issue_candidate", Handler::Sync(handle_promote_issue_candidate)),
    ("query_canonical_specs", Handler::Async(run_query_canonical_specs)),
    ("record_outcome", Handler::Sync(super::handle_record_outcome)),
    ("consume_outcome", Handler::Sync(super::handle_consume_outcome)),
    ("record_submission", Handler::Sync(super::handle_record_submission)),
    ("consume_submission", Handler::Sync(super::handle_consume_submission)),
];

/// Resolve `action` against [`DISPATCH_TABLE`] and run its handler, awaiting
/// the async ones. Returns the canonical unknown-action error when no row
/// matches — identical to the prior hand-maintained `match` default arm.
pub(super) async fn dispatch(action: &str, parsed: &Value, state: &ControlState) -> Value {
    match DISPATCH_TABLE.iter().find(|(name, _)| *name == action) {
        Some((_, Handler::Sync(f))) => f(parsed, state),
        Some((_, Handler::Async(f))) => f(parsed, state).await,
        None => json!({"ok": false, "error": format!("unknown action: {action}")}),
    }
}
