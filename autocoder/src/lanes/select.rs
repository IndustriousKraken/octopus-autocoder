//! Cross-lane unit selection (a009 §4).
//!
//! Within the existing per-repo serializer (the busy-marker — one unit of
//! work per repository at a time), each polling iteration selects the
//! highest-precedence READY unit in the order `issues > changes >
//! audits`, extending the established changes-over-audits precedence.
//! Within a lane, selection is alphabetical. Issue-precedence is STRICT:
//! a ready issue beats a ready change. Anti-starvation is provided by the
//! promotion gate (issues enter the lane only after maintainer approval),
//! NOT by a scheduling fairness rule.
//!
//! The caller is responsible for the feature gate: when `features.issues`
//! is off, it passes an empty `issues_ready` slice, so the selector falls
//! straight through to the changes lane.

/// One selectable unit of work, tagged by its lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneUnit {
    Issue(String),
    Change(String),
    Audit(String),
}

impl LaneUnit {
    /// The lane name, for logging.
    pub fn lane(&self) -> &'static str {
        match self {
            LaneUnit::Issue(_) => "issues",
            LaneUnit::Change(_) => "changes",
            LaneUnit::Audit(_) => "audits",
        }
    }

    /// The selected unit's slug/name.
    pub fn name(&self) -> &str {
        match self {
            LaneUnit::Issue(s) | LaneUnit::Change(s) | LaneUnit::Audit(s) => s,
        }
    }
}

/// Pick the highest-precedence ready unit across the three lanes in the
/// order `issues > changes > audits`, alphabetical within a lane.
///
/// Each slice is that lane's ready units; this function does not assume
/// the slices are pre-sorted — it picks the alphabetically-smallest entry
/// of the highest non-empty lane. Returns `None` when every lane is
/// empty.
pub fn select_next_unit(
    issues_ready: &[String],
    changes_ready: &[String],
    audits_ready: &[String],
) -> Option<LaneUnit> {
    if let Some(s) = min_alpha(issues_ready) {
        return Some(LaneUnit::Issue(s));
    }
    if let Some(s) = min_alpha(changes_ready) {
        return Some(LaneUnit::Change(s));
    }
    if let Some(s) = min_alpha(audits_ready) {
        return Some(LaneUnit::Audit(s));
    }
    None
}

/// Alphabetically-smallest entry of `items`, or `None` when empty.
fn min_alpha(items: &[String]) -> Option<String> {
    items.iter().min().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ready_issue_beats_ready_change_and_audit() {
        let got = select_next_unit(&v(&["fix-a"]), &v(&["change-a"]), &v(&["audit-a"]));
        assert_eq!(got, Some(LaneUnit::Issue("fix-a".to_string())));
    }

    #[test]
    fn ready_change_beats_ready_audit_when_no_issue() {
        let got = select_next_unit(&v(&[]), &v(&["change-a"]), &v(&["audit-a"]));
        assert_eq!(got, Some(LaneUnit::Change("change-a".to_string())));
    }

    #[test]
    fn audit_selected_when_only_audits_ready() {
        let got = select_next_unit(&v(&[]), &v(&[]), &v(&["audit-z"]));
        assert_eq!(got, Some(LaneUnit::Audit("audit-z".to_string())));
    }

    #[test]
    fn alphabetical_within_issues_lane() {
        let got = select_next_unit(&v(&["zeta", "alpha", "mid"]), &v(&[]), &v(&[]));
        assert_eq!(got, Some(LaneUnit::Issue("alpha".to_string())));
    }

    #[test]
    fn alphabetical_within_changes_lane() {
        let got = select_next_unit(&v(&[]), &v(&["02-b", "01-a"]), &v(&[]));
        assert_eq!(got, Some(LaneUnit::Change("01-a".to_string())));
    }

    #[test]
    fn none_when_all_lanes_empty() {
        assert_eq!(select_next_unit(&v(&[]), &v(&[]), &v(&[])), None);
    }

    #[test]
    fn feature_off_caller_passes_empty_issues_and_change_wins() {
        // The gate is the caller's responsibility: feature-off → empty
        // issues slice, so a ready change is selected over an absent issue.
        let got = select_next_unit(&v(&[]), &v(&["change-a"]), &v(&[]));
        assert_eq!(got, Some(LaneUnit::Change("change-a".to_string())));
    }
}
