//! Workspace-cache size cap + least-recently-used eviction (a65).
//!
//! Per-repo workspaces live under `<cache>/workspaces/<key>` and
//! accumulate a build-artifact tree (a Rust repo's `target/debug`, a
//! Node repo's `node_modules/`, …) with no bound. Left alone, any daemon
//! running several repos long enough fills its disk. This module:
//!
//! - measures the total size of `<cache>/workspaces/` AND each
//!   workspace's size (a symlink-safe directory walk);
//! - caches each workspace's measured size under `<state>/` so the
//!   per-iteration cap check does NOT re-walk every idle workspace on
//!   every poll tick (see below);
//! - maintains a per-workspace last-used timestamp under `<state>/`
//!   (recorded at each iteration that uses the workspace), read to order
//!   eviction candidates oldest-first;
//! - evicts whole least-recently-used IDLE workspaces
//!   (`remove_dir_all`) when an operator-set cap is exceeded.
//!
//! Per-workspace size cache: a naive cap check would recursively walk
//! every workspace on every iteration of every repo — with N repos that
//! is ~N² full `target/`-sized walks per poll cycle, a real I/O storm
//! even when the cache is far under the cap. Instead, the enforcement
//! pass re-measures ONLY the workspace whose repo is currently iterating
//! (the only one whose size can have changed — an idle workspace is not
//! being written to between the iterations that use it) and reuses each
//! other workspace's last measured size from `<state>/workspace-sizes/`.
//! Steady-state per-tick I/O drops from "walk all N workspaces" to "walk
//! the one current workspace + read N-1 tiny size files". A workspace
//! with no cached size yet (first pass after a cap is set, or a clone
//! predating this feature) is measured once and cached, so the full walk
//! happens at most once per workspace, never every tick.
//!
//! Whole-workspace eviction is deliberately language-agnostic: it makes
//! NO assumption about which subdirectories are build artifacts; it
//! removes the entire least-recently-used clone. Eviction is lossless —
//! an evicted repo re-clones via the existing workspace-init path on its
//! next iteration, and per-PR revision state AND audit state live under
//! `<state>/`, not the workspace.
//!
//! Safety invariants:
//! - the repo currently iterating is NEVER evicted;
//! - any workspace holding a per-repo busy marker is NEVER evicted (so a
//!   concurrently-iterating repo is never removed out from under itself);
//! - if only non-evictable workspaces remain and they exceed the cap, the
//!   daemon logs a WARN AND proceeds — eviction NEVER blocks or fails an
//!   iteration.
//!
//! The `DaemonPaths` reference is threaded explicitly into every public
//! function (function-parameter pattern per the canonical `Production
//! paths SHALL be threaded` requirement).

use crate::paths::DaemonPaths;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::Path;

/// Bytes per gigabyte (1024^3). `cache.workspaces_max_gb` is expressed in
/// these units; the cap is converted to bytes for comparison against the
/// measured directory sizes.
pub const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

/// Summary of one cap-enforcement pass. Returned for logging AND so tests
/// can assert on the decision without scraping the tracing log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EvictionReport {
    /// Basenames evicted this pass, in eviction order (oldest-used first).
    pub evicted: Vec<String>,
    /// Total bytes reclaimed by the evictions in this pass.
    pub reclaimed_bytes: u64,
    /// The measured total size of `<cache>/workspaces/` after the pass.
    pub final_total_bytes: u64,
    /// `true` iff the pass finished still over the cap because only
    /// non-evictable (current + busy) workspaces remained. The caller has
    /// already logged the WARN; this flag lets tests assert the outcome.
    pub over_cap_after: bool,
}

