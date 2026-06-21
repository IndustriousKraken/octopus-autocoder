use super::*;

/// Assemble the `ReviewContext` for the reviewer: archived-change briefs
/// (proposal/design/tasks), full contents of every modified file, and the
/// unified diff. Reviewer enforces the 2M-char prompt budget when
/// rendering; this builder is unconstrained — it gathers everything and
/// lets the reviewer drop/include in priority order.
pub(crate) fn build_review_context(
    workspace: &Path,
    repo: &RepositoryConfig,
    processed: &[String],
    processed_issues: &[String],
    reviewer_kind: crate::config::ReviewerKind,
) -> Result<crate::code_reviewer::ReviewContext> {
    let diff = git::diff_three_dot(workspace, &repo.base_branch, &repo.agent_branch)?;
    let file_list = git::diff_files_changed(workspace, &repo.base_branch, &repo.agent_branch)?;

    // a58 revision: the agentic reviewer reads files on demand through its
    // read-only sandbox — `render_agentic_review_prompt` lists only the
    // changed-file PATHS and never inlines contents. So for the agentic
    // transport, skip the eager full-file `read_to_string` of every touched
    // file: those reads are wasted I/O AND the dominant memory allocation on
    // large passes, partially defeating the agentic reviewer's whole point.
    // The oneshot path still pre-dumps contents into its prompt, so it keeps
    // reading them. Deleted files (absent on disk) stay excluded from the
    // path list in BOTH transports, matching the prior behavior.
    let include_file_contents = matches!(reviewer_kind, crate::config::ReviewerKind::Oneshot);

    let mut changed_files = Vec::with_capacity(file_list.len());
    for path in &file_list {
        let abs = workspace.join(path);
        if include_file_contents {
            match std::fs::read_to_string(&abs) {
                Ok(contents) => changed_files.push(crate::code_reviewer::ChangedFile {
                    path: path.clone(),
                    contents,
                }),
                // Deleted files appear in the diff but have no current
                // content. Their removal is captured by the diff itself.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        path = %path,
                        "skipping changed-file read for reviewer: {e}"
                    );
                    continue;
                }
            }
        } else if abs.exists() {
            // Agentic transport: list the path with empty contents (the agent
            // reads on demand). A cheap existence check still excludes deleted
            // files so the agent is not pointed at a path that no longer
            // exists — but no file body is read into memory.
            changed_files.push(crate::code_reviewer::ChangedFile {
                path: path.clone(),
                contents: String::new(),
            });
        }
    }

    let archive_root = workspace.join("openspec/changes/archive");
    let mut archived_changes = Vec::with_capacity(processed.len());
    for name in processed {
        let dir = match locate_archive_dir(&archive_root, name)? {
            Some(d) => d,
            None => {
                tracing::warn!(
                    change = %name,
                    "archive directory not found while building review context"
                );
                continue;
            }
        };
        let proposal = std::fs::read_to_string(dir.join("proposal.md")).unwrap_or_default();
        let design = std::fs::read_to_string(dir.join("design.md")).ok();
        let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
        archived_changes.push(crate::code_reviewer::ChangeBrief {
            name: name.clone(),
            proposal,
            design,
            tasks,
        });
    }

    // Issue briefs (a009): a worked issue is archived under `issues/archive/`,
    // not `changes/archive/`, and carries no proposal/design — its `issue.md`
    // (the report + acceptance criteria) and `tasks.md` (the fix steps) are
    // the reviewer's intent context. Load them as briefs so an issue PR is
    // reviewed WITH the issue's intent, like a change PR with its proposal.
    let issues_archive_root = crate::lanes::issues::archive_root(workspace);
    for slug in processed_issues {
        match locate_archived_issue(&issues_archive_root, slug)? {
            Some((proposal, tasks)) => {
                archived_changes.push(crate::code_reviewer::ChangeBrief {
                    name: slug.clone(),
                    proposal,
                    design: None,
                    tasks,
                });
            }
            None => {
                tracing::warn!(
                    issue = %slug,
                    "issue archive entry not found while building review context; \
                     reviewing the diff without the issue brief"
                );
            }
        }
    }

    Ok(crate::code_reviewer::ReviewContext {
        archived_changes,
        changed_files,
        diff,
        target: None,
    })
}

