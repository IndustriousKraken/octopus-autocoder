//! `autocoder sync-specs --rebuild` — rebuild all canonical specs from
//! archive history. Single mode for v1: full chronological replay. See
//! `openspec/changes/rebuild-canonical-specs-from-archive/proposal.md` for
//! the why-incremental-is-unsafe rationale.

use crate::busy_marker;
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
#[derive(Debug, Clone, Default, Serialize)]
pub struct RebuildReport {
    pub processed: usize,
    pub successful: usize,
    pub failed: usize,
    pub successes: Vec<ChangeOutcome>,
    pub failures: Vec<ChangeOutcome>,
    pub spec_files: Vec<SpecFileOutcome>,
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
    let archive_root = workspace.join("openspec/changes/archive");
    if !archive_root.is_dir() {
        return Err(anyhow!(
            "archive directory not found at {}",
            archive_root.display()
        ));
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

        match run_openspec_archive(workspace, &slug) {
            Ok(()) => {
                let today_name = today_dated_name(&slug);
                let today_path = archive_root.join(&today_name);
                if today_name != original_name {
                    if let Err(e) = std::fs::rename(&today_path, archive_root.join(&original_name)) {
                        tracing::error!(
                            slug = %slug,
                            "rebuild: in-place rename {} -> {} failed: {e}",
                            today_path.display(),
                            archive_root.join(&original_name).display()
                        );
                        // The change DID archive successfully; rename
                        // failure is a record-keeping concern. Track as
                        // a failure so the operator notices the date
                        // prefix shifted.
                        report.failed += 1;
                        report.failures.push(ChangeOutcome {
                            slug: slug.clone(),
                            original_name: original_name.clone(),
                            success: false,
                            failure_reason: format!(
                                "openspec archive succeeded but date-prefix restore failed: {e}"
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
            Err(reason) => {
                tracing::error!(
                    slug = %slug,
                    "rebuild: openspec archive failed: {reason}"
                );
                // Leave the change at the active path for the operator
                // to inspect. Continue with subsequent changes.
                report.failed += 1;
                report.failures.push(ChangeOutcome {
                    slug,
                    original_name,
                    success: false,
                    failure_reason: reason,
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

/// Invoke `openspec archive <slug> -y` in `workspace`. On non-zero exit,
/// return the (truncated) stderr as the failure reason.
fn run_openspec_archive(workspace: &Path, slug: &str) -> Result<(), String> {
    match std::process::Command::new("openspec")
        .args(["archive", slug, "-y"])
        .current_dir(workspace)
        .output()
    {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let combined = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("openspec exited {:?} with no output", out.status.code())
            };
            Err(truncate_for_report(&combined))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("openspec binary not found on PATH".to_string())
        }
        Err(e) => Err(format!("spawning openspec: {e}")),
    }
}

fn truncate_for_report(s: &str) -> String {
    const MAX: usize = 500;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        s.chars().take(MAX).collect::<String>() + "…"
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
/// `<UTC YYYY-MM-DD>-<slug>`.
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
}
