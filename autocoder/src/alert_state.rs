//! Per-workspace persistence for predictable-failure alert throttling.
//!
//! Layout: `<state_dir>/alert-state/<workspace-basename>.json`
//! (resolved via `DaemonPaths::alert_state_path()`). The file lives
//! OUTSIDE the managed repository's workspace — daemon bookkeeping
//! never appears in `git status` or any `git checkout` clobber-protection
//! check. The first-startup migration in `state/alert_state_migration.rs`
//! moves any legacy `<workspace>/.alert-state.json` files to the new
//! location.
//!
//! The `DaemonPaths` reference is threaded explicitly into every public
//! function (function-parameter pattern per the canonical
//! `Production paths SHALL be threaded` requirement).

use crate::paths::DaemonPaths;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// File name of the legacy workspace-local alert-state file. Kept as a
/// constant because the migration code, the workspace-init invariant
/// check, and the test-mode fallback all need to refer to it.
pub(crate) const LEGACY_ALERT_STATE_FILE: &str = ".alert-state.json";

/// Categories of predictable infrastructure failure that autocoder alerts on.
/// Other failure surfaces (executor-`Failed`, reviewer-failed, chatops-post-
/// failed) are explicitly out of scope and never produce an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertCategory {
    WorkspaceInitFailure,
    WorkspaceDirtyMidIteration,
    BranchPushFailure,
    PrCreationFailure,
    /// A periodic audit attempted a disallowed write per its declared
    /// `WritePolicy`. The scheduler reverts the workspace, leaves the
    /// audit's state entry untouched so the cadence will retrigger on
    /// the next iteration, and emits a throttled alert under this
    /// category so the operator can investigate.
    AuditWritePolicyViolation,
    /// The executor returned `SpecNeedsRevision` for a change: one or
    /// more tasks in tasks.md require capabilities outside the agent's
    /// sandbox. autocoder writes a `.needs-spec-revision.json` marker
    /// and emits a throttled alert under this category so the operator
    /// can revise tasks.md.
    SpecNeedsRevision,
    /// A pending change's dated archive directory already exists on
    /// disk, so `queue::archive` would fail at the end of the iteration.
    /// autocoder pre-flights the collision before invoking the executor
    /// and emits a throttled alert under this category so the operator
    /// can resolve the structural condition (typically: remove the
    /// active dir or revert the prior merge).
    ArchiveCollision,
}

