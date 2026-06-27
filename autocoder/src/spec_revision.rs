//! Per-change spec-needs-revision marker. When the executor returns
//! `ExecutorOutcome::SpecNeedsRevision` for a change, autocoder writes
//! `<workspace>/openspec/changes/<change>/.needs-spec-revision.json`. The
//! marker's presence is a presence-only flag consulted by
//! `queue::list_pending` — the change is excluded from the queue until the
//! operator removes the marker manually (typically after editing tasks.md
//! to remove or revise the flagged tasks).

use crate::executor::UnimplementableTask;
use crate::preflight::canon_contradiction::CanonContradictionFinding;
use crate::preflight::change_contradiction::ContradictionFinding;
use crate::preflight::spec_archivability::UnarchivableDelta;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MARKER_FILE: &str = ".needs-spec-revision.json";
const OPERATOR_ACTION: &str = "Edit openspec/changes/<change>/tasks.md to remove or revise the flagged tasks, commit + push, then delete this marker file.";
const OPERATOR_ACTION_UNARCHIVABLE: &str = "Edit openspec/changes/<change>/specs/<capability>/spec.md so each delta block's `### Requirement:` header matches the canonical openspec/specs/<capability>/spec.md. Commit + push, then `@<bot> clear-revision <repo> <change>` from chat (or delete this marker file directly).";
const OPERATOR_ACTION_GATE_ERROR: &str = "A verifier gate could NOT run — the change is held because it was NOT evaluated, NOT because a problem was found. Fix the gate (e.g. install/authenticate the configured CLI, check the daemon control socket), then `@<bot> clear-revision <repo> <change>` to retry. Clearing without fixing the gate will re-hold on the next attempt.";

/// A verifier gate that could not be evaluated (a fail-CLOSED hold). Distinct
/// from a finding: the gate did not determine the change is wrong, it could not
/// run at all, so the change is held rather than waved through (gatekeepers fail
/// closed). `gate` is the gate label (`[in]` / `[canon]`); `cause` is the
/// human-readable reason (CLI unavailable, session error, no submission, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateErrorRecord {
    pub gate: String,
    pub cause: String,
}

/// Outcome details captured at the moment the marker is written. Exactly one
/// population is non-empty per write (the schema permits several):
/// `unimplementable_tasks` (executor `SpecNeedsRevision`), `unarchivable_deltas`
/// (pre-executor archivability check), `canon_editing_tasks` (a task directing a
/// canon edit), OR `gate_error` (a verifier gate that could not run — a
/// fail-closed hold). `revision_suggestion` always carries the human-readable
/// narrative.
#[derive(Debug, Clone, Default)]
pub struct SpecNeedsRevisionDetail {
    pub unimplementable_tasks: Vec<UnimplementableTask>,
    pub unarchivable_deltas: Vec<UnarchivableDelta>,
    /// The text of each `tasks.md` task that directs a direct edit to the
    /// canonical specs (the pre-executor canon-editing-tasks check). Populated
    /// when that pre-flight flags the change; empty otherwise.
    pub canon_editing_tasks: Vec<String>,
    pub revision_suggestion: String,
    pub gate_error: Option<GateErrorRecord>,
    /// The CURRENT contradiction set in STRUCTURED form (additive to the
    /// prose `revision_suggestion`). Populated when a contradiction gate
    /// flags the change, AND refreshed by [`refresh_marker_contradictions`]
    /// on a `send it` re-gate that still contradicts, so the marker is the
    /// durable record of what currently contradicts.
    pub contradictions: Vec<ContradictionFindingRecord>,
}

/// JSON-friendly mirror of [`UnarchivableDelta`]. The on-disk JSON
/// stores `kind` as a stable string ("Added" / "Modified" / "Removed" /
/// "Renamed") so operators reading the marker by eye don't need to
/// memorise an enum-tag convention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnarchivableDeltaRecord {
    pub capability: String,
    pub kind: String,
    pub header: String,
    pub reason: String,
}

impl From<&UnarchivableDelta> for UnarchivableDeltaRecord {
    fn from(d: &UnarchivableDelta) -> Self {
        Self {
            capability: d.capability.clone(),
            kind: d.kind.as_str().to_string(),
            header: d.header.clone(),
            reason: d.reason.clone(),
        }
    }
}

/// Which gate surfaced a recorded contradiction. Serialized as a stable string
/// (`"in"` / `"canon"`) so an operator reading the marker by eye, AND the
/// executor enumerating findings, can tell the within-change conflict apart
/// from the change-vs-canon one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionGate {
    /// The `[in]` gate (a change that contradicts itself).
    In,
    /// The `[canon]` gate (a change that contradicts an already-canonical
    /// requirement).
    Canon,
}

