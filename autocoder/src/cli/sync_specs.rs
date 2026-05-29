//! `autocoder sync-specs --rebuild` — rebuild all canonical specs from
//! archive history. Single mode for v1: full chronological replay. See
//! `openspec/changes/rebuild-canonical-specs-from-archive/proposal.md` for
//! the why-incremental-is-unsafe rationale.

use crate::busy_marker;
use crate::cli::sync_specs_deps::{
    self, RebuildAbortReason, RenameRecord,
};
use crate::openspec_archive::{
    self, ArchiveFailure, openspec_archive_with_postcondition,
};
use anyhow::{Context, Result, anyhow};
#[cfg(test)]
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// Shared archive types now live in `crate::openspec_archive` so both
// `queue::archive` and the rebuild loop call the same post-condition
// logic. Re-exported here for backwards compatibility and so test
// modules under `cli::sync_specs::*` can keep using the short names.
// `ArchiveRunOutput` and `detect_openspec_abort` aren't referenced
// by non-test code in this module, but the re-exports are kept so the
// rebuild-path tests below and any external callers from prior
// versions of `sync_specs.rs` still resolve.
#[allow(unused_imports)]
pub use crate::openspec_archive::{
    ArchiveRunOutput, ArchiveRunner, RealArchiveRunner, detect_openspec_abort,
};

/// CLI args for `autocoder sync-specs`.
#[derive(Debug, Clone)]
pub struct SyncSpecsArgs {
    pub workspace: PathBuf,
    pub rebuild: bool,
    pub immediate: bool,
}

/// Per-change record in a `RebuildReport`.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeOutcome {
    pub slug: String,
    pub original_name: String,
    pub success: bool,
    /// Truncated openspec stderr when the archive subprocess failed; empty
    /// on success.
    pub failure_reason: String,
}

/// Per-spec-file record in a `RebuildReport`. `modified` reflects whether
/// the rebuilt content differs byte-for-byte from the pre-rebuild content
/// (or whether the file is wholly new after rebuild).
#[derive(Debug, Clone, Serialize)]
pub struct SpecFileOutcome {
    pub path: String,
    pub modified: bool,
}

/// Outcome of one rebuild invocation. `successful + failed == processed`.
/// `rolled_back` counts changes whose archive call failed in a way that
/// triggered a rollback of the active-path directory back to archive (a
/// subset of `failed`).
///
/// `prefix_renames` records the dependency-aware ordering pre-pass's
/// applied renames; empty when the pre-pass produced no renames.
/// `abort_reason` is `Some(_)` when the pre-pass aborted the rebuild
/// (cycle, cross-day backward dependency, scan failure); in that case the
/// per-change-loop ran with zero entries and no canonical specs were
/// modified.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RebuildReport {
    pub processed: usize,
    pub successful: usize,
    pub failed: usize,
    pub rolled_back: usize,
    pub successes: Vec<ChangeOutcome>,
    pub failures: Vec<ChangeOutcome>,
    pub spec_files: Vec<SpecFileOutcome>,
    #[serde(default)]
    pub prefix_renames: Vec<RenameRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<RebuildAbortReason>,
}

impl RebuildReport {
    pub fn modified_files(&self) -> usize {
        self.spec_files.iter().filter(|f| f.modified).count()
    }

    pub fn failed_slugs(&self) -> Vec<String> {
        self.failures.iter().map(|f| f.slug.clone()).collect()
    }
}

