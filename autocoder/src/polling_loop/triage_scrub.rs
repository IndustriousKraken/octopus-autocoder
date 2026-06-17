use super::*;

/// a43 / architecture-advisory-redesign: discard every working-tree change
/// OUTSIDE the two planning lanes — the spec lane (`openspec/changes/`) AND
/// the issues lane (`openspec/issues/`, the path the issues walker reads via
/// [`crate::lanes::issues::ISSUES_SUBDIR`]) — reverting each out-of-scope
/// path to its committed (HEAD) state so the triage PR's commit is genuinely
/// lane-only. Returns the sorted, de-duplicated list of discarded paths so
/// the caller can log them AND surface them to chatops.
///
/// Revert strategy per non-spec path, chosen by where the path lives:
///   - **Untracked addition** (`??`): removed from disk — it has no HEAD
///     blob to restore.
///   - **Tracked path present in HEAD** (a modification, deletion,
///     type-change, OR the SOURCE side of a rename): reverted with `git
///     checkout HEAD -- <path>`, which rewrites BOTH the index AND the
///     worktree to the committed content — so a code edit the executor
///     staged with `git add` cannot survive into the spec commit.
///   - **Tracked path absent from HEAD** (a brand-new file the executor
///     created AND `git add`ed — porcelain `A ` — OR the DESTINATION of a
///     rename): unstaged with `git reset HEAD -- <path>` (which demotes it
///     to untracked) AND then removed from disk. This case is handled
///     WITHOUT `git checkout HEAD -- <path>` / `git restore --source=HEAD`
///     on purpose: those reject a pathspec absent from HEAD with a
///     "pathspec did not match any file(s) known to git" error on some git
///     versions, which would abort the whole triage flow exactly when the
///     executor `git add`ed a new code file — the common case.
///
/// Triage executor runs are planning-lane-only: any code-path write the
/// agent made despite the prompt restriction is dropped here BEFORE the
/// triage-PR commit so the PR diff carries only planning-lane content. Both
/// lanes' content is kept regardless of the executor's chosen slug — the
/// keep boundary is each lane's root rather than a single
/// `<lane>/<slug>/` path, because the executor picks its own (LLM-chosen)
/// slug AND a single triage may produce several directories. `spec_slug` is
/// the handler's derived slug, threaded for diagnostic logging. A clean
/// working tree (and a lane-only diff) both return an empty list with no
/// side effects.
pub fn discard_non_spec_writes(workspace: &Path, spec_slug: &str) -> Result<Vec<String>> {
    const SPEC_PREFIX: &str = "openspec/changes/";
    // The issues lane the issues walker reads (today `openspec/issues/`).
    let issues_prefix = format!("{}/", crate::lanes::issues::ISSUES_SUBDIR);
    let keep = |path: &str| path.starts_with(SPEC_PREFIX) || path.starts_with(&issues_prefix);
    tracing::debug!(
        spec_slug = %spec_slug,
        "discard_non_spec_writes: keeping openspec/changes/ AND issues-lane content"
    );
    let mut discarded: Vec<String> = Vec::new();
    // `status_entries` yields one record per change with the rename/copy
    // source in `orig_path`; flatten back to `(is_untracked, path)` pairs
    // — destination first, then the source as a tracked (never untracked)
    // change — so both sides of a staged rename are reverted relative to
    // HEAD.
    for (is_untracked, path) in git::status_entries(workspace)
        .with_context(|| "discard_non_spec_writes: reading git status".to_string())?
        .into_iter()
        .flat_map(|e| {
            let is_untracked = e.staged == '?' && e.worktree == '?';
            std::iter::once((is_untracked, e.path)).chain(e.orig_path.map(|o| (false, o)))
        })
    {
        if keep(&path) {
            continue;
        }
        if is_untracked {
            // Untracked addition: no HEAD blob to restore, so remove it
            // from disk.
            remove_non_spec_path_from_disk(workspace, &path, spec_slug)?;
        } else if path_exists_in_head(workspace, &path)? {
            // Tracked change to a path that EXISTS in HEAD (modification,
            // deletion, type-change, OR a rename source). `git checkout
            // HEAD -- <path>` rewrites BOTH the index AND the worktree to
            // the committed content — so a code edit the executor staged
            // with `git add` cannot survive into the spec commit.
            run_git_revert(
                workspace,
                &["checkout", "-q", "HEAD", "--", path.as_str()],
                &path,
                spec_slug,
            )?;
        } else {
            // Tracked change to a path ABSENT from HEAD: a brand-new file
            // the executor `git add`ed (porcelain `A `) OR a rename
            // destination. `git checkout HEAD -- <path>` / `git restore
            // --source=HEAD` reject a not-in-HEAD pathspec on some git
            // versions, so unstage it (`git reset HEAD -- <path>` demotes
            // it to untracked) AND remove it from disk.
            run_git_revert(
                workspace,
                &["reset", "-q", "HEAD", "--", path.as_str()],
                &path,
                spec_slug,
            )?;
            remove_non_spec_path_from_disk(workspace, &path, spec_slug)?;
        }
        discarded.push(path);
    }
    discarded.sort();
    discarded.dedup();
    Ok(discarded)
}

