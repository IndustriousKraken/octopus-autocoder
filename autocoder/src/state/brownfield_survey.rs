//! Brownfield-survey state IO for the `brownfield-survey` chatops verb (a29).
//!
//! When an operator posts `@<bot> brownfield-survey <repo> [guidance]`, the
//! polling iteration's survey handler runs an executor pass that produces a
//! curated list of proposed capabilities. The handler persists the result as
//! a `BrownfieldSurveyState` JSON file under
//! `<workspace>/.state/brownfield_surveys/<request_id>.json`. The `send it`
//! verb posted as a reply inside the survey's lifecycle thread transitions
//! the state through `Pending → InProgress → Completed` while the batch
//! handler drains one item per iteration into a single-capability brownfield
//! run (per `a23`).
//!
//! State files are JSON, atomically written via tempfile-then-rename so a
//! torn write is never visible to a concurrent reader.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One survey item — the executor's per-capability proposal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurveyItem {
    /// 1-indexed sequential identifier within the survey run.
    pub id: usize,
    /// Proposed capability slug; matches `^[a-z][a-z0-9-]*$`.
    pub slug: String,
    /// One-line description of the capability.
    pub summary: String,
    /// Short paragraph naming what's IN this capability.
    pub scope_in: String,
    /// Short paragraph naming related concerns NOT in this capability.
    pub scope_out: String,
    /// Source-tree paths the capability covers (e.g. `src/scheduler/`).
    pub source_modules: Vec<String>,
    /// Estimated complexity bucket — heuristic the LLM applies.
    pub estimated_complexity: ComplexityBand,
    /// Per-item lifecycle status (see [`ItemStatus`]).
    pub status: ItemStatus,
    /// Spec PR URL populated when status reaches `Completed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// One-line reason populated when status reaches `Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

/// Complexity band the survey LLM applies to each proposed capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityBand {
    Small,
    Medium,
    Large,
}

impl ComplexityBand {
    /// Human-readable label used in chat replies AND log lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// Parse a free-text complexity value. Accepts the canonical
    /// lowercase strings only — anything else is a validation failure.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            other => Err(format!(
                "estimated_complexity `{other}` is not one of small | medium | large"
            )),
        }
    }
}

/// Top-level survey-run status. Transitions:
/// `Pending` → `InProgress` (when `send it` lands AND batch begins)
/// `InProgress` → `Completed` (when every item is terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurveyStatus {
    Pending,
    InProgress,
    Completed,
}

impl SurveyStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

/// Per-item lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    Generating,
    Completed,
    Skipped,
    Failed,
}

impl ItemStatus {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Generating => "generating",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }

    /// `true` when no further work will be done on this item.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Skipped | Self::Failed)
    }
}

/// On-disk shape for one brownfield-survey run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrownfieldSurveyState {
    pub request_id: String,
    pub repo_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    pub head_sha_at_survey: String,
    pub completed_at: DateTime<Utc>,
    pub thread_ts: String,
    pub channel: String,
    pub items: Vec<SurveyItem>,
    pub status: SurveyStatus,
}

/// Per-workspace state directory: `<workspace>/.state/brownfield_surveys/`.
pub fn state_dir(workspace: &Path) -> PathBuf {
    workspace.join(".state").join("brownfield_surveys")
}

/// Canonical state file path.
pub fn state_path(workspace: &Path, request_id: &str) -> PathBuf {
    state_dir(workspace).join(format!("{request_id}.json"))
}