/// Recursively sum the byte sizes of every regular file under `path`,
/// WITHOUT following symlinks out of the tree. A symlink contributes its
/// own (link) size, never its target's — so a workspace that contains a
/// symlink pointing outside the cache root can never cause us to measure
/// (or later delete) anything beyond the workspace itself.
pub fn dir_size(path: &Path) -> u64 {
    fn walk(dir: &Path, acc: &mut u64) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let meta = match entry.path().symlink_metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let ft = meta.file_type();
            if ft.is_symlink() {
                // Count the link's own size; never traverse its target.
                *acc = acc.saturating_add(meta.len());
            } else if ft.is_dir() {
                walk(&entry.path(), acc);
            } else {
                *acc = acc.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0;
    match path.symlink_metadata() {
        Ok(m) if m.file_type().is_dir() => walk(path, &mut total),
        Ok(m) => total = m.len(),
        Err(_) => {}
    }
    total
}

/// Record `basename` as used right now: write the current UTC timestamp
/// (RFC3339) to `<state>/workspace-last-used/<basename>`. Best-effort —
/// a failure is logged at DEBUG and never propagates, because a missing
/// timestamp only affects eviction ORDERING (the workspace is treated as
/// oldest), never correctness.
pub fn record_last_used(paths: &DaemonPaths, basename: &str) {
    let dir = paths.workspace_last_used_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(
            dir = %dir.display(),
            "workspace-cache: could not create last-used dir (eviction ordering may degrade): {e}"
        );
        return;
    }
    let path = paths.workspace_last_used_path(basename);
    let now = Utc::now().to_rfc3339();
    if let Err(e) = std::fs::write(&path, now) {
        tracing::debug!(
            path = %path.display(),
            "workspace-cache: could not record last-used timestamp: {e}"
        );
    }
}

/// Read the last-used timestamp for `basename`, or `None` when the marker
/// is absent OR unparseable. `None` orders the workspace as oldest (a
/// prime eviction candidate) — appropriate for a workspace the daemon has
/// never recorded usage for (e.g. a clone predating this feature).
pub fn read_last_used(paths: &DaemonPaths, basename: &str) -> Option<DateTime<Utc>> {
    let path = paths.workspace_last_used_path(basename);
    let raw = std::fs::read_to_string(&path).ok()?;
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Read the cached measured size (bytes) for `basename`, or `None` when
/// the marker is absent OR unparseable. A `None` means "size unknown" —
/// the caller measures the workspace fresh and caches the result. The
/// cache is only ever trusted for an IDLE workspace (whose size cannot
/// have changed since it was recorded); the currently-iterating
/// workspace is always re-measured.
pub fn read_cached_size(paths: &DaemonPaths, basename: &str) -> Option<u64> {
    let path = paths.workspace_size_path(basename);
    std::fs::read_to_string(&path).ok()?.trim().parse::<u64>().ok()
}

/// Cache `size` (bytes) as `basename`'s last measured size under
/// `<state>/workspace-sizes/<basename>`. Best-effort — a failure is
/// logged at DEBUG and never propagates, because a missing cache entry
/// only costs a fresh re-measure on the next pass, never correctness.
pub fn write_cached_size(paths: &DaemonPaths, basename: &str, size: u64) {
    let dir = paths.workspace_sizes_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(
            dir = %dir.display(),
            "workspace-cache: could not create sizes dir (cap check will re-walk): {e}"
        );
        return;
    }
    let path = paths.workspace_size_path(basename);
    if let Err(e) = std::fs::write(&path, size.to_string()) {
        tracing::debug!(
            path = %path.display(),
            "workspace-cache: could not cache measured size: {e}"
        );
    }
}

