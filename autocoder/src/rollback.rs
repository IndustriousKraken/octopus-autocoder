//! Code-rollback recovery (code-rollback-recovery): roll a repository's CODE
//! back by a commit count OR to a target SHA WHILE returning the OpenSpec
//! changes AND issues that were archived in the rolled-back range to the
//! active lanes — so the untrusted implementation is discarded but the
//! sound spec/issue work re-enters the pipeline to be re-implemented under
//! the controls.
//!
//! This module holds the pure, daemon-independent core:
//!   - [`resolve_plan`] — given a [`RollbackDepth`] (count OR SHA) on the
//!     base branch, compute the rolled-back commit range AND the set of
//!     changes/issues archived within it (the range→archived-units
//!     resolver).
//!   - [`prepare_rolled_back_tree`] — on the agent branch, restore the
//!     lane-external code to the target, unarchive each in-range change
//!     (canon fold undone) AND issue, AND stage + commit the result.
//!   - [`format_preview`] — the dry-run/preview text: exactly what WOULD be
//!     rolled back AND unarchived, changing nothing.
//!
//! The push + PR step is NOT here: the operation rides the SAME push +
//! PR-creation path as any change (honoring `auto_submit_pr`), which lives
//! in the daemon (`control_socket` → `polling_loop::open_pull_request`).

use crate::lanes::issues;
use crate::{git, queue};
use anyhow::{Context, Result, anyhow};
use std::path::Path;

/// The two ways an operator expresses how far back to roll: by a commit
/// COUNT (the last N commits on the base branch) OR to a target commit SHA
/// (roll back to that commit, discarding everything after it). Both resolve
/// to the SAME target commit AND therefore the same operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackDepth {
    /// Roll back the last `n` commits on the base branch (`n >= 1`).
    Count(usize),
    /// Roll back to (and including the state at) this commit. Any
    /// committish git can resolve — a full or short SHA.
    Sha(String),
}

/// One archived unit (change OR issue) that the resolver found inside the
/// rolled-back range AND will unarchive. `slug` is the canonical slug (the
/// dated archive prefix stripped); `archive_path` is the workspace-relative
/// path of the dated archive directory the unit currently lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedUnit {
    pub slug: String,
    pub archive_path: String,
}

/// A fully-resolved rollback plan: the target the code is restored to, the
/// rolled-back commit range, AND the changes/issues archived within it.
/// Produced by [`resolve_plan`]; consumed by [`prepare_rolled_back_tree`]
/// AND [`format_preview`]. Resolving a plan reads git only — it changes
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackPlan {
    /// The base branch the rollback runs against.
    pub base_branch: String,
    /// 40-char SHA of the base-branch tip BEFORE the rollback (the newest
    /// commit being discarded).
    pub from_sha: String,
    /// 40-char SHA of the rollback target — the commit the code is
    /// restored to.
    pub target_sha: String,
    /// Subjects of the commits being rolled back (newest-first), for the
    /// PR body / preview enumeration.
    pub rolled_back_subjects: Vec<String>,
    /// OpenSpec changes archived within the range, to be unarchived
    /// (canon fold undone). Sorted by slug.
    pub changes: Vec<ArchivedUnit>,
    /// Issues archived within the range, to be unarchived. Sorted by slug.
    pub issues: Vec<ArchivedUnit>,
}

impl RollbackPlan {
    /// True when the range archived NO changes AND NO issues — a code-only
    /// rollback with no unarchive step.
    pub fn is_code_only(&self) -> bool {
        self.changes.is_empty() && self.issues.is_empty()
    }

    /// Number of commits being rolled back.
    pub fn commit_count(&self) -> usize {
        self.rolled_back_subjects.len()
    }
}