impl AlertCategory {
    /// Short human-readable label used inside the alert text (e.g.
    /// "workspace init keeps failing").
    pub fn label(&self) -> &'static str {
        match self {
            Self::WorkspaceInitFailure => "workspace init keeps failing",
            Self::WorkspaceDirtyMidIteration => "workspace dirty mid-iteration",
            Self::BranchPushFailure => "branch push keeps failing",
            Self::PrCreationFailure => "PR creation keeps failing",
            Self::AuditWritePolicyViolation => "audit attempted disallowed write",
            Self::SpecNeedsRevision => "spec needs revision",
            Self::ArchiveCollision => "archive collision",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEntry {
    pub last_alerted_at: DateTime<Utc>,
    pub last_error_excerpt: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertState {
    #[serde(default)]
    pub alerts: HashMap<AlertCategory, AlertEntry>,
    /// Per-change perma-stuck alert throttle. Keyed by change name. The
    /// 24h throttle ensures that a repeat fix-test-fail cycle on a single
    /// change doesn't spam the alert channel.
    #[serde(default)]
    pub perma_stuck_alerts: HashMap<String, AlertEntry>,
    /// Per-change spec-needs-revision alert throttle. Keyed by change
    /// name. Same 24h throttle as `perma_stuck_alerts`. The marker file
    /// itself excludes the change from `list_pending`, so under normal
    /// operation this alert fires at most once per (change, marker
    /// lifecycle).
    #[serde(default)]
    pub spec_revision_alerts: HashMap<String, AlertEntry>,
    /// Per-comment revise-lifecycle notification deduplication map.
    /// Keyed by GitHub `comment_id` (the operator's `@<bot> revise
    /// <text>` PR comment). Each entry tracks whether the three
    /// lifecycle notifications (picked up, succeeded, failed) have
    /// already been posted for that comment. A second pass on the same
    /// comment (e.g. autocoder restarts mid-revision) consults this map
    /// to avoid double-posting.
    #[serde(default)]
    pub revise_notifications: HashMap<String, ReviseNotificationEntry>,
    /// Per-comment code-review-lifecycle notification deduplication map
    /// (a33). Keyed by GitHub `comment_id` (the operator's
    /// `@<bot> code-review` PR comment). Each entry tracks whether the
    /// three lifecycle notifications (triggered, complete, failed) have
    /// already been posted for that comment.
    #[serde(default)]
    pub code_review_notifications: HashMap<String, CodeReviewNotificationEntry>,
    /// Per-PR re-review suggestion deduplication (a33). Keyed by the
    /// per-PR identifier `<owner>/<repo>#<pr_number>` (or a fallback
    /// such as the PR URL). Records the `revisions_applied` count at
    /// which the most recent suggestion fired, mirroring the per-PR
    /// state file's `last_suggested_rereview_at_revisions_count` field.
    /// The dedup field lives ALSO in the PR state file; this map is a
    /// best-effort secondary store that survives state-file pruning.
    #[serde(default)]
    pub rereview_suggestion_dedup: HashMap<String, u32>,
}

/// Per-comment record of which revise-lifecycle notifications have
/// already been posted. Lives inside [`AlertState::revise_notifications`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviseNotificationEntry {
    #[serde(default)]
    pub posted_picked_up_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub posted_succeeded_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub posted_failed_at: Option<DateTime<Utc>>,
}

/// Discriminant for the three points in the revise lifecycle at which
/// the daemon posts a chatops notification. Passed to
/// [`AlertState::record_revise_notification`] /
/// [`AlertState::revise_notification_already_posted`] to select the
/// matching field on [`ReviseNotificationEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviseNotificationKind {
    PickedUp,
    Succeeded,
    Failed,
}

/// Per-comment record of which code-review-lifecycle notifications have
/// already been posted (a33). Lives inside
/// [`AlertState::code_review_notifications`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeReviewNotificationEntry {
    #[serde(default)]
    pub posted_triggered_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub posted_complete_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub posted_failed_at: Option<DateTime<Utc>>,
}

/// Discriminant for the three points in the code-review lifecycle at
/// which the daemon posts a chatops notification (a33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeReviewNotificationKind {
    Triggered,
    Complete,
    Failed,
}

/// Resolve the on-disk path of `<workspace>`'s alert-state file under
/// the threaded `DaemonPaths` instance.
fn alert_state_path(paths: &DaemonPaths, workspace: &Path) -> PathBuf {
    let basename = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());
    paths.alert_state_path(&basename)
}