/// Remove a whole workspace clone (`<cache>/workspaces/<basename>`) AND
/// its last-used marker, returning the bytes reclaimed. Symlink-safe: if
/// the workspace path is itself a symlink, the link is removed (never its
/// target), so eviction can never delete data outside the cache root.
///
/// The caller is responsible for the per-eviction INFO log (key,
/// reclaimed bytes, new total) since it tracks the running total; this
/// helper only performs the removal.
pub fn evict_workspace(paths: &DaemonPaths, basename: &str) -> std::io::Result<u64> {
    let dir = paths.workspaces_dir().join(basename);
    let meta = dir.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        // A symlink at the workspace slot is anomalous; remove the link
        // itself (not its target) and report zero reclaimed.
        std::fs::remove_file(&dir)?;
        let _ = std::fs::remove_file(paths.workspace_last_used_path(basename));
        let _ = std::fs::remove_file(paths.workspace_size_path(basename));
        return Ok(0);
    }
    let reclaimed = dir_size(&dir);
    std::fs::remove_dir_all(&dir)?;
    // The last-used + size markers are now orphaned (their workspace is
    // gone). Remove them so the state dir does not accumulate stale
    // markers; a re-clone records fresh ones.
    let _ = std::fs::remove_file(paths.workspace_last_used_path(basename));
    let _ = std::fs::remove_file(paths.workspace_size_path(basename));
    Ok(reclaimed)
}

/// Set of workspace basenames currently holding a per-repo busy marker
/// (`<runtime>/busy/<basename>.json`). These are NEVER eviction
/// candidates — a marker means a pass is (or recently was) working on
/// that repo. Conservative by design: a present marker blocks eviction
/// regardless of PID liveness; a stale marker is cleared by the owning
/// repo's own next iteration via `busy_marker::try_acquire`.
fn busy_basenames(paths: &DaemonPaths) -> HashSet<String> {
    let mut set = HashSet::new();
    let dir = paths.busy_markers_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return set,
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str()
            && let Some(base) = name.strip_suffix(".json")
        {
            set.insert(base.to_string());
        }
    }
    set
}

/// Sort key for LRU ordering: a recorded timestamp's epoch-millis, or
/// `i64::MIN` (oldest) for a workspace with no recorded last-used time.
fn lru_key(ts: &Option<DateTime<Utc>>) -> i64 {
    ts.map(|t| t.timestamp_millis()).unwrap_or(i64::MIN)
}

/// Enforce the workspace-cache cap at a repo's iteration start.
///
/// When `max_gb` is `None` (unbounded, the default), this is a no-op —
/// no measurement, no eviction. When set, the total `<cache>/workspaces/`
/// size is measured; if it exceeds `max_gb` gigabytes, least-recently-
/// used IDLE workspaces are evicted (oldest last-used first) until the
/// total is under the cap OR only non-evictable workspaces remain.
///
/// `current_basename` is the basename of the workspace for the repo whose
/// iteration is running; it is never evicted. Workspaces holding a busy
/// marker are never evicted either. If, after evicting every evictable
/// candidate, the total is still over the cap (the non-evictable set
/// alone exceeds it), a WARN is logged AND the pass returns normally —
/// eviction never blocks or fails an iteration.
pub fn enforce_cap(
    paths: &DaemonPaths,
    max_gb: Option<u64>,
    current_basename: &str,
) -> EvictionReport {
    // Convert the operator-facing gigabyte cap to bytes and delegate to
    // the byte-granular core. `None` (unbounded) stays `None`.
    let cap_bytes = max_gb.map(|gb| gb.saturating_mul(BYTES_PER_GB));
    enforce_cap_bytes(paths, cap_bytes, current_basename)
}

