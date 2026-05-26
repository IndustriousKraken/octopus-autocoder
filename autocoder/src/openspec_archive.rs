//! Shared `openspec archive <slug> -y` invocation + post-condition
//! verification. Both `queue::archive` (in-iteration self-heal and
//! per-pass archive) and `cli::sync_specs` (rebuild loop) delegate here
//! so they apply the same abort-marker detection and filesystem checks.
//!
//! The trait-based runner mirrors the pattern already used by the
//! rebuild path so tests can inject stubs without spawning real
//! subprocesses.

use regex::Regex;
use std::path::{Path, PathBuf};

const CHANGES_SUBDIR: &str = "openspec/changes";
const ARCHIVE_SUBDIR: &str = "openspec/changes/archive";
const MAX_REPORT_CHARS: usize = 500;

/// Captured output from a single `openspec archive <slug> -y`
/// invocation. The `Err(String)` returned by `ArchiveRunner::run` is
/// reserved for spawn failure only; non-zero exit codes land in `Ok`
/// so the post-condition logic applies uniformly.
#[derive(Debug, Clone)]
pub struct ArchiveRunOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Pluggable runner for `openspec archive`. The production
/// implementation shells out to the binary; tests substitute stubs to
/// simulate success, silent skip, abort-marker, or non-zero exit
/// without touching the host's openspec install.
pub trait ArchiveRunner: Send + Sync {
    fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String>;
}

/// Production runner — shells out to `openspec archive <slug> -y`.
pub struct RealArchiveRunner;

impl ArchiveRunner for RealArchiveRunner {
    fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
        match std::process::Command::new("openspec")
            .args(["archive", slug, "-y"])
            .current_dir(workspace)
            .output()
        {
            Ok(out) => Ok(ArchiveRunOutput {
                status: out.status,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err("openspec binary not found on PATH".to_string())
            }
            Err(e) => Err(format!("spawning openspec: {e}")),
        }
    }
}

/// Structured archive failure. Each variant names a specific failure
/// mode the shared helper detects. Callers map this to their own
/// domain-appropriate error type whose message includes the openspec
/// output excerpt explaining the cause.
#[derive(Debug, Clone)]
pub enum ArchiveFailure {
    /// openspec returned non-zero, OR the helper couldn't spawn the
    /// binary at all (encoded as `code: None`, message in `stderr`).
    NonZeroExit {
        code: Option<i32>,
        stderr: String,
        stdout: String,
    },
    /// openspec exited 0 but its stdout contained a line whose first
    /// non-whitespace token is `Aborted.`. `reason` is the most
    /// informative preceding line (or the marker line itself if no
    /// preceding non-empty line exists). `full_output` is the
    /// truncated openspec output for inclusion in operator messages.
    AbortedMarker {
        reason: String,
        full_output: String,
    },
    /// openspec exited 0 with no marker, but
    /// `openspec/changes/<slug>/` still exists — the silent-skip case
    /// where openspec quietly refused to perform the rename.
    ActivePathStillPresent {
        path: PathBuf,
        full_output: String,
    },
    /// openspec exited 0 with no marker, active path is gone, but no
    /// `openspec/changes/archive/*-<slug>/` directory matches. The
    /// data-loss-shaped case.
    NoArchiveEntryFound { full_output: String },
}