/// Resolve a [`RollbackDepth`] against `base_branch` into a [`RollbackPlan`]:
/// compute the rolled-back commit range (`target..from`) AND the set of
/// changes/issues archived within it. Reads git only — NEVER modifies a
/// branch, the workspace, the archive, or canon.
///
/// The range→archived-units resolver maps `git diff --diff-filter=A
/// <target>..<from>` over the two archive lanes to slugs: an archive move
/// records the new dated path (`openspec/changes/archive/<YYYY-MM-DD>-<slug>/...`
/// for changes, `issues/archive/<YYYY-MM-DD>-<slug>/...` for issues) as an
/// ADD, so any add under those roots that the range introduced names a unit
/// archived in the range. A unit is identified once (by its dated archive
/// directory), regardless of how many files it contains. Everything archived
/// OUTSIDE the range is invisible to this diff AND is left alone.
pub fn resolve_plan(
    workspace: &Path,
    base_branch: &str,
    depth: &RollbackDepth,
) -> Result<RollbackPlan> {
    let from_sha = git::rev_parse(workspace, base_branch)
        .with_context(|| format!("resolving base branch `{base_branch}`"))?;

    let target_sha = match depth {
        RollbackDepth::Count(n) => {
            if *n == 0 {
                return Err(anyhow!("rollback count must be at least 1"));
            }
            // `base_branch~N` is the commit N back from the tip.
            let target_rev = format!("{base_branch}~{n}");
            git::rev_parse(workspace, &target_rev).with_context(|| {
                format!(
                    "resolving rollback target `{target_rev}` (count {n} exceeds the branch's \
                     history?)"
                )
            })?
        }
        RollbackDepth::Sha(sha) => {
            let resolved = git::rev_parse(workspace, sha)
                .with_context(|| format!("resolving rollback target SHA `{sha}`"))?;
            if resolved == from_sha {
                return Err(anyhow!(
                    "rollback target `{sha}` is the current base-branch tip — nothing to roll back"
                ));
            }
            resolved
        }
    };

    // The rolled-back range is target..from (the commits being discarded).
    let range = format!("{target_sha}..{from_sha}");
    let mut rolled_back_subjects = git::log_subjects(workspace, &range)?;
    // `log_subjects` returns chronological (`--reverse`); the preview / PR
    // body wants newest-first.
    rolled_back_subjects.reverse();

    if rolled_back_subjects.is_empty() {
        return Err(anyhow!(
            "the rollback range {range} contains no commits — target is not an ancestor of the \
             base-branch tip"
        ));
    }

    let changes = resolve_units(
        workspace,
        &target_sha,
        &from_sha,
        "openspec/changes/archive",
    )?;
    let issues = resolve_units(workspace, &target_sha, &from_sha, "issues/archive")?;

    Ok(RollbackPlan {
        base_branch: base_branch.to_string(),
        from_sha,
        target_sha,
        rolled_back_subjects,
        changes,
        issues,
    })
}

/// Enumerate the archived units (changes OR issues) introduced under
/// `archive_root` within `target..from`. Each ADD under
/// `<archive_root>/<YYYY-MM-DD>-<slug>/...` collapses to one [`ArchivedUnit`]
/// keyed by its dated directory; the slug is the directory name with the
/// `YYYY-MM-DD-` date prefix stripped. Sorted by slug, deduplicated.
fn resolve_units(
    workspace: &Path,
    target: &str,
    from: &str,
    archive_root: &str,
) -> Result<Vec<ArchivedUnit>> {
    let added = git::added_paths_in_range(workspace, target, from, archive_root)?;
    let mut seen: std::collections::BTreeMap<String, ArchivedUnit> =
        std::collections::BTreeMap::new();
    let prefix = format!("{archive_root}/");
    for path in added {
        // Path: <archive_root>/<dated-dir>/<rest...>. Take the first
        // segment after the archive root as the dated directory.
        let rest = match path.strip_prefix(&prefix) {
            Some(r) => r,
            None => continue,
        };
        let dated_dir = match rest.split('/').next() {
            Some(d) if !d.is_empty() => d,
            _ => continue,
        };
        let slug = strip_date_prefix(dated_dir);
        let archive_path = format!("{prefix}{dated_dir}");
        seen.entry(slug.clone()).or_insert(ArchivedUnit {
            slug,
            archive_path,
        });
    }
    Ok(seen.into_values().collect())
}