/// One contradiction recorded in the marker, in STRUCTURED form (the durable
/// source of truth for what currently contradicts). Carries the conflicting
/// requirement IDENTITY so the executor can enumerate each distinct conflict
/// AND so escalation can track the same finding across `send it`s without
/// string-matching message text. The shape unifies the `[in]` gate's
/// within-change pair AND the `[canon]` gate's change-vs-canon pair; the
/// gate-specific fields default to empty so either kind round-trips.
///
/// `gate == In`: `requirement_a` / `requirement_b` are the two conflicting
/// change requirements; `canonical_capability` is empty.
/// `gate == Canon`: `requirement_a` is the change requirement, `requirement_b`
/// is the canonical requirement, `canonical_capability` names its capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContradictionFindingRecord {
    pub gate: ContradictionGate,
    pub requirement_a: String,
    pub requirement_b: String,
    /// The canonical requirement's capability (`[canon]` findings only; empty
    /// for `[in]`).
    #[serde(default)]
    pub canonical_capability: String,
    /// One-line explanation of WHY the two requirements conflict.
    #[serde(default)]
    pub summary: String,
    /// Concrete edit plan from the gate (`suggested_fix`), additive — empty
    /// when the gate (or an older daemon) omitted it.
    #[serde(default)]
    pub suggested_fix: String,
}

/// Stable identity of a recorded contradiction (the gate plus the conflicting-
/// requirement pair, and capability for `[canon]`). The escalation in the
/// spec-revision executor tracks this across attempts to detect a finding that
/// survives the bounded converge loop — keyed on structure, NOT on message text.
pub type ContradictionIdentity = (ContradictionGate, String, String, String);

impl ContradictionFindingRecord {
    /// Stable identity for escalation: the gate plus the conflicting-requirement
    /// pair (and capability for `[canon]`), independent of the why-`summary` or
    /// the `suggested_fix`. Two re-gate findings naming the same requirements
    /// are "the same" even if the model phrased the summary differently.
    pub fn identity(&self) -> ContradictionIdentity {
        (
            self.gate,
            self.requirement_a.clone(),
            self.requirement_b.clone(),
            self.canonical_capability.clone(),
        )
    }
}

impl From<&ContradictionFinding> for ContradictionFindingRecord {
    fn from(f: &ContradictionFinding) -> Self {
        Self {
            gate: ContradictionGate::In,
            requirement_a: f.requirement_a.clone(),
            requirement_b: f.requirement_b.clone(),
            canonical_capability: String::new(),
            summary: f.summary.clone(),
            suggested_fix: f.suggested_fix.clone(),
        }
    }
}

