//! Canon-and-archive guard (guard-canon-and-archive).
//!
//! The invariant: **canon and the archive are autocoder-only.** Folding a
//! change's deltas into the canonical specs under `openspec/specs/`, AND
//! archiving a change (running `openspec archive`, OR creating/moving an entry
//! under `openspec/changes/archive/` or `<issues>/archive/`), are the daemon's
//! responsibilities — performed ONLY after a change is implemented AND merged.
//! No executor session — the changes-lane implementer, the spec-revision
//! (`send it`) executor, a spec-writing audit, audit triage, OR a proposer —
//! may modify canon or archive a change as part of its run. A session that does
//! so bypasses the post-executor gate AND double-applies the deltas on merge
//! (the archive folds them a second time).
//!
//! Enforcement is defense in depth across three layers; this module is the
//! single source of the reusable pieces:
//!
//! 1. **Teach (prompt).** [`with_octopus_directive`] points a writing session
//!    at the repository's `OCTOPUS.md` for the workflow conventions AND these
//!    constraints when that file is present; absence is a graceful no-op.
//! 2. **Prevent (sandbox).** [`SANDBOX_DENY_ENTRIES`] are folded into every
//!    session's CLI tool-use deny list by
//!    [`crate::audits::write_sandbox_settings`] — denying writes under
//!    `openspec/specs/` AND execution of `openspec archive`. The session's own
//!    planning artifact (`openspec/changes/<slug>/`, the issue unit) AND the
//!    implementer's code edits stay writable.
//! 3. **Catch (revert).** [`enforce`] reverts any canon edit AND any archive
//!    entry the session produced, BEFORE its commit, preserving its legitimate
//!    writes — the fail-closed backstop, modeled on the audit planning-lane
//!    post-hoc revert ([`crate::polling_loop::triage_scrub`]). It runs
//!    regardless of whether the prompt or sandbox layers held.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::git;

/// The canonical-specs root. Modifying anything under it is autocoder-only.
pub const CANON_PREFIX: &str = "openspec/specs/";

/// The change-archive root. Creating/moving an entry under it is autocoder-only.
pub const CHANGES_ARCHIVE_PREFIX: &str = "openspec/changes/archive/";

/// The filename of the repository-root workflow guide a writing session is
/// pointed at when present.
pub const OCTOPUS_FILENAME: &str = "OCTOPUS.md";

/// The issues-lane archive prefix (`<ISSUES_SUBDIR>/archive/`). Bound to the
/// constant the issues walker reads ([`crate::lanes::issues::ISSUES_SUBDIR`])
/// so canon (`issues/` today) and this guard never drift.
pub fn issues_archive_prefix() -> String {
    format!("{}/archive/", crate::lanes::issues::ISSUES_SUBDIR)
}

/// CLI tool-use deny entries that block canon writes AND `openspec archive`
/// for EVERY session. Folded into the settings deny list by
/// [`crate::audits::write_sandbox_settings`] so the executor (which otherwise
/// allows `Write`/`Edit`) still cannot write canon or archive a change, while
/// read-only roles — which already deny `Write(*)`/`Edit(*)` — are unaffected
/// (these are redundant no-ops for them). The `openspec archive` bash deny is
/// included here unconditionally so it holds even if an operator's config
/// overrides the default `disallowed_bash_patterns`. Both a workspace-relative
/// spelling AND a `**/`-prefixed spelling are listed so the deny matches
/// whether the agent passes a relative or an absolute path.
pub const SANDBOX_DENY_ENTRIES: &[&str] = &[
    "Write(openspec/specs/**)",
    "Edit(openspec/specs/**)",
    "Write(**/openspec/specs/**)",
    "Edit(**/openspec/specs/**)",
    "Bash(openspec archive:*)",
    "Bash(openspec unarchive:*)",
];

/// Which autocoder-only boundary a guarded path crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// A canonical spec under `openspec/specs/`.
    Canon,
    /// An archive entry under `openspec/changes/archive/` or `<issues>/archive/`.
    Archive,
}