/// CLI entry point. Validates args, coordinates with any running daemon
/// per `--immediate`, runs the rebuild, prints a human-readable summary,
/// and returns Err when any archived change failed to re-archive (so the
/// process exits non-zero).
pub async fn execute(args: SyncSpecsArgs) -> Result<()> {
    validate_args(&args)?;

    if !args.rebuild {
        return Err(anyhow!(
            "sync-specs currently supports only --rebuild mode; pass --rebuild"
        ));
    }

    coordinate_with_daemon(&args.workspace, args.immediate).await?;

    let report = rebuild_canonical(&args.workspace).await?;
    print_report(&report);

    if report.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn validate_args(args: &SyncSpecsArgs) -> Result<()> {
    if !args.workspace.exists() {
        return Err(anyhow!(
            "workspace path does not exist: {}",
            args.workspace.display()
        ));
    }
    let archive_dir = args.workspace.join("openspec/changes/archive");
    if !archive_dir.is_dir() {
        return Err(anyhow!(
            "no archive directory at {} — is this an OpenSpec-managed workspace?",
            archive_dir.display()
        ));
    }
    Ok(())
}

/// Coordinate with a running daemon on this workspace. If `immediate`,
/// SIGTERM the executor subprocess via the busy marker's sidecar pid and
/// wait up to 30s for release. If not `immediate`, poll-wait politely
/// until the busy marker is released. When no busy marker exists, both
/// modes are a no-op.
pub async fn coordinate_with_daemon(workspace: &Path, immediate: bool) -> Result<()> {
    let marker_path = busy_marker::marker_path(workspace);
    if !marker_path.exists() {
        // No daemon iteration in progress; nothing to coordinate.
        return Ok(());
    }

    if immediate {
        tracing::info!(
            workspace = %workspace.display(),
            "sync-specs --immediate: busy marker present; sending SIGTERM to executor subprocess"
        );
        if let Some(pid) = busy_marker::read_subprocess_marker(workspace) {
            if pid > 0 {
                // SIGTERM to the subprocess pgid (= pid, since executor
                // spawns with process_group(0)).
                let rc = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    tracing::warn!(
                        pgid = pid,
                        "sync-specs: SIGTERM to executor process group failed: {err}"
                    );
                }
            } else {
                tracing::warn!(
                    "sync-specs: subprocess sidecar pid is non-positive; cannot SIGTERM"
                );
            }
        } else {
            tracing::warn!(
                "sync-specs: no subprocess sidecar present alongside busy marker; \
                 cannot SIGTERM (the iteration may not have spawned an executor yet)"
            );
        }
        wait_for_marker_release(&marker_path, Duration::from_secs(30)).await;
        if marker_path.exists() {
            tracing::warn!(
                marker = %marker_path.display(),
                "sync-specs: busy marker still held after 30s; proceeding anyway \
                 (rebuild's dirty-workspace recovery will clean partial state)"
            );
        }
    } else {
        tracing::info!(
            workspace = %workspace.display(),
            "sync-specs: busy marker present; waiting for current iteration to finish"
        );
        // Poll every few seconds with a periodic INFO so the operator
        // sees progress. No hard upper bound — the operator can Ctrl-C
        // if they decide to switch to --immediate.
        let start = Instant::now();
        let mut next_log = start + Duration::from_secs(30);
        loop {
            if !marker_path.exists() {
                tracing::info!(
                    waited_secs = start.elapsed().as_secs(),
                    "sync-specs: iteration finished; proceeding with rebuild"
                );
                break;
            }
            if Instant::now() >= next_log {
                tracing::info!(
                    waited_secs = start.elapsed().as_secs(),
                    "sync-specs: still waiting for iteration to release busy marker"
                );
                next_log = Instant::now() + Duration::from_secs(30);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    Ok(())
}

async fn wait_for_marker_release(marker_path: &Path, max: Duration) {
    let start = Instant::now();
    while start.elapsed() < max {
        if !marker_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Rebuild every canonical spec under `openspec/specs/` by replaying the
/// archived changes in chronological order. Returns a report describing
/// per-change outcomes plus per-canonical-file modified-vs-unchanged
/// status.
pub async fn rebuild_canonical(workspace: &Path) -> Result<RebuildReport> {
    rebuild_canonical_with_runner(workspace, &RealArchiveRunner).await
}

/// Test-injectable variant of `rebuild_canonical`. The production entry
/// point delegates to this with a `RealArchiveRunner`.
pub async fn rebuild_canonical_with_runner(
    workspace: &Path,
    runner: &dyn ArchiveRunner,
) -> Result<RebuildReport> {
    let archive_root = workspace.join("openspec/changes/archive");
    if !archive_root.is_dir() {
        return Err(anyhow!(
            "archive directory not found at {}",
            archive_root.display()
        ));
    }

    // 0. Dependency-aware ordering pre-pass: scan every archived change's
    //    spec deltas, build a dependency graph, and topologically reorder
    //    same-day archives by `aNN-` directory-name prefix. On a graph
    //    that cannot be resolved by within-day prefix renames (cycle, or
    //    cross-day backward dependency), abort the rebuild before any
    //    canonical-spec mutation.
    let plan = match sync_specs_deps::compute_dependency_prefix_renames(&archive_root) {
        Ok(p) => p,
        Err(reason) => {
            tracing::error!(
                "rebuild aborted by dependency pre-pass: {}",
                reason.summary()
            );
            let report = RebuildReport {
                abort_reason: Some(reason),
                ..RebuildReport::default()
            };
            return Ok(report);
        }
    };

    let prefix_renames: Vec<RenameRecord> = plan.iter().map(RenameRecord::from).collect();
    if !plan.is_empty()
        && let Err(e) = sync_specs_deps::apply_rename_plan(&archive_root, &plan)
    {
        tracing::error!("apply_rename_plan returned at least one io error: {e}");
        // Per-rename failures were logged; the plan tried every rename.
        // The subsequent chronological loop will pick up whatever ended
        // up on disk.
    }

    // 1. Snapshot existing canonical content for the modified-vs-unchanged
    //    diff at the end.
    let specs_root = workspace.join("openspec/specs");
    let before = snapshot_specs(&specs_root)?;

    // 2. Clear all canonical capability dirs (preserve the parent
    //    `openspec/specs/` dir itself).
    clear_specs_dir(&specs_root)?;

    // 3. Enumerate archived changes in chronological order.
    let mut archived: Vec<(String, String)> = Vec::new(); // (original_name, slug)
    let date_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-").expect("static regex compiles");
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&archive_root)
        .with_context(|| format!("reading {}", archive_root.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !date_re.is_match(&name) {
            // Not a dated archive directory; skip (could be a nested
            // archive/ or some operator-placed sidecar).
            continue;
        }
        let slug = match strip_date_prefix(&name) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        archived.push((name, slug));
    }

    let mut report = RebuildReport {
        processed: archived.len(),
        prefix_renames,
        ..RebuildReport::default()
    };

    let changes_root = workspace.join("openspec/changes");
    for (original_name, slug) in archived {
        let from = archive_root.join(&original_name);
        let to = changes_root.join(&slug);

        // If a stale active dir exists from a prior interrupted run, bail
        // on this change with a clear reason rather than clobbering it.
        if to.exists() {
            tracing::error!(
                slug = %slug,
                "rebuild: active change directory already exists at {}; skipping (operator must remove or rename it before retry)",
                to.display()
            );
            report.failed += 1;
            report.failures.push(ChangeOutcome {
                slug: slug.clone(),
                original_name: original_name.clone(),
                success: false,
                failure_reason: format!(
                    "active change directory already exists at {}",
                    to.display()
                ),
            });
            continue;
        }

        if let Err(e) = std::fs::rename(&from, &to) {
            tracing::error!(
                slug = %slug,
                "rebuild: rename {} -> {} failed: {e}",
                from.display(),
                to.display()
            );
            report.failed += 1;
            report.failures.push(ChangeOutcome {
                slug: slug.clone(),
                original_name: original_name.clone(),
                success: false,
                failure_reason: format!("pre-archive rename failed: {e}"),
            });
            continue;
        }

        // Delegate to the shared archive-with-postcondition helper. The
        // helper performs spawn → exit check → abort-marker scan →
        // active-path check → archive-glob check, returning a
        // structured failure for each detected mode. The rebuild path
        // adds its own multi-match guard on top (a rebuild-specific
        // concern: the chronological replay can leave stale archive
        // entries the helper can't disambiguate).
        // `cli::sync_specs` operates on a single in-place workspace
        // (the operator's working tree containing openspec/), so wrap
        // it in a SpecRoot::from_parts shim — the rebuild loop does
        // not have a RepositoryConfig to honor spec_storage from.
        let spec_root = crate::spec_root::SpecRoot::from_parts(
            workspace.to_path_buf(),
            workspace.join("openspec"),
            false,
        );
        match openspec_archive_with_postcondition(runner, &spec_root, &slug) {
            Ok(actual_path) => {
                // Rebuild-only extra check: if there are multiple
                // archive matches for this slug (a stale entry from a
                // prior interrupted rebuild), refuse to rename — the
                // operator must consolidate.
                let matches =
                    openspec_archive::find_archive_entries_for_slug(&archive_root, &slug);
                if matches.len() > 1 {
                    let joined: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    let reason = format!(
                        "openspec archive exited 0 but post-condition failed: multiple matching archive directories — operator must consolidate: {}",
                        joined.join(", ")
                    );
                    tracing::error!(slug = %slug, "rebuild: post-condition failed: {reason}");
                    report.failed += 1;
                    report.failures.push(ChangeOutcome {
                        slug: slug.clone(),
                        original_name: original_name.clone(),
                        success: false,
                        failure_reason: reason,
                    });
                    continue;
                }

                // Happy path. Rename the matched archive entry back to
                // the original date-prefixed name if they differ.
                let original_os = std::ffi::OsStr::new(&original_name);
                let needs_rename = actual_path.file_name() != Some(original_os);
                if needs_rename {
                    let target = archive_root.join(&original_name);
                    if let Err(e) = std::fs::rename(&actual_path, &target) {
                        tracing::error!(
                            slug = %slug,
                            "rebuild: rename {} -> {} failed: {e}",
                            actual_path.display(),
                            target.display()
                        );
                        // The change DID archive successfully; this is a
                        // record-keeping failure (the archive entry now
                        // has openspec's name instead of the historical
                        // date prefix). Report it so the operator can
                        // notice.
                        report.failed += 1;
                        report.failures.push(ChangeOutcome {
                            slug: slug.clone(),
                            original_name: original_name.clone(),
                            success: false,
                            failure_reason: format!(
                                "openspec archive succeeded but rename {} -> {} failed: {e}",
                                actual_path.display(),
                                target.display()
                            ),
                        });
                        continue;
                    }
                }
                report.successful += 1;
                report.successes.push(ChangeOutcome {
                    slug,
                    original_name,
                    success: true,
                    failure_reason: String::new(),
                });
            }
            Err(ArchiveFailure::NonZeroExit { code, stderr, stdout }) => {
                // Spawn failure encodes as `code: None`; non-zero exit
                // as `code: Some(n)`. Both render the same way: prefer
                // stderr, fall back to stdout, signal absence
                // explicitly.
                let body = if !stderr.is_empty() {
                    openspec_archive::truncate_for_report(&stderr)
                } else if !stdout.is_empty() {
                    openspec_archive::truncate_for_report(&stdout)
                } else {
                    "(no output)".to_string()
                };
                let reason = format!("openspec exited {code:?}: {body}");
                tracing::error!(
                    slug = %slug,
                    "rebuild: openspec archive failed: {reason}"
                );
                record_failure_with_rollback(
                    workspace,
                    &archive_root,
                    &slug,
                    &original_name,
                    reason,
                    &mut report,
                );
            }
            Err(ArchiveFailure::AbortedMarker { reason, full_output }) => {
                let combined = format!(
                    "openspec refused to apply: {reason}; full output: {full_output}"
                );
                tracing::error!(
                    slug = %slug,
                    "rebuild: openspec abort marker detected: {reason}"
                );
                record_failure_with_rollback(
                    workspace,
                    &archive_root,
                    &slug,
                    &original_name,
                    combined,
                    &mut report,
                );
            }
            Err(ArchiveFailure::ActivePathStillPresent { path, full_output }) => {
                let combined = format!(
                    "openspec archive exited 0 but post-condition failed: active path still present at {}; openspec output: {full_output}",
                    path.display()
                );
                tracing::error!(
                    slug = %slug,
                    "rebuild: post-condition failed: active path still present at {}",
                    path.display()
                );
                record_failure_with_rollback(
                    workspace,
                    &archive_root,
                    &slug,
                    &original_name,
                    combined,
                    &mut report,
                );
            }
            Err(ArchiveFailure::NoArchiveEntryFound { full_output }) => {
                let combined = format!(
                    "openspec archive exited 0 but post-condition failed: openspec archive reported success but the change is missing from both the active path and the archive; openspec output: {full_output}"
                );
                tracing::error!(
                    slug = %slug,
                    "rebuild: post-condition failed (data-loss shape): {combined}"
                );
                report.failed += 1;
                report.failures.push(ChangeOutcome {
                    slug: slug.clone(),
                    original_name: original_name.clone(),
                    success: false,
                    failure_reason: combined,
                });
            }
        }
    }

    // 4. Walk specs/ post-rebuild and compute the modified-vs-unchanged
    //    list against the pre-rebuild snapshot.
    let after = snapshot_specs(&specs_root)?;
    let mut all_paths: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    all_paths.extend(before.keys().cloned());
    all_paths.extend(after.keys().cloned());
    for rel in all_paths {
        let pre = before.get(&rel);
        let post = after.get(&rel);
        let modified = pre != post;
        // A file that exists only in `after` is "new" — counts as modified.
        // A file that exists only in `before` is "deleted" — counts as
        // modified. A file present in both with identical bytes is
        // unchanged.
        report.spec_files.push(SpecFileOutcome {
            path: format!("openspec/specs/{rel}"),
            modified,
        });
    }

    Ok(report)
}

/// Move `openspec/changes/<slug>/` back to
/// `openspec/changes/archive/<original_name>/`. Idempotent against a
/// missing source (no-op). Errors only on an actual rename failure or
/// destination-already-exists.
pub fn rollback_to_archive(
    workspace: &Path,
    slug: &str,
    original_name: &str,
) -> Result<(), std::io::Error> {
    let active = workspace.join("openspec/changes").join(slug);
    if !active.exists() {
        return Ok(());
    }
    let archive_target = workspace
        .join("openspec/changes/archive")
        .join(original_name);
    if archive_target.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "rollback destination already exists: {}",
                archive_target.display()
            ),
        ));
    }
    std::fs::rename(&active, &archive_target)
}

/// Record a per-change failure with a rollback attempt. Used for the
/// failure paths that need to restore `changes/<slug>/` back to archive
/// (spawn failure, non-zero exit, ActivePathStillPresent). If the
/// rollback ITSELF fails (rare), log CRITICAL and concatenate both
/// errors into the failure reason; the rebuild continues to the next
/// change rather than crashing.
fn record_failure_with_rollback(
    workspace: &Path,
    _archive_root: &Path,
    slug: &str,
    original_name: &str,
    base_reason: String,
    report: &mut RebuildReport,
) {
    match rollback_to_archive(workspace, slug, original_name) {
        Ok(()) => {
            report.failed += 1;
            report.rolled_back += 1;
            report.failures.push(ChangeOutcome {
                slug: slug.to_string(),
                original_name: original_name.to_string(),
                success: false,
                failure_reason: base_reason,
            });
        }
        Err(rollback_err) => {
            tracing::error!(
                slug = %slug,
                "CRITICAL: rebuild rollback failed. original={base_reason}; rollback={rollback_err}"
            );
            report.failed += 1;
            report.failures.push(ChangeOutcome {
                slug: slug.to_string(),
                original_name: original_name.to_string(),
                success: false,
                failure_reason: format!(
                    "{base_reason}; rollback ALSO failed: {rollback_err}"
                ),
            });
        }
    }
}

/// Strip a `YYYY-MM-DD-` date prefix from an archive directory name and
/// return the slug. Errors if `name` doesn't match the expected shape.
pub fn strip_date_prefix(name: &str) -> Result<&str> {
    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}-(.+)$").expect("static regex compiles");
    match re.captures(name) {
        Some(c) => c
            .get(1)
            .map(|m| m.as_str())
            .ok_or_else(|| anyhow!("date-prefix regex matched but capture group missing")),
        None => Err(anyhow!(
            "name `{name}` does not match `YYYY-MM-DD-<slug>` shape"
        )),
    }
}