impl AlertState {
    /// Load the per-workspace alert state. A missing file is not an error —
    /// it parses to an empty state (no prior alerts).
    pub fn load_or_default(paths: &DaemonPaths, workspace: &Path) -> Self {
        let path = alert_state_path(paths, workspace);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!(
                    "alert-state file at {} is corrupt; starting empty: {e:#}",
                    path.display()
                );
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(
                    "alert-state file at {} unreadable; starting empty: {e:#}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Atomically persist this state at the resolved alert-state path
    /// via tempfile-then-rename in the same directory so a torn write
    /// can never be observed by a concurrent reader.
    pub fn save(&self, paths: &DaemonPaths, workspace: &Path) -> Result<()> {
        let path = alert_state_path(paths, workspace);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
        let tmp = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating tempfile in {}", parent.display()))?;
        serde_json::to_writer_pretty(&tmp, self)
            .with_context(|| format!("serializing alert state for {}", path.display()))?;
        tmp.persist(&path)
            .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
        Ok(())
    }

    /// Insert-or-update the timestamp field on the per-comment
    /// `revise_notifications` entry that matches `kind`. Used by the
    /// three revise-lifecycle notification helpers AFTER a successful
    /// post, before saving the alert-state file.
    pub fn record_revise_notification(
        &mut self,
        comment_id: &str,
        kind: ReviseNotificationKind,
        when: DateTime<Utc>,
    ) {
        let entry = self
            .revise_notifications
            .entry(comment_id.to_string())
            .or_default();
        match kind {
            ReviseNotificationKind::PickedUp => entry.posted_picked_up_at = Some(when),
            ReviseNotificationKind::Succeeded => entry.posted_succeeded_at = Some(when),
            ReviseNotificationKind::Failed => entry.posted_failed_at = Some(when),
        }
    }

    /// `true` when the per-comment entry for `comment_id` already
    /// records a timestamp for `kind` (the corresponding `Option` is
    /// `Some(_)`). Returns `false` for a missing entry OR a present
    /// entry whose matching field is `None`.
    pub fn revise_notification_already_posted(
        &self,
        comment_id: &str,
        kind: ReviseNotificationKind,
    ) -> bool {
        let Some(entry) = self.revise_notifications.get(comment_id) else {
            return false;
        };
        match kind {
            ReviseNotificationKind::PickedUp => entry.posted_picked_up_at.is_some(),
            ReviseNotificationKind::Succeeded => entry.posted_succeeded_at.is_some(),
            ReviseNotificationKind::Failed => entry.posted_failed_at.is_some(),
        }
    }

    /// Insert-or-update the timestamp field on the per-comment
    /// `code_review_notifications` entry that matches `kind` (a33).
    pub fn record_code_review_notification(
        &mut self,
        comment_id: &str,
        kind: CodeReviewNotificationKind,
        when: DateTime<Utc>,
    ) {
        let entry = self
            .code_review_notifications
            .entry(comment_id.to_string())
            .or_default();
        match kind {
            CodeReviewNotificationKind::Triggered => entry.posted_triggered_at = Some(when),
            CodeReviewNotificationKind::Complete => entry.posted_complete_at = Some(when),
            CodeReviewNotificationKind::Failed => entry.posted_failed_at = Some(when),
        }
    }

    /// `true` when the per-comment entry for `comment_id` already
    /// records a timestamp for `kind` (a33).
    pub fn code_review_notification_already_posted(
        &self,
        comment_id: &str,
        kind: CodeReviewNotificationKind,
    ) -> bool {
        let Some(entry) = self.code_review_notifications.get(comment_id) else {
            return false;
        };
        match kind {
            CodeReviewNotificationKind::Triggered => entry.posted_triggered_at.is_some(),
            CodeReviewNotificationKind::Complete => entry.posted_complete_at.is_some(),
            CodeReviewNotificationKind::Failed => entry.posted_failed_at.is_some(),
        }
    }

    /// Idempotent removal of the alert-state file. A missing file is a
    /// success, not an error.
    pub fn clear(paths: &DaemonPaths, workspace: &Path) -> Result<()> {
        let path = alert_state_path(paths, workspace);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;

    fn workspace_under(paths: &DaemonPaths) -> PathBuf {
        paths.cache.join("workspaces").join("ws")
    }

    #[test]
    fn load_missing_returns_empty() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let state = AlertState::load_or_default(&paths, &ws);
        assert!(state.alerts.is_empty());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let mut state = AlertState::default();
        let now = Utc::now();
        state.alerts.insert(
            AlertCategory::BranchPushFailure,
            AlertEntry {
                last_alerted_at: now,
                last_error_excerpt: "refusing to update protected branch".into(),
            },
        );
        state.save(&paths, &ws).unwrap();

        let reloaded = AlertState::load_or_default(&paths, &ws);
        let entry = reloaded
            .alerts
            .get(&AlertCategory::BranchPushFailure)
            .expect("entry roundtrips");
        // Timestamps may differ in trailing-precision encoding; compare via
        // round-trip serialization rather than direct equality.
        assert_eq!(entry.last_error_excerpt, "refusing to update protected branch");
        let diff = (entry.last_alerted_at - now).num_milliseconds().abs();
        assert!(diff < 5, "timestamps must roundtrip within 5ms; diff = {diff}");
    }

    #[test]
    fn clear_is_idempotent() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let mut state = AlertState::default();
        state.alerts.insert(
            AlertCategory::PrCreationFailure,
            AlertEntry {
                last_alerted_at: Utc::now(),
                last_error_excerpt: "403 Forbidden".into(),
            },
        );
        state.save(&paths, &ws).unwrap();
        assert!(alert_state_path(&paths, &ws).exists());
        AlertState::clear(&paths, &ws).expect("first clear ok");
        assert!(!alert_state_path(&paths, &ws).exists());
        // Second clear must also succeed.
        AlertState::clear(&paths, &ws).expect("second clear ok");
    }

    #[test]
    fn clear_does_not_error_on_missing() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        // File never created.
        AlertState::clear(&paths, &ws).expect("clear without prior save must succeed");
    }