/// Remove a non-spec working-tree path from disk (symlink-safe), treating
/// an already-absent path as success. Shared by the untracked-addition
/// branch AND the not-in-HEAD tracked branch (a staged add OR rename
/// destination that `git reset` has just demoted to untracked) of
/// `discard_non_spec_writes`. Any failure other than "already gone" is
/// fatal: a surviving file would be swept into the spec-only commit by the
/// caller's subsequent `git add -A`, silently violating the spec-only
/// invariant, so we fail loudly rather than let the write leak.
fn remove_non_spec_path_from_disk(workspace: &Path, path: &str, spec_slug: &str) -> Result<()> {
    let abs = workspace.join(path);
    // Decide dir-vs-file from the path's OWN metadata (lstat), not
    // `is_dir()` which follows symlinks: git reports an untracked symlink
    // as a single entry, AND following it into `remove_dir_all` could
    // delete the link TARGET's contents. `symlink_metadata` reports a
    // symlink as a non-dir, so it is unlinked via `remove_file` (dropping
    // just the link). A real untracked directory still routes to
    // `remove_dir_all`.
    let is_real_dir = abs.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false);
    let removal = if is_real_dir {
        std::fs::remove_dir_all(&abs)
    } else {
        std::fs::remove_file(&abs)
    };
    if let Err(e) = removal
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            spec_slug = %spec_slug,
            path = %path,
            "discard_non_spec_writes: failed to remove non-spec write: {e:#}"
        );
        return Err(anyhow!(
            "discard_non_spec_writes: failed to remove non-spec write `{path}`: {e}; \
             refusing to proceed so it does not leak into the spec-only PR"
        ));
    }
    Ok(())
}

/// Whether `path` exists as a blob in HEAD — i.e. `git cat-file -e
/// HEAD:<path>` succeeds. Picks the revert strategy for a tracked non-spec
/// change in `discard_non_spec_writes`: a path IN HEAD is reverted with
/// `git checkout HEAD -- <path>`; a path NOT in HEAD (a staged add OR a
/// rename destination) is unstaged AND deleted instead. A failure to spawn
/// git propagates; a clean non-zero exit (the blob is absent, with git's
/// diagnostic captured rather than spilled to stderr) is reported as
/// `false`.
fn path_exists_in_head(workspace: &Path, path: &str) -> Result<bool> {
    let out = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("discard_non_spec_writes: spawning git cat-file for `{path}`"))?;
    Ok(out.status.success())
}

/// Run a git working-tree revert subprocess (`checkout`/`reset`) for
/// `discard_non_spec_writes`, capturing diagnostics via `output()` (rather
/// than letting `status()` spill git's stderr to the daemon's inherited
/// stderr) AND surfacing them in the error so a failed revert is
/// debuggable (mirrors the `git::run_git` contract). A non-zero exit is
/// fatal: we refuse to proceed so the non-spec write cannot leak into the
/// spec-only PR.
fn run_git_revert(workspace: &Path, args: &[&str], path: &str, spec_slug: &str) -> Result<()> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| {
            format!(
                "discard_non_spec_writes: spawning `git {}` for `{path}`",
                args.join(" ")
            )
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let diag = match (stderr.is_empty(), stdout.is_empty()) {
            (false, _) => stderr,
            (true, false) => stdout,
            (true, true) => format!("(no output; exit {:?})", out.status.code()),
        };
        tracing::warn!(
            spec_slug = %spec_slug,
            path = %path,
            "discard_non_spec_writes: `git {}` exited non-zero reverting non-spec write: {diag}",
            args.join(" ")
        );
        return Err(anyhow!(
            "discard_non_spec_writes: `git {}` exited non-zero ({diag}); \
             refusing to proceed so the non-spec write does not leak into the spec-only PR",
            args.join(" ")
        ));
    }
    Ok(())
}

/// Read the workspace's `openspec/specs/` directory and produce a brief
/// listing of the canonical spec names available. Used by the triage
/// prompt's `{{canonical_specs_index}}` substitution.
pub(crate) fn build_canonical_specs_index(workspace: &Path) -> String {
    let specs_dir = workspace.join("openspec/specs");
    if !specs_dir.is_dir() {
        return "(no openspec/specs/ directory found)".to_string();
    }
    let mut names: Vec<String> = match std::fs::read_dir(&specs_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => return "(error reading openspec/specs/)".to_string(),
    };
    names.sort();
    if names.is_empty() {
        return "(no specs in openspec/specs/)".to_string();
    }
    names
        .iter()
        .map(|n| format!("- {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Flip the audit-thread state to `TriageFailed` and post the failure
/// to the audit thread. Best-effort — every failure path here logs and
/// continues so the surrounding iteration is unaffected.
pub(crate) async fn mark_triage_failed(
    _paths: &DaemonPaths,
    state_root: &Path,
    state: &mut crate::audits::threads::AuditThreadState,
    reason: String,
    chatops_ctx: Option<&ChatOpsContext>,
) {
    use crate::audits::threads::{self, AuditThreadStatus};
    state.status = AuditThreadStatus::TriageFailed;
    state.reason = Some(reason.clone());
    if let Err(e) = threads::write_state(state_root, state) {
        tracing::warn!(
            thread_ts = %state.thread_ts,
            "audit-triage: failed to record TriageFailed state: {e:#}"
        );
    }
    if let Some(ctx) = chatops_ctx {
        let body = format!(
            "✗ Triage for `{audit_type}` on `{repo_url}` failed: {reason}\n\nReply `@<bot> send it` to retry, or revise the audit and re-run.",
            audit_type = state.audit_type,
            repo_url = state.repo_url,
        );
        if let Err(e) = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.thread_ts, &body)
            .await
        {
            tracing::warn!(
                thread_ts = %state.thread_ts,
                "audit-triage: TriageFailed thread reply failed: {e:#}"
            );
        }
    }
}
