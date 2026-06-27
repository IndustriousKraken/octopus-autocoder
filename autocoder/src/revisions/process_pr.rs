//! Per-PR revision/code-review comment dispatch — the orchestration formerly
//! carried by the single ~900-line `process_one_pr` in `revisions.rs`.
//!
//! Split out as a behavior-preserving decomposition (no observable contract
//! change — PR outcomes AND outcome semantics are identical). The work is now
//! organized as:
//!
//! - [`ReviseCtx`] — the invariant per-PR context (the borrows every phase
//!   needs), so the phase helpers don't each carry a ~16-parameter list. The
//!   mutable [`RevisionState`] AND the running seen-marker are threaded
//!   separately as `&mut`, since they change across the comment loop.
//! - [`process_one_pr`] — the entry point: load/init state, fetch new
//!   comments, run the per-comment loop, persist the final seen-marker.
//! - [`ReviseCtx::process_comment`] — one comment's pipeline (skip guards →
//!   authorization gate → verb dispatch → caps → context assembly → execute →
//!   outcome), returning a [`CommentFlow`] that directs the loop.
//! - [`ReviseCtx::handle_revision_outcome`] — the executor-outcome match. Its
//!   failure-shaped arms share [`ReviseCtx::finalize_revise_failure`] (post
//!   alert → post reply → count the attempt → advance + persist), so each arm
//!   carries only its unique alert reason, reply body, and post-error policy.
//!
//! Every helper here is private to this module; only `process_one_pr` is
//! re-exported (`pub(crate) use process_pr::process_one_pr;` in the parent),
//! so the one call site in `process_revision_requests_at` resolves exactly as
//! before the split. `use super::*` makes the parent module's items — the
//! state helpers, the `compose_*`/`execute_*`/`apply_*` helpers, the imported
//! types, AND `advance_seen` — visible here unchanged.
use super::*;

/// Invariant context for processing one PR's revision/code-review comments.
/// Bundles the borrows that every per-comment step needs so the extracted
/// phase helpers don't each carry a ~16-parameter list. Mutable per-PR state
/// (`RevisionState`) AND the running seen-marker are threaded separately as
/// `&mut`, since they change across the comment loop.
struct ReviseCtx<'a> {
    paths: &'a crate::paths::DaemonPaths,
    workspace: &'a Path,
    repo: &'a RepositoryConfig,
    github_cfg: &'a GithubConfig,
    pr: &'a github::PrSummary,
    owner: &'a str,
    repo_name: &'a str,
    token: &'a str,
    bot_username: &'a str,
    reviewer: Option<&'a CodeReviewer>,
    executor: &'a dyn Executor,
    chatops_ctx: Option<&'a ChatOpsCtx<'a>>,
    human_revise_cap: Option<u32>,
    push_remote: &'a str,
    api_base: &'a str,
    forge: GithubForge,
}

/// Directs the per-comment loop in [`process_one_pr`] after a single comment
/// is dispatched. Mirrors the three exits the original inline body used:
/// `Continue` (advance to the next comment), `Break` (stop the loop, then run
/// the final seen-marker persist), AND `Return` (the caller returns
/// immediately — the step that produced this already persisted progress on
/// prior comments, deliberately NOT advancing past the current one).
enum CommentFlow {
    Continue,
    Break,
    Return,
}

/// PR-sourced material assembled for one revise dispatch (per a20a5). The
/// three `String` fields are consumed by `execute_revision` (moved in by
/// value); `change_name` / `change_list_summary` / `comment_id_str` are
/// retained for the post-execute outcome handling.
struct RevisePlan {
    change_name: String,
    change_list_summary: String,
    comment_id_str: String,
    pr_body: String,
    pr_change_list_str: String,
    agent_implementation_notes: String,
}

/// Process all new comments on a single PR. Returns Ok on success;
/// errors propagate (the caller logs at WARN and proceeds to the next PR).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_one_pr(
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    pr: &github::PrSummary,
    owner: &str,
    repo_name: &str,
    token: &str,
    bot_username: &str,
    reviewer: Option<&CodeReviewer>,
    executor: &dyn Executor,
    chatops_ctx: Option<&ChatOpsCtx<'_>>,
    revision_cap: u32,
    human_revise_cap: Option<u32>,
    push_remote: &str,
    api_base: &str,
    cancel: CancellationToken,
) -> Result<()> {
    let _ = reviewer; // wired through; consumed by the code-review branch (task 4)
    // a007: forge-trait handle for this PR's comment fetches + reply posting.
    let forge = GithubForge::with_api_base(api_base);
    // Load or initialize per-PR state. The revision_cap stored in state
    // reflects the cap in effect when the PR was first observed; callers
    // that change `executor.max_auto_revisions_per_pr` mid-PR live with
    // the older cap until the PR closes (matches the chatops-channel
    // hot-reload contract: changes apply to new work, not in-flight).
    let code_review_cap_initial: Option<u32> =
        reviewer.and_then(|r| r.max_code_reviews_per_pr());
    let mut state =
        load_or_init_revision_state(paths, workspace, repo, pr, revision_cap, code_review_cap_initial)?;

    // NOTE: there is deliberately NO whole-PR fast-skip when the automatic
    // cap is reached + declined. Under a47 the cap bounds only AUTOMATIC
    // (reviewer-marked) revisions; human `@<bot> revise` comments must
    // still process even after the automatic decline has been posted. Each
    // comment is classified per-iteration in the loop below, so an
    // over-cap automatic trigger is silently advanced while a human
    // trigger interleaved on the same PR is dispatched normally.

    let ctx = ReviseCtx {
        paths,
        workspace,
        repo,
        github_cfg,
        pr,
        owner,
        repo_name,
        token,
        bot_username,
        reviewer,
        executor,
        chatops_ctx,
        human_revise_cap,
        push_remote,
        api_base,
        forge,
    };

    let comments = ctx
        .forge
        .list_comments_since(token, owner, repo_name, pr.number, state.last_seen_comment_at)
        .await?;
    if comments.is_empty() {
        return Ok(());
    }
    let mut latest_seen: Option<DateTime<Utc>> = None;
    for comment in comments {
        if cancel.is_cancelled() {
            // Persist whatever progress we made and return.
            if let Some(t) = latest_seen {
                state.last_seen_comment_at = t;
                write_state(paths, workspace, &state)?;
            }
            return Ok(());
        }
        match ctx
            .process_comment(&comment, &mut state, &mut latest_seen)
            .await?
        {
            CommentFlow::Continue => {}
            CommentFlow::Break => break,
            // `Return` cases already persisted progress on prior comments
            // (deliberately NOT advancing past the current one) before
            // signalling; just return.
            CommentFlow::Return => return Ok(()),
        }
    }
    if let Some(t) = latest_seen
        && t > state.last_seen_comment_at
    {
        state.last_seen_comment_at = t;
        write_state(paths, workspace, &state)?;
    }
    Ok(())
}