/// Strip a leading `YYYY-MM-DD-` date prefix from an archived directory name,
/// yielding the canonical slug. A name without the prefix is returned
/// unchanged (defensive — every autocoder archive entry is dated).
fn strip_date_prefix(dated: &str) -> String {
    // Expect `dddd-dd-dd-<slug>`: 10 date chars, a dash, then the slug.
    let bytes = dated.as_bytes();
    if dated.len() > 11
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'-'
        && dated[..4].chars().all(|c| c.is_ascii_digit())
        && dated[5..7].chars().all(|c| c.is_ascii_digit())
        && dated[8..10].chars().all(|c| c.is_ascii_digit())
    {
        dated[11..].to_string()
    } else {
        dated.to_string()
    }
}

/// A unit whose unarchive would collide with an existing active directory of
/// the same slug. Reported (never silently overwritten) per the spec edge
/// case. `lane` is `"change"` or `"issue"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub lane: &'static str,
    pub slug: String,
    pub active_path: String,
}

/// Detect — without modifying anything — every in-range unit whose unarchive
/// would collide with an already-present active directory of the same slug.
/// The dry-run/preview AND the prepare step both consult this so a collision
/// is reported rather than silently overwriting active work.
pub fn detect_collisions(workspace: &Path, plan: &RollbackPlan) -> Vec<Collision> {
    let mut out = Vec::new();
    for unit in &plan.changes {
        let active = workspace
            .join("openspec/changes")
            .join(&unit.slug);
        if active.exists() {
            out.push(Collision {
                lane: "change",
                slug: unit.slug.clone(),
                active_path: format!("openspec/changes/{}", unit.slug),
            });
        }
    }
    for unit in &plan.issues {
        let active = issues::issue_dir(workspace, &unit.slug);
        if active.exists() {
            out.push(Collision {
                lane: "issue",
                slug: unit.slug.clone(),
                active_path: format!("issues/{}", unit.slug),
            });
        }
    }
    out
}

/// Prepare the rolled-back state on the agent branch and commit it. Restores
/// every path OUTSIDE `openspec/` AND the issues lane to the rollback target
/// (discards the untrusted implementation), restores `openspec/specs/` to the
/// target (undoing the in-range changes' canon folds), unarchives each
/// in-range change (reusing [`queue::unarchive`]) AND issue, then stages +
/// commits the result on the agent branch.
///
/// Caller contract: the workspace is on a freshly-recreated `agent_branch`
/// at the base-branch tip (the daemon does `git::recreate_branch` before
/// calling this), AND the plan was resolved against the same tip. A detected
/// collision (see [`detect_collisions`]) is an error — the caller must
/// surface it rather than overwrite active work.
///
/// Returns the commit message used (so the daemon can log / thread it).
pub fn prepare_rolled_back_tree(
    workspace: &Path,
    plan: &RollbackPlan,
) -> Result<String> {
    let collisions = detect_collisions(workspace, plan);
    if !collisions.is_empty() {
        let detail = collisions
            .iter()
            .map(|c| format!("{} `{}` (active dir {} exists)", c.lane, c.slug, c.active_path))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(anyhow!(
            "rollback aborted: {} in-range unit(s) would collide with existing active \
             directories: {detail}",
            collisions.len()
        ));
    }

    // 1. Restore lane-external code to the target. `git checkout <target> --
    //    . ':!openspec' ':!issues'` restores every tracked path that exists
    //    at the target outside the two lanes; it does NOT delete paths added
    //    after the target, so we then remove lane-external paths the range
    //    introduced.
    restore_code_to_target(workspace, &plan.target_sha)?;

    // 2. Restore canon (openspec/specs) to the target — undoing every canon
    //    fold applied after the target, which is exactly the in-range
    //    changes' folds (issues never touch canon; out-of-range archives
    //    predate the target). A capability directory introduced wholesale by
    //    an in-range change is removed (it does not exist at the target).
    restore_canon_to_target(workspace, &plan.target_sha)?;

    // 3. Unarchive each in-range change (dated archive dir → active), with
    //    its canon fold already undone by step 2 — it is pending again.
    for unit in &plan.changes {
        queue::unarchive(workspace, &unit.slug)
            .with_context(|| format!("unarchiving change `{}`", unit.slug))?;
    }

    // 4. Unarchive each in-range issue (issues/archive/<dated> → issues/<slug>).
    for unit in &plan.issues {
        unarchive_issue(workspace, unit)
            .with_context(|| format!("unarchiving issue `{}`", unit.slug))?;
    }

    // 5. Stage + commit the prepared tree on the agent branch.
    let message = rollback_commit_message(plan);
    git::add_all(workspace)?;
    git::commit(workspace, &message)?;
    Ok(message)
}