/// Assemble one `PerChangeContext` per change in `processed`, used by
/// the reviewer's `per_change` mode dispatch. Each per-change context is
/// scoped to:
/// - the change's own brief (proposal/design/tasks),
/// - the diff of the commit(s) for that change (NOT the union diff),
/// - the workspace-state contents of the files touched by those commits,
/// - a cross-change preamble naming the OTHER changes in the same pass.
///
/// Commits are located by subject-prefix (`<change>:`) using
/// `git::commits_for_change`. A change with no matching commit (or whose
/// touched-file list is empty) still produces a context, but with an
/// empty diff/files set — the reviewer's prompt for that change still
/// includes the brief + preamble, so the operator sees a deliberate
/// `## Code Review: <slug>` section instead of a silent skip.
pub(crate) fn build_per_change_contexts(
    workspace: &Path,
    repo: &RepositoryConfig,
    processed: &[String],
) -> Result<Vec<PerChangeContext>> {
    // First pass: gather briefs for all changes. The cross-change
    // preamble for change `i` needs the OTHER changes' briefs in full,
    // so we collect them all first.
    let archive_root = workspace.join("openspec/changes/archive");
    let mut briefs: Vec<crate::code_reviewer::ChangeBrief> = Vec::with_capacity(processed.len());
    for name in processed {
        let dir = match locate_archive_dir(&archive_root, name)? {
            Some(d) => d,
            None => {
                tracing::warn!(
                    change = %name,
                    "archive directory not found while building per-change review context"
                );
                continue;
            }
        };
        let proposal = std::fs::read_to_string(dir.join("proposal.md")).unwrap_or_default();
        let design = std::fs::read_to_string(dir.join("design.md")).ok();
        let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
        briefs.push(crate::code_reviewer::ChangeBrief {
            name: name.clone(),
            proposal,
            design,
            tasks,
        });
    }

    let mut contexts: Vec<PerChangeContext> = Vec::with_capacity(briefs.len());
    for brief in &briefs {
        let shas = git::commits_for_change(
            workspace,
            &repo.base_branch,
            &repo.agent_branch,
            &brief.name,
        )
        .unwrap_or_else(|e| {
            tracing::warn!(
                change = %brief.name,
                "git log --grep failed locating per-change commits; falling back to empty list: {e:#}"
            );
            Vec::new()
        });
        let diff = if shas.is_empty() {
            String::new()
        } else {
            git::diff_for_commits(workspace, &shas).unwrap_or_default()
        };
        let file_paths = if shas.is_empty() {
            Vec::new()
        } else {
            git::files_for_commits(workspace, &shas).unwrap_or_default()
        };
        let mut changed_files = Vec::with_capacity(file_paths.len());
        for path in &file_paths {
            let abs = workspace.join(path);
            match std::fs::read_to_string(&abs) {
                Ok(contents) => {
                    changed_files.push(crate::code_reviewer::ChangedFile {
                        path: path.clone(),
                        contents,
                    });
                }
                // Deleted files have no current content but still appear
                // in the per-change diff — that's fine, the diff body
                // captures the deletion.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        path = %path,
                        change = %brief.name,
                        "skipping per-change file read for reviewer: {e}"
                    );
                    continue;
                }
            }
        }

        let context = crate::code_reviewer::ReviewContext {
            archived_changes: vec![brief.clone()],
            changed_files,
            diff,
            target: None,
        };
        let preamble = build_cross_change_preamble(&brief.name, &briefs);
        contexts.push(PerChangeContext {
            change_slug: brief.name.clone(),
            context,
            cross_change_preamble: preamble,
        });
    }
    Ok(contexts)
}