/// Load the per-PR revision state file, initializing a fresh
/// [`RevisionState`] when none exists. The fresh state stamps the
/// caps in effect when the PR was first observed (see the note at the
/// `process_one_pr` call site).
fn load_or_init_revision_state(
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    pr: &github::PrSummary,
    revision_cap: u32,
    code_review_cap_initial: Option<u32>,
) -> Result<RevisionState> {
    Ok(match read_state(paths, workspace, pr.number)? {
        Some(s) => s,
        None => RevisionState {
            pr_number: pr.number,
            agent_branch: repo.agent_branch.clone(),
            last_seen_comment_at: pr.created_at,
            auto_revisions_applied: 0,
            revision_cap,
            cap_decline_posted: false,
            human_revise_count: 0,
            human_revise_cap_decline_posted: false,
            code_reviews_applied: 0,
            code_review_cap: code_review_cap_initial,
            cap_decline_posted_for_code_review: false,
            last_suggested_rereview_at_revisions_count: None,
            original_review_head_sha: None,
        },
    })
}

impl<'a> ReviseCtx<'a> {
    /// Run one comment through the full dispatch pipeline, returning the
    /// [`CommentFlow`] that directs the enclosing loop. Preserves the
    /// original inline ordering: strict-since guard → bot-author filter →
    /// authorization gate → verb dispatch (code-review or revise) → cap
    /// checks → PR-context assembly → executor invocation → outcome handling.
    async fn process_comment(
        &self,
        comment: &github::IssueComment,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
    ) -> Result<CommentFlow> {
        let bot_username = self.bot_username;
        // Strict-since client-side guard. GitHub's `since` filter on
        // `/issues/<num>/comments` is inclusive at the sub-second boundary
        // (it compares against the comment's full-precision `updated_at`),
        // so a marker truncated to seconds — OR a marker advanced exactly
        // to a comment's creation time — can produce a re-fetch of an
        // already-processed comment. Skip any comment at OR before the
        // marker; the corresponding `advance_seen` is a no-op (the local
        // `latest_seen` is only used to advance the persisted marker, and
        // the value is already at or behind it).
        if comment.created_at <= state.last_seen_comment_at {
            advance_seen(latest_seen, comment.created_at);
            return Ok(CommentFlow::Continue);
        }
        // Bot-authored comments are filtered out before parsing — UNLESS
        // the body starts with the reviewer-revision HTML-comment marker,
        // which is the one sanctioned bypass. The reviewer pipeline posts
        // comments on the bot's behalf; without the bypass the dispatcher
        // would (correctly) treat them as the bot's own replies and drop
        // them. All other bot-authored comments (the dispatcher's own
        // success/failure/cap-decline replies, any future bot content)
        // continue to be filtered.
        if comment.user_login().eq_ignore_ascii_case(bot_username)
            && !comment
                .body
                .trim_start()
                .starts_with(REVIEWER_REVISION_MARKER)
        {
            advance_seen(latest_seen, comment.created_at);
            return Ok(CommentFlow::Continue);
        }
        // a000: authorization gate. A "trusted automatic" trigger is the
        // bot's OWN reviewer-revision comment — bot-authored AND carrying
        // the `<!-- reviewer-revision -->` marker (the reviewer pipeline
        // posting on the bot's behalf). ONLY that combination bypasses the
        // gate, mirroring the bot-self-author bypass above. A NON-bot
        // author who merely prepends the marker is NOT trusted — otherwise
        // any member of the public could defeat the gate with one HTML
        // comment — so the gate still applies to them.
        //
        // For every comment that parses as a comment-sourced verb
        // (`revise` or `code-review`) and is not a trusted automatic
        // trigger, the commenter must be authorized
        // (`author_association ∈ allowed_associations` OR `login ∈
        // allowed_users`). An unauthorized verb-comment is dropped BEFORE
        // dispatch (default-deny): no executor/reviewer work, the
        // seen-marker is advanced so it does not re-fire, the drop is
        // logged at INFO, and — only when `decline_comment` is set — a
        // single decline reply is posted. The marker advance + immediate
        // persist make the reply post at-most-once.
        let is_reviewer_marked = comment
            .body
            .trim_start()
            .starts_with(REVIEWER_REVISION_MARKER);
        let is_bot_authored = comment.user_login().eq_ignore_ascii_case(bot_username);
        let is_trusted_automatic = is_reviewer_marked && is_bot_authored;
        let parses_as_verb = parse_revision_trigger(&comment.body, bot_username).is_some()
            || parse_code_review_trigger(&comment.body, bot_username);
        if parses_as_verb
            && !is_trusted_automatic
            && !is_comment_authorized(comment, &self.github_cfg.command_authorization)
        {
            self.drop_unauthorized_verb(comment, state, latest_seen).await?;
            return Ok(CommentFlow::Continue);
        }
        // a33: try the code-review parser BEFORE the revise parser when the
        // revise parser does not match. The two verbs are mutually
        // exclusive on the leading-mention line; whichever fires first
        // wins per the existing dispatcher's leading-mention semantic.
        let revision_text = match parse_revision_trigger(&comment.body, bot_username) {
            Some(t) => t,
            None => {
                if parse_code_review_trigger(&comment.body, bot_username) {
                    // Dispatch the code-review verb in this branch and
                    // continue the comment loop.
                    return self
                        .handle_code_review_comment(comment, state, latest_seen)
                        .await;
                }
                // never-silent-when-addressed: a comment whose first token is
                // `@<bot>` but whose verb is neither `revise` nor
                // `code-review`, from an AUTHORIZED commenter, earns a
                // one-time command-affordance reply (deduplicated by comment
                // id). Bot-authored comments were already filtered above. A
                // non-addressing comment (`is_addressed_but_unrecognized` is
                // false) OR an unauthorized author falls through to the silent
                // skip below — matching the access-control gate's policy.
                if is_addressed_but_unrecognized(&comment.body, bot_username)
                    && is_comment_authorized(comment, &self.github_cfg.command_authorization)
                {
                    self.maybe_post_affordance_reply(comment).await;
                }
                advance_seen(latest_seen, comment.created_at);
                return Ok(CommentFlow::Continue);
            }
        };
        // a47: classify the triggering comment. AUTOMATIC revisions are
        // the bot's OWN reviewer-marked comments — bot-authored AND
        // carrying the `<!-- reviewer-revision -->` marker the
        // code-reviewer auto-revise path posts (a000 ties this to bot
        // authorship so a spoofed marker from a non-bot author is NOT
        // miscounted as automatic). Everything else is a deliberate human
        // `@<bot> revise` request. Only automatic revisions count against
        // `max_auto_revisions_per_pr` AND are subject to the auto
        // cap/decline; human requests are bounded by the separate
        // `max_revise_triggers_per_pr` cap and never touch the automatic
        // counter.
        let is_automatic = is_trusted_automatic;
        if is_automatic && state.auto_revisions_applied >= state.revision_cap {
            // Automatic cap hit. Post the one-time decline (if not posted),
            // then silently ignore THIS automatic trigger. We `continue`
            // (rather than `break`) so a human `@<bot> revise` comment
            // interleaved later on the same PR still gets processed. We
            // advance `latest_seen` to the decline-triggering comment so
            // re-running the iteration doesn't loop on the same comment.
            advance_seen(latest_seen, comment.created_at);
            self.maybe_post_auto_cap_decline(state).await?;
            return Ok(CommentFlow::Continue);
        }
        // human-revise-cap-opt-in: the per-PR human-revise cap is OPTIONAL.
        // When `executor.max_revise_triggers_per_pr` is `None` (the default)
        // a human `@<bot> revise` is NEVER gated — it always invokes the
        // executor regardless of how many revises this PR has already had,
        // mirroring the opt-in `reviewer.max_code_reviews_per_pr`. When set to
        // `Some(cap)`, a human `@<bot> revise` (NOT reviewer-marked) is bounded
        // by the cap (read live from config and tracked separately from the
        // automatic + re-review counters): at the cap the trigger is declined
        // WITHOUT invoking the executor — post the one-time per-PR notice
        // (guarded by `human_revise_cap_decline_posted` so a burst of over-cap
        // comments does not spam replies), advance the seen-marker, and
        // continue (so a later interleaved automatic trigger still processes).
        // The automatic + re-review caps are untouched.
        if !is_automatic
            && let Some(cap) = self.human_revise_cap
            && state.human_revise_count >= cap
        {
            advance_seen(latest_seen, comment.created_at);
            self.maybe_post_human_cap_decline(state, cap).await?;
            return Ok(CommentFlow::Continue);
        }
        // Per a20a5: assemble the executor's revision context from
        // PR-sourced material. A fetch failure posts a clear failure
        // comment AND does NOT advance the comment-seen marker — the next
        // iteration re-attempts assembly so transient API errors don't lose
        // the operator's revise comment. Signalled here by `Break`.
        let plan = match self.assemble_revise_plan(comment, &revision_text).await {
            Some(p) => p,
            None => return Ok(CommentFlow::Break),
        };
        let RevisePlan {
            change_name,
            change_list_summary,
            comment_id_str,
            pr_body,
            pr_change_list_str,
            agent_implementation_notes,
        } = plan;

        let revise_started_at = std::time::Instant::now();
        let outcome = execute_revision(
            self.workspace,
            self.repo,
            self.executor,
            &change_name,
            &revision_text,
            pr_body,
            pr_change_list_str,
            agent_implementation_notes,
        )
        .await;
        let revise_duration = revise_started_at.elapsed();
        self.handle_revision_outcome(
            outcome,
            &change_name,
            &revision_text,
            &change_list_summary,
            &comment_id_str,
            is_automatic,
            revise_duration,
            comment,
            state,
            latest_seen,
        )
        .await
    }