impl From<&CanonContradictionFinding> for ContradictionFindingRecord {
    fn from(f: &CanonContradictionFinding) -> Self {
        Self {
            gate: ContradictionGate::Canon,
            requirement_a: f.change_requirement.clone(),
            requirement_b: f.canonical_requirement.clone(),
            canonical_capability: f.canonical_capability.clone(),
            summary: f.summary.clone(),
            suggested_fix: f.suggested_fix.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecNeedsRevisionMarker {
    pub change: String,
    pub marked_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unimplementable_tasks: Vec<UnimplementableTask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unarchivable_deltas: Vec<UnarchivableDeltaRecord>,
    /// The text of each `tasks.md` task that directs a canon edit (the
    /// canon-editing-tasks pre-flight). Omitted from JSON when empty so a marker
    /// written without it (an older daemon, or another hold reason) still parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canon_editing_tasks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_error: Option<GateErrorRecord>,
    /// The current contradiction set in STRUCTURED form (additive to
    /// `revision_suggestion`). Omitted from JSON when empty so a marker
    /// written without it (an older daemon, or a non-contradiction hold)
    /// still parses (`#[serde(default)]` — back-compat).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradictions: Vec<ContradictionFindingRecord>,
    pub revision_suggestion: String,
    pub operator_action: String,
}

fn marker_path(workspace: &Path, change: &str) -> PathBuf {
    workspace
        .join("openspec/changes")
        .join(change)
        .join(MARKER_FILE)
}

/// True when `<workspace>/openspec/changes/<change>/.needs-spec-revision.json`
/// exists. Pure filesystem check — no JSON parsing.
pub fn marker_exists(workspace: &Path, change: &str) -> bool {
    marker_path(workspace, change).exists()
}

/// Write the marker file atomically (tempfile + rename in the change
/// directory). The change directory must already exist.
pub fn write_marker(
    workspace: &Path,
    change: &str,
    detail: &SpecNeedsRevisionDetail,
) -> Result<()> {
    let path = marker_path(workspace, change);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        return Err(anyhow!(
            "change directory does not exist: {}",
            parent.display()
        ));
    }
    let unarchivable_records: Vec<UnarchivableDeltaRecord> = detail
        .unarchivable_deltas
        .iter()
        .map(UnarchivableDeltaRecord::from)
        .collect();
    // Pick the operator_action that matches the populated case: a gate that
    // could not run (fix the gate, retry), an unarchivable delta (spec-file
    // edit), else the default tasks.md edit.
    let operator_action = if detail.gate_error.is_some() {
        OPERATOR_ACTION_GATE_ERROR
    } else if !unarchivable_records.is_empty() && detail.unimplementable_tasks.is_empty() {
        OPERATOR_ACTION_UNARCHIVABLE
    } else {
        OPERATOR_ACTION
    };
    let marker = SpecNeedsRevisionMarker {
        change: change.to_string(),
        marked_at: Utc::now(),
        unimplementable_tasks: detail.unimplementable_tasks.clone(),
        unarchivable_deltas: unarchivable_records,
        canon_editing_tasks: detail.canon_editing_tasks.clone(),
        gate_error: detail.gate_error.clone(),
        contradictions: detail.contradictions.clone(),
        revision_suggestion: detail.revision_suggestion.clone(),
        operator_action: operator_action.to_string(),
    };
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &marker).with_context(|| {
        format!("serializing spec-needs-revision marker for {}", path.display())
    })?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Idempotent removal of the marker. A missing file is success.
pub fn remove_marker(workspace: &Path, change: &str) -> Result<()> {
    let path = marker_path(workspace, change);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Parse the marker for `change`, if present. `Ok(None)` when the file does not
/// exist; `Err` on a read or parse failure. Used by the spec-revision executor
/// to ground its prompt in the marker's CURRENT structured findings.
pub fn read_marker(workspace: &Path, change: &str) -> Result<Option<SpecNeedsRevisionMarker>> {
    let path = marker_path(workspace, change);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let marker: SpecNeedsRevisionMarker = serde_json::from_str(&raw)
                .with_context(|| format!("parsing spec-needs-revision marker at {}", path.display()))?;
            Ok(Some(marker))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Refresh the marker's recorded contradiction set to `contradictions`,
/// REPLACING the prior set, so the durable marker states what CURRENTLY
/// contradicts (per "The spec-revision marker carries the current
/// contradiction set"). Updates findings ONLY — it does not clear the marker
/// and is not staged into any PR (the marker is gitignored runtime state).
///
/// Preserves an existing marker's other fields when one is present. When no
/// marker exists yet (the change dir survives but the file was removed), writes
/// a fresh marker carrying just the findings so the next `send it` is still
/// grounded. The change directory must exist (the marker lives inside it).
///
/// Callers treat this as best-effort: a write failure is logged AND never
/// changes the revision outcome.
pub fn refresh_marker_contradictions(
    workspace: &Path,
    change: &str,
    contradictions: &[ContradictionFindingRecord],
) -> Result<()> {
    let existing = read_marker(workspace, change).unwrap_or(None);
    let mut marker = existing.unwrap_or_else(|| SpecNeedsRevisionMarker {
        change: change.to_string(),
        marked_at: Utc::now(),
        unimplementable_tasks: Vec::new(),
        unarchivable_deltas: Vec::new(),
        canon_editing_tasks: Vec::new(),
        gate_error: None,
        contradictions: Vec::new(),
        revision_suggestion: String::new(),
        operator_action: OPERATOR_ACTION.to_string(),
    });
    marker.contradictions = contradictions.to_vec();
    marker.marked_at = Utc::now();

    let path = marker_path(workspace, change);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("destination path has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        return Err(anyhow!(
            "change directory does not exist: {}",
            parent.display()
        ));
    }
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &marker).with_context(|| {
        format!("serializing refreshed spec-needs-revision marker for {}", path.display())
    })?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_change_dir(workspace: &Path, name: &str) {
        std::fs::create_dir_all(workspace.join("openspec/changes").join(name)).unwrap();
    }

    fn fixture_detail() -> SpecNeedsRevisionDetail {
        SpecNeedsRevisionDetail {
            unimplementable_tasks: vec![
                UnimplementableTask {
                    task_id: "5.2".into(),
                    task_text: "install actionlint locally".into(),
                    reason: "no apt access in sandbox".into(),
                },
                UnimplementableTask {
                    task_id: "15.3".into(),
                    task_text: "smoke-test on macOS".into(),
                    reason: "no macOS host available".into(),
                },
            ],
            unarchivable_deltas: Vec::new(),
            canon_editing_tasks: Vec::new(),
            revision_suggestion:
                "Replace 5.2 with a CI gate. Drop 15.3 — the workflow's own first real run is the integration test.".into(),
            gate_error: None,
            contradictions: Vec::new(),
        }
    }

    #[test]
    fn write_then_exists_returns_true() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        assert!(!marker_exists(ws, "foo"));
        write_marker(ws, "foo", &fixture_detail()).unwrap();
        assert!(marker_exists(ws, "foo"));
    }