/// Atomically write `state` to its canonical file under `workspace`.
pub fn write_state(workspace: &Path, state: &BrownfieldSurveyState) -> Result<()> {
    let dir = state_dir(workspace);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating brownfield_surveys dir {}", dir.display()))?;
    let path = state_path(workspace, &state.request_id);
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, state)
        .with_context(|| format!("serializing brownfield-survey state for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the survey state for `request_id`. Returns `Ok(None)` when no
/// file exists; surfaces an error on read/parse.
pub fn read_state(workspace: &Path, request_id: &str) -> Result<Option<BrownfieldSurveyState>> {
    let path = state_path(workspace, request_id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    serde_json::from_str::<BrownfieldSurveyState>(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// List every `(request_id, mtime)` pair under the survey-state
/// directory, sorted by mtime descending (newest first).
pub fn list_surveys_by_mtime(workspace: &Path) -> Result<Vec<(String, std::time::SystemTime)>> {
    let dir = state_dir(workspace);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(String, std::time::SystemTime)> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading brownfield_surveys dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => std::time::SystemTime::UNIX_EPOCH,
        };
        entries.push((stem, mtime));
    }
    entries.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    Ok(entries)
}

/// Delete every survey-state file under `workspace`. Returns the count
/// of files deleted. Missing directory is a no-op (returns 0).
pub fn clear_all(workspace: &Path) -> Result<usize> {
    let dir = state_dir(workspace);
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0usize;
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading brownfield_surveys dir {}", dir.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => count += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow!("removing {}: {e}", path.display())),
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_item(id: usize, slug: &str) -> SurveyItem {
        SurveyItem {
            id,
            slug: slug.into(),
            summary: format!("Capability {slug} summary."),
            scope_in: "What's in: lots of things.".into(),
            scope_out: "What's out: other things.".into(),
            source_modules: vec![format!("src/{slug}/")],
            estimated_complexity: ComplexityBand::Medium,
            status: ItemStatus::Pending,
            pr_url: None,
            failure_reason: None,
        }
    }

    fn fixture_state(request_id: &str) -> BrownfieldSurveyState {
        BrownfieldSurveyState {
            request_id: request_id.into(),
            repo_url: "git@github.com:acme/myrepo.git".into(),
            guidance: Some("focus on the data layer".into()),
            head_sha_at_survey: "abc1234".into(),
            completed_at: Utc::now(),
            thread_ts: "1748399999.001234".into(),
            channel: "C_OPS".into(),
            items: vec![fixture_item(1, "scheduler"), fixture_item(2, "auth")],
            status: SurveyStatus::Pending,
        }
    }

    #[test]
    fn read_missing_state_file_returns_ok_none() {
        let tmp = TempDir::new().unwrap();
        let got = read_state(tmp.path(), "no-such").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let tmp = TempDir::new().unwrap();
        let state = fixture_state("req-1");
        write_state(tmp.path(), &state).unwrap();
        let got = read_state(tmp.path(), &state.request_id).unwrap().unwrap();
        assert_eq!(got, state);
    }

    #[test]
    fn state_path_is_under_workspace_subdir() {
        let p = state_path(Path::new("/tmp/ws"), "req-x");
        let s = p.to_string_lossy();
        assert!(s.starts_with("/tmp/ws/"), "{s}");
        assert!(s.contains(".state/brownfield_surveys"), "{s}");
        assert!(s.ends_with("req-x.json"), "{s}");
    }

    #[test]
    fn concurrent_writes_do_not_leak_tempfiles() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            let s = fixture_state(&format!("req-{i}"));
            write_state(tmp.path(), &s).unwrap();
        }
        let dir = state_dir(tmp.path());
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 5);
        assert!(!names.iter().any(|n| n.contains(".tmp")), "{names:?}");
    }

    #[test]
    fn clear_all_removes_every_survey() {
        let tmp = TempDir::new().unwrap();
        for i in 0..3 {
            write_state(tmp.path(), &fixture_state(&format!("req-{i}"))).unwrap();
        }
        let n = clear_all(tmp.path()).unwrap();
        assert_eq!(n, 3);
        let remaining = list_surveys_by_mtime(tmp.path()).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn clear_all_no_surveys_returns_zero_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(clear_all(tmp.path()).unwrap(), 0);
        assert_eq!(clear_all(tmp.path()).unwrap(), 0);
    }

    #[test]
    fn list_surveys_by_mtime_orders_newest_first() {
        let tmp = TempDir::new().unwrap();
        write_state(tmp.path(), &fixture_state("a")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_state(tmp.path(), &fixture_state("b")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_state(tmp.path(), &fixture_state("c")).unwrap();
        let surveys = list_surveys_by_mtime(tmp.path()).unwrap();
        let ids: Vec<&str> = surveys.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn complexity_band_round_trips_via_parse_and_label() {
        for band in [
            ComplexityBand::Small,
            ComplexityBand::Medium,
            ComplexityBand::Large,
        ] {
            let label = band.label();
            assert_eq!(ComplexityBand::parse(label).unwrap(), band);
        }
        assert!(ComplexityBand::parse("xxl").is_err());
    }

    #[test]
    fn item_status_terminal_is_complete_skipped_failed() {
        assert!(!ItemStatus::Pending.is_terminal());
        assert!(!ItemStatus::Generating.is_terminal());
        assert!(ItemStatus::Completed.is_terminal());
        assert!(ItemStatus::Skipped.is_terminal());
        assert!(ItemStatus::Failed.is_terminal());
    }

    #[test]
    fn enqueue_dequeue_via_vecdeque_preserves_order() {
        use std::collections::VecDeque;
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("req-a".into());
        q.push_back("req-b".into());
        q.push_back("req-c".into());
        assert_eq!(q.pop_front().as_deref(), Some("req-a"));
        assert_eq!(q.pop_front().as_deref(), Some("req-b"));
        assert_eq!(q.pop_front().as_deref(), Some("req-c"));
        assert!(q.pop_front().is_none());
    }

    #[test]
    fn status_transition_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let initial = fixture_state("req-trans");
        write_state(tmp.path(), &initial).unwrap();
        let mut updated = initial.clone();
        updated.status = SurveyStatus::InProgress;
        write_state(tmp.path(), &updated).unwrap();
        let got = read_state(tmp.path(), &initial.request_id).unwrap().unwrap();
        assert_eq!(got.status, SurveyStatus::InProgress);
        assert_eq!(got.items, initial.items);
        assert_eq!(got.thread_ts, initial.thread_ts);
    }
}