/// Byte-granular core of [`enforce_cap`]. Split out so the gigabyte-unit
/// public API stays operator-facing while tests can drive a small,
/// byte-precise cap without multi-gigabyte fixtures.
///
/// To avoid an I/O storm, this does NOT recursively re-walk every
/// workspace on every call: only `current_basename` (the workspace whose
/// repo is iterating — the only one that can have changed size) is
/// measured fresh; every other workspace's size is read from the
/// `<state>/workspace-sizes/` cache, falling back to a one-time fresh
/// measurement (and caching the result) when no cached size exists.
pub(crate) fn enforce_cap_bytes(
    paths: &DaemonPaths,
    cap_bytes: Option<u64>,
    current_basename: &str,
) -> EvictionReport {
    let mut report = EvictionReport::default();
    let Some(cap_bytes) = cap_bytes else {
        // Unbounded: today's behaviour, nothing to do.
        return report;
    };
    let root = paths.workspaces_dir();

    // Measure every workspace directly under the cache root. Skip
    // symlinks and stray files — only real directories are workspaces.
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => {
            // Workspaces root absent (nothing cloned yet) → nothing to do.
            return report;
        }
    };
    let mut sizes: Vec<(String, u64)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let meta = match entry.path().symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.file_type().is_dir() {
            continue;
        }
        // Re-measure the currently-iterating workspace fresh (its build
        // may have grown it) and refresh its cached size. For every other
        // (idle) workspace, reuse the cached size — its size cannot have
        // changed since it was recorded — and only fall back to a fresh
        // walk when no cached size exists yet (then cache it, so the walk
        // happens at most once per workspace, not every tick).
        let size = if name == current_basename {
            let measured = dir_size(&entry.path());
            write_cached_size(paths, &name, measured);
            measured
        } else {
            match read_cached_size(paths, &name) {
                Some(cached) => cached,
                None => {
                    let measured = dir_size(&entry.path());
                    write_cached_size(paths, &name, measured);
                    measured
                }
            }
        };
        total = total.saturating_add(size);
        sizes.push((name, size));
    }

    report.final_total_bytes = total;
    if total <= cap_bytes {
        // Under cap already — no eviction.
        return report;
    }

    // Build the evictable-candidate list: every workspace that is NOT the
    // current repo's AND NOT busy. Order oldest-last-used first.
    let busy = busy_basenames(paths);
    let mut candidates: Vec<(String, u64, Option<DateTime<Utc>>)> = sizes
        .into_iter()
        .filter(|(name, _)| name != current_basename && !busy.contains(name))
        .map(|(name, size)| {
            let lu = read_last_used(paths, &name);
            (name, size, lu)
        })
        .collect();
    candidates.sort_by_key(|c| lru_key(&c.2));

    let mut running = total;
    for (name, _size, _lu) in candidates {
        if running <= cap_bytes {
            break;
        }
        match evict_workspace(paths, &name) {
            Ok(reclaimed) => {
                running = running.saturating_sub(reclaimed);
                report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(reclaimed);
                report.evicted.push(name.clone());
                tracing::info!(
                    workspace = %name,
                    reclaimed_bytes = reclaimed,
                    new_total_bytes = running,
                    cap_bytes,
                    "workspace-cache: evicted least-recently-used idle workspace"
                );
            }
            Err(e) => {
                // A removal failure (permissions, transient FS) must not
                // abort the pass. Log and move on to the next candidate.
                tracing::warn!(
                    workspace = %name,
                    "workspace-cache: eviction failed (iteration continues): {e}"
                );
            }
        }
    }

    report.final_total_bytes = running;
    if running > cap_bytes {
        report.over_cap_after = true;
        tracing::warn!(
            total_bytes = running,
            cap_bytes,
            current = %current_basename,
            "workspace-cache: cannot reclaim to target — only non-evictable \
             (current + busy) workspaces remain; proceeding over cap"
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;
    use std::path::PathBuf;

    /// Create a workspace directory `<cache>/workspaces/<basename>` holding
    /// a single file of `bytes` bytes. Returns the workspace path.
    fn make_workspace(paths: &DaemonPaths, basename: &str, bytes: usize) -> PathBuf {
        let ws = paths.workspaces_dir().join(basename);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("blob.bin"), vec![0u8; bytes]).unwrap();
        ws
    }

    /// Write a busy marker for `basename` under `<runtime>/busy/`.
    fn write_busy_marker(paths: &DaemonPaths, basename: &str) {
        let dir = paths.busy_markers_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{basename}.json")), "{}").unwrap();
    }

    /// Set `basename`'s last-used marker to `secs` seconds ago.
    fn set_last_used_ago(paths: &DaemonPaths, basename: &str, secs: i64) {
        std::fs::create_dir_all(paths.workspace_last_used_dir()).unwrap();
        let ts = (Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339();
        std::fs::write(paths.workspace_last_used_path(basename), ts).unwrap();
    }

    #[test]
    fn dir_size_sums_nested_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("top.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("a/mid.bin"), vec![0u8; 200]).unwrap();
        std::fs::write(root.join("a/b/deep.bin"), vec![0u8; 300]).unwrap();
        assert_eq!(dir_size(&root), 600);
    }

    #[test]
    fn dir_size_does_not_follow_symlinks_out_of_tree() {
        let dir = tempfile::TempDir::new().unwrap();
        // A large file OUTSIDE the workspace tree.
        let outside = dir.path().join("outside.bin");
        std::fs::write(&outside, vec![0u8; 10_000]).unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("small.bin"), vec![0u8; 50]).unwrap();
        // Symlink inside the workspace pointing at the big outside file.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
        // The symlink's own (small) size is counted, NOT the 10_000-byte
        // target — so the measured size stays near 50, never ~10_050.
        let measured = dir_size(&ws);
        assert!(
            measured < 1_000,
            "symlink target must not be followed; measured {measured}"
        );
    }

    #[test]
    fn unset_cap_is_a_noop() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "repo_a", 1_000);
        make_workspace(&paths, "repo_b", 1_000);
        let report = enforce_cap(&paths, None, "repo_a");
        assert!(report.evicted.is_empty(), "unset cap must never evict");
        assert_eq!(report.reclaimed_bytes, 0);
        // Both workspaces survive.
        assert!(paths.workspaces_dir().join("repo_a").is_dir());
        assert!(paths.workspaces_dir().join("repo_b").is_dir());
    }

    #[test]
    fn under_cap_does_not_evict() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "repo_a", 1_000);
        make_workspace(&paths, "repo_b", 1_000);
        // 1 GB cap, ~2 KB on disk → well under cap.
        let report = enforce_cap(&paths, Some(1), "repo_a");
        assert!(report.evicted.is_empty());
        assert!(paths.workspaces_dir().join("repo_a").is_dir());
        assert!(paths.workspaces_dir().join("repo_b").is_dir());
    }

    /// Over-cap cache evicts oldest-used idle workspaces until under cap.
    /// Uses a byte cap via the `enforce_cap_bytes` test seam so the test
    /// stays small (no multi-GB fixtures).
    #[test]
    fn over_cap_evicts_oldest_first_until_under() {
        let (_td, paths) = test_daemon_paths();
        // Three idle workspaces, 1000 bytes each → 3000 total.
        make_workspace(&paths, "old", 1_000);
        make_workspace(&paths, "mid", 1_000);
        make_workspace(&paths, "new", 1_000);
        set_last_used_ago(&paths, "old", 300);
        set_last_used_ago(&paths, "mid", 200);
        set_last_used_ago(&paths, "new", 100);
        // Current repo is a separate, also-present workspace.
        make_workspace(&paths, "current", 1_000);
        set_last_used_ago(&paths, "current", 50);

        // Cap of 2500 bytes: total 4000 → must evict until <= 2500.
        let report = enforce_cap_bytes(&paths, 2_500, "current");
        // Evicts "old" (1000 → 3000) then "mid" (1000 → 2000 <= 2500).
        assert_eq!(report.evicted, vec!["old".to_string(), "mid".to_string()]);
        assert_eq!(report.reclaimed_bytes, 2_000);
        assert!(!report.over_cap_after);
        assert!(!paths.workspaces_dir().join("old").exists());
        assert!(!paths.workspaces_dir().join("mid").exists());
        assert!(paths.workspaces_dir().join("new").is_dir());
        assert!(paths.workspaces_dir().join("current").is_dir());
        // The evicted workspaces' last-used markers are cleaned up.
        assert!(!paths.workspace_last_used_path("old").exists());
        assert!(!paths.workspace_last_used_path("mid").exists());
    }

    #[test]
    fn never_evicts_current_or_busy_workspace() {
        let (_td, paths) = test_daemon_paths();
        // current + busy are the two oldest, but must be protected.
        make_workspace(&paths, "current", 1_000);
        make_workspace(&paths, "busy", 1_000);
        make_workspace(&paths, "idle", 1_000);
        set_last_used_ago(&paths, "current", 999);
        set_last_used_ago(&paths, "busy", 998);
        set_last_used_ago(&paths, "idle", 100);
        write_busy_marker(&paths, "busy");

        // Cap of 1500 bytes: total 3000. Only "idle" is evictable.
        let report = enforce_cap_bytes(&paths, 1_500, "current");
        assert_eq!(report.evicted, vec!["idle".to_string()]);
        // current + busy survive even though they are older.
        assert!(paths.workspaces_dir().join("current").is_dir());
        assert!(paths.workspaces_dir().join("busy").is_dir());
        assert!(!paths.workspaces_dir().join("idle").exists());
        // current + busy = 2000 bytes > 1500 cap → over-cap WARN path.
        assert!(report.over_cap_after);
    }

    #[test]
    fn warns_and_proceeds_when_only_non_evictable_remain() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "current", 2_000);
        make_workspace(&paths, "busy", 2_000);
        write_busy_marker(&paths, "busy");
        // Cap of 1000 bytes: total 4000, but NOTHING is evictable.
        let report = enforce_cap_bytes(&paths, 1_000, "current");
        assert!(report.evicted.is_empty(), "nothing is evictable");
        assert!(report.over_cap_after, "must flag over-cap");
        // Both workspaces survive; the iteration is expected to proceed.
        assert!(paths.workspaces_dir().join("current").is_dir());
        assert!(paths.workspaces_dir().join("busy").is_dir());
    }

    #[test]
    fn missing_workspaces_root_is_noop() {
        let (_td, paths) = test_daemon_paths();
        // Do not create <cache>/workspaces/ at all.
        let report = enforce_cap(&paths, Some(1), "anything");
        assert!(report.evicted.is_empty());
        assert_eq!(report.final_total_bytes, 0);
    }

    #[test]
    fn workspace_with_no_marker_is_treated_as_oldest() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "no_marker", 1_000);
        make_workspace(&paths, "recent", 1_000);
        set_last_used_ago(&paths, "recent", 10);
        // "no_marker" has no last-used file → sorts oldest → evicted first.
        make_workspace(&paths, "current", 1);
        let report = enforce_cap_bytes(&paths, 1_500, "current");
        assert_eq!(report.evicted.first().map(String::as_str), Some("no_marker"));
    }

    #[test]
    fn record_and_read_last_used_roundtrip() {
        let (_td, paths) = test_daemon_paths();
        assert!(read_last_used(&paths, "repo_x").is_none());
        record_last_used(&paths, "repo_x");
        let ts = read_last_used(&paths, "repo_x").expect("timestamp recorded");
        let delta = (Utc::now() - ts).num_seconds().abs();
        assert!(delta < 60, "recorded timestamp should be ~now; delta {delta}s");
    }

    /// a65 tasks 3.4 + 4.5: an evicted repo re-clones losslessly. Eviction
    /// removes the whole workspace, but per-PR revision state AND audit
    /// state live under `<state>/` (NOT the workspace), so they survive;
    /// `workspace::ensure_initialized` re-creates the workspace cleanly on
    /// the next iteration.
    #[test]
    fn eviction_is_lossless_state_survives_and_workspace_reclones() {
        use std::process::Command;

        fn run_git(path: &Path, args: &[&str]) {
            let status = Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        }

        let (_td, paths) = test_daemon_paths();
        let scratch = tempfile::TempDir::new().unwrap();

        // A fixture "remote" with one commit.
        let remote = scratch.path().join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        run_git(&remote, &["init", "-q", "-b", "main"]);
        run_git(&remote, &["config", "user.email", "t@example.com"]);
        run_git(&remote, &["config", "user.name", "t"]);
        std::fs::write(remote.join("README.md"), "hi\n").unwrap();
        run_git(&remote, &["add", "README.md"]);
        run_git(&remote, &["commit", "-q", "-m", "init"]);
        let url = remote.to_string_lossy().to_string();

        // Clone the "cold" repo into the cache root via the production
        // workspace-init path.
        let basename = "github_com_owner_cold";
        let workspace = paths.workspaces_dir().join(basename);
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        crate::workspace::ensure_initialized(&paths, &workspace, &url, None).unwrap();
        assert!(workspace.join(".git").is_dir(), "fixture clone must succeed");

        // Per-PR revision state lives under <state>/revisions/, keyed by
        // workspace basename — NOT inside the workspace. Seed one so we can
        // prove it survives the eviction.
        let revisions_dir = paths.revisions_dir();
        std::fs::create_dir_all(&revisions_dir).unwrap();
        let revision_state = revisions_dir.join(format!("{basename}.json"));
        std::fs::write(&revision_state, r#"{"pr":7,"revisions":2}"#).unwrap();

        // Drive eviction with a tiny byte cap so the cold clone is over
        // budget. `current` is a different (non-existent) basename so the
        // cold workspace is the eviction target.
        let report = super::enforce_cap_bytes(&paths, Some(1), "github_com_owner_current");
        assert!(
            report.evicted.contains(&basename.to_string()),
            "the cold workspace must be evicted; report: {report:?}"
        );
        assert!(
            !workspace.exists(),
            "the evicted workspace directory must be gone"
        );

        // Lossless: the per-PR revision state file is untouched.
        assert!(
            revision_state.exists(),
            "per-PR revision state under <state>/ must survive workspace eviction"
        );
        assert_eq!(
            std::fs::read_to_string(&revision_state).unwrap(),
            r#"{"pr":7,"revisions":2}"#,
            "revision state content must be intact after eviction"
        );

        // Next iteration: the existing workspace-init path re-clones cleanly.
        crate::workspace::ensure_initialized(&paths, &workspace, &url, None).unwrap();
        assert!(
            workspace.join(".git").is_dir(),
            "evicted repo must re-clone cleanly via ensure_initialized"
        );
        assert!(
            workspace.join("README.md").is_file(),
            "re-clone must restore the repo contents"
        );
    }

    #[test]
    fn evict_workspace_reports_reclaimed_and_removes_dir() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "doomed", 4_096);
        record_last_used(&paths, "doomed");
        write_cached_size(&paths, "doomed", 4_096);
        let reclaimed = evict_workspace(&paths, "doomed").unwrap();
        assert!(reclaimed >= 4_096, "reclaimed must cover the blob: {reclaimed}");
        assert!(!paths.workspaces_dir().join("doomed").exists());
        assert!(!paths.workspace_last_used_path("doomed").exists());
        // The cached-size marker is cleaned up alongside the last-used one.
        assert!(!paths.workspace_size_path("doomed").exists());
    }

    #[test]
    fn cached_size_roundtrips_and_missing_is_none() {
        let (_td, paths) = test_daemon_paths();
        assert!(
            read_cached_size(&paths, "ws").is_none(),
            "absent cache entry reads as None (caller measures fresh)"
        );
        write_cached_size(&paths, "ws", 123_456);
        assert_eq!(read_cached_size(&paths, "ws"), Some(123_456));
    }

    /// The cap check reuses an IDLE workspace's cached size instead of
    /// recursively re-walking it on every pass. A deliberately stale-small
    /// cached size keeps an over-budget-on-disk idle workspace from being
    /// evicted — proof the pass read the cache rather than re-measuring
    /// (a fresh walk would have measured it large and evicted it).
    #[test]
    fn idle_workspace_size_is_read_from_cache_not_rewalked() {
        let (_td, paths) = test_daemon_paths();
        // A large idle workspace on disk, but cached as tiny.
        make_workspace(&paths, "idle", 8_000);
        write_cached_size(&paths, "idle", 10);
        set_last_used_ago(&paths, "idle", 500);
        // The currently-iterating workspace (always measured fresh).
        make_workspace(&paths, "current", 10);
        set_last_used_ago(&paths, "current", 50);

        // Cap 1000 bytes. A fresh walk of "idle" would see 8000, pushing
        // the total to ~8010 > 1000 and evicting it. With the cache, the
        // total is current(10, fresh) + idle(10, cached) = 20 → no evict.
        let report = enforce_cap_bytes(&paths, 1_000, "current");
        assert!(
            report.evicted.is_empty(),
            "idle workspace must be sized from the cache, not re-walked: {report:?}"
        );
        assert!(paths.workspaces_dir().join("idle").is_dir());
    }

    /// Each pass measures the current workspace fresh (refreshing its
    /// cached size) AND seeds a cached size for any uncached workspace it
    /// had to measure, so later passes skip the recursive walk.
    #[test]
    fn pass_caches_current_and_seeds_idle_sizes() {
        let (_td, paths) = test_daemon_paths();
        make_workspace(&paths, "current", 1_000);
        make_workspace(&paths, "idle", 1_000);
        // No cached sizes yet.
        assert!(read_cached_size(&paths, "current").is_none());
        assert!(read_cached_size(&paths, "idle").is_none());

        // Generous cap → no eviction, but the pass still measures + caches.
        let report = enforce_cap_bytes(&paths, 1_000_000, "current");
        assert!(report.evicted.is_empty());
        assert_eq!(
            read_cached_size(&paths, "current"),
            Some(1_000),
            "current workspace's size is measured fresh AND cached"
        );
        assert_eq!(
            read_cached_size(&paths, "idle"),
            Some(1_000),
            "an uncached idle workspace is measured once AND its size seeded"
        );
    }

    /// The current workspace is re-measured fresh every pass even when a
    /// (now-stale) cached size exists: a build that grew it since the last
    /// pass is reflected, so the cap decision is never made on a stale
    /// self-size.
    #[test]
    fn current_workspace_is_remeasured_over_stale_cache() {
        let (_td, paths) = test_daemon_paths();
        // On disk the current workspace is large; the cache claims tiny.
        make_workspace(&paths, "current", 5_000);
        write_cached_size(&paths, "current", 1);
        set_last_used_ago(&paths, "current", 10);

        // Cap 1000 bytes. The stale cache (1) would read under cap, but a
        // fresh measure (5000) is over cap. There is nothing evictable
        // (only the protected current workspace), so the pass flags
        // over-cap — which can only happen if it measured 5000 fresh.
        let report = enforce_cap_bytes(&paths, 1_000, "current");
        assert!(
            report.over_cap_after,
            "current workspace must be measured fresh (over cap), not read from stale cache"
        );
        // And its cache was refreshed to the true size.
        assert_eq!(read_cached_size(&paths, "current"), Some(5_000));
    }

    // ---- test seam -----------------------------------------------------
    //
    // `enforce_cap` takes the cap in gigabytes (the operator-facing unit);
    // the unit tests above need a byte-granular cap to avoid multi-GB
    // fixtures. `enforce_cap_bytes` is the same algorithm with the cap
    // pre-converted to bytes, exercised ONLY by tests. Production always
    // calls `enforce_cap`, which converts GB → bytes and delegates here.
    fn enforce_cap_bytes(
        paths: &DaemonPaths,
        cap_bytes: u64,
        current_basename: &str,
    ) -> EvictionReport {
        super::enforce_cap_bytes(paths, Some(cap_bytes), current_basename)
    }
}