    #[test]
    fn archive_collision_variant_roundtrips_through_save_and_load() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let mut state = AlertState::default();
        let now = Utc::now();
        state.alerts.insert(
            AlertCategory::ArchiveCollision,
            AlertEntry {
                last_alerted_at: now,
                last_error_excerpt: "archive destination already exists".into(),
            },
        );
        state.save(&paths, &ws).unwrap();

        let reloaded = AlertState::load_or_default(&paths, &ws);
        let entry = reloaded
            .alerts
            .get(&AlertCategory::ArchiveCollision)
            .expect("ArchiveCollision entry must round-trip");
        assert_eq!(entry.last_error_excerpt, "archive destination already exists");
        let diff = (entry.last_alerted_at - now).num_milliseconds().abs();
        assert!(diff < 5, "timestamps must roundtrip within 5ms; diff = {diff}");

        // Pin the on-disk JSON key.
        let raw = std::fs::read_to_string(alert_state_path(&paths, &ws)).unwrap();
        assert!(
            raw.contains("archive_collision"),
            "archive collision must serialize as snake_case `archive_collision`; got: {raw}"
        );
        assert_eq!(AlertCategory::ArchiveCollision.label(), "archive collision");
    }

    #[test]
    fn revise_notifications_round_trip_through_save_and_load() {
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let mut state = AlertState::default();
        let now = Utc::now();
        state.record_revise_notification(
            "comment-42",
            ReviseNotificationKind::PickedUp,
            now,
        );
        state.record_revise_notification(
            "comment-42",
            ReviseNotificationKind::Succeeded,
            now + chrono::Duration::minutes(3),
        );
        state.record_revise_notification(
            "comment-43",
            ReviseNotificationKind::Failed,
            now,
        );
        state.save(&paths, &ws).unwrap();

        let reloaded = AlertState::load_or_default(&paths, &ws);
        let e42 = reloaded
            .revise_notifications
            .get("comment-42")
            .expect("comment-42 entry must round-trip");
        assert!(e42.posted_picked_up_at.is_some());
        assert!(e42.posted_succeeded_at.is_some());
        assert!(e42.posted_failed_at.is_none());
        let e43 = reloaded
            .revise_notifications
            .get("comment-43")
            .expect("comment-43 entry must round-trip");
        assert!(e43.posted_picked_up_at.is_none());
        assert!(e43.posted_succeeded_at.is_none());
        assert!(e43.posted_failed_at.is_some());

        // Accessors observe the same fields after round-trip.
        assert!(reloaded.revise_notification_already_posted(
            "comment-42",
            ReviseNotificationKind::PickedUp,
        ));
        assert!(reloaded.revise_notification_already_posted(
            "comment-42",
            ReviseNotificationKind::Succeeded,
        ));
        assert!(!reloaded.revise_notification_already_posted(
            "comment-42",
            ReviseNotificationKind::Failed,
        ));
        assert!(reloaded.revise_notification_already_posted(
            "comment-43",
            ReviseNotificationKind::Failed,
        ));
        assert!(!reloaded.revise_notification_already_posted(
            "comment-missing",
            ReviseNotificationKind::PickedUp,
        ));
    }

    #[test]
    fn revise_notifications_field_defaults_to_empty_when_absent_in_json() {
        // Simulate an alert-state file written by an older daemon that
        // doesn't know about `revise_notifications`. Loading must succeed
        // with the field defaulting to an empty map.
        let (_temp, paths) = test_daemon_paths();
        let ws = workspace_under(&paths);
        let legacy_json = serde_json::json!({
            "alerts": {},
            "perma_stuck_alerts": {},
            "spec_revision_alerts": {}
        });
        let path = alert_state_path(&paths, &ws);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&legacy_json).unwrap())
            .unwrap();

        let state = AlertState::load_or_default(&paths, &ws);
        assert!(
            state.revise_notifications.is_empty(),
            "missing revise_notifications field must default to an empty map"
        );
        assert!(!state.revise_notification_already_posted(
            "any-id",
            ReviseNotificationKind::PickedUp,
        ));
    }

    /// Canonical "Concurrent tests do not collide on disk" scenario:
    /// two threads each construct DIFFERENT `DaemonPaths` via
    /// `test_daemon_paths()` AND invoke `AlertState::load_or_default`
    /// + `save`. Each thread's writes land under its own tempdir;
    /// neither thread observes the other's state.
    #[test]
    fn concurrent_threads_with_different_paths_do_not_collide() {
        use std::thread;

        let h1 = thread::spawn(|| {
            let (temp, paths) = test_daemon_paths();
            let ws = paths.cache.join("workspaces").join("ws");
            let mut state = AlertState::default();
            state.alerts.insert(
                AlertCategory::BranchPushFailure,
                AlertEntry {
                    last_alerted_at: Utc::now(),
                    last_error_excerpt: "thread-1".into(),
                },
            );
            state.save(&paths, &ws).unwrap();
            let reloaded = AlertState::load_or_default(&paths, &ws);
            (
                temp.path().to_path_buf(),
                alert_state_path(&paths, &ws),
                reloaded
                    .alerts
                    .get(&AlertCategory::BranchPushFailure)
                    .map(|e| e.last_error_excerpt.clone()),
            )
        });

        let h2 = thread::spawn(|| {
            let (temp, paths) = test_daemon_paths();
            let ws = paths.cache.join("workspaces").join("ws");
            let mut state = AlertState::default();
            state.alerts.insert(
                AlertCategory::BranchPushFailure,
                AlertEntry {
                    last_alerted_at: Utc::now(),
                    last_error_excerpt: "thread-2".into(),
                },
            );
            state.save(&paths, &ws).unwrap();
            let reloaded = AlertState::load_or_default(&paths, &ws);
            (
                temp.path().to_path_buf(),
                alert_state_path(&paths, &ws),
                reloaded
                    .alerts
                    .get(&AlertCategory::BranchPushFailure)
                    .map(|e| e.last_error_excerpt.clone()),
            )
        });

        let (root1, file1, excerpt1) = h1.join().unwrap();
        let (root2, file2, excerpt2) = h2.join().unwrap();

        assert_ne!(root1, root2, "two threads must use distinct tempdir roots");
        assert_ne!(file1, file2, "alert-state files must live under disjoint tempdirs");
        assert_eq!(excerpt1.as_deref(), Some("thread-1"));
        assert_eq!(excerpt2.as_deref(), Some("thread-2"));
        assert!(file1.starts_with(&root1), "file1 must live under root1");
        assert!(file2.starts_with(&root2), "file2 must live under root2");
    }

    #[test]
    fn record_revise_notification_updates_existing_entry_in_place() {
        let mut state = AlertState::default();
        let t1 = Utc::now();
        let t2 = t1 + chrono::Duration::seconds(30);
        state.record_revise_notification("c1", ReviseNotificationKind::PickedUp, t1);
        state.record_revise_notification("c1", ReviseNotificationKind::Failed, t2);
        let entry = state.revise_notifications.get("c1").unwrap();
        assert_eq!(entry.posted_picked_up_at, Some(t1));
        assert_eq!(entry.posted_failed_at, Some(t2));
        assert_eq!(entry.posted_succeeded_at, None);
        assert_eq!(state.revise_notifications.len(), 1);
    }

    #[test]
    fn json_keys_use_snake_case_for_categories() {
        // The on-disk schema labels the categories in snake_case;
        // guard against accidental rename downstream.
        let mut state = AlertState::default();
        state.alerts.insert(
            AlertCategory::WorkspaceInitFailure,
            AlertEntry {
                last_alerted_at: Utc::now(),
                last_error_excerpt: "x".into(),
            },
        );
        let s = serde_json::to_string(&state).unwrap();
        assert!(
            s.contains("workspace_init_failure"),
            "json must use snake_case category key; got: {s}"
        );
    }

    /// Task 4.4: spawn two `std::thread::spawn` threads that each
    /// construct DIFFERENT `DaemonPaths` via `test_daemon_paths()` AND
    /// invoke `AlertState::load_or_default` + `save` against their own
    /// tempdir. Assert: the two threads' writes land in DIFFERENT
    /// tempdirs (no cross-contamination). Pins the canonical
    /// "Concurrent tests do not collide on disk" scenario.
    #[test]
    fn concurrent_threads_do_not_collide_on_disk() {
        use std::sync::Arc;
        use std::sync::Barrier;

        let barrier = Arc::new(Barrier::new(2));
        let b1 = barrier.clone();
        let b2 = barrier.clone();

        let t1 = std::thread::spawn(move || {
            let (temp, paths) = test_daemon_paths();
            let ws = paths.cache.join("workspaces").join("ws-thread-1");
            let mut state = AlertState::default();
            state.alerts.insert(
                AlertCategory::BranchPushFailure,
                AlertEntry {
                    last_alerted_at: Utc::now(),
                    last_error_excerpt: "thread-1 marker".into(),
                },
            );
            // Sync write to maximize the chance of a collision if paths
            // were shared.
            b1.wait();
            state.save(&paths, &ws).expect("thread-1 save");
            let written = alert_state_path(&paths, &ws);
            let body = std::fs::read_to_string(&written).expect("thread-1 read");
            assert!(body.contains("thread-1 marker"));
            // Keep tempdir alive until we've returned its root for the
            // collision assertion below.
            (temp, written, body)
        });

        let t2 = std::thread::spawn(move || {
            let (temp, paths) = test_daemon_paths();
            let ws = paths.cache.join("workspaces").join("ws-thread-2");
            let mut state = AlertState::default();
            state.alerts.insert(
                AlertCategory::BranchPushFailure,
                AlertEntry {
                    last_alerted_at: Utc::now(),
                    last_error_excerpt: "thread-2 marker".into(),
                },
            );
            b2.wait();
            state.save(&paths, &ws).expect("thread-2 save");
            let written = alert_state_path(&paths, &ws);
            let body = std::fs::read_to_string(&written).expect("thread-2 read");
            assert!(body.contains("thread-2 marker"));
            (temp, written, body)
        });

        let (temp_1, path_1, body_1) = t1.join().expect("thread-1 panicked");
        let (temp_2, path_2, body_2) = t2.join().expect("thread-2 panicked");

        // The two threads' state-file paths MUST live under DIFFERENT
        // tempdir roots. No prefix overlap.
        assert_ne!(
            temp_1.path(),
            temp_2.path(),
            "the two test_daemon_paths() calls must return distinct tempdir roots"
        );
        assert!(
            path_1.starts_with(temp_1.path()),
            "thread-1's write must live under its own tempdir root"
        );
        assert!(
            path_2.starts_with(temp_2.path()),
            "thread-2's write must live under its own tempdir root"
        );
        assert!(
            !path_1.starts_with(temp_2.path()),
            "thread-1's write must NOT live under thread-2's tempdir"
        );
        assert!(
            !path_2.starts_with(temp_1.path()),
            "thread-2's write must NOT live under thread-1's tempdir"
        );
        // Each thread sees its OWN marker text — neither read the
        // other's write.
        assert!(body_1.contains("thread-1 marker"));
        assert!(!body_1.contains("thread-2 marker"));
        assert!(body_2.contains("thread-2 marker"));
        assert!(!body_2.contains("thread-1 marker"));
    }

}