/// Classify a workspace-relative path against the two autocoder-only roots:
/// `Some(Canon)` under `openspec/specs/`, `Some(Archive)` under a change/issue
/// archive root, `None` for anything else (a change's own delta dir, an issue
/// unit, code — all writable).
pub fn classify(path: &str) -> Option<GuardKind> {
    if path.starts_with(CANON_PREFIX) {
        Some(GuardKind::Canon)
    } else if path.starts_with(CHANGES_ARCHIVE_PREFIX) || path.starts_with(&issues_archive_prefix())
    {
        Some(GuardKind::Archive)
    } else {
        None
    }
}

/// What a guard pass found (and, after [`enforce`], reverted): the canon edits
/// AND the archive entries the session produced, each a sorted, de-duplicated
/// list of workspace-relative paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardReport {
    pub canon: Vec<String>,
    pub archive: Vec<String>,
}

impl GuardReport {
    /// No violation — the session touched neither canon nor the archive.
    pub fn is_empty(&self) -> bool {
        self.canon.is_empty() && self.archive.is_empty()
    }

    /// Every guarded path, canon then archive, sorted and de-duplicated.
    pub fn all_paths(&self) -> Vec<String> {
        let mut out: Vec<String> =
            self.canon.iter().chain(self.archive.iter()).cloned().collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Pure detection: scan parsed working-tree status entries for canon edits AND
/// archive entries. Both sides of a rename/copy are considered (the dest AND
/// the `orig_path`), so a `git mv` of a change dir INTO the archive is caught
/// by its destination. Testable without a git workspace.
pub fn detect(entries: &[git::StatusEntry]) -> GuardReport {
    let mut report = GuardReport::default();
    for e in entries {
        for path in std::iter::once(&e.path).chain(e.orig_path.as_ref()) {
            match classify(path) {
                Some(GuardKind::Canon) => report.canon.push(path.clone()),
                Some(GuardKind::Archive) => report.archive.push(path.clone()),
                None => {}
            }
        }
    }
    report.canon.sort();
    report.canon.dedup();
    report.archive.sort();
    report.archive.dedup();
    report
}

/// The operator-visible alert naming the session role AND what was reverted
/// (the canon edit(s) and/or the archive entr(ies)). Surfaced by [`enforce`].
pub fn alert_message(role: &str, report: &GuardReport) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !report.canon.is_empty() {
        parts.push(format!(
            "canon edit(s) under {CANON_PREFIX}: {}",
            report.canon.join(", ")
        ));
    }
    if !report.archive.is_empty() {
        parts.push(format!("archive entr(ies): {}", report.archive.join(", ")));
    }
    format!(
        "🚫 canon/archive guard: the {role} session attempted to {what} — \
         reverted before commit (canon and the archive are autocoder-only).",
        what = parts.join("; "),
    )
}

/// The catch layer: revert every canon edit AND archive entry the session
/// produced, preserving all other writes (the change/issue dir, code), then
/// surface the violation. Reads the working-tree status, reverts each offending
/// path to its HEAD state, logs an operator-visible alert naming `role` AND the
/// reverted paths, AND returns the [`GuardReport`] so the caller can react
/// (e.g. post to chatops). A clean (no-violation) tree returns an empty report
/// with no side effects.
///
/// The revert is fail-closed: it runs regardless of whether the prompt or
/// sandbox layers held. Per-path revert strategy mirrors
/// [`crate::polling_loop::triage_scrub`]:
///   - a guarded path **renamed/moved away** (e.g. a real `openspec archive`
///     moves `changes/<slug>` into the archive): its `orig_path` is restored
///     with `git checkout HEAD -- <orig>` so the change dir is not lost;
///   - an **untracked addition**: removed from disk (no HEAD blob);
///   - a **tracked path present in HEAD** (a modification): reverted with `git
///     checkout HEAD -- <path>` (rewrites index AND worktree to HEAD);
///   - a **tracked path absent from HEAD** (a staged add / rename dest):
///     unstaged with `git reset HEAD -- <path>` AND removed from disk.
pub fn enforce(workspace: &Path, role: &str) -> Result<GuardReport> {
    let entries = git::status_entries(workspace)
        .with_context(|| "canon_guard: reading git status".to_string())?;
    let report = detect(&entries);
    if report.is_empty() {
        return Ok(report);
    }
    for e in &entries {
        let dest_guarded = classify(&e.path).is_some();
        // Restore the SOURCE of a rename whose source OR destination is
        // guarded: a guarded path moved away (canon renamed out), OR a change
        // dir moved INTO the archive (the move's source must come back so the
        // change is not silently lost when its archive dest is removed below).
        if let Some(orig) = &e.orig_path
            && (dest_guarded || classify(orig).is_some())
            && path_exists_in_head(workspace, orig)?
        {
            run_git_revert(
                workspace,
                &["checkout", "-q", "HEAD", "--", orig.as_str()],
                orig,
                role,
            )?;
        }
        if !dest_guarded {
            continue;
        }
        let is_untracked = e.staged == '?' && e.worktree == '?';
        if is_untracked {
            // Untracked addition: no HEAD blob to restore — remove from disk.
            remove_path_from_disk(workspace, &e.path, role)?;
        } else if path_exists_in_head(workspace, &e.path)? {
            // Tracked modification of an existing canon file: rewrite BOTH the
            // index AND the worktree to the committed content.
            run_git_revert(
                workspace,
                &["checkout", "-q", "HEAD", "--", e.path.as_str()],
                &e.path,
                role,
            )?;
        } else {
            // Staged add OR rename destination (absent from HEAD): unstage
            // (demote to untracked) AND remove from disk. `git checkout HEAD --`
            // rejects a not-in-HEAD pathspec on some git versions, so avoid it.
            run_git_revert(
                workspace,
                &["reset", "-q", "HEAD", "--", e.path.as_str()],
                &e.path,
                role,
            )?;
            remove_path_from_disk(workspace, &e.path, role)?;
        }
    }
    tracing::warn!(role = role, "{}", alert_message(role, &report));
    Ok(report)
}

/// The directive appended to a writing session's prompt pointing it at
/// `OCTOPUS.md` for the workflow conventions AND the canon/archive constraints.
/// `Some` only when the file is present at the repository root; `None` is the
/// graceful no-op (the session proceeds; the sandbox AND revert layers still
/// enforce the invariant). The directive REFERENCES the file rather than
/// re-inlining the rules so the prompt and `OCTOPUS.md` share one source.
pub fn octopus_directive(workspace: &Path) -> Option<String> {
    if workspace.join(OCTOPUS_FILENAME).is_file() {
        Some(format!(
            "Before writing, read `{OCTOPUS_FILENAME}` at the repository root for the \
             workflow conventions AND the canon/archive constraints: the canonical specs \
             under `{CANON_PREFIX}` and archiving a change are autocoder-only — do NOT \
             modify canon or archive the change in this session; write only your own \
             change/issue directory (and, if implementing, code)."
        ))
    } else {
        None
    }
}

/// Append the [`octopus_directive`] to `prompt` when `OCTOPUS.md` is present,
/// returning the prompt unchanged when absent. The seam a writing session uses
/// so its prompt points at `OCTOPUS.md` without re-inlining the rules.
pub fn with_octopus_directive(workspace: &Path, prompt: &str) -> String {
    match octopus_directive(workspace) {
        Some(directive) => format!("{prompt}\n\n{directive}"),
        None => prompt.to_string(),
    }
}

/// Whether `path` exists as a blob in HEAD — `git cat-file -e HEAD:<path>`
/// exits 0. Chooses the revert strategy in [`enforce`]: a path IN HEAD is
/// reverted with `git checkout HEAD -- <path>`; a path NOT in HEAD (a staged
/// add OR a rename destination) is unstaged AND deleted instead. A failure to
/// spawn git propagates; a clean non-zero exit (blob absent) is `false`.
fn path_exists_in_head(workspace: &Path, path: &str) -> Result<bool> {
    let out = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("canon_guard: spawning git cat-file for `{path}`"))?;
    Ok(out.status.success())
}