/// Run `openspec archive <slug> -y` via `runner` and verify the
/// post-condition. Returns `Ok(matched_archive_path)` only when the
/// archive directory actually moved and openspec reported clean
/// success. The helper performs, in order:
///
/// 1. Spawn via `runner.run(workspace, slug)`. Spawn failure → `Err(NonZeroExit{code: None, ..})`.
/// 2. Non-zero exit → `Err(NonZeroExit{..})`.
/// 3. Scan stdout for the `Aborted.` marker (`detect_openspec_abort`).
///    Matched → `Err(AbortedMarker{..})`.
/// 4. `openspec/changes/<slug>/` still exists → `Err(ActivePathStillPresent{..})`.
/// 5. Glob `openspec/changes/archive/*-<slug>/` for matches. None →
///    `Err(NoArchiveEntryFound{..})`.
/// 6. Otherwise return `Ok(<lex-highest match path>)`.
///
/// When the archive root contains multiple matches for the same slug
/// (a stale archive from a prior interrupted rebuild), the helper
/// returns the lex-highest entry (the most recent date prefix). Callers
/// that consider multiple matches a failure perform their own check via
/// [`find_archive_entries_for_slug`].
pub fn openspec_archive_with_postcondition(
    runner: &dyn ArchiveRunner,
    workspace: &Path,
    slug: &str,
) -> Result<PathBuf, ArchiveFailure> {
    let out = match runner.run(workspace, slug) {
        Ok(out) => out,
        Err(spawn_err) => {
            return Err(ArchiveFailure::NonZeroExit {
                code: None,
                stderr: spawn_err,
                stdout: String::new(),
            });
        }
    };

    if !out.status.success() {
        return Err(ArchiveFailure::NonZeroExit {
            code: out.status.code(),
            stderr: out.stderr.trim().to_string(),
            stdout: out.stdout.trim().to_string(),
        });
    }

    let full_output = format_full_output(&out);

    if let Some(reason) = detect_openspec_abort(&out.stdout) {
        return Err(ArchiveFailure::AbortedMarker {
            reason,
            full_output,
        });
    }

    let active_path = workspace.join(CHANGES_SUBDIR).join(slug);
    if active_path.exists() {
        return Err(ArchiveFailure::ActivePathStillPresent {
            path: active_path,
            full_output,
        });
    }

    let archive_root = workspace.join(ARCHIVE_SUBDIR);
    let mut matches = find_archive_entries_for_slug(&archive_root, slug);
    if matches.is_empty() {
        return Err(ArchiveFailure::NoArchiveEntryFound { full_output });
    }
    // matches is sorted ascending; lex-highest is the most recent date prefix.
    Ok(matches.pop().expect("non-empty"))
}

/// Scan `stdout` for openspec's `Aborted.` marker — the signal openspec
/// emits when it refuses to apply a change (e.g. a broken `MODIFIED`
/// reference) but still exits 0. Detection is line-based: only a line
/// whose first non-whitespace token is exactly `Aborted.` (with the
/// trailing period) triggers a match. A line containing `aborted`
/// lowercase, or `Aborted` without the trailing period, or `Aborted.`
/// mid-line, does NOT match.
///
/// Returns `Some(reason)` where `reason` is the most informative
/// preceding line — the nearest non-empty line above the `Aborted.` line
/// if one exists, otherwise the trimmed `Aborted.` line itself. This
/// captures real-world cases where openspec prints a diagnostic line
/// (`MODIFIED failed for header "..." - not found`) immediately before
/// `Aborted. No files were changed.`. Returns `None` when no matching
/// `Aborted.` line is present.
pub fn detect_openspec_abort(stdout: &str) -> Option<String> {
    let lines: Vec<&str> = stdout.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let first_token = match line.split_whitespace().next() {
            Some(t) => t,
            None => continue,
        };
        if first_token != "Aborted." {
            continue;
        }
        if i > 0 {
            for j in (0..i).rev() {
                let prev = lines[j].trim();
                if !prev.is_empty() {
                    return Some(prev.to_string());
                }
            }
        }
        return Some(line.trim().to_string());
    }
    None
}

/// Read `archive_root` and return all entries whose name matches
/// `<date>-<slug>` or `<date>-<slug>-<N>` (the openspec collision
/// suffix), where `<date>` is `YYYY-MM-DD`. Excludes entries without a
/// date prefix and entries that share only an unrelated suffix.
/// Returned vector is sorted ascending by file name (lex-highest =
/// most recent date prefix).
pub fn find_archive_entries_for_slug(archive_root: &Path, slug: &str) -> Vec<PathBuf> {
    let pattern = format!(
        r"^\d{{4}}-\d{{2}}-\d{{2}}-{}(?:-\d+)?$",
        regex::escape(slug)
    );
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let read = match std::fs::read_dir(archive_root) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in read.flatten() {
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
        if !re.is_match(&name) {
            continue;
        }
        out.push(entry.path());
    }
    out.sort();
    out
}

/// Truncate a long openspec excerpt to a fixed character cap. Used so
/// failure-reason strings stay reasonable in chatops alerts and logs.
pub fn truncate_for_report(s: &str) -> String {
    if s.chars().count() <= MAX_REPORT_CHARS {
        s.to_string()
    } else {
        s.chars().take(MAX_REPORT_CHARS).collect::<String>() + "…"
    }
}