/// Restore every tracked path OUTSIDE `openspec/` AND `issues/` to its state
/// at `target`, then delete any lane-external path the range ADDED (which
/// `git checkout <target> -- <path>` leaves in place).
fn restore_code_to_target(workspace: &Path, target: &str) -> Result<()> {
    // Restore tracked content that exists at the target.
    git::restore_pathspec_to_target_excluding(
        workspace,
        target,
        &[":!openspec", ":!issues"],
    )?;
    // Delete lane-external files introduced after the target so the tree
    // matches the target exactly even for newly-added paths.
    let added = git::added_paths_in_range(workspace, target, "HEAD", ".")?;
    for path in added {
        if path.starts_with("openspec/") || path == "openspec"
            || path.starts_with("issues/") || path == "issues"
        {
            continue;
        }
        git::remove_path(workspace, &path)?;
    }
    Ok(())
}

/// Restore `openspec/specs/` to `target` (undo canon folds), then delete any
/// `openspec/specs/...` path the range ADDED (a capability folder introduced
/// wholesale by an in-range change, which does not exist at the target).
fn restore_canon_to_target(workspace: &Path, target: &str) -> Result<()> {
    let specs_at_target_exists = git::path_exists_at(workspace, target, "openspec/specs")?;
    if specs_at_target_exists {
        git::restore_pathspec_to_target(workspace, target, "openspec/specs")?;
    }
    let added = git::added_paths_in_range(workspace, target, "HEAD", "openspec/specs")?;
    for path in added {
        git::remove_path(workspace, &path)?;
    }
    Ok(())
}