    #[test]
    fn write_marker_roundtrips_all_fields() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        let detail = fixture_detail();
        write_marker(ws, "foo", &detail).unwrap();

        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.needs-spec-revision.json"),
        )
        .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.change, "foo");
        assert_eq!(parsed.unimplementable_tasks, detail.unimplementable_tasks);
        assert_eq!(parsed.revision_suggestion, detail.revision_suggestion);
        assert!(parsed
            .operator_action
            .contains("delete this marker file"));
        // marked_at is recent.
        let age = (Utc::now() - parsed.marked_at).num_seconds().abs();
        assert!(age < 5, "marked_at must be ~now; age = {age}s");
    }

    #[test]
    fn write_marker_errors_when_change_directory_absent() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let detail = fixture_detail();
        let err = write_marker(ws, "missing", &detail)
            .expect_err("write_marker should fail when change dir is absent");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("change directory does not exist"),
            "error must mention missing change dir: {msg}"
        );
    }

    use crate::preflight::spec_archivability::{DeltaKind, UnarchivableDelta};

    fn fixture_unarchivable_detail() -> SpecNeedsRevisionDetail {
        SpecNeedsRevisionDetail {
            unimplementable_tasks: Vec::new(),
            unarchivable_deltas: vec![UnarchivableDelta {
                capability: "code-reviewer".into(),
                kind: DeltaKind::Modified,
                header: "Reviewer prompt budget is operator-configurable".into(),
                reason: "header not found in canonical openspec/specs/code-reviewer/spec.md (this is the a07-style bug; check spelling AND capitalization)".into(),
            }],
            canon_editing_tasks: Vec::new(),
            revision_suggestion: "Pre-flight check found 1 unarchivable spec delta:\n- capability=code-reviewer kind=Modified header=\"...\" reason=\"...\"".into(),
            gate_error: None,
            contradictions: Vec::new(),
        }
    }

    #[test]
    fn write_marker_gate_error_serialises_and_sets_gate_operator_action() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "held");
        let detail = SpecNeedsRevisionDetail {
            unimplementable_tasks: Vec::new(),
            unarchivable_deltas: Vec::new(),
            canon_editing_tasks: Vec::new(),
            revision_suggestion: "the [verifier:in] gate could not run".into(),
            gate_error: Some(GateErrorRecord {
                gate: "[verifier:in]".into(),
                cause: "CLI strategy unavailable".into(),
            }),
            contradictions: Vec::new(),
        };
        write_marker(ws, "held", &detail).unwrap();
        let raw = std::fs::read_to_string(ws.join("openspec/changes/held/.needs-spec-revision.json"))
            .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        let ge = parsed.gate_error.expect("gate_error must serialize");
        assert_eq!(ge.gate, "[verifier:in]");
        assert_eq!(ge.cause, "CLI strategy unavailable");
        // The held marker's operator_action explains the gate failure (fix +
        // clear to retry), NOT a tasks.md/spec edit.
        assert!(
            parsed.operator_action.contains("could NOT run"),
            "operator_action explains the gate failure: {}",
            parsed.operator_action
        );
    }

    #[test]
    fn write_marker_serialises_unarchivable_deltas() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        let detail = fixture_unarchivable_detail();
        write_marker(ws, "foo", &detail).unwrap();

        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.needs-spec-revision.json"),
        )
        .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.change, "foo");
        assert_eq!(parsed.unarchivable_deltas.len(), 1);
        assert_eq!(parsed.unarchivable_deltas[0].capability, "code-reviewer");
        assert_eq!(parsed.unarchivable_deltas[0].kind, "Modified");
        // unimplementable_tasks omitted from JSON when empty.
        assert!(parsed.unimplementable_tasks.is_empty());
        // The operator action targets the spec file, not tasks.md.
        assert!(
            parsed.operator_action.contains("specs/<capability>/spec.md"),
            "operator_action must point at spec edit for unarchivable-deltas marker: {:?}",
            parsed.operator_action
        );
    }

    /// A canon-editing-tasks marker serialises its offending task lines AND
    /// keeps the default tasks.md-edit operator action (the fix is to remove the
    /// offending task, not edit a spec file).
    #[test]
    fn write_marker_serialises_canon_editing_tasks() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "foo");
        let detail = SpecNeedsRevisionDetail {
            canon_editing_tasks: vec![
                "1.1 Apply the ADDED block to openspec/specs/cap/spec.md".into(),
            ],
            revision_suggestion: "A task directs a canon edit; remove it.".into(),
            ..Default::default()
        };
        write_marker(ws, "foo", &detail).unwrap();
        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/foo/.needs-spec-revision.json"),
        )
        .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.canon_editing_tasks.len(), 1);
        assert!(parsed.canon_editing_tasks[0].contains("openspec/specs/cap/spec.md"));
        assert!(parsed.unarchivable_deltas.is_empty());
        assert!(parsed.unimplementable_tasks.is_empty());
        // The fix is to remove the flagged task from tasks.md — the default
        // operator action, not the spec-file-edit one.
        assert!(
            parsed.operator_action.contains("tasks.md"),
            "operator_action must point at tasks.md: {:?}",
            parsed.operator_action
        );
    }

    /// Pre-spec markers (only `unimplementable_tasks`, no
    /// `unarchivable_deltas` field) must still deserialize. Verifies the
    /// `#[serde(default)]` on the new field.
    #[test]
    fn pre_spec_marker_without_unarchivable_field_deserializes() {
        let raw = r#"{
            "change": "old",
            "marked_at": "2026-05-27T10:00:00Z",
            "unimplementable_tasks": [
                {"task_id": "5.2", "task_text": "install actionlint", "reason": "no apt access"}
            ],
            "revision_suggestion": "Replace 5.2 with a CI gate.",
            "operator_action": "Edit tasks.md, commit + push, then delete this marker file."
        }"#;
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.change, "old");
        assert_eq!(parsed.unimplementable_tasks.len(), 1);
        assert!(parsed.unarchivable_deltas.is_empty());
    }

    /// A marker that records structured contradictions round-trips them
    /// (task 1.1): the gate, the conflicting-requirement pair, the canonical
    /// capability (for `[canon]`), AND the additive prose all survive.
    #[test]
    fn marker_with_structured_contradictions_roundtrips() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "flagged");
        let detail = SpecNeedsRevisionDetail {
            contradictions: vec![
                ContradictionFindingRecord {
                    gate: ContradictionGate::In,
                    requirement_a: "Secrets in env vars".into(),
                    requirement_b: "API key in config.yaml".into(),
                    canonical_capability: String::new(),
                    summary: "A and B cannot both hold".into(),
                    suggested_fix: "drop the config.yaml clause".into(),
                },
                ContradictionFindingRecord {
                    gate: ContradictionGate::Canon,
                    requirement_a: "Retry 5 times".into(),
                    requirement_b: "Retries are capped at 3".into(),
                    canonical_capability: "executor".into(),
                    summary: "the change exceeds the canonical cap".into(),
                    suggested_fix: "MODIFY the canonical cap or align to 3".into(),
                },
            ],
            revision_suggestion: "two contradictions remain".into(),
            ..Default::default()
        };
        write_marker(ws, "flagged", &detail).unwrap();
        let parsed = read_marker(ws, "flagged").unwrap().unwrap();
        assert_eq!(parsed.contradictions.len(), 2);
        assert_eq!(parsed.contradictions[0].gate, ContradictionGate::In);
        assert_eq!(parsed.contradictions[0].requirement_a, "Secrets in env vars");
        assert_eq!(parsed.contradictions[1].gate, ContradictionGate::Canon);
        assert_eq!(parsed.contradictions[1].canonical_capability, "executor");
        assert_eq!(parsed.contradictions[1].requirement_b, "Retries are capped at 3");
    }

    /// Back-compat (task 1.1): a marker JSON with NO `contradictions` field
    /// (an older daemon, or a non-contradiction hold) still parses, with the
    /// field defaulting to empty.
    #[test]
    fn marker_without_contradictions_field_deserializes() {
        let raw = r#"{
            "change": "old",
            "marked_at": "2026-05-27T10:00:00Z",
            "unimplementable_tasks": [],
            "revision_suggestion": "no structured findings here",
            "operator_action": "Edit tasks.md, commit + push, then delete this marker file."
        }"#;
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.change, "old");
        assert!(parsed.contradictions.is_empty());
    }

    /// `refresh_marker_contradictions` REPLACES the prior structured set with
    /// the current one (task 3) AND preserves the marker's other fields.
    #[test]
    fn refresh_replaces_contradiction_set_and_preserves_other_fields() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "c");
        // Seed a marker carrying the ORIGINAL contradiction + prose.
        let detail = SpecNeedsRevisionDetail {
            contradictions: vec![ContradictionFindingRecord {
                gate: ContradictionGate::In,
                requirement_a: "ORIGINAL A".into(),
                requirement_b: "ORIGINAL B".into(),
                canonical_capability: String::new(),
                summary: "original".into(),
                suggested_fix: String::new(),
            }],
            revision_suggestion: "original narrative".into(),
            ..Default::default()
        };
        write_marker(ws, "c", &detail).unwrap();
        // Refresh with a DIFFERENT, current finding set.
        let current = vec![ContradictionFindingRecord {
            gate: ContradictionGate::Canon,
            requirement_a: "NEW change req".into(),
            requirement_b: "NEW canonical req".into(),
            canonical_capability: "security".into(),
            summary: "current".into(),
            suggested_fix: String::new(),
        }];
        refresh_marker_contradictions(ws, "c", &current).unwrap();
        let parsed = read_marker(ws, "c").unwrap().unwrap();
        // The set is REPLACED, not appended.
        assert_eq!(parsed.contradictions.len(), 1);
        assert_eq!(parsed.contradictions[0].requirement_a, "NEW change req");
        assert_eq!(parsed.contradictions[0].gate, ContradictionGate::Canon);
        // The prose narrative is preserved (refresh updates findings only).
        assert_eq!(parsed.revision_suggestion, "original narrative");
    }

    /// `identity()` keys on the gate + requirement pair (+ capability), NOT the
    /// summary or suggested_fix — so a re-gate that rephrases the summary is
    /// still recognized as the SAME finding (escalation, task 6.3).
    #[test]
    fn finding_identity_ignores_summary_and_fix() {
        let a = ContradictionFindingRecord {
            gate: ContradictionGate::Canon,
            requirement_a: "X".into(),
            requirement_b: "Y".into(),
            canonical_capability: "cap".into(),
            summary: "phrasing one".into(),
            suggested_fix: "fix one".into(),
        };
        let b = ContradictionFindingRecord {
            summary: "completely different phrasing".into(),
            suggested_fix: "fix two".into(),
            ..a.clone()
        };
        assert_eq!(a.identity(), b.identity());
        // A different requirement pair is a different identity.
        let c = ContradictionFindingRecord {
            requirement_b: "Z".into(),
            ..a.clone()
        };
        assert_ne!(a.identity(), c.identity());
    }

    /// Round-trip a marker with BOTH arrays populated (rare in practice
    /// but the schema permits it). Verifies serialization preserves both.
    #[test]
    fn marker_with_mixed_population_roundtrips() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        make_change_dir(ws, "mixed");
        let detail = SpecNeedsRevisionDetail {
            unimplementable_tasks: vec![UnimplementableTask {
                task_id: "1.1".into(),
                task_text: "x".into(),
                reason: "y".into(),
            }],
            unarchivable_deltas: vec![UnarchivableDelta {
                capability: "cap".into(),
                kind: DeltaKind::Renamed,
                header: "from A to B".into(),
                reason: "from-title not found".into(),
            }],
            canon_editing_tasks: Vec::new(),
            revision_suggestion: "fix both".into(),
            gate_error: None,
            contradictions: Vec::new(),
        };
        write_marker(ws, "mixed", &detail).unwrap();
        let raw = std::fs::read_to_string(
            ws.join("openspec/changes/mixed/.needs-spec-revision.json"),
        )
        .unwrap();
        let parsed: SpecNeedsRevisionMarker = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.unimplementable_tasks.len(), 1);
        assert_eq!(parsed.unarchivable_deltas.len(), 1);
        assert_eq!(parsed.unarchivable_deltas[0].kind, "Renamed");
        assert_eq!(parsed.unarchivable_deltas[0].header, "from A to B");
    }
}