/// Run a git working-tree revert subprocess (`checkout`/`reset`), capturing
/// diagnostics via `output()` so git's stderr is surfaced in the error rather
/// than spilled to the daemon's inherited stderr. A non-zero exit is fatal: we
/// refuse to proceed so the canon/archive write cannot leak into the commit.
fn run_git_revert(workspace: &Path, args: &[&str], path: &str, role: &str) -> Result<()> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| {
            format!("canon_guard: spawning `git {}` for `{path}`", args.join(" "))
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
            role = role,
            path = %path,
            "canon_guard: `git {}` exited non-zero reverting a canon/archive write: {diag}",
            args.join(" ")
        );
        return Err(anyhow!(
            "canon_guard: `git {}` exited non-zero ({diag}); refusing to proceed so the \
             canon/archive write does not leak into the {role} commit",
            args.join(" ")
        ));
    }
    Ok(())
}

/// Remove a working-tree path from disk (symlink-safe), treating an
/// already-absent path as success. Used for an untracked addition AND for a
/// not-in-HEAD tracked path `git reset` has just demoted to untracked. Any
/// failure other than "already gone" is fatal: a surviving file would be swept
/// into the commit by the caller's subsequent `git add`, silently violating the
/// invariant.
fn remove_path_from_disk(workspace: &Path, path: &str, role: &str) -> Result<()> {
    let abs = workspace.join(path);
    // Decide dir-vs-file from the path's OWN metadata (lstat), not `is_dir()`
    // which follows symlinks — following a symlink into `remove_dir_all` could
    // delete the link TARGET's contents.
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
            role = role,
            path = %path,
            "canon_guard: failed to remove a canon/archive write: {e:#}"
        );
        return Err(anyhow!(
            "canon_guard: failed to remove canon/archive write `{path}`: {e}; refusing to \
             proceed so it does not leak into the {role} commit"
        ));
    }
    // Prune now-empty ancestor directories the removed entry left behind (git
    // tracks files, not dirs, so removing the last file of a freshly-created
    // archive dir would otherwise leave an empty `.../archive/<slug>/` tree).
    // Stops at the first non-empty directory OR the workspace root; only empty
    // dirs are removed, so a legitimate sibling write is never touched.
    prune_empty_parents(workspace, path);
    Ok(())
}