/// Move `issues/archive/<dated>/` back to active `issues/<slug>/`. Mirrors
/// [`queue::unarchive`] for the issues lane: collision (active dir exists)
/// is an error, not an overwrite.
fn unarchive_issue(workspace: &Path, unit: &ArchivedUnit) -> Result<()> {
    let src = workspace.join(&unit.archive_path);
    if !src.is_dir() {
        return Err(anyhow!(
            "archived issue directory not found: {}",
            src.display()
        ));
    }
    let dst = issues::issue_dir(workspace, &unit.slug);
    if dst.exists() {
        return Err(anyhow!(
            "unarchive destination already exists: {}",
            dst.display()
        ));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::rename(&src, &dst)
        .with_context(|| format!("renaming {} to {}", src.display(), dst.display()))?;
    Ok(())
}

/// The commit message stamped on the rollback commit. Names the count AND
/// the unarchived-unit summary so `git log` (and reviewers) see the intent.
fn rollback_commit_message(plan: &RollbackPlan) -> String {
    let short_target = plan.target_sha.chars().take(7).collect::<String>();
    let unit_clause = if plan.is_code_only() {
        "code-only".to_string()
    } else {
        format!(
            "unarchive {} change(s) + {} issue(s)",
            plan.changes.len(),
            plan.issues.len()
        )
    };
    format!(
        "rollback: restore code to {short_target} ({} commit(s) rolled back); {unit_clause}",
        plan.commit_count()
    )
}

/// The dry-run / preview / PR-body text: exactly WHAT would be rolled back
/// AND unarchived. Reports collisions inline. Changes nothing.
pub fn format_preview(workspace: &Path, plan: &RollbackPlan) -> String {
    let short_from = plan.from_sha.chars().take(7).collect::<String>();
    let short_target = plan.target_sha.chars().take(7).collect::<String>();
    let mut out = String::new();
    out.push_str(&format!(
        "Rollback plan for `{}`: restore code to {short_target} (discarding {} commit(s) since \
         {short_from}).\n",
        plan.base_branch,
        plan.commit_count()
    ));
    out.push_str("\nCommits to roll back (newest first):\n");
    for subject in &plan.rolled_back_subjects {
        out.push_str(&format!("  - {subject}\n"));
    }

    if plan.is_code_only() {
        out.push_str(
            "\nThis range archived NO changes and NO issues — code-only rollback, no unarchive \
             step.\n",
        );
    } else {
        if !plan.changes.is_empty() {
            out.push_str("\nOpenSpec changes to unarchive (canon fold undone, pending again):\n");
            for unit in &plan.changes {
                out.push_str(&format!("  - {}\n", unit.slug));
            }
        }
        if !plan.issues.is_empty() {
            out.push_str("\nIssues to unarchive (returned to the active lane):\n");
            for unit in &plan.issues {
                out.push_str(&format!("  - {}\n", unit.slug));
            }
        }
    }

    let collisions = detect_collisions(workspace, plan);
    if !collisions.is_empty() {
        out.push_str(
            "\n⚠ COLLISIONS — these in-range units cannot be unarchived because an active \
             directory of the same slug already exists (resolve before running):\n",
        );
        for c in &collisions {
            out.push_str(&format!("  - {} `{}` ({})\n", c.lane, c.slug, c.active_path));
        }
    }

    out.push_str(
        "\nThe code is DISCARDED; the spec/issue work is returned to the pipeline to be \
         re-gated AND re-implemented under the controls. This rides the normal push + PR flow \
         (a PR when auto_submit_pr is enabled; otherwise a pushed branch with no PR). Git \
         history remains the backstop.\n",
    );
    out
}

/// Build the PR body for a confirmed rollback: the same enumeration as the
/// preview, framed as the merge artifact. Reused by the daemon's PR-assembly
/// step.
pub fn build_pr_body(workspace: &Path, plan: &RollbackPlan) -> String {
    let mut body = String::from("## Code-rollback recovery\n\n");
    body.push_str(&format_preview(workspace, plan));
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn run(path: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed in {}", path.display());
    }

    fn head(path: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Build a workspace simulating autocoder's commit shape: each "change
    /// shipped" commit (a) writes implementation code, (b) moves the change
    /// dir to `openspec/changes/archive/<date>-<slug>/`, AND (c) folds the
    /// change's delta into `openspec/specs/<cap>/spec.md`. Returns the
    /// workspace path AND the SHA after the initial (pre-rollback-window)
    /// commit so a test can target it.
    fn fixture() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        run(&ws, &["init", "-q", "-b", "main"]);
        run(&ws, &["config", "user.email", "t@e.com"]);
        run(&ws, &["config", "user.name", "t"]);
        // Initial state: some code, an already-archived change folded into
        // canon (this one is OUTSIDE any future rollback window).
        std::fs::write(ws.join("README.md"), "v0\n").unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/lib.rs"), "// base\n").unwrap();
        std::fs::create_dir_all(ws.join("openspec/specs/widget")).unwrap();
        std::fs::write(
            ws.join("openspec/specs/widget/spec.md"),
            "CANON widget v1\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("openspec/changes/archive/2026-01-01-old-change"))
            .unwrap();
        std::fs::write(
            ws.join("openspec/changes/archive/2026-01-01-old-change/proposal.md"),
            "old\n",
        )
        .unwrap();
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "base + old archived change"]);
        (dir, ws)
    }

    /// Ship a change: write code, fold canon, AND drop a dated archive dir —
    /// all in one commit (the autocoder shape the rollback must unwind).
    fn ship_change(ws: &Path, slug: &str, code_file: &str, canon_line: &str) {
        std::fs::write(ws.join(code_file), format!("// {slug}\n")).unwrap();
        // Fold canon: append a line to the widget spec.
        let canon = ws.join("openspec/specs/widget/spec.md");
        let mut body = std::fs::read_to_string(&canon).unwrap();
        body.push_str(canon_line);
        body.push('\n');
        std::fs::write(&canon, body).unwrap();
        // Drop the dated archive directory.
        let archived = ws
            .join("openspec/changes/archive")
            .join(format!("2026-05-01-{slug}"));
        std::fs::create_dir_all(&archived).unwrap();
        std::fs::write(archived.join("proposal.md"), format!("## Why\n{slug}\n")).unwrap();
        std::fs::write(archived.join("tasks.md"), "- [x] 1.1 done\n").unwrap();
        std::fs::create_dir_all(archived.join("specs/widget")).unwrap();
        std::fs::write(
            archived.join("specs/widget/spec.md"),
            format!("## MODIFIED\n{canon_line}\n"),
        )
        .unwrap();
        run(ws, &["add", "-A"]);
        run(ws, &["commit", "-q", "-m", &format!("{slug}: ship change")]);
    }

    /// Ship an issue fix: write code AND drop a dated issues/archive dir
    /// (issues never touch canon).
    fn ship_issue(ws: &Path, slug: &str, code_file: &str) {
        std::fs::write(ws.join(code_file), format!("// fix {slug}\n")).unwrap();
        let archived = ws.join("issues/archive").join(format!("2026-05-02-{slug}"));
        std::fs::create_dir_all(&archived).unwrap();
        std::fs::write(archived.join("issue.md"), format!("## Report\n{slug}\n")).unwrap();
        std::fs::write(archived.join("tasks.md"), "- [x] fix\n").unwrap();
        run(ws, &["add", "-A"]);
        run(ws, &["commit", "-q", "-m", &format!("{slug}: ship issue fix")]);
    }

    // ----- 6.1: resolver maps range → exactly the in-range units -----

    #[test]
    fn resolver_maps_range_to_in_range_units_only() {
        let (_dir, ws) = fixture();
        let target = head(&ws); // rollback target = pre-window tip
        // Ship two changes AND one issue (the window to roll back).
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        ship_change(&ws, "feature-b", "src/b.rs", "MODIFIED b");
        ship_issue(&ws, "fix-thing", "src/c.rs");

        // Count form: roll back the last 3 commits.
        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(3)).unwrap();
        assert_eq!(plan.target_sha, target);
        let change_slugs: Vec<&str> = plan.changes.iter().map(|u| u.slug.as_str()).collect();
        assert_eq!(change_slugs, vec!["feature-a", "feature-b"]);
        let issue_slugs: Vec<&str> = plan.issues.iter().map(|u| u.slug.as_str()).collect();
        assert_eq!(issue_slugs, vec!["fix-thing"]);
        // The out-of-range `old-change` (archived before the target) is NOT
        // present.
        assert!(
            !change_slugs.contains(&"old-change"),
            "out-of-range archive must be excluded"
        );
    }

    #[test]
    fn resolver_excludes_units_archived_before_the_range() {
        let (_dir, ws) = fixture();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        let target_after_a = head(&ws);
        ship_change(&ws, "feature-b", "src/b.rs", "MODIFIED b");

        // Roll back only the last commit (feature-b). feature-a is now
        // BEFORE the range.
        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(1)).unwrap();
        assert_eq!(plan.target_sha, target_after_a);
        let change_slugs: Vec<&str> = plan.changes.iter().map(|u| u.slug.as_str()).collect();
        assert_eq!(change_slugs, vec!["feature-b"]);
        assert!(!change_slugs.contains(&"feature-a"));
    }

    // ----- 6.3: count and SHA forms produce identical structure -----

    #[test]
    fn count_and_sha_forms_are_equivalent() {
        let (_dir, ws) = fixture();
        let target = head(&ws);
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        ship_issue(&ws, "fix-thing", "src/c.rs");

        let by_count = resolve_plan(&ws, "main", &RollbackDepth::Count(2)).unwrap();
        let by_sha = resolve_plan(&ws, "main", &RollbackDepth::Sha(target.clone())).unwrap();
        assert_eq!(by_count.target_sha, by_sha.target_sha);
        assert_eq!(by_count.changes, by_sha.changes);
        assert_eq!(by_count.issues, by_sha.issues);
        assert_eq!(by_count.rolled_back_subjects, by_sha.rolled_back_subjects);
    }

    // ----- 6.2: prepared tree has code-at-target + in-range units active -----

    #[test]
    fn prepared_tree_restores_code_and_unarchives_in_range_units() {
        let (_dir, ws) = fixture();
        let target = head(&ws);
        let canon_at_target =
            std::fs::read_to_string(ws.join("openspec/specs/widget/spec.md")).unwrap();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        ship_change(&ws, "feature-b", "src/b.rs", "MODIFIED b");
        ship_issue(&ws, "fix-thing", "src/c.rs");

        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(3)).unwrap();
        // Move onto a fresh agent branch at the base tip (daemon's job).
        run(&ws, &["checkout", "-q", "-B", "agent-q"]);
        prepare_rolled_back_tree(&ws, &plan).unwrap();

        // Code restored to target: the implementation files added in-range
        // are GONE.
        assert!(!ws.join("src/a.rs").exists(), "feature-a code discarded");
        assert!(!ws.join("src/b.rs").exists(), "feature-b code discarded");
        assert!(!ws.join("src/c.rs").exists(), "issue-fix code discarded");
        assert!(ws.join("src/lib.rs").exists(), "base code preserved");

        // Canon fold UNDONE: the widget spec is back to its target content.
        let canon_now =
            std::fs::read_to_string(ws.join("openspec/specs/widget/spec.md")).unwrap();
        assert_eq!(
            canon_now, canon_at_target,
            "canon fold for in-range changes must be undone"
        );

        // In-range changes are ACTIVE (pending again), NOT reverted to
        // non-existence.
        assert!(
            ws.join("openspec/changes/feature-a/proposal.md").is_file(),
            "feature-a active"
        );
        assert!(
            ws.join("openspec/changes/feature-b/proposal.md").is_file(),
            "feature-b active"
        );
        // And their archive entries are gone (moved, not copied).
        assert!(
            !ws.join("openspec/changes/archive/2026-05-01-feature-a").exists(),
            "feature-a archive entry moved out"
        );

        // In-range issue is ACTIVE.
        assert!(
            ws.join("issues/fix-thing/issue.md").is_file(),
            "issue fix-thing active"
        );
        assert!(
            !ws.join("issues/archive/2026-05-02-fix-thing").exists(),
            "issue archive entry moved out"
        );

        // Out-of-range archive untouched (still archived).
        assert!(
            ws.join("openspec/changes/archive/2026-01-01-old-change").exists(),
            "out-of-range archive preserved"
        );

        // The working tree is clean (everything was committed).
        let st = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(
            st.stdout.is_empty(),
            "tree must be clean after prepare: {}",
            String::from_utf8_lossy(&st.stdout)
        );
        let _ = target;
    }

    // ----- 5.1: code-only range → plain rollback, no unarchive -----

    #[test]
    fn code_only_range_has_no_unarchive_step() {
        let (_dir, ws) = fixture();
        let target = head(&ws);
        // Two code-only commits (no archive moves, no canon folds).
        std::fs::write(ws.join("src/x.rs"), "// x\n").unwrap();
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "code only 1"]);
        std::fs::write(ws.join("src/y.rs"), "// y\n").unwrap();
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "code only 2"]);

        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(2)).unwrap();
        assert!(plan.is_code_only(), "no archived units in a code-only range");
        assert_eq!(plan.target_sha, target);

        run(&ws, &["checkout", "-q", "-B", "agent-q"]);
        prepare_rolled_back_tree(&ws, &plan).unwrap();
        assert!(!ws.join("src/x.rs").exists());
        assert!(!ws.join("src/y.rs").exists());
        assert!(ws.join("src/lib.rs").exists());

        let preview = format_preview(&ws, &plan);
        assert!(
            preview.contains("code-only"),
            "preview must say code-only: {preview}"
        );
    }

    // ----- 5.2: in-range collision is reported, not overwritten -----

    #[test]
    fn collision_with_active_dir_is_reported_not_overwritten() {
        let (_dir, ws) = fixture();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(1)).unwrap();

        // Plant an active dir of the same slug — the unarchive would collide.
        std::fs::create_dir_all(ws.join("openspec/changes/feature-a")).unwrap();
        std::fs::write(
            ws.join("openspec/changes/feature-a/proposal.md"),
            "active work\n",
        )
        .unwrap();
        run(&ws, &["add", "-A"]);
        run(&ws, &["commit", "-q", "-m", "active feature-a"]);

        let collisions = detect_collisions(&ws, &plan);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].slug, "feature-a");
        assert_eq!(collisions[0].lane, "change");

        let preview = format_preview(&ws, &plan);
        assert!(preview.contains("COLLISIONS"), "preview flags collision: {preview}");

        // prepare must refuse rather than overwrite the active dir.
        run(&ws, &["checkout", "-q", "-B", "agent-q"]);
        let err = prepare_rolled_back_tree(&ws, &plan).expect_err("collision must abort");
        let msg = format!("{err:#}");
        assert!(msg.contains("collide"), "error names the collision: {msg}");
        // The active dir is intact.
        assert_eq!(
            std::fs::read_to_string(ws.join("openspec/changes/feature-a/proposal.md")).unwrap(),
            "active work\n"
        );
    }

    // ----- 6.4: dry-run / preview changes nothing -----

    #[test]
    fn preview_changes_nothing() {
        let (_dir, ws) = fixture();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        ship_issue(&ws, "fix-thing", "src/c.rs");
        let before = head(&ws);
        let status_before = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&ws)
            .output()
            .unwrap();

        let plan = resolve_plan(&ws, "main", &RollbackDepth::Count(2)).unwrap();
        let preview = format_preview(&ws, &plan);
        // The preview enumerates BOTH the commits AND the units.
        assert!(preview.contains("feature-a"), "names the change: {preview}");
        assert!(preview.contains("fix-thing"), "names the issue: {preview}");

        // Nothing changed: same HEAD, same (clean) status, archives intact,
        // active dirs absent.
        assert_eq!(head(&ws), before, "preview must not move HEAD");
        let status_after = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&ws)
            .output()
            .unwrap();
        assert_eq!(status_before.stdout, status_after.stdout);
        assert!(ws.join("openspec/changes/archive/2026-05-01-feature-a").exists());
        assert!(!ws.join("openspec/changes/feature-a").exists());
        assert!(ws.join("issues/archive/2026-05-02-fix-thing").exists());
        assert!(!ws.join("issues/fix-thing").exists());
    }

    // ----- depth validation -----

    #[test]
    fn count_zero_errors() {
        let (_dir, ws) = fixture();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        let err = resolve_plan(&ws, "main", &RollbackDepth::Count(0)).expect_err("zero errors");
        assert!(format!("{err:#}").contains("at least 1"));
    }

    #[test]
    fn sha_equal_to_tip_errors() {
        let (_dir, ws) = fixture();
        ship_change(&ws, "feature-a", "src/a.rs", "MODIFIED a");
        let tip = head(&ws);
        let err = resolve_plan(&ws, "main", &RollbackDepth::Sha(tip))
            .expect_err("rolling back to the tip is a no-op error");
        assert!(format!("{err:#}").contains("nothing to roll back"));
    }

    #[test]
    fn strip_date_prefix_strips_dated_and_passes_undated() {
        assert_eq!(strip_date_prefix("2026-05-01-feature-a"), "feature-a");
        assert_eq!(strip_date_prefix("2026-12-31-a01-foo-bar"), "a01-foo-bar");
        assert_eq!(strip_date_prefix("nondated"), "nondated");
    }
}
