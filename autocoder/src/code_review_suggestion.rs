//! Diff-overlap re-review suggestion (a33).
//!
//! After each operator-initiated revision iteration's Completed outcome
//! AND successful push, the daemon may post an informational chatops
//! notification recommending `@<bot> code-review` when the cumulative-
//! since-original-review diff overlap exceeds the operator-configured
//! threshold (`reviewer.suggest_rereview_threshold`).
//!
//! Overlap is computed as:
//!
//! ```text
//! overlap = lines_changed(original_review_head_sha → current_agent_head_sha)
//!         / lines_changed(pr.base_sha → original_review_head_sha)
//! ```
//!
//! Both counts use `git diff --numstat` semantics (additions + deletions,
//! ignoring binary files which contribute zero). The numerator is the
//! cumulative lines changed across ALL revisions on the PR since the
//! original review's head; the denominator is the lines changed in the
//! original PR diff (the diff the original review evaluated).

use anyhow::Result;
use std::path::Path;

use crate::git;

/// Inputs to [`compute_overlap`]. Carries the three SHAs the formula
/// needs AND the workspace path used to invoke `git`. Kept as a struct
/// (rather than a 4-arg function) so adding future fields (e.g. an
/// optional `paths_filter` for path-scoped overlap) does not break the
/// call site.
#[derive(Debug, Clone)]
pub struct OverlapInputs<'a> {
    pub workspace: &'a Path,
    pub base_sha: &'a str,
    pub original_review_head_sha: &'a str,
    pub current_agent_head_sha: &'a str,
}

/// Computed overlap result. Returned by [`compute_overlap`] — the
/// caller compares `ratio` against the operator-configured threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlapResult {
    pub revision_lines: usize,
    pub original_lines: usize,
    pub ratio: f32,
}

/// Compute the diff overlap per the canonical formula (a33). Returns
/// `Ok(None)` when the denominator is zero (the original PR diff had
/// zero non-binary line changes — no baseline to compare against; the
/// caller skips the suggestion). Errors propagate from the underlying
/// git invocations.
pub fn compute_overlap(inputs: &OverlapInputs<'_>) -> Result<Option<OverlapResult>> {
    let original_lines = git::diff_numstat_total(
        inputs.workspace,
        inputs.base_sha,
        inputs.original_review_head_sha,
    )?;
    if original_lines == 0 {
        return Ok(None);
    }
    let revision_lines = git::diff_numstat_total(
        inputs.workspace,
        inputs.original_review_head_sha,
        inputs.current_agent_head_sha,
    )?;
    let ratio = (revision_lines as f32) / (original_lines as f32);
    Ok(Some(OverlapResult {
        revision_lines,
        original_lines,
        ratio,
    }))
}

/// Pre-flight check inputs (a33 task 7.3): the dedup state needed to
/// decide whether the suggestion would fire BEFORE running the (more
/// expensive) git computation. Pure-data struct so the gating logic can
/// be unit-tested without spinning up a real workspace.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SuggestionGate {
    pub threshold: Option<f32>,
    pub original_review_head_sha_set: bool,
    pub last_suggested_at_revisions_count: Option<u32>,
    pub revisions_applied: u32,
    pub failure_alerts_enabled: bool,
}

/// Decide whether the daemon should compute overlap AND post the
/// suggestion. Mirrors the gating logic in `maybe_post_rereview_suggestion`.
/// Returns `Some(threshold)` when the suggestion path should run; `None`
/// otherwise. Exposed for unit-testability.
#[allow(dead_code)]
pub fn should_compute_suggestion(gate: SuggestionGate) -> Option<f32> {
    let threshold = gate.threshold?;
    if !gate.original_review_head_sha_set {
        return None;
    }
    if gate.last_suggested_at_revisions_count == Some(gate.revisions_applied) {
        return None;
    }
    if !gate.failure_alerts_enabled {
        return None;
    }
    Some(threshold)
}

/// Quantize an overlap ratio to a `<percent>` value for the canonical
/// notification text. Uses `(ratio * 100).round()`, clamped to
/// `[0, u32::MAX]`. Pure function for testability.
pub fn percent_for_text(ratio: f32) -> u32 {
    let rounded = (ratio * 100.0).round();
    if rounded.is_nan() || rounded < 0.0 {
        0
    } else if rounded > u32::MAX as f32 {
        u32::MAX
    } else {
        rounded as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_for_text_rounds_half_up_at_classic_marks() {
        assert_eq!(percent_for_text(0.0), 0);
        assert_eq!(percent_for_text(0.005), 1);
        assert_eq!(percent_for_text(0.5), 50);
        assert_eq!(percent_for_text(0.6), 60);
        assert_eq!(percent_for_text(0.999), 100);
        assert_eq!(percent_for_text(1.0), 100);
        assert_eq!(percent_for_text(1.5), 150);
    }

    #[test]
    fn percent_for_text_clamps_negative_or_nan() {
        assert_eq!(percent_for_text(-0.5), 0);
        assert_eq!(percent_for_text(f32::NAN), 0);
    }

    fn base_gate() -> SuggestionGate {
        SuggestionGate {
            threshold: Some(0.5),
            original_review_head_sha_set: true,
            last_suggested_at_revisions_count: None,
            revisions_applied: 1,
            failure_alerts_enabled: true,
        }
    }

    /// Task 7.6: a revision Completed with overlap 60% AND threshold
    /// 0.5 posts the suggestion (gating returns `Some(0.5)` AND the
    /// caller's `overlap >= threshold` arithmetic fires).
    #[test]
    fn gate_allows_when_threshold_set_and_first_iteration() {
        assert_eq!(should_compute_suggestion(base_gate()), Some(0.5));
    }

    /// Task 7.7: the same setup on a second polling cycle (same
    /// `revisions_applied` count) does NOT re-post.
    #[test]
    fn gate_blocks_when_already_suggested_at_same_revisions_count() {
        let gate = SuggestionGate {
            last_suggested_at_revisions_count: Some(1),
            ..base_gate()
        };
        assert!(should_compute_suggestion(gate).is_none());
    }

    /// Task 7.8: threshold unset → no suggestion regardless of overlap.
    #[test]
    fn gate_blocks_when_threshold_unset() {
        let gate = SuggestionGate {
            threshold: None,
            ..base_gate()
        };
        assert!(should_compute_suggestion(gate).is_none());
    }

    /// Task 7.9: `original_review_head_sha` absent → no suggestion
    /// (state file from before the field was added).
    #[test]
    fn gate_blocks_when_baseline_sha_absent() {
        let gate = SuggestionGate {
            original_review_head_sha_set: false,
            ..base_gate()
        };
        assert!(should_compute_suggestion(gate).is_none());
    }

    /// Task 7.10: `failure_alerts_enabled: false` → no suggestion
    /// regardless of threshold.
    #[test]
    fn gate_blocks_when_failure_alerts_off() {
        let gate = SuggestionGate {
            failure_alerts_enabled: false,
            ..base_gate()
        };
        assert!(should_compute_suggestion(gate).is_none());
    }

    /// New revision iteration becomes a fresh opportunity (state
    /// scenario): when a previous suggestion was at count 2 AND the
    /// current count is 3, the gate allows.
    #[test]
    fn gate_allows_new_revision_iteration_after_prior_suggestion() {
        let gate = SuggestionGate {
            last_suggested_at_revisions_count: Some(2),
            revisions_applied: 3,
            ..base_gate()
        };
        assert_eq!(should_compute_suggestion(gate), Some(0.5));
    }
}