/// Remove now-empty ancestor directories of `path`, walking up from its parent
/// and stopping at the first non-empty directory OR the workspace root.
/// `std::fs::remove_dir` only succeeds on an empty directory, so a directory
/// holding any other (legitimate) write is left intact.
fn prune_empty_parents(workspace: &Path, path: &str) {
    let mut cur = workspace.join(path);
    while let Some(parent) = cur.parent() {
        if parent == workspace || !parent.starts_with(workspace) {
            break;
        }
        match std::fs::remove_dir(parent) {
            Ok(()) => cur = parent.to_path_buf(),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    fn run(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A fixture repo with canon committed at HEAD, returning the temp guard
    /// AND the workspace path. Canon file: `openspec/specs/foo/spec.md`.
    fn fixture_repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        run(&ws, &["init", "-q", "-b", "main"]);
        run(&ws, &["config", "user.email", "test@example.com"]);
        run(&ws, &["config", "user.name", "test"]);
        write(&ws, "README.md", "hello\n");
        write(&ws, "openspec/specs/foo/spec.md", "# canon foo\noriginal\n");
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "initial"]);
        (dir, ws)
    }

    fn write(ws: &Path, rel: &str, body: &str) {
        let p = ws.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    // ---- classify / detect (pure) ----

    #[test]
    fn classify_distinguishes_canon_archive_and_writable() {
        assert_eq!(classify("openspec/specs/foo/spec.md"), Some(GuardKind::Canon));
        assert_eq!(
            classify("openspec/changes/archive/2026-01-01-x/proposal.md"),
            Some(GuardKind::Archive)
        );
        assert_eq!(
            classify("issues/archive/2026-01-01-x/issue.md"),
            Some(GuardKind::Archive)
        );
        // A change's OWN delta dir is NOT canon — it is writable.
        assert_eq!(classify("openspec/changes/my-change/specs/foo/spec.md"), None);
        assert_eq!(classify("openspec/changes/my-change/proposal.md"), None);
        assert_eq!(classify("src/lib.rs"), None);
    }

    #[test]
    fn detect_collects_canon_and_archive_from_entries() {
        let entries = vec![
            git::StatusEntry {
                staged: ' ',
                worktree: 'M',
                path: "openspec/specs/foo/spec.md".into(),
                orig_path: None,
            },
            git::StatusEntry {
                staged: '?',
                worktree: '?',
                path: "openspec/changes/archive/2026-01-01-x/proposal.md".into(),
                orig_path: None,
            },
            git::StatusEntry {
                staged: 'A',
                worktree: ' ',
                path: "openspec/changes/my-change/specs/foo/spec.md".into(),
                orig_path: None,
            },
        ];
        let report = detect(&entries);
        assert_eq!(report.canon, vec!["openspec/specs/foo/spec.md".to_string()]);
        assert_eq!(
            report.archive,
            vec!["openspec/changes/archive/2026-01-01-x/proposal.md".to_string()]
        );
        assert!(!report.is_empty());
    }

    #[test]
    fn detect_empty_when_only_change_dir_and_code() {
        let entries = vec![
            git::StatusEntry {
                staged: 'A',
                worktree: ' ',
                path: "openspec/changes/my-change/specs/foo/spec.md".into(),
                orig_path: None,
            },
            git::StatusEntry {
                staged: ' ',
                worktree: 'M',
                path: "src/lib.rs".into(),
                orig_path: None,
            },
        ];
        assert!(detect(&entries).is_empty());
    }

    #[test]
    fn sandbox_deny_entries_cover_canon_and_archive() {
        assert!(SANDBOX_DENY_ENTRIES
            .iter()
            .any(|e| e.starts_with("Write(") && e.contains("openspec/specs/")));
        assert!(SANDBOX_DENY_ENTRIES
            .iter()
            .any(|e| e.starts_with("Edit(") && e.contains("openspec/specs/")));
        assert!(SANDBOX_DENY_ENTRIES
            .iter()
            .any(|e| e.contains("openspec archive")));
    }

    #[test]
    fn alert_message_names_role_and_paths() {
        let report = GuardReport {
            canon: vec!["openspec/specs/foo/spec.md".into()],
            archive: vec!["openspec/changes/archive/2026-01-01-x/proposal.md".into()],
        };
        let msg = alert_message("changes-lane implementer", &report);
        assert!(msg.contains("changes-lane implementer"));
        assert!(msg.contains("openspec/specs/foo/spec.md"));
        assert!(msg.contains("openspec/changes/archive/2026-01-01-x/proposal.md"));
    }

    // ---- task 4.4: OCTOPUS.md directive present/absent ----

    #[test]
    fn octopus_directive_present_when_file_exists_absent_otherwise() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // Absent: graceful no-op.
        assert!(octopus_directive(ws).is_none());
        assert_eq!(with_octopus_directive(ws, "PROMPT"), "PROMPT");
        // Present: directive derived AND references OCTOPUS.md.
        std::fs::write(ws.join(OCTOPUS_FILENAME), "# guide\n").unwrap();
        let directive = octopus_directive(ws).expect("directive present");
        assert!(directive.contains(OCTOPUS_FILENAME));
        let composed = with_octopus_directive(ws, "PROMPT");
        assert!(composed.starts_with("PROMPT"));
        assert!(composed.contains(OCTOPUS_FILENAME));
        assert_ne!(composed, "PROMPT");
    }

    // ---- task 4.1: canon edit reverted, rest preserved, alert surfaced ----

    #[tokio::test]
    async fn canon_edit_reverted_rest_preserved() {
        let (_t, ws) = fixture_repo();
        // Session modifies canon AND writes its own legitimate change dir + code.
        write(&ws, "openspec/specs/foo/spec.md", "# canon foo\nTAMPERED\n");
        write(&ws, "openspec/changes/my-change/proposal.md", "legit\n");
        write(&ws, "src/feature.rs", "fn feature() {}\n");

        let report = enforce(&ws, "changes-lane implementer").unwrap();

        assert_eq!(report.canon, vec!["openspec/specs/foo/spec.md".to_string()]);
        // Canon restored to its committed content.
        assert_eq!(
            std::fs::read_to_string(ws.join("openspec/specs/foo/spec.md")).unwrap(),
            "# canon foo\noriginal\n"
        );
        // Legitimate writes preserved.
        assert!(ws.join("openspec/changes/my-change/proposal.md").exists());
        assert!(ws.join("src/feature.rs").exists());
    }

    #[tokio::test]
    async fn new_canon_file_removed() {
        let (_t, ws) = fixture_repo();
        write(&ws, "openspec/specs/bar/spec.md", "brand new canon\n");
        write(&ws, "openspec/changes/my-change/proposal.md", "legit\n");

        let report = enforce(&ws, "proposer").unwrap();

        assert_eq!(report.canon, vec!["openspec/specs/bar/spec.md".to_string()]);
        assert!(!ws.join("openspec/specs/bar/spec.md").exists());
        assert!(ws.join("openspec/changes/my-change/proposal.md").exists());
    }

    // ---- task 4.2: archive entry reverted and surfaced ----

    #[tokio::test]
    async fn archive_entry_reverted_rest_preserved() {
        let (_t, ws) = fixture_repo();
        // Simulate `openspec archive` by creating an entry under the archive
        // root, alongside a legitimate change-dir write.
        write(
            &ws,
            "openspec/changes/archive/2026-06-20-my-change/proposal.md",
            "archived\n",
        );
        write(&ws, "openspec/changes/my-change/proposal.md", "legit\n");

        let report = enforce(&ws, "audit triage").unwrap();

        assert_eq!(
            report.archive,
            vec!["openspec/changes/archive/2026-06-20-my-change/proposal.md".to_string()]
        );
        assert!(!ws
            .join("openspec/changes/archive/2026-06-20-my-change/proposal.md")
            .exists());
        assert!(ws.join("openspec/changes/my-change/proposal.md").exists());
    }

    // ---- task 4.3: change's own delta dir + code preserved (no violation) ----

    #[tokio::test]
    async fn own_delta_dir_and_code_preserved() {
        let (_t, ws) = fixture_repo();
        write(
            &ws,
            "openspec/changes/my-change/specs/foo/spec.md",
            "## ADDED Requirements\n",
        );
        write(&ws, "openspec/changes/my-change/proposal.md", "why\n");
        write(&ws, "src/feature.rs", "fn feature() {}\n");

        let report = enforce(&ws, "spec-revision executor").unwrap();

        assert!(report.is_empty(), "delta dir + code are not a violation");
        assert!(ws
            .join("openspec/changes/my-change/specs/foo/spec.md")
            .exists());
        assert!(ws.join("openspec/changes/my-change/proposal.md").exists());
        assert!(ws.join("src/feature.rs").exists());
    }

    #[tokio::test]
    async fn clean_tree_is_noop() {
        let (_t, ws) = fixture_repo();
        let report = enforce(&ws, "implementer").unwrap();
        assert!(report.is_empty());
    }

    // A real `openspec archive` rename (git mv changes/<slug> ->
    // changes/archive/...) is reverted: the archive dest is removed AND the
    // source change dir is restored so the change is not lost.
    #[tokio::test]
    async fn archive_rename_restores_source_and_removes_dest() {
        let (_t, ws) = fixture_repo();
        // Commit a change dir so a rename has a HEAD source.
        write(&ws, "openspec/changes/my-change/proposal.md", "why\n");
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "add change"]);
        // Now simulate the archive move via git mv.
        std::fs::create_dir_all(ws.join("openspec/changes/archive")).unwrap();
        run(
            &ws,
            &[
                "mv",
                "openspec/changes/my-change",
                "openspec/changes/archive/2026-06-20-my-change",
            ],
        );

        let report = enforce(&ws, "implementer").unwrap();

        assert!(!report.archive.is_empty(), "archive dest detected");
        // Source change dir restored.
        assert!(ws.join("openspec/changes/my-change/proposal.md").exists());
        // Archive dest removed.
        assert!(!ws.join("openspec/changes/archive/2026-06-20-my-change").exists());
    }
}