/// Format the dated archive directory name openspec produces today:
/// `<UTC YYYY-MM-DD>-<slug>`. Retained for test fixtures only — the
/// production success path now observes the actual archive directory
/// via `openspec_archive::openspec_archive_with_postcondition` rather
/// than guessing today's date.
#[cfg(test)]
pub fn today_dated_name(slug: &str) -> String {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    format!("{today}-{slug}")
}

/// Recursively snapshot every file under `specs_root`, keyed by relative
/// path. Returns an empty map if `specs_root` is absent. Symlinks are
/// followed transparently because openspec writes plain files.
fn snapshot_specs(specs_root: &Path) -> Result<std::collections::HashMap<String, Vec<u8>>> {
    let mut out: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();
    if !specs_root.is_dir() {
        return Ok(out);
    }
    let mut stack: Vec<PathBuf> = vec![specs_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(specs_root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.to_string_lossy().into_owned());
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.insert(rel, bytes);
            }
        }
    }
    Ok(out)
}

/// Remove every per-capability subdirectory under `specs_root`, preserving
/// `specs_root` itself. Loose files at the top level of `specs_root` are
/// also removed (openspec puts canonical content in capability subdirs,
/// so loose top-level files are stale and should be discarded too).
fn clear_specs_dir(specs_root: &Path) -> Result<()> {
    if !specs_root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(specs_root)
        .with_context(|| format!("reading {}", specs_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

fn print_report(report: &RebuildReport) {
    println!("Rebuild complete.");
    println!();
    println!(
        "Processed: {} changes (in chronological order)",
        report.processed
    );
    println!("Successful: {}", report.successful);
    println!("Failed:     {}", report.failed);

    if report.rolled_back > 0 {
        println!(
            "{} change(s) rolled back to archive due to silent-skip or post-condition failure — see per-change reasons above",
            report.rolled_back
        );
    }

    if !report.failures.is_empty() {
        println!();
        println!("Failures:");
        for f in &report.failures {
            let first_line = f.failure_reason.lines().next().unwrap_or("");
            println!("  - {}: {}", f.original_name, first_line);
        }
    }

    if !report.spec_files.is_empty() {
        println!();
        println!("Canonical specs:");
        for sf in &report.spec_files {
            let tag = if sf.modified { "modified" } else { "unchanged" };
            println!("  - {} ({tag})", sf.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn strip_date_prefix_extracts_slug() {
        assert_eq!(strip_date_prefix("2026-05-15-foo-bar").unwrap(), "foo-bar");
        assert_eq!(
            strip_date_prefix("2025-12-31-multi-dash-name").unwrap(),
            "multi-dash-name"
        );
    }

    #[test]
    fn strip_date_prefix_rejects_non_matching() {
        assert!(strip_date_prefix("no-date-prefix").is_err());
        assert!(strip_date_prefix("2026-foo").is_err());
        assert!(strip_date_prefix("").is_err());
    }

    #[test]
    fn today_dated_name_includes_slug() {
        let name = today_dated_name("my-slug");
        assert!(name.ends_with("-my-slug"), "got {name}");
        // Shape: YYYY-MM-DD-my-slug, i.e. 10 chars + dash + slug.
        let re = Regex::new(r"^\d{4}-\d{2}-\d{2}-my-slug$").unwrap();
        assert!(re.is_match(&name), "got {name}");
    }

    #[test]
    fn validate_args_missing_workspace_errors() {
        let args = SyncSpecsArgs {
            workspace: PathBuf::from("/definitely/not/a/real/path/qwertyuiop"),
            rebuild: true,
            immediate: false,
        };
        let err = validate_args(&args).expect_err("missing path must error");
        assert!(format!("{err}").contains("does not exist"));
    }

    #[test]
    fn validate_args_missing_archive_dir_errors() {
        let dir = TempDir::new().unwrap();
        let args = SyncSpecsArgs {
            workspace: dir.path().to_path_buf(),
            rebuild: true,
            immediate: false,
        };
        let err = validate_args(&args).expect_err("missing archive dir must error");
        assert!(format!("{err}").contains("no archive directory"));
    }

    #[test]
    fn validate_args_with_archive_dir_succeeds() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("openspec/changes/archive")).unwrap();
        let args = SyncSpecsArgs {
            workspace: dir.path().to_path_buf(),
            rebuild: true,
            immediate: false,
        };
        validate_args(&args).unwrap();
    }

    #[test]
    fn snapshot_specs_returns_empty_when_dir_absent() {
        let dir = TempDir::new().unwrap();
        let absent = dir.path().join("openspec/specs");
        let map = snapshot_specs(&absent).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn snapshot_specs_walks_recursive_content() {
        let dir = TempDir::new().unwrap();
        let specs_root = dir.path().join("specs");
        std::fs::create_dir_all(specs_root.join("cap-a")).unwrap();
        std::fs::write(specs_root.join("cap-a/spec.md"), b"hello").unwrap();
        std::fs::create_dir_all(specs_root.join("cap-b/nested")).unwrap();
        std::fs::write(specs_root.join("cap-b/nested/x.md"), b"world").unwrap();
        let map = snapshot_specs(&specs_root).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("cap-a/spec.md").map(|v| v.as_slice()), Some(b"hello".as_slice()));
        assert_eq!(
            map.get("cap-b/nested/x.md").map(|v| v.as_slice()),
            Some(b"world".as_slice())
        );
    }

    #[test]
    fn clear_specs_dir_removes_subdirs_and_files() {
        let dir = TempDir::new().unwrap();
        let specs_root = dir.path().join("specs");
        std::fs::create_dir_all(specs_root.join("cap-a")).unwrap();
        std::fs::write(specs_root.join("cap-a/spec.md"), b"hello").unwrap();
        std::fs::write(specs_root.join("loose.md"), b"loose").unwrap();
        clear_specs_dir(&specs_root).unwrap();
        assert!(specs_root.exists());
        let remaining: Vec<_> = std::fs::read_dir(&specs_root).unwrap().collect();
        assert!(remaining.is_empty(), "specs_root should be empty after clear");
    }

    #[test]
    fn report_modified_files_counts_only_modified() {
        let report = RebuildReport {
            spec_files: vec![
                SpecFileOutcome {
                    path: "a".into(),
                    modified: true,
                },
                SpecFileOutcome {
                    path: "b".into(),
                    modified: false,
                },
                SpecFileOutcome {
                    path: "c".into(),
                    modified: true,
                },
            ],
            ..Default::default()
        };
        assert_eq!(report.modified_files(), 2);
    }

    #[test]
    fn report_failed_slugs_collects_in_order() {
        let report = RebuildReport {
            failures: vec![
                ChangeOutcome {
                    slug: "a".into(),
                    original_name: "2026-01-01-a".into(),
                    success: false,
                    failure_reason: "x".into(),
                },
                ChangeOutcome {
                    slug: "b".into(),
                    original_name: "2026-01-02-b".into(),
                    success: false,
                    failure_reason: "y".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(report.failed_slugs(), vec!["a".to_string(), "b".to_string()]);
    }

    /// End-to-end rebuild against a synthetic workspace. The test
    /// constructs:
    ///   - `openspec/specs/example/spec.md` baseline (will be discarded
    ///      then re-created by openspec from the archived deltas).
    ///   - two archived changes that ADD requirements to the `example`
    ///      capability.
    /// Asserts the rebuild restores the requirements, preserves the
    /// archive's original date prefixes, and reports zero failures.
    ///
    /// Skipped (printed) when `openspec` is not on PATH so the test
    /// suite stays green on hosts without it.
    #[tokio::test]
    async fn rebuild_canonical_e2e_via_openspec() {
        if std::process::Command::new("openspec")
            .arg("--version")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping rebuild_canonical_e2e_via_openspec: openspec not on PATH");
            return;
        }

        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("openspec/specs")).unwrap();
        std::fs::create_dir_all(ws.join("openspec/changes/archive")).unwrap();
        // openspec config file — needed for `openspec archive` to find
        // the project root.
        std::fs::write(
            ws.join("openspec/project.md"),
            "# Project\n\nFixture for rebuild test.\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("openspec/AGENTS.md"),
            "# AGENTS\n\nFixture for rebuild test.\n",
        )
        .unwrap();

        // Pre-rebuild canonical content: empty placeholder spec for the
        // capability so the rebuild has something to clear.
        std::fs::create_dir_all(ws.join("openspec/specs/example")).unwrap();
        std::fs::write(
            ws.join("openspec/specs/example/spec.md"),
            "# example Specification\n\n## Purpose\n\nFixture.\n",
        )
        .unwrap();

        // Archive entry 1: ADD requirement "Foo"
        let entry1_name = "2026-05-15-add-foo";
        let entry1 = ws.join("openspec/changes/archive").join(entry1_name);
        std::fs::create_dir_all(entry1.join("specs/example")).unwrap();
        std::fs::write(
            entry1.join("proposal.md"),
            "## Why\nAdd foo.\n\n## What Changes\n- New foo requirement\n\n## Impact\n- specs: example\n",
        )
        .unwrap();
        std::fs::write(entry1.join("tasks.md"), "## 1. Foo\n- [x] 1.1 done\n").unwrap();
        std::fs::write(
            entry1.join("specs/example/spec.md"),
            "## ADDED Requirements\n\n### Requirement: Foo\nThe system SHALL foo.\n\n#### Scenario: Foo happens\n- **WHEN** asked to foo\n- **THEN** it foos\n",
        )
        .unwrap();

        // Archive entry 2: ADD requirement "Bar"
        let entry2_name = "2026-05-18-add-bar";
        let entry2 = ws.join("openspec/changes/archive").join(entry2_name);
        std::fs::create_dir_all(entry2.join("specs/example")).unwrap();
        std::fs::write(
            entry2.join("proposal.md"),
            "## Why\nAdd bar.\n\n## What Changes\n- New bar requirement\n\n## Impact\n- specs: example\n",
        )
        .unwrap();
        std::fs::write(entry2.join("tasks.md"), "## 1. Bar\n- [x] 1.1 done\n").unwrap();
        std::fs::write(
            entry2.join("specs/example/spec.md"),
            "## ADDED Requirements\n\n### Requirement: Bar\nThe system SHALL bar.\n\n#### Scenario: Bar happens\n- **WHEN** asked to bar\n- **THEN** it bars\n",
        )
        .unwrap();

        let report = rebuild_canonical(ws).await.unwrap();
        if report.failed > 0 {
            for f in &report.failures {
                eprintln!("  fail: {} — {}", f.slug, f.failure_reason);
            }
        }
        assert_eq!(report.failed, 0, "expected zero failures");
        assert_eq!(report.processed, 2);
        assert_eq!(report.successful, 2);

        // Canonical spec exists with both requirements.
        let canonical = std::fs::read_to_string(ws.join("openspec/specs/example/spec.md"))
            .expect("canonical spec produced");
        assert!(
            canonical.contains("Foo") && canonical.contains("Bar"),
            "canonical spec should contain both ADDED requirements:\n---\n{canonical}\n---"
        );

        // Archive's original date prefixes preserved.
        for name in [entry1_name, entry2_name] {
            assert!(
                ws.join("openspec/changes/archive").join(name).is_dir(),
                "archive entry {name} should still be present with its original date prefix"
            );
        }
    }

    // ----- Test helpers: workspace builder + fake archive runners -----

    fn make_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("openspec/specs")).unwrap();
        std::fs::create_dir_all(ws.join("openspec/changes/archive")).unwrap();
        std::fs::write(ws.join("openspec/project.md"), "# Project\nFixture.\n").unwrap();
        std::fs::write(ws.join("openspec/AGENTS.md"), "# AGENTS\nFixture.\n").unwrap();
        dir
    }

    /// Create a fixture archive directory at
    /// `openspec/changes/archive/<name>/` with minimal files. Useful for
    /// rebuild-loop tests that don't need real openspec content.
    fn make_archive_entry(ws: &Path, name: &str) {
        let dir = ws.join("openspec/changes/archive").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proposal.md"), "fixture\n").unwrap();
    }

    fn fake_exit(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    /// Runner stub: exits 0, prints `"would archive <slug>"` to stdout,
    /// performs no fs work. Reproduces the production silent-skip case.
    struct SilentSkipArchiveRunner;
    impl ArchiveRunner for SilentSkipArchiveRunner {
        fn run(&self, _workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!("would archive {slug}\n"),
                stderr: String::new(),
            })
        }
    }

    /// Runner stub that performs the archive correctly (moves
    /// `changes/<slug>/` into `archive/<today>-<slug>/`).
    struct SuccessfulArchiveRunner;
    impl ArchiveRunner for SuccessfulArchiveRunner {
        fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            let from = workspace.join("openspec/changes").join(slug);
            let today = today_dated_name(slug);
            let to = workspace.join("openspec/changes/archive").join(&today);
            std::fs::rename(&from, &to)
                .map_err(|e| format!("test runner rename failed: {e}"))?;
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!("archived {slug}\n"),
                stderr: String::new(),
            })
        }
    }

    /// Runner stub that exits non-zero with stderr.
    struct FailingArchiveRunner;
    impl ArchiveRunner for FailingArchiveRunner {
        fn run(&self, _workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            Ok(ArchiveRunOutput {
                status: fake_exit(1),
                stdout: String::new(),
                stderr: format!("validation error for {slug}\n"),
            })
        }
    }

    /// Runner stub: exits 0, prints an `Aborted.` marker preceded by a
    /// diagnostic line, performs no fs work. Reproduces openspec's
    /// "refused to apply a delta" case observed in production when an
    /// archived change references a header that no longer exists.
    struct AbortedOutputArchiveRunner;
    impl ArchiveRunner for AbortedOutputArchiveRunner {
        fn run(&self, _workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!(
                    "{slug} MODIFIED failed for header \"### Requirement: X\" - not found\nAborted. No files were changed.\n"
                ),
                stderr: String::new(),
            })
        }
    }

    /// Mixed-outcome runner: silent-skips slugs in `skip_slugs`, succeeds
    /// otherwise.
    struct MixedArchiveRunner {
        skip_slugs: std::collections::HashSet<String>,
    }
    impl ArchiveRunner for MixedArchiveRunner {
        fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            if self.skip_slugs.contains(slug) {
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: format!("would archive {slug}\n"),
                    stderr: String::new(),
                })
            } else {
                let from = workspace.join("openspec/changes").join(slug);
                let today = today_dated_name(slug);
                let to = workspace.join("openspec/changes/archive").join(&today);
                std::fs::rename(&from, &to)
                    .map_err(|e| format!("test runner rename failed: {e}"))?;
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: format!("archived {slug}\n"),
                    stderr: String::new(),
                })
            }
        }
    }

    // Note: `detect_openspec_abort`, `format_full_output`,
    // `find_archive_entries_for_slug`, and the four
    // `ArchiveFailure` post-condition variants are now exercised in
    // `crate::openspec_archive::tests`. The rebuild loop tests below
    // still cover end-to-end behaviour through the shared helper.

    // ----- rollback_to_archive tests -----

    #[test]
    fn rollback_restores_active_to_archive() {
        let dir = make_workspace();
        let ws = dir.path();
        let active = ws.join("openspec/changes/foo");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("proposal.md"), "fixture\n").unwrap();

        rollback_to_archive(ws, "foo", "2026-05-15-foo").unwrap();

        assert!(!active.exists(), "active path should be empty after rollback");
        let restored = ws.join("openspec/changes/archive/2026-05-15-foo");
        assert!(restored.is_dir(), "restored archive entry should exist");
        assert!(restored.join("proposal.md").exists());
    }

    #[test]
    fn rollback_noop_when_source_missing() {
        let dir = make_workspace();
        let ws = dir.path();
        // No changes/foo to roll back.
        rollback_to_archive(ws, "foo", "2026-05-15-foo").unwrap();
        // archive should still be empty.
        let archive = ws.join("openspec/changes/archive/2026-05-15-foo");
        assert!(!archive.exists());
    }

    #[test]
    fn rollback_errors_on_destination_collision() {
        let dir = make_workspace();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
        std::fs::create_dir_all(ws.join("openspec/changes/archive/2026-05-15-foo"))
            .unwrap();

        let err = rollback_to_archive(ws, "foo", "2026-05-15-foo")
            .expect_err("destination collision should error");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    // ----- rebuild_canonical_with_runner tests -----

    #[tokio::test]
    async fn rebuild_silent_skip_rolls_back_all_changes() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");
        make_archive_entry(ws, "2026-05-18-bar");
        make_archive_entry(ws, "2026-05-20-baz");

        let report = rebuild_canonical_with_runner(ws, &SilentSkipArchiveRunner)
            .await
            .unwrap();

        assert_eq!(report.processed, 3);
        assert_eq!(report.successful, 0);
        assert_eq!(report.failed, 3);
        assert_eq!(report.rolled_back, 3);

        // All failure reasons should include "would archive" from the stub's stdout.
        for f in &report.failures {
            assert!(
                f.failure_reason.contains("would archive"),
                "expected stub stdout in reason, got: {}",
                f.failure_reason
            );
        }

        // Archive entries restored with original date prefixes.
        for name in ["2026-05-15-foo", "2026-05-18-bar", "2026-05-20-baz"] {
            assert!(
                ws.join("openspec/changes/archive").join(name).is_dir(),
                "expected archive entry {name} to be restored"
            );
        }

        // changes/ directory should contain no active-path slug entries
        // (only the archive subdirectory).
        let changes_root = ws.join("openspec/changes");
        for entry in std::fs::read_dir(&changes_root).unwrap() {
            let entry = entry.unwrap();
            assert_eq!(
                entry.file_name(),
                std::ffi::OsString::from("archive"),
                "unexpected leftover in changes/: {:?}",
                entry.file_name()
            );
        }
    }

    #[tokio::test]
    async fn rebuild_mixed_outcomes_rolls_back_only_skipped() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");
        make_archive_entry(ws, "2026-05-18-bar");
        make_archive_entry(ws, "2026-05-20-baz");

        let mut skip = std::collections::HashSet::new();
        skip.insert("bar".to_string());
        let runner = MixedArchiveRunner { skip_slugs: skip };

        let report = rebuild_canonical_with_runner(ws, &runner).await.unwrap();

        assert_eq!(report.processed, 3);
        assert_eq!(report.successful, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.rolled_back, 1);
        assert_eq!(report.failures[0].slug, "bar");
        // Skipped slug back at original archive location.
        assert!(
            ws.join("openspec/changes/archive/2026-05-18-bar").is_dir(),
            "skipped slug should be rolled back to original name"
        );
        // Successful slugs renamed back to original.
        assert!(ws.join("openspec/changes/archive/2026-05-15-foo").is_dir());
        assert!(ws.join("openspec/changes/archive/2026-05-20-baz").is_dir());
        // No active-path leakage.
        assert!(!ws.join("openspec/changes/foo").exists());
        assert!(!ws.join("openspec/changes/bar").exists());
        assert!(!ws.join("openspec/changes/baz").exists());
    }

    #[tokio::test]
    async fn rebuild_non_zero_exit_rolls_back() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");

        let report = rebuild_canonical_with_runner(ws, &FailingArchiveRunner)
            .await
            .unwrap();

        assert_eq!(report.failed, 1);
        assert_eq!(report.rolled_back, 1);
        assert!(
            report.failures[0]
                .failure_reason
                .contains("validation error"),
            "expected stderr in failure reason, got: {}",
            report.failures[0].failure_reason
        );
        assert!(ws.join("openspec/changes/archive/2026-05-15-foo").is_dir());
        assert!(!ws.join("openspec/changes/foo").exists());
    }

    #[tokio::test]
    async fn rebuild_success_path_renames_to_original_when_dates_differ() {
        let dir = make_workspace();
        let ws = dir.path();
        // Original archive entry has a date well in the past; the
        // SuccessfulArchiveRunner will produce today-prefixed dirs.
        make_archive_entry(ws, "2026-01-01-foo");

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();

        assert_eq!(report.successful, 1);
        assert_eq!(report.failed, 0);
        // Original date prefix restored.
        assert!(ws.join("openspec/changes/archive/2026-01-01-foo").is_dir());
        // Today's path should NOT exist any more (it was renamed back).
        assert!(!ws.join("openspec/changes/archive").join(today_dated_name("foo")).exists());
    }

    #[tokio::test]
    async fn rebuild_success_path_no_rename_when_already_original() {
        let dir = make_workspace();
        let ws = dir.path();
        // Original archive entry IS today-prefixed; rename is a no-op.
        let original = today_dated_name("foo");
        make_archive_entry(ws, &original);

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();

        assert_eq!(report.successful, 1);
        assert_eq!(report.failed, 0);
        assert!(ws.join("openspec/changes/archive").join(&original).is_dir());
    }

    #[tokio::test]
    async fn rebuild_success_path_handles_collision_suffix_rename() {
        let dir = make_workspace();
        let ws = dir.path();
        // Original archive entry from long ago.
        make_archive_entry(ws, "2026-01-01-foo");

        // Runner that produces a collision-suffix name (`<today>-foo-2`).
        struct CollisionRunner;
        impl ArchiveRunner for CollisionRunner {
            fn run(
                &self,
                workspace: &Path,
                slug: &str,
            ) -> Result<ArchiveRunOutput, String> {
                let from = workspace.join("openspec/changes").join(slug);
                let today = today_dated_name(slug);
                let to = workspace
                    .join("openspec/changes/archive")
                    .join(format!("{today}-2"));
                std::fs::rename(&from, &to)
                    .map_err(|e| format!("test runner rename failed: {e}"))?;
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }

        let report = rebuild_canonical_with_runner(ws, &CollisionRunner)
            .await
            .unwrap();

        assert_eq!(report.successful, 1);
        assert_eq!(report.failed, 0);
        // Renamed back to original name even with the suffix in actual_path.
        assert!(ws.join("openspec/changes/archive/2026-01-01-foo").is_dir());
    }

    #[tokio::test]
    async fn rebuild_data_loss_case_no_rollback_no_crash() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");

        // Runner that pretends archive succeeded but deletes the
        // change directory without producing an archive entry.
        struct DataLossRunner;
        impl ArchiveRunner for DataLossRunner {
            fn run(
                &self,
                workspace: &Path,
                slug: &str,
            ) -> Result<ArchiveRunOutput, String> {
                let from = workspace.join("openspec/changes").join(slug);
                std::fs::remove_dir_all(&from)
                    .map_err(|e| format!("test removal failed: {e}"))?;
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }

        let report = rebuild_canonical_with_runner(ws, &DataLossRunner)
            .await
            .unwrap();

        assert_eq!(report.failed, 1);
        // No rollback expected (active path was already empty).
        assert_eq!(report.rolled_back, 0);
        assert!(
            report.failures[0]
                .failure_reason
                .contains("missing from both the active path and the archive"),
            "expected data-loss description, got: {}",
            report.failures[0].failure_reason
        );
    }

    #[tokio::test]
    async fn rebuild_multiple_matches_no_rollback_no_rename() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");

        // Runner that produces TWO archive entries for the same slug.
        // The second uses a `-<digits>` collision suffix so both match
        // the post-condition glob.
        struct CollisionPairRunner;
        impl ArchiveRunner for CollisionPairRunner {
            fn run(
                &self,
                workspace: &Path,
                slug: &str,
            ) -> Result<ArchiveRunOutput, String> {
                let from = workspace.join("openspec/changes").join(slug);
                let today = today_dated_name(slug);
                let to = workspace.join("openspec/changes/archive").join(&today);
                std::fs::rename(&from, &to)
                    .map_err(|e| format!("test runner rename failed: {e}"))?;
                // And add a collision-suffix duplicate.
                let extra = workspace
                    .join("openspec/changes/archive")
                    .join(format!("{today}-2"));
                std::fs::create_dir_all(&extra)
                    .map_err(|e| format!("test extra-dir create failed: {e}"))?;
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }

        let report = rebuild_canonical_with_runner(ws, &CollisionPairRunner)
            .await
            .unwrap();

        assert_eq!(report.failed, 1);
        assert_eq!(report.rolled_back, 0);
        assert!(
            report.failures[0]
                .failure_reason
                .contains("multiple matching archive directories"),
            "expected multi-match description, got: {}",
            report.failures[0].failure_reason
        );
        // Both directories still present (rebuild did not pick one).
        let today = today_dated_name("foo");
        assert!(ws.join("openspec/changes/archive").join(&today).is_dir());
        assert!(
            ws.join("openspec/changes/archive")
                .join(format!("{today}-2"))
                .is_dir()
        );
    }

    #[tokio::test]
    async fn rebuild_aborted_marker_rolls_back_and_records_headline() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");

        let report = rebuild_canonical_with_runner(ws, &AbortedOutputArchiveRunner)
            .await
            .unwrap();

        assert_eq!(report.processed, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.successful, 0);
        assert_eq!(report.rolled_back, 1);
        let reason = &report.failures[0].failure_reason;
        assert!(
            reason.starts_with("openspec refused to apply:"),
            "expected refused-to-apply headline, got: {reason}"
        );
        assert!(
            reason.contains("MODIFIED failed for header"),
            "expected diagnostic line in headline, got: {reason}"
        );
        // Rolled back to original archive name; active path empty.
        assert!(ws.join("openspec/changes/archive/2026-05-15-foo").is_dir());
        assert!(!ws.join("openspec/changes/foo").exists());
    }

    /// Mixed-outcome runner combining all three categories: clean
    /// success, marker-less silent skip, and `Aborted.`-marker abort.
    /// Used by `rebuild_mixed_marker_and_silent_skip_get_distinct_reasons`.
    struct MixedAbortAndSilentRunner {
        abort_slugs: std::collections::HashSet<String>,
        skip_slugs: std::collections::HashSet<String>,
    }
    impl ArchiveRunner for MixedAbortAndSilentRunner {
        fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            if self.abort_slugs.contains(slug) {
                return Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: format!(
                        "{slug} MODIFIED failed for header \"### Requirement: X\" - not found\nAborted. No files were changed.\n"
                    ),
                    stderr: String::new(),
                });
            }
            if self.skip_slugs.contains(slug) {
                return Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: format!("would archive {slug}\n"),
                    stderr: String::new(),
                });
            }
            let from = workspace.join("openspec/changes").join(slug);
            let today = today_dated_name(slug);
            let to = workspace.join("openspec/changes/archive").join(&today);
            std::fs::rename(&from, &to)
                .map_err(|e| format!("test runner rename failed: {e}"))?;
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!("archived {slug}\n"),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn rebuild_mixed_marker_and_silent_skip_get_distinct_reasons() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-ok");
        make_archive_entry(ws, "2026-05-16-aborts");
        make_archive_entry(ws, "2026-05-17-silently-skips");

        let mut abort = std::collections::HashSet::new();
        abort.insert("aborts".to_string());
        let mut skip = std::collections::HashSet::new();
        skip.insert("silently-skips".to_string());
        let runner = MixedAbortAndSilentRunner {
            abort_slugs: abort,
            skip_slugs: skip,
        };

        let report = rebuild_canonical_with_runner(ws, &runner).await.unwrap();

        assert_eq!(report.processed, 3);
        assert_eq!(report.successful, 1);
        assert_eq!(report.failed, 2);
        assert_eq!(report.rolled_back, 2);

        let abort_failure = report
            .failures
            .iter()
            .find(|f| f.slug == "aborts")
            .expect("abort entry should be in failures");
        assert!(
            abort_failure
                .failure_reason
                .starts_with("openspec refused to apply:"),
            "abort headline mismatch, got: {}",
            abort_failure.failure_reason
        );

        let silent_failure = report
            .failures
            .iter()
            .find(|f| f.slug == "silently-skips")
            .expect("silent-skip entry should be in failures");
        assert!(
            silent_failure
                .failure_reason
                .starts_with("openspec archive exited 0 but post-condition failed:"),
            "silent-skip headline mismatch, got: {}",
            silent_failure.failure_reason
        );

        // All three archive entries restored to original names.
        assert!(ws.join("openspec/changes/archive/2026-05-15-ok").is_dir());
        assert!(ws.join("openspec/changes/archive/2026-05-16-aborts").is_dir());
        assert!(
            ws.join("openspec/changes/archive/2026-05-17-silently-skips")
                .is_dir()
        );
    }

    #[tokio::test]
    async fn rebuild_rollback_collision_records_combined_failure() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry(ws, "2026-05-15-foo");

        // Runner that silent-skips but also pre-creates a stale entry at
        // the rollback destination so the rollback hits AlreadyExists.
        struct PoisonedRunner;
        impl ArchiveRunner for PoisonedRunner {
            fn run(
                &self,
                workspace: &Path,
                slug: &str,
            ) -> Result<ArchiveRunOutput, String> {
                // Pre-poison the rollback target so the rebuild's rollback
                // can't put `changes/foo` back into archive/2026-05-15-foo.
                std::fs::create_dir_all(
                    workspace
                        .join("openspec/changes/archive/2026-05-15-foo"),
                )
                .map_err(|e| format!("test poison failed: {e}"))?;
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: format!("would archive {slug}\n"),
                    stderr: String::new(),
                })
            }
        }

        let report = rebuild_canonical_with_runner(ws, &PoisonedRunner)
            .await
            .unwrap();

        assert_eq!(report.failed, 1);
        // Rollback failed, so we shouldn't count it as rolled-back.
        assert_eq!(report.rolled_back, 0);
        let reason = &report.failures[0].failure_reason;
        assert!(
            reason.contains("rollback ALSO failed"),
            "expected combined failure reason, got: {reason}"
        );
        assert!(
            reason.contains("would archive"),
            "expected original failure context in reason, got: {reason}"
        );
    }

    // ----- pre-pass integration tests -----

    /// Write a minimal archived-change fixture with a `specs/<cap>/spec.md`
    /// file carrying `body`. Used by pre-pass integration tests below.
    fn make_archive_entry_with_delta(ws: &Path, name: &str, cap: &str, body: &str) {
        let entry = ws.join("openspec/changes/archive").join(name);
        std::fs::create_dir_all(entry.join("specs").join(cap)).unwrap();
        std::fs::write(entry.join("proposal.md"), "fixture\n").unwrap();
        std::fs::write(entry.join("specs").join(cap).join("spec.md"), body).unwrap();
    }

    #[tokio::test]
    async fn rebuild_pre_pass_renames_inversion() {
        let dir = make_workspace();
        let ws = dir.path();
        // The canonical inversion case: a MODIFIES requirement F added
        // by another same-day change, but the MODIFIER sorts alphabetically
        // first ("no-op…" < "self-healing…"). The pre-pass must prefix
        // the ADD with `a01-` so it sorts first within the day-group.
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-no-op-completion-is-failure",
            "orchestrator",
            "## MODIFIED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-self-healing-deployment",
            "orchestrator",
            "## ADDED Requirements\n\n### Requirement: Reject archive-only iterations as Failed\nBody.\n",
        );

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();

        assert_eq!(
            report.prefix_renames.len(),
            1,
            "expected one prefix rename, got {:?}",
            report.prefix_renames
        );
        let rn = &report.prefix_renames[0];
        assert_eq!(rn.from, "2026-05-14-self-healing-deployment");
        assert_eq!(rn.to, "2026-05-14-a01-self-healing-deployment");
        assert_eq!(rn.day, "2026-05-14");
        assert!(!rn.dependency_summary.is_empty(), "expected a summary");

        // The directory was actually renamed on disk before the
        // chronological loop ran. The post-rebuild canonical-spec
        // contents prove the loop processed both successfully (the
        // SuccessfulArchiveRunner's rename-back-to-original step
        // restores the date-prefixed name, so we cannot assert on the
        // post-rebuild directory name; but the prefix_renames record is
        // proof the pre-pass fired).
        assert_eq!(report.processed, 2);
        // SuccessfulArchiveRunner only performs the fs rename; it does
        // not actually apply deltas to canonical specs. So we don't
        // assert anything about canonical content here.
    }

    #[tokio::test]
    async fn rebuild_pre_pass_no_dependencies_zero_renames() {
        let dir = make_workspace();
        let ws = dir.path();
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-foo",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-bar",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Bar\nBody.\n",
        );

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();
        assert!(
            report.prefix_renames.is_empty(),
            "expected no renames, got {:?}",
            report.prefix_renames
        );
        assert!(report.abort_reason.is_none());
    }

    #[tokio::test]
    async fn rebuild_pre_pass_cycle_aborts_without_mutation() {
        let dir = make_workspace();
        let ws = dir.path();
        // Pre-populate a canonical spec file; we'll verify it's UNTOUCHED
        // after the abort (no canonical mutation must occur).
        std::fs::create_dir_all(ws.join("openspec/specs/cap")).unwrap();
        let canonical_path = ws.join("openspec/specs/cap/spec.md");
        std::fs::write(&canonical_path, b"pre-existing\n").unwrap();

        // A ADDs Foo and MODIFIES Bar; B ADDs Bar and MODIFIES Foo. Cycle.
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-a",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n\n## MODIFIED Requirements\n\n### Requirement: Bar\nBody.\n",
        );
        make_archive_entry_with_delta(
            ws,
            "2026-05-14-b",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Bar\nBody.\n\n## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();
        assert!(matches!(
            report.abort_reason,
            Some(crate::cli::sync_specs_deps::RebuildAbortReason::Cycle { .. })
        ));
        assert!(report.prefix_renames.is_empty());
        assert_eq!(report.processed, 0, "no entries should have been processed");
        // Canonical spec file untouched.
        let after = std::fs::read(&canonical_path).unwrap();
        assert_eq!(after, b"pre-existing\n");
        // Archive directories unrenamed.
        assert!(ws.join("openspec/changes/archive/2026-05-14-a").is_dir());
        assert!(ws.join("openspec/changes/archive/2026-05-14-b").is_dir());
    }

    #[tokio::test]
    async fn rebuild_pre_pass_cross_day_backward_dependency_aborts() {
        let dir = make_workspace();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("openspec/specs/cap")).unwrap();
        let canonical_path = ws.join("openspec/specs/cap/spec.md");
        std::fs::write(&canonical_path, b"pre-existing\n").unwrap();

        make_archive_entry_with_delta(
            ws,
            "2026-05-10-modify-foo",
            "cap",
            "## MODIFIED Requirements\n\n### Requirement: Foo\nBody.\n",
        );
        make_archive_entry_with_delta(
            ws,
            "2026-05-15-add-foo",
            "cap",
            "## ADDED Requirements\n\n### Requirement: Foo\nBody.\n",
        );

        let report = rebuild_canonical_with_runner(ws, &SuccessfulArchiveRunner)
            .await
            .unwrap();
        assert!(matches!(
            report.abort_reason,
            Some(crate::cli::sync_specs_deps::RebuildAbortReason::CrossDayBackwardDependency { .. })
        ));
        assert!(report.prefix_renames.is_empty());
        assert_eq!(report.processed, 0);
        let after = std::fs::read(&canonical_path).unwrap();
        assert_eq!(after, b"pre-existing\n");
    }
}