/// Find the date-prefixed archive directory matching the given change name
/// (e.g. `openspec/changes/archive/2026-05-14-foo/` for `foo`). Returns
/// `Ok(None)` if no matching directory exists.
pub(crate) fn locate_archive_dir(
    archive_root: &Path,
    change: &str,
) -> Result<Option<std::path::PathBuf>> {
    if !archive_root.is_dir() {
        return Ok(None);
    }
    let suffix = format!("-{change}");
    for entry in std::fs::read_dir(archive_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name.ends_with(&suffix) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Locate an archived issue unit in EITHER form under `archive_root` and
/// return its `(proposal, tasks)` brief content. A directory-form issue
/// `<date>-<slug>/` reads `issue.md` (proposal) + `tasks.md` (tasks); a
/// single-file issue `<date>-<slug>.md` reads the file as the proposal and
/// splits out an optional `## Tasks` section as the tasks. Returns
/// `Ok(None)` when no archived entry for `slug` exists in either form.
fn locate_archived_issue(
    archive_root: &Path,
    slug: &str,
) -> Result<Option<(String, String)>> {
    if !archive_root.is_dir() {
        return Ok(None);
    }
    let dir_suffix = format!("-{slug}");
    let file_suffix = format!("-{slug}.md");
    for entry in std::fs::read_dir(archive_root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if file_type.is_dir() && name.ends_with(&dir_suffix) {
            let dir = entry.path();
            let proposal = std::fs::read_to_string(dir.join("issue.md")).unwrap_or_default();
            let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
            return Ok(Some((proposal, tasks)));
        }
        if file_type.is_file() && name.ends_with(&file_suffix) {
            let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
            return Ok(Some(crate::lanes::issues::split_brief(&body)));
        }
    }
    Ok(None)
}

/// a005: post the SINGLE aggregated `<!-- reviewer-revision -->` PR comment
/// for a review's actionable concerns. All concerns from one review ride in
/// one comment (a numbered list built by
/// [`crate::revisions::build_aggregated_reviewer_revision_comment`]), so the
/// dispatcher issues exactly one executor run — one `max_auto_revisions_per_pr`
/// increment — for the whole batch rather than one run per concern. A
/// failure logs at WARN and never aborts; the iteration's PR creation has
/// already succeeded.
pub(crate) async fn post_reviewer_revision_comments(
    api_base: &str,
    upstream_owner: &str,
    upstream_repo: &str,
    pr_number: u64,
    concerns: &[ReviewConcern],
    token: &str,
) {
    // Resolve the bot's GitHub login once — the trigger pattern is
    // `@<bot> revise ...`. Without the username we cannot construct a
    // valid trigger, so we abort the posting step (logging a WARN); the
    // iteration's PR creation still succeeded.
    let bot_username = match github::self_bot_username(api_base, token).await {
        Ok(name) => name,
        Err(e) => {
            tracing::warn!(
                pr_number,
                "reviewer-revision posting skipped: bot-username lookup failed: {e:#}"
            );
            return;
        }
    };
    let Some(body) =
        crate::revisions::build_aggregated_reviewer_revision_comment(&bot_username, concerns)
    else {
        // No actionable requests survived (shouldn't happen; the caller
        // filters to the revisable set). Nothing to post.
        return;
    };
    // a007: review/comment posting routes through the `Forge` trait's
    // `post_review` (the `Bearer`-auth comments path that the reviewer uses).
    // `api_base` is `DEFAULT_API_BASE` in production and a mockito URL in
    // tests; the GitHub provider threads it via `with_api_base`.
    use crate::forge::Forge;
    // The aggregated reviewer-revision comment is a request-changes verdict;
    // GithubForge posts it as a PR comment regardless (see `post_review`).
    let post_result = crate::forge::GithubForge::with_api_base(api_base)
        .post_review(
            upstream_owner,
            upstream_repo,
            pr_number,
            &body,
            crate::forge::ReviewDecision::RequestChanges,
            token,
        )
        .await;
    if let Err(e) = post_result {
        tracing::warn!(
            pr_number,
            "aggregated reviewer-revision comment post failed: {e:#}"
        );
    }
}

/// a005: gate + collect the reviewer-initiated revision concerns for one
/// review. Returns the set of concerns to forward as the SINGLE aggregated
/// `<!-- reviewer-revision -->` comment, or empty when nothing should fire.
///
/// Gating:
/// - `revision_cap == 0` disables reviewer-initiated revisions entirely (the
///   dispatcher is gated on the same value), so nothing is forwarded.
/// - The `auto_revise` tri-state decides whether to fire for this verdict:
///   `block` only on a `Block` verdict (`verdict_is_block`), `actionable`
///   regardless of verdict, `off` never (see [`crate::config::AutoRevise`]).
///
/// When the gate fires, ALL revisable concerns are collected — unlike the
/// pre-a005 per-concern cap budget, the whole set rides in one aggregated
/// run consuming exactly one cap slot, so no concern is dropped here.
pub(crate) fn reviewer_revisions_for_review(
    reviewer: &CodeReviewer,
    report: &ReviewReport,
    verdict_is_block: bool,
    revision_cap: u32,
) -> Vec<ReviewConcern> {
    if revision_cap == 0 || !reviewer.auto_revise().fires(verdict_is_block) {
        return Vec::new();
    }
    collect_reviewer_revisions(report)
}

/// a005: collect every revisable concern from `report` for aggregation into
/// the single reviewer-initiated revision run. A concern is revisable when
/// `should_request_revision == true` AND `actionable_request` is non-empty
/// (see [`crate::code_reviewer::ReviewConcern::is_revisable`]). The verdict
/// is NOT consulted here — verdict gating is the caller's `auto_revise`
/// tri-state decision. Unlike the pre-a005
/// `partition_and_annotate_reviewer_revisions`, this does NOT drop concerns
/// against the per-PR cap: the whole set is dispatched as ONE aggregated run
/// consuming exactly one `max_auto_revisions_per_pr` slot, so all concerns
/// ride together. Logs a WARN when concerns were surfaced but none were
/// revisable (the "flag flipped but the template emits no actionable concerns"
/// misconfiguration). A completely clean review (zero concerns) is NOT a
/// misconfiguration, so it is gated out of the WARN — otherwise the daemon
/// would spam the warning for every clean PR under `auto_revise: actionable`.
pub(crate) fn collect_reviewer_revisions(report: &ReviewReport) -> Vec<ReviewConcern> {
    let revisable: Vec<ReviewConcern> = report
        .concerns
        .iter()
        .filter(|c| c.is_revisable())
        .cloned()
        .collect();
    if revisable.is_empty() && !report.concerns.is_empty() {
        tracing::warn!(
            "reviewer auto-revise is enabled but no concerns had `actionable_request` + `should_request_revision: true` populated; verify the reviewer prompt template emits these fields."
        );
    }
    revisable
}

#[cfg(test)]
mod tests {
    use super::locate_archived_issue;
    use tempfile::TempDir;

    /// single-file-issues §3.2: the reviewer issue brief locates an archived
    /// unit in EITHER form and reads its body.
    #[test]
    fn locate_archived_issue_reads_both_forms() {
        let td = TempDir::new().unwrap();
        let archive_root = td.path().join("issues/archive");
        std::fs::create_dir_all(&archive_root).unwrap();

        // Directory form: issue.md (proposal) + tasks.md.
        let dir = archive_root.join("2026-06-06-dir-fix");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("issue.md"), "the directory report").unwrap();
        std::fs::write(dir.join("tasks.md"), "- [x] dir task").unwrap();
        let (proposal, tasks) = locate_archived_issue(&archive_root, "dir-fix")
            .unwrap()
            .expect("directory issue located");
        assert_eq!(proposal, "the directory report");
        assert_eq!(tasks, "- [x] dir task");

        // Single-file form: body split into proposal + `## Tasks`.
        std::fs::write(
            archive_root.join("2026-06-07-file-fix.md"),
            "the file report\n\n## Tasks\n\n- [x] file task\n",
        )
        .unwrap();
        let (proposal, tasks) = locate_archived_issue(&archive_root, "file-fix")
            .unwrap()
            .expect("single-file issue located");
        assert!(proposal.contains("the file report"));
        assert!(!proposal.contains("## Tasks"));
        assert!(tasks.contains("file task"));

        // Absent slug → None.
        assert!(locate_archived_issue(&archive_root, "nope").unwrap().is_none());
    }
}