    /// a000: drop an unauthorized comment-sourced verb before dispatch
    /// (default-deny). Logs at INFO, advances the seen-marker, posts a
    /// single decline reply when `decline_comment` is set, AND persists the
    /// advanced marker immediately so the decline is never re-sent across
    /// restarts.
    async fn drop_unauthorized_verb(
        &self,
        comment: &github::IssueComment,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
    ) -> Result<()> {
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let login = comment.user_login().to_string();
        let assoc = comment
            .author_association()
            .unwrap_or("<none>")
            .to_string();
        tracing::info!(
            url = %repo.url,
            pr_number = pr.number,
            login = %login,
            author_association = %assoc,
            "a000: dropping unauthorized comment-sourced verb before dispatch (default-deny)"
        );
        advance_seen(latest_seen, comment.created_at);
        if self.github_cfg.command_authorization.decline_comment {
            let body = format!(
                "🚫 This `@{bot_username}` command was ignored: only repository owners, members, and collaborators (or configured allowed users) can trigger it. (author_association: {assoc})"
            );
            if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &body,
            )
            .await
            {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "failed to post authorization-decline PR comment: {e:#}"
                );
            }
        }
        // Persist the advanced marker immediately so the decline (if
        // posted) is never re-sent across restarts.
        state.last_seen_comment_at = comment.created_at;
        write_state(paths, workspace, state)?;
        Ok(())
    }

    /// never-silent-when-addressed: post the one-time command-affordance
    /// reply for an addressed-but-unrecognized comment, deduplicated by
    /// comment id via the alert-state file's `affordance_replies` map so the
    /// every-iteration comment fetch posts it at most once. Best-effort: a
    /// post failure leaves the dedup entry unrecorded so a later iteration
    /// can retry; a save failure is logged but does not abort the loop. The
    /// reply itself is bot-authored, so it is filtered before parsing on the
    /// next pass (no recursion) — the dedup map is the primary guard and the
    /// bot-author filter the backstop.
    async fn maybe_post_affordance_reply(&self, comment: &github::IssueComment) {
        let comment_id = comment.id.to_string();
        let mut alert_state =
            crate::alert_state::AlertState::load_or_default(self.paths, self.workspace);
        if alert_state.affordance_reply_already_posted(&comment_id) {
            return;
        }
        let body = compose_affordance_reply(self.bot_username);
        if let Err(e) = self
            .forge
            .post_comment(self.token, self.owner, self.repo_name, self.pr.number, &body)
            .await
        {
            tracing::warn!(
                url = %self.repo.url,
                pr_number = self.pr.number,
                comment_id = %comment_id,
                "failed to post command-affordance PR comment: {e:#}"
            );
            return;
        }
        alert_state.record_affordance_reply(&comment_id, Utc::now());
        if let Err(e) = alert_state.save(self.paths, self.workspace) {
            tracing::warn!(
                url = %self.repo.url,
                pr_number = self.pr.number,
                comment_id = %comment_id,
                "failed to persist affordance-reply dedup state: {e:#}"
            );
        }
    }

    /// Dispatch a `@<bot> code-review` verb comment (a33). Handles the
    /// per-PR re-review cap, the lifecycle alerts, and the four
    /// `CodeReviewOutcome` terminals. Always returns `Continue` — the
    /// code-review path never breaks or returns the loop.
    async fn handle_code_review_comment(
        &self,
        comment: &github::IssueComment,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
    ) -> Result<CommentFlow> {
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let chatops_ctx = self.chatops_ctx;

        let comment_id_str = comment.id.to_string();
        let operator_login = comment.user_login().to_string();
        let change_list = extract_change_list_from_pr_body(pr.body.as_deref());
        // Cap check. The re-review cap is opt-in: when
        // `code_review_cap` is `None` (the a47 default) there is
        // no ceiling — `@<bot> code-review` always dispatches and
        // no decline is ever posted. The cap only engages when
        // the operator set a value.
        if let Some(cap) = state.code_review_cap
            && state.code_reviews_applied >= cap
        {
            advance_seen(latest_seen, comment.created_at);
            self.maybe_post_code_review_cap_decline(state, cap).await?;
            return Ok(CommentFlow::Continue);
        }
        // Lifecycle: triggered.
        crate::polling_loop::maybe_post_code_review_triggered_alert(
            paths,
            chatops_ctx,
            repo,
            pr.number,
            &pr.url,
            &operator_login,
            &comment_id_str,
        )
        .await;
        let outcome = execute_code_review(
            workspace,
            repo,
            self.reviewer,
            pr,
            &change_list,
            state,
            self.api_base,
            token,
            owner,
            repo_name,
            bot_username,
        )
        .await;
        match outcome {
            Ok(CodeReviewOutcome::ReviewerDisabled) => {
                let body = "✗ Code review not available: reviewer is disabled in config".to_string();
                if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &body,
                )
                .await
                {
                    tracing::warn!(
                        url = %repo.url,
                        pr_number = pr.number,
                        "failed to post reviewer-disabled PR comment: {e:#}"
                    );
                }
                advance_seen(latest_seen, comment.created_at);
                write_state(paths, workspace, state)?;
            }
            Ok(CodeReviewOutcome::CapExceeded) => {
                // Should have been caught above; defensive fallthrough.
                advance_seen(latest_seen, comment.created_at);
                write_state(paths, workspace, state)?;
            }
            Ok(CodeReviewOutcome::Completed { verdict }) => {
                crate::polling_loop::maybe_post_code_review_complete_alert(
                    paths,
                    chatops_ctx,
                    repo,
                    pr.number,
                    &pr.url,
                    verdict.label(),
                    &comment_id_str,
                )
                .await;
                advance_seen(latest_seen, comment.created_at);
                write_state(paths, workspace, state)?;
            }
            Ok(CodeReviewOutcome::Failed { reason }) => {
                crate::polling_loop::maybe_post_code_review_failed_alert(
                    paths,
                    chatops_ctx,
                    repo,
                    pr.number,
                    &pr.url,
                    &reason,
                    &comment_id_str,
                )
                .await;
                let body = format!(
                    "✗ Code review failed: {reason}. The PR is unchanged. Reply with another `@{bot_username} code-review` to retry."
                );
                if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &body,
                )
                .await
                {
                    tracing::warn!(
                        url = %repo.url,
                        pr_number = pr.number,
                        "failed to post re-review failed PR comment: {e:#}"
                    );
                }
                advance_seen(latest_seen, comment.created_at);
                write_state(paths, workspace, state)?;
            }
            Err(e) => {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "code-review execution errored: {e:#}"
                );
                crate::polling_loop::maybe_post_code_review_failed_alert(
                    paths,
                    chatops_ctx,
                    repo,
                    pr.number,
                    &pr.url,
                    &format!("execution error: {e}"),
                    &comment_id_str,
                )
                .await;
                advance_seen(latest_seen, comment.created_at);
                write_state(paths, workspace, state)?;
            }
        }
        Ok(CommentFlow::Continue)
    }

    /// Post the one-time code-review cap-decline (PR comment + chatops
    /// notification), guarded by `cap_decline_posted_for_code_review` so a
    /// burst of over-cap requests does not spam replies. Persists the flag.
    async fn maybe_post_code_review_cap_decline(
        &self,
        state: &mut RevisionState,
        cap: u32,
    ) -> Result<()> {
        if state.cap_decline_posted_for_code_review {
            return Ok(());
        }
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let pr_text = format!(
            "🛑 Code review cap reached ({} reruns). Further @{} code-review requests will be ignored. Close + re-open the PR or merge as-is.",
            cap, bot_username,
        );
        if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &pr_text,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "failed to post code-review cap-decline PR comment: {e:#}"
            );
        }
        if let Some(ctx) = self.chatops_ctx {
            let chat_text = format!(
                "🛑 {}: PR #{} hit the code-review cap of {}. Further @{} code-review requests ignored.",
                repo.url, pr.number, cap, bot_username,
            );
            if let Err(e) = ctx
                .chatops
                .post_notification(ctx.channel, &chat_text)
                .await
            {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "failed to post code-review cap-decline chatops notification: {e:#}"
                );
            }
        }
        state.cap_decline_posted_for_code_review = true;
        write_state(paths, workspace, state)?;
        Ok(())
    }

    /// Post the one-time automatic-revision cap-decline (PR comment +
    /// chatops notification), guarded by `cap_decline_posted`. Persists the
    /// flag.
    async fn maybe_post_auto_cap_decline(&self, state: &mut RevisionState) -> Result<()> {
        if state.cap_decline_posted {
            return Ok(());
        }
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let pr_text = format!(
            "🛑 Revision cap reached ({} automatic revisions). Further reviewer-initiated revisions on this PR will be ignored; human `@{} revise` requests still process. Close + re-open or merge as-is.",
            state.revision_cap, bot_username,
        );
        if let Err(e) = forge.post_comment(token,
            owner,
            repo_name,
            pr.number,
            &pr_text,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "failed to post cap-decline PR comment: {e:#}"
            );
        }
        if let Some(ctx) = self.chatops_ctx {
            let chat_text = format!(
                "🛑 {}: PR #{} hit the revision cap of {}. Further revision requests ignored.",
                repo.url, pr.number, state.revision_cap,
            );
            if let Err(e) = ctx.chatops.post_notification(ctx.channel, &chat_text).await {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "failed to post cap-decline chatops notification: {e:#}"
                );
            }
        }
        state.cap_decline_posted = true;
        write_state(paths, workspace, state)?;
        Ok(())
    }

    /// Post the one-time human-revise cap-decline (PR comment + chatops
    /// notification), guarded by `human_revise_cap_decline_posted`. Persists
    /// the flag.
    async fn maybe_post_human_cap_decline(
        &self,
        state: &mut RevisionState,
        human_revise_cap: u32,
    ) -> Result<()> {
        if state.human_revise_cap_decline_posted {
            return Ok(());
        }
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let pr_text = format!(
            "🛑 Human-revision cap reached ({} `@{} revise` requests on this PR). Further revise requests will be ignored. Close + re-open or merge as-is.",
            human_revise_cap, bot_username,
        );
        if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &pr_text,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "failed to post human-revise cap-decline PR comment: {e:#}"
            );
        }
        if let Some(ctx) = self.chatops_ctx {
            let chat_text = format!(
                "🛑 {}: PR #{} hit the human-revise cap of {}. Further @{} revise requests ignored.",
                repo.url, pr.number, human_revise_cap, bot_username,
            );
            if let Err(e) = ctx.chatops.post_notification(ctx.channel, &chat_text).await {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "failed to post human-revise cap-decline chatops notification: {e:#}"
                );
            }
        }
        state.human_revise_cap_decline_posted = true;
        write_state(paths, workspace, state)?;
        Ok(())
    }

    /// Per a20a5: assemble the executor's revision context from PR-sourced
    /// material. The change name is derived from the PR body (first change
    /// listed; v1 supports a single revision target per PR — multi-change
    /// resolution is delegated to the LLM via the `pr_change_list` field).
    /// Fetches all-time PR comments to extract the original implementer's
    /// `## Agent implementation notes`, then posts the best-effort
    /// "picked up" lifecycle alert.
    ///
    /// Returns `None` when the all-comments fetch fails: a clear failure
    /// comment is posted AND the caller must NOT advance the seen-marker, so
    /// the next iteration re-attempts assembly. (The caller maps `None` to
    /// `CommentFlow::Break`.)
    async fn assemble_revise_plan(
        &self,
        comment: &github::IssueComment,
        revision_text: &str,
    ) -> Option<RevisePlan> {
        let forge = &self.forge;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        // Apply the revision. The change name is derived from the PR's
        // body (the first change listed); v1 supports a single revision
        // target per PR. (Multi-change resolution is delegated to the
        // LLM via a20a5's pr_change_list field — the dispatcher still
        // uses the first slug for state-file naming + log routing.)
        let change_list = extract_change_list_from_pr_body(pr.body.as_deref());
        let change_name = change_list
            .first()
            .cloned()
            .unwrap_or_else(|| repo.agent_branch.clone());

        // Per a20a5: assemble the executor's revision context from
        // PR-sourced material. Fetch all-time PR comments to extract
        // the original implementer's `## Agent implementation notes`.
        // If the fetch fails, post a clear failure comment AND DO NOT
        // advance the comment-seen marker — the next iteration's
        // dispatcher pass re-attempts the assembly so transient API
        // errors don't lose the operator's revise comment.
        let all_comments = match forge.list_comments_since(token,
            owner,
            repo_name,
            pr.number,
            chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        )
        .await
        {
            Ok(cs) => cs,
            Err(e) => {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "revise: PR-context assembly failed (comments fetch): {e:#}; refusing without advancing the seen-marker"
                );
                let truncated_err: String =
                    format!("{e:#}").chars().take(200).collect();
                let body = format!(
                    "✗ Cannot revise: failed to fetch PR context: {truncated_err}. The daemon will retry on the next polling iteration. If this persists, check journalctl for the daemon's GitHub API errors AND verify the bot's token has Read access on this repo."
                );
                let _ = forge.post_comment(token, owner, repo_name, pr.number, &body,
                )
                .await;
                // CRITICAL: do NOT advance latest_seen — re-attempt
                // on the next iteration. Signal the caller to break out
                // of the trigger loop; subsequent triggers (if any) also
                // get re-fetched on the next iteration.
                return None;
            }
        };
        let agent_implementation_notes =
            extract_agent_implementation_notes(&all_comments);
        let pr_body = pr.body.clone().unwrap_or_default();
        let pr_change_list_str = change_list.join("\n");

        // Revise-lifecycle "picked up" notification (best-effort,
        // deduplicated per comment_id). Posted BEFORE the executor
        // subprocess launches so the operator sees near-immediate
        // acknowledgment in chat. The change-list summary mirrors
        // the PR-title shape: `<first_change>` plus an optional
        // `+N more` when the bundled iteration covers multiple
        // changes.
        let comment_id_str = comment.id.to_string();
        let change_list_summary =
            crate::polling_loop::format_revise_change_list_summary(&change_list);
        let operator_quote =
            crate::polling_loop::truncate_operator_comment(revision_text, 80);
        crate::polling_loop::maybe_post_revise_picked_up_alert(
            self.paths,
            self.chatops_ctx,
            repo,
            pr.number,
            &pr.url,
            &change_list_summary,
            &operator_quote,
            &comment_id_str,
        )
        .await;

        Some(RevisePlan {
            change_name,
            change_list_summary,
            comment_id_str,
            pr_body,
            pr_change_list_str,
            agent_implementation_notes,
        })
    }

    /// Dispatch on the executor's revision outcome. Each failure-shaped arm
    /// (`Failed`, `PreconditionUnmet`, `SpecNeedsRevision`,
    /// `IterationRequested`, `Err`) carries only its unique alert reason,
    /// reply body, count policy, AND post-error policy, then defers the
    /// shared tail (post alert → post reply → count attempt → advance +
    /// persist) to [`ReviseCtx::finalize_revise_failure`]. `Completed` has
    /// its own commit/no-change/re-review logic in
    /// [`ReviseCtx::handle_completed_revision`]; `AskUser` AND `Aborted`
    /// persist prior progress and signal `Return`.
    #[allow(clippy::too_many_arguments)]
    async fn handle_revision_outcome(
        &self,
        outcome: Result<ExecutorOutcome>,
        change_name: &str,
        revision_text: &str,
        change_list_summary: &str,
        comment_id_str: &str,
        is_automatic: bool,
        revise_duration: std::time::Duration,
        comment: &github::IssueComment,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
    ) -> Result<CommentFlow> {
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let bot_username = self.bot_username;
        match outcome {
            Ok(ExecutorOutcome::Completed { final_answer }) => {
                self.handle_completed_revision(
                    final_answer,
                    change_name,
                    revision_text,
                    change_list_summary,
                    comment_id_str,
                    is_automatic,
                    revise_duration,
                    comment,
                    state,
                    latest_seen,
                )
                .await?;
            }
            Ok(ExecutorOutcome::AskUser { question, resume_handle }) => {
                // AskUser → existing chatops escalation. No commit, no
                // count increment, no PR reply. `last_seen_comment_at`
                // is NOT advanced past this comment so the next iteration
                // can resume against it.
                let _handle = resume_handle;
                if let Some(ctx) = self.chatops_ctx {
                    let chat_text = format!(
                        "❓ Revision on {} PR #{} needs clarification: {}",
                        repo.url, pr.number, question,
                    );
                    if let Err(e) = ctx.chatops.post_notification(ctx.channel, &chat_text).await {
                        tracing::warn!(
                            url = %repo.url,
                            pr_number = pr.number,
                            "failed to post AskUser chatops notification: {e:#}"
                        );
                    }
                }
                // Persist progress on prior comments only — do NOT advance
                // past the current (unresolved) comment.
                if let Some(t) = *latest_seen {
                    state.last_seen_comment_at = t;
                    write_state(paths, workspace, state)?;
                }
                return Ok(CommentFlow::Return);
            }
            Ok(ExecutorOutcome::Failed { reason }) => {
                let body = format!(
                    "✗ Revision attempt failed: {}. The PR is unchanged. Reply with another `@{} revise ...` to retry, or close the PR if the request cannot be satisfied.",
                    reason, bot_username
                );
                self.finalize_revise_failure(
                    state,
                    latest_seen,
                    comment,
                    comment_id_str,
                    is_automatic,
                    true,
                    &reason,
                    &body,
                    Some("failed to post failure PR comment"),
                )
                .await?;
            }
            Ok(ExecutorOutcome::PreconditionUnmet { reason }) => {
                // a74: the agent subprocess never STARTED — a required
                // precondition was unmet (e.g. the a006 OS-sandbox-mechanism
                // gate refused to spawn with no usable mechanism AND no
                // unsandboxed opt-in). No revision work was attempted, so this
                // does NOT charge a revision slot (neither the automatic
                // `auto_revisions_applied` nor the human `human_revise_count`
                // counter is incremented). We still post a failure reply that
                // directs the operator to resolve the precondition AND post a
                // new revision request, AND we advance the seen-marker so the
                // daemon does NOT auto-retry — an unmet precondition will not
                // heal between polls, so a deliberate operator re-trigger is
                // the right recovery. No commit or push is made.
                let body = format!(
                    "✗ Revision could not start: {}. The agent subprocess never started, so no revision was attempted AND this does NOT count against the revision cap. Resolve the precondition (see the message above), then reply with another `@{} revise ...` to re-trigger — the daemon will NOT retry automatically.",
                    reason, bot_username
                );
                self.finalize_revise_failure(
                    state,
                    latest_seen,
                    comment,
                    comment_id_str,
                    is_automatic,
                    // No count increment — no revision was attempted.
                    false,
                    &reason,
                    &body,
                    Some("failed to post precondition-unmet PR comment"),
                )
                .await?;
            }
            Ok(ExecutorOutcome::SpecNeedsRevision { .. }) => {
                // The revise-lifecycle "failed" notification surfaces the
                // iteration framing for chat operators. The pending-side
                // `maybe_post_spec_revision_alert` continues to fire from
                // its own canonical site when a SpecNeedsRevision marker
                // is observed during a pending-change run; this lifecycle
                // notification is additive and per-revise-comment.
                let body = "✗ Revision attempt failed: executor reported the original change spec is unimplementable. The PR is unchanged."
                    .to_string();
                self.finalize_revise_failure(
                    state,
                    latest_seen,
                    comment,
                    comment_id_str,
                    is_automatic,
                    true,
                    "spec needs revision (see PR comment for details)",
                    &body,
                    None,
                )
                .await?;
            }
            Ok(ExecutorOutcome::IterationRequested { .. }) => {
                // Revisions are single-shot bug fixes against a merged PR;
                // they don't have the iteration-pending state machine that
                // pending changes do. Treat IterationRequested as a Failed-
                // equivalent so the PR comment surfaces the unhandled case.
                let body = format!(
                    "✗ Revision attempt failed: executor returned IterationRequested (iteration sequences are not supported on the revise path). The PR is unchanged. Reply with another `@{} revise ...` to retry.",
                    bot_username
                );
                self.finalize_revise_failure(
                    state,
                    latest_seen,
                    comment,
                    comment_id_str,
                    is_automatic,
                    true,
                    "executor returned IterationRequested (iteration sequences are not supported on the revise path)",
                    &body,
                    None,
                )
                .await?;
            }
            Ok(ExecutorOutcome::Aborted { reason }) => {
                // a39: subprocess killed by the daemon's own SIGTERM
                // cascade. Do NOT bump auto_revisions_applied, do NOT post
                // a failure alert, AND do NOT advance latest_seen — so
                // the next iteration after restart re-enters this same
                // comment AND retries.
                tracing::info!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "revision: executor aborted by daemon shutdown: {reason}"
                );
                // Persist progress on prior comments only.
                if let Some(t) = *latest_seen {
                    state.last_seen_comment_at = t;
                    write_state(paths, workspace, state)?;
                }
                return Ok(CommentFlow::Return);
            }
            Err(e) => {
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "revision executor invocation errored: {e:#}"
                );
                let executor_error_reason = format!("executor error: {e:#}");
                let body = format!(
                    "✗ Revision attempt failed: {}. The PR is unchanged. Reply with another `@{} revise ...` to retry, or close the PR if the request cannot be satisfied.",
                    e, bot_username
                );
                self.finalize_revise_failure(
                    state,
                    latest_seen,
                    comment,
                    comment_id_str,
                    is_automatic,
                    true,
                    &executor_error_reason,
                    &body,
                    None,
                )
                .await?;
            }
        }
        Ok(CommentFlow::Continue)
    }

    /// Handle the `Completed` revision outcome (a52): branch on the
    /// working-tree state. A dirty tree is an applied change to commit +
    /// push (a commit/push failure routes to the failure reply + auto-cap
    /// increment, exactly as before a52); a clean tree is a reasoned
    /// no-change declination that must NOT read as a failure. Both terminal
    /// branches count the attempt against the cap, fire the success
    /// lifecycle alert, post the appropriate reply, advance the seen-marker,
    /// AND persist.
    #[allow(clippy::too_many_arguments)]
    async fn handle_completed_revision(
        &self,
        final_answer: Option<String>,
        change_name: &str,
        revision_text: &str,
        change_list_summary: &str,
        comment_id_str: &str,
        is_automatic: bool,
        revise_duration: std::time::Duration,
        comment: &github::IssueComment,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
    ) -> Result<()> {
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        let bot_username = self.bot_username;
        let chatops_ctx = self.chatops_ctx;
        let push_remote = self.push_remote;

        let commit_subject = build_commit_subject(change_name, revision_text);
        // a52: a `Completed` outcome may carry code changes OR be a
        // deliberate no-change declination (the agent verified the
        // request's claim against the cited code and concluded it was
        // wrong, so it made no edit). Branch on the working-tree
        // state: a dirty tree is an applied change to commit + push;
        // a clean tree is a reported declination that must NOT be
        // treated as a commit/push failure.
        let tree_dirty = match crate::git::status_porcelain(workspace) {
            Ok(porcelain) => !porcelain.is_empty(),
            Err(e) => {
                // Reading the tree state failed; assume dirty so the
                // commit path runs (preserving pre-a52 behavior). A
                // genuinely empty commit still surfaces via the
                // commit/push-failure branch below.
                tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    "revision: could not read working-tree state; assuming dirty: {e:#}"
                );
                true
            }
        };
        // Short-circuit: `apply_revision_commit` is only invoked on a
        // dirty tree (the clean branch never commits). A genuine
        // commit/push failure routes to the failure comment + cap
        // increment, exactly as before a52.
        if tree_dirty
            && let Err(e) =
                apply_revision_commit(workspace, repo, push_remote, &commit_subject)
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "revision commit/push failed; reporting as failed: {e:#}"
            );
            let push_failure_reason = format!("push to {} failed: {e}", repo.agent_branch);
            crate::polling_loop::maybe_post_revise_failed_alert(
                paths,
                chatops_ctx,
                repo,
                pr.number,
                &pr.url,
                &push_failure_reason,
                comment_id_str,
            )
            .await;
            let body = format!(
                "✗ Revision attempt failed: commit/push failed: {e}. The PR is unchanged. Reply with another `@{} revise ...` to retry, or close the PR if the request cannot be satisfied.",
                bot_username
            );
            let _ =
                forge.post_comment(token, owner, repo_name, pr.number, &body)
                    .await;
            if is_automatic {
                state.auto_revisions_applied = state.auto_revisions_applied.saturating_add(1);
            }
            advance_seen(latest_seen, comment.created_at);
            write_state(paths, workspace, state)?;
            return Ok(());
        }
        // Both branches count the attempt against the cap AND fire the
        // same chatops success notification — the revision was
        // processed, whether or not it produced a diff.
        count_revise_attempt(state, is_automatic);
        crate::polling_loop::maybe_post_revise_succeeded_alert(
            paths,
            chatops_ctx,
            repo,
            pr.number,
            &pr.url,
            change_list_summary,
            &repo.agent_branch,
            revise_duration,
            comment_id_str,
        )
        .await;
        // a52: the dirty branch posts `✅ Revision applied:`; the
        // clean branch posts the distinct `✅ Revision evaluated, no
        // change made:` line. Both carry the agent's `final_answer`.
        let reply = if tree_dirty {
            compose_revision_success_comment(
                &commit_subject,
                is_automatic,
                state.auto_revisions_applied,
                state.revision_cap,
                final_answer.as_deref(),
            )
        } else {
            compose_revision_no_change_comment(
                &commit_subject,
                is_automatic,
                state.auto_revisions_applied,
                state.revision_cap,
                final_answer.as_deref(),
            )
        };
        if let Err(e) = forge.post_comment(token, owner, repo_name, pr.number, &reply,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "failed to post success PR comment: {e:#}"
            );
        }
        // A successfully applied (dirty-tree) revision clears the change's
        // `.needs-spec-revision.json` marker if present: the commit + push
        // succeeded above, so the flagged spec has been revised in this PR
        // and the marker's hold is redundant (the open PR already parks the
        // repo). Best-effort — the marker is non-authoritative runtime state,
        // so a delete failure is logged at WARN but does NOT fail the
        // revision. The clean-tree declination branch deliberately skips this:
        // no revision was applied, so a flagged concern may still stand.
        if tree_dirty {
            match crate::queue::remove_revision_marker_idempotent(workspace, change_name) {
                Ok(true) => tracing::info!(
                    url = %repo.url,
                    pr_number = pr.number,
                    change = %change_name,
                    "cleared .needs-spec-revision.json marker after applied revision"
                ),
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    url = %repo.url,
                    pr_number = pr.number,
                    change = %change_name,
                    "failed to clear .needs-spec-revision.json marker after applied revision (revision still succeeded): {e:#}"
                ),
            }
        }
        // a33 task 7.3: maybe-post the re-review suggestion. Only the
        // dirty branch moved the agent-branch head, so the clean
        // (no-change) branch skips it — there is nothing new to
        // re-review.
        if tree_dirty {
            maybe_post_rereview_suggestion(
                workspace,
                repo,
                self.reviewer,
                pr,
                state,
                chatops_ctx,
            )
            .await;
        }
        advance_seen(latest_seen, comment.created_at);
        write_state(paths, workspace, state)?;
        Ok(())
    }

    /// Shared post-processing for the failure-shaped executor-outcome arms:
    /// post the revise-lifecycle "failed" alert, post the failure reply,
    /// optionally count the attempt against the cap, advance the
    /// seen-marker, AND persist. Each caller supplies only its unique
    /// `alert_reason`, `reply_body`, count policy (`count_attempt`), AND
    /// post-error policy (`post_err_warn`: `Some(msg)` warns with that
    /// message on a failed reply post; `None` swallows the error, matching
    /// the original per-arm behavior).
    #[allow(clippy::too_many_arguments)]
    async fn finalize_revise_failure(
        &self,
        state: &mut RevisionState,
        latest_seen: &mut Option<DateTime<Utc>>,
        comment: &github::IssueComment,
        comment_id_str: &str,
        is_automatic: bool,
        count_attempt: bool,
        alert_reason: &str,
        reply_body: &str,
        post_err_warn: Option<&str>,
    ) -> Result<()> {
        let forge = &self.forge;
        let paths = self.paths;
        let workspace = self.workspace;
        let repo = self.repo;
        let pr = self.pr;
        let owner = self.owner;
        let repo_name = self.repo_name;
        let token = self.token;
        crate::polling_loop::maybe_post_revise_failed_alert(
            paths,
            self.chatops_ctx,
            repo,
            pr.number,
            &pr.url,
            alert_reason,
            comment_id_str,
        )
        .await;
        let post = forge
            .post_comment(token, owner, repo_name, pr.number, reply_body)
            .await;
        if let (Some(msg), Err(e)) = (post_err_warn, &post) {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "{msg}: {e:#}"
            );
        }
        if count_attempt {
            count_revise_attempt(state, is_automatic);
        }
        advance_seen(latest_seen, comment.created_at);
        write_state(paths, workspace, state)?;
        Ok(())
    }
}

/// Count a terminal revision attempt against the appropriate per-PR cap:
/// AUTOMATIC (reviewer-marked) revisions bump `auto_revisions_applied`;
/// HUMAN `@<bot> revise` requests bump `human_revise_count` (a000). Mirrors
/// the increment the original outcome arms repeated inline.
fn count_revise_attempt(state: &mut RevisionState, is_automatic: bool) {
    if is_automatic {
        state.auto_revisions_applied = state.auto_revisions_applied.saturating_add(1);
    } else {
        // a000: a human revise attempt counts toward the per-PR
        // human-revise cap, mirroring the automatic counter's
        // terminal-outcome increment.
        state.human_revise_count = state.human_revise_count.saturating_add(1);
    }
}