/// Format an `ArchiveRunOutput` as a single excerpt suitable for
/// inclusion in a failure-reason message. Prefers `stderr`; falls back
/// to `stdout` if stderr is empty; emits `"(no output)"` when both are
/// empty. Always includes the exit-status code prefix so the operator
/// can correlate with the exit semantics. The body is truncated via
/// [`truncate_for_report`].
fn format_full_output(out: &ArchiveRunOutput) -> String {
    let stderr = out.stderr.trim();
    let stdout = out.stdout.trim();
    let body = if !stderr.is_empty() {
        truncate_for_report(stderr)
    } else if !stdout.is_empty() {
        truncate_for_report(stdout)
    } else {
        "(no output)".to_string()
    };
    format!("openspec exited {:?}: {}", out.status.code(), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fake_exit(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    fn today_dated_name(slug: &str) -> String {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        format!("{today}-{slug}")
    }

    fn make_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("openspec/specs")).unwrap();
        std::fs::create_dir_all(ws.join("openspec/changes/archive")).unwrap();
        dir
    }

    fn make_active_change(ws: &Path, slug: &str) {
        let dir = ws.join("openspec/changes").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proposal.md"), "fixture\n").unwrap();
    }

    /// Runner stub that performs the archive correctly (moves
    /// `changes/<slug>/` into `archive/<today>-<slug>/`).
    struct SuccessRunner;
    impl ArchiveRunner for SuccessRunner {
        fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            let from = workspace.join("openspec/changes").join(slug);
            let to = workspace
                .join("openspec/changes/archive")
                .join(today_dated_name(slug));
            std::fs::rename(&from, &to)
                .map_err(|e| format!("mock rename failed: {e}"))?;
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!("archived {slug}\n"),
                stderr: String::new(),
            })
        }
    }

    /// Runner stub: exit 0, emits `Aborted.` marker, performs no fs work.
    struct AbortedRunner;
    impl ArchiveRunner for AbortedRunner {
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

    /// Runner stub: exit 0, benign stdout, performs no fs work
    /// (silent-skip case without the marker).
    struct SilentSkipRunner;
    impl ArchiveRunner for SilentSkipRunner {
        fn run(&self, _workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: format!("would archive {slug}\n"),
                stderr: String::new(),
            })
        }
    }

    /// Runner stub: exit 0, removes the change dir but produces no
    /// archive entry (data-loss case).
    struct DataLossRunner;
    impl ArchiveRunner for DataLossRunner {
        fn run(&self, workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            let from = workspace.join("openspec/changes").join(slug);
            std::fs::remove_dir_all(&from)
                .map_err(|e| format!("mock removal failed: {e}"))?;
            Ok(ArchiveRunOutput {
                status: fake_exit(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    /// Runner stub: exit non-zero with stderr.
    struct FailingRunner;
    impl ArchiveRunner for FailingRunner {
        fn run(&self, _workspace: &Path, slug: &str) -> Result<ArchiveRunOutput, String> {
            Ok(ArchiveRunOutput {
                status: fake_exit(1),
                stdout: String::new(),
                stderr: format!("validation error for {slug}\n"),
            })
        }
    }

    /// Runner stub: simulates spawn failure (the openspec binary
    /// isn't on PATH).
    struct SpawnFailRunner;
    impl ArchiveRunner for SpawnFailRunner {
        fn run(&self, _workspace: &Path, _slug: &str) -> Result<ArchiveRunOutput, String> {
            Err("openspec binary not found on PATH".to_string())
        }
    }

    #[test]
    fn helper_happy_path_returns_ok_with_archive_path() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let path = openspec_archive_with_postcondition(&SuccessRunner, ws, "foo")
            .expect("happy path returns Ok");
        let expected = ws
            .join("openspec/changes/archive")
            .join(today_dated_name("foo"));
        assert_eq!(path, expected);
        // Active path actually moved.
        assert!(!ws.join("openspec/changes/foo").exists());
    }

    #[test]
    fn helper_aborted_marker_returns_structured_err() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let err = openspec_archive_with_postcondition(&AbortedRunner, ws, "foo")
            .expect_err("aborted marker must surface as Err");
        match err {
            ArchiveFailure::AbortedMarker {
                reason,
                full_output,
            } => {
                assert!(
                    reason.contains("MODIFIED failed for header"),
                    "reason should be the preceding diagnostic line: {reason}"
                );
                assert!(
                    full_output.contains("Aborted."),
                    "full_output should include the marker line: {full_output}"
                );
            }
            other => panic!("expected AbortedMarker, got {other:?}"),
        }
        // Active path NOT moved (runner did no fs work).
        assert!(ws.join("openspec/changes/foo").exists());
    }

    #[test]
    fn helper_silent_skip_without_marker_returns_active_path_still_present() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let err = openspec_archive_with_postcondition(&SilentSkipRunner, ws, "foo")
            .expect_err("silent-skip must surface as Err");
        match err {
            ArchiveFailure::ActivePathStillPresent { path, full_output } => {
                assert_eq!(path, ws.join("openspec/changes/foo"));
                assert!(
                    full_output.contains("would archive"),
                    "full_output should include the stub stdout: {full_output}"
                );
            }
            other => panic!("expected ActivePathStillPresent, got {other:?}"),
        }
    }

    #[test]
    fn helper_data_loss_returns_no_archive_entry_found() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let err = openspec_archive_with_postcondition(&DataLossRunner, ws, "foo")
            .expect_err("data-loss must surface as Err");
        match err {
            ArchiveFailure::NoArchiveEntryFound { full_output } => {
                assert!(
                    full_output.contains("(no output)"),
                    "full_output should signal empty output: {full_output}"
                );
            }
            other => panic!("expected NoArchiveEntryFound, got {other:?}"),
        }
    }

    #[test]
    fn helper_non_zero_exit_returns_structured_err() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let err = openspec_archive_with_postcondition(&FailingRunner, ws, "foo")
            .expect_err("non-zero exit must surface as Err");
        match err {
            ArchiveFailure::NonZeroExit {
                code,
                stderr,
                stdout,
            } => {
                assert_eq!(code, Some(1));
                assert!(
                    stderr.contains("validation error"),
                    "stderr should be preserved: {stderr}"
                );
                assert!(stdout.is_empty(), "stdout was empty in stub: {stdout}");
            }
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
        // Active path untouched (runner did no fs work).
        assert!(ws.join("openspec/changes/foo").exists());
    }

    #[test]
    fn helper_spawn_failure_encoded_as_non_zero_exit_with_none_code() {
        let dir = make_workspace();
        let ws = dir.path();
        make_active_change(ws, "foo");

        let err = openspec_archive_with_postcondition(&SpawnFailRunner, ws, "foo")
            .expect_err("spawn failure must surface as Err");
        match err {
            ArchiveFailure::NonZeroExit {
                code,
                stderr,
                stdout,
            } => {
                assert_eq!(code, None, "spawn failure encodes code as None");
                assert!(
                    stderr.contains("binary not found on PATH"),
                    "spawn error message should be in stderr: {stderr}"
                );
                assert!(stdout.is_empty());
            }
            other => panic!("expected NonZeroExit with code=None, got {other:?}"),
        }
    }

    #[test]
    fn helper_picks_lex_highest_when_multiple_archive_matches() {
        let dir = make_workspace();
        let ws = dir.path();
        // Pre-populate two archive entries for the slug — older + newer.
        let older = ws.join("openspec/changes/archive/2026-01-01-foo");
        let newer = ws.join("openspec/changes/archive/2026-05-04-foo");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::create_dir_all(&newer).unwrap();
        // No active change dir; runner is a no-op (active path
        // already gone, archive entries already in place).
        struct NoopRunner;
        impl ArchiveRunner for NoopRunner {
            fn run(
                &self,
                _workspace: &Path,
                _slug: &str,
            ) -> Result<ArchiveRunOutput, String> {
                Ok(ArchiveRunOutput {
                    status: fake_exit(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let path = openspec_archive_with_postcondition(&NoopRunner, ws, "foo")
            .expect("happy with multi-match returns Ok");
        assert_eq!(path, newer, "lex-highest entry should be picked");
    }

    // ----- detect_openspec_abort tests (moved from sync_specs.rs) -----

    #[test]
    fn detect_abort_real_world_with_preceding_diagnostic() {
        let stdout = "member-saved-cards MODIFIED failed for header \"### Requirement: Foo\" - not found\nAborted. No files were changed.\n";
        let got = detect_openspec_abort(stdout);
        assert_eq!(
            got.as_deref(),
            Some(
                "member-saved-cards MODIFIED failed for header \"### Requirement: Foo\" - not found"
            )
        );
    }

    #[test]
    fn detect_abort_alone_with_trailing_text() {
        let stdout = "Aborted. No files were changed.\n";
        let got = detect_openspec_abort(stdout);
        assert_eq!(got.as_deref(), Some("Aborted. No files were changed."));
    }

    #[test]
    fn detect_abort_alone_no_trailing_text() {
        let stdout = "Aborted.\n";
        let got = detect_openspec_abort(stdout);
        assert_eq!(got.as_deref(), Some("Aborted."));
    }

    #[test]
    fn detect_abort_skips_blank_preceding_lines() {
        let stdout = "real reason here\n\n\nAborted. No files were changed.\n";
        let got = detect_openspec_abort(stdout);
        assert_eq!(got.as_deref(), Some("real reason here"));
    }

    #[test]
    fn detect_abort_clean_archive_returns_none() {
        let stdout = "Specs to update: example\nApplying changes to openspec/specs/example/spec.md\nTotals: +3 lines\nSpecs updated successfully.\n";
        assert_eq!(detect_openspec_abort(stdout), None);
    }

    #[test]
    fn detect_abort_lowercase_aborted_returns_none() {
        let stdout = "the operation was aborted by the user\n";
        assert_eq!(detect_openspec_abort(stdout), None);
    }

    #[test]
    fn detect_abort_without_trailing_period_returns_none() {
        let stdout = "Aborted\n";
        assert_eq!(detect_openspec_abort(stdout), None);
    }

    #[test]
    fn detect_abort_mid_line_returns_none() {
        let stdout = "some prefix Aborted. No files were changed.\n";
        assert_eq!(detect_openspec_abort(stdout), None);
    }

    #[test]
    fn detect_abort_indented_marker_still_matches() {
        let stdout = "preceding reason\n    Aborted. No files were changed.\n";
        let got = detect_openspec_abort(stdout);
        assert_eq!(got.as_deref(), Some("preceding reason"));
    }

    #[test]
    fn detect_abort_empty_stdout_returns_none() {
        assert_eq!(detect_openspec_abort(""), None);
    }

    // ----- find_archive_entries_for_slug tests -----

    #[test]
    fn find_archive_entries_filters_by_date_prefix_and_slug() {
        let dir = make_workspace();
        let ws = dir.path();
        let root = ws.join("openspec/changes/archive");
        std::fs::create_dir_all(root.join("2026-01-01-foo")).unwrap();
        std::fs::create_dir_all(root.join("2026-05-04-foo")).unwrap();
        std::fs::create_dir_all(root.join("2026-05-04-foo-2")).unwrap();
        std::fs::create_dir_all(root.join("foo-foo")).unwrap();
        std::fs::create_dir_all(root.join("2026-05-04-bar")).unwrap();

        let matches = find_archive_entries_for_slug(&root, "foo");
        let names: Vec<String> = matches
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "2026-01-01-foo".to_string(),
                "2026-05-04-foo".to_string(),
                "2026-05-04-foo-2".to_string(),
            ]
        );
    }

    #[test]
    fn find_archive_entries_missing_root_returns_empty() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let root = ws.join("nonexistent");
        assert!(find_archive_entries_for_slug(&root, "foo").is_empty());
    }

    // ----- truncate_for_report tests -----

    #[test]
    fn truncate_for_report_passthrough_when_short() {
        let s = "hello";
        assert_eq!(truncate_for_report(s), "hello");
    }

    #[test]
    fn truncate_for_report_truncates_long_input() {
        let s = "x".repeat(MAX_REPORT_CHARS * 3);
        let out = truncate_for_report(&s);
        assert!(out.ends_with('…'));
        // Truncated body has MAX_REPORT_CHARS 'x's + one '…' char.
        assert_eq!(out.chars().count(), MAX_REPORT_CHARS + 1);
    }
}
