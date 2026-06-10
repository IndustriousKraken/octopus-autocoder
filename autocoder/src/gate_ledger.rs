//! Default-deny verifier-gate verdict ledger (verifier-gates-fail-closed §5–§6).
//!
//! The verifier gates' fail-closed disposition is enforced **structurally** —
//! by a per-change verdict ledger whose default is non-passing — NOT by per-path
//! inspection of each gate's result. Inspection requires every code path (every
//! result arm, every error, every future early-return) to be classified
//! correctly; a single missed path inherits the fall-through, which is how a gate
//! silently fails open. This ledger removes that class of bug: "open" requires an
//! affirmative, completed [`GateVerdict::Pass`], so a crash, an unhandled path,
//! or a runner that never ran leaves the change [`GateVerdict::Pending`] — held
//! by construction.
//!
//! Every gate slot (`[in]`, `[canon]`, `[out]`) starts at [`GateVerdict::Pending`].
//! A verdict becomes [`GateVerdict::Pass`] ONLY by an explicit, completed clean
//! result. A disabled gate is NOT a skip path — its runner is a STUB that records
//! [`GateVerdict::Disabled`], so "disabled" is a recorded verdict, never an
//! absence a reader must remember to treat as a pass.
//!
//! The proceed decision reads the ledger: [`GateLedger::blocking_ok`] is true iff
//! every blocking gate (`[in]`, `[canon]`) is `Pass` or `Disabled`. The executor
//! runs ONLY when that holds. [`GateLedger::render_pr_section`] renders the ledger
//! into the PR body as a compliance record — per gate: identifier, model, verdict
//! (with a one-line summary for `Fail` / `FailedToRun`) — so a `Pass` is visible
//! rather than inferred from the silent absence of an alert.

use crate::verifier_gate::VerifierGate;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One gate slot's verdict. The default is [`GateVerdict::Pending`] (a
/// non-passing state): "open" requires an affirmative, completed `Pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GateVerdict {
    /// No runner recorded a verdict (it crashed, an unhandled path was taken,
    /// or it never ran). Non-passing by construction — the change is HELD.
    #[default]
    Pending,
    /// The gate ran AND produced a clean result. The ONLY verdict that lets a
    /// blocking gate proceed (besides `Disabled`).
    Pass,
    /// The gate ran AND found a problem (findings). Blocking gates hold.
    Fail,
    /// The gate ran but could not produce a verdict (session error, no
    /// submission, unregistered strategy). Blocking gates hold (the change was
    /// NOT evaluated).
    FailedToRun,
    /// The gate is not configured. A non-blocking verdict recorded by a STUB
    /// runner — NOT an absence. Treated like `Pass` for gating, rendered
    /// honestly as "disabled".
    Disabled,
}

impl GateVerdict {
    /// Operator-facing token rendered into the PR section.
    pub fn as_str(self) -> &'static str {
        match self {
            GateVerdict::Pending => "PENDING",
            GateVerdict::Pass => "PASS",
            GateVerdict::Fail => "FAIL",
            GateVerdict::FailedToRun => "FAILED TO RUN",
            GateVerdict::Disabled => "DISABLED",
        }
    }

    /// Whether this verdict lets a BLOCKING gate proceed: only an affirmative
    /// `Pass` OR a recorded `Disabled`. `Pending` / `Fail` / `FailedToRun` are
    /// non-passing, so the executor is held.
    pub fn is_blocking_ok(self) -> bool {
        matches!(self, GateVerdict::Pass | GateVerdict::Disabled)
    }
}

/// One gate slot's recorded outcome: its verdict, the model that ran it (when
/// known), AND a one-line summary (populated for `Fail` / `FailedToRun` so the
/// PR section names the cause).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GateEntry {
    pub verdict: GateVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// The per-change verdict ledger: one [`GateEntry`] per gate slot, all
/// initialized to [`GateVerdict::Pending`] (default-deny).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GateLedger {
    pub r#in: GateEntry,
    pub canon: GateEntry,
    pub out: GateEntry,
}

impl GateLedger {
    /// A fresh ledger: every gate slot `Pending` (held by construction until a
    /// runner affirmatively records a verdict).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the `[in]` gate's verdict (and the model / summary that produced
    /// it). Setter per gate so the dispatch records each slot affirmatively.
    pub fn set_in(&mut self, verdict: GateVerdict, model: Option<String>, summary: Option<String>) {
        self.r#in = GateEntry {
            verdict,
            model,
            summary,
        };
    }

    /// Record the `[canon]` gate's verdict.
    pub fn set_canon(
        &mut self,
        verdict: GateVerdict,
        model: Option<String>,
        summary: Option<String>,
    ) {
        self.canon = GateEntry {
            verdict,
            model,
            summary,
        };
    }

    /// Record the `[out]` gate's verdict.
    pub fn set_out(&mut self, verdict: GateVerdict, model: Option<String>, summary: Option<String>) {
        self.out = GateEntry {
            verdict,
            model,
            summary,
        };
    }

    /// The fail-closed proceed decision: true iff EVERY blocking gate (`[in]`,
    /// `[canon]`) is `Pass` or `Disabled`. A blocking gate that is `Pending`,
    /// `Fail`, or `FailedToRun` returns false — so the executor is held. The
    /// advisory `[out]` gate never participates in the gating decision.
    pub fn blocking_ok(&self) -> bool {
        self.r#in.verdict.is_blocking_ok() && self.canon.verdict.is_blocking_ok()
    }

    /// Render the ledger into a `## Gate verdicts` PR-body section: per gate, its
    /// identifier, the model that ran it (when known), its verdict, AND a
    /// one-line summary for `Fail` / `FailedToRun`. A `Pass` is therefore
    /// VISIBLE in the PR rather than inferred from the silent absence of an
    /// alert.
    pub fn render_pr_section(&self) -> String {
        let mut out = String::from("## Gate verdicts\n\n");
        for (gate, entry) in [
            (VerifierGate::In, &self.r#in),
            (VerifierGate::Canon, &self.canon),
            (VerifierGate::Out, &self.out),
        ] {
            out.push_str(&render_entry_line(gate, entry));
        }
        out
    }
}

/// Where the per-change gate ledger is persisted: under the workspace's `.git/`
/// directory — NOT the managed working tree (a16: daemon bookkeeping never lives
/// in the repo tree; mirrors `agentic_run::sandbox_settings_dir`). The file is
/// overwritten each pass AND read back at PR-body assembly, so it can never leak
/// into a PR, trip the dirty-workspace check, or linger as gitignored litter.
fn ledger_path(workspace: &Path, change: &str) -> PathBuf {
    workspace
        .join(".git")
        .join("autocoder-gate-ledger")
        .join(format!("{change}.json"))
}

/// Persist the per-change ledger atomically (tempfile + rename) under `.git/`.
/// The pre-executor dispatch writes the `[in]`/`[canon]` verdicts here; the
/// post-executor PR-assembly path reads them back, folds in `[out]`, AND renders
/// the combined section. Best-effort at the call site — a write failure is
/// logged, NOT propagated (the in-memory ledger still gates the executor; only
/// the PR-render record is at risk).
pub fn write_ledger(workspace: &Path, change: &str, ledger: &GateLedger) -> Result<()> {
    let path = ledger_path(workspace, change);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("gate-ledger path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating gate-ledger dir {}", parent.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating gate-ledger tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, ledger)
        .with_context(|| format!("serializing gate ledger for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Read the per-change ledger back. Returns `None` when the file is absent (the
/// pre-executor dispatch never ran for this change — e.g. it was resumed from
/// the waiting set) OR cannot be parsed; the PR-render path then renders only the
/// post-executor `[out]` verdict it has in hand.
pub fn read_ledger(workspace: &Path, change: &str) -> Option<GateLedger> {
    let path = ledger_path(workspace, change);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Render one ledger row: `- [verifier:<id>] <verdict>` plus an optional model
/// (` — model: <m>`) AND, for non-clean verdicts, a one-line summary.
fn render_entry_line(gate: VerifierGate, entry: &GateEntry) -> String {
    let mut line = format!("- {} {}", gate.label(), entry.verdict.as_str());
    if let Some(model) = entry.model.as_deref().filter(|m| !m.trim().is_empty()) {
        line.push_str(&format!(" — model: {model}"));
    }
    if matches!(
        entry.verdict,
        GateVerdict::Fail | GateVerdict::FailedToRun
    ) && let Some(summary) = entry.summary.as_deref().filter(|s| !s.trim().is_empty())
    {
        line.push_str(&format!(" — {}", one_line(summary)));
    }
    line.push('\n');
    line
}

/// Collapse a (possibly multi-line) summary into a single PR-row line.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- blocking_ok truth table ----

    #[test]
    fn fresh_ledger_is_all_pending_and_not_blocking_ok() {
        let l = GateLedger::new();
        assert_eq!(l.r#in.verdict, GateVerdict::Pending);
        assert_eq!(l.canon.verdict, GateVerdict::Pending);
        assert_eq!(l.out.verdict, GateVerdict::Pending);
        // Default-deny: a Pending blocking gate holds the change.
        assert!(!l.blocking_ok());
    }

    #[test]
    fn blocking_ok_truth_table() {
        use GateVerdict::*;
        // Exhaustive over the two blocking gates; `[out]` is advisory and must
        // never affect the gating decision.
        let cases: &[(GateVerdict, GateVerdict, bool)] = &[
            (Pass, Pass, true),
            (Pass, Disabled, true),
            (Disabled, Pass, true),
            (Disabled, Disabled, true),
            (Pass, Pending, false),
            (Pending, Pass, false),
            (Pass, Fail, false),
            (Fail, Pass, false),
            (Pass, FailedToRun, false),
            (FailedToRun, Pass, false),
            (Pending, Pending, false),
            (Fail, Fail, false),
        ];
        for &(in_v, canon_v, expect) in cases {
            let mut l = GateLedger::new();
            l.set_in(in_v, None, None);
            l.set_canon(canon_v, None, None);
            // The advisory [out] gate is set to a non-passing verdict to prove
            // it does NOT gate.
            l.set_out(Fail, None, Some("advisory only".into()));
            assert_eq!(
                l.blocking_ok(),
                expect,
                "blocking_ok([in]={in_v:?}, [canon]={canon_v:?}) must be {expect}"
            );
        }
    }

    #[test]
    fn disabled_blocking_gate_proceeds() {
        let mut l = GateLedger::new();
        l.set_in(GateVerdict::Disabled, None, None);
        l.set_canon(GateVerdict::Disabled, None, None);
        assert!(
            l.blocking_ok(),
            "two disabled blocking gates must let the executor proceed"
        );
    }

    // ---- render_pr_section ----

    #[test]
    fn render_pr_section_names_each_gate_model_and_verdict() {
        let mut l = GateLedger::new();
        l.set_in(GateVerdict::Pass, Some("anthropic/claude-x".into()), None);
        l.set_canon(GateVerdict::Disabled, None, None);
        l.set_out(
            GateVerdict::Fail,
            Some("anthropic/claude-y".into()),
            Some("two gaps found".into()),
        );
        let section = l.render_pr_section();
        assert!(section.starts_with("## Gate verdicts"));
        // Each gate identifier is present.
        assert!(section.contains("[verifier:in]"), "{section}");
        assert!(section.contains("[verifier:canon]"), "{section}");
        assert!(section.contains("[verifier:out]"), "{section}");
        // Verdicts.
        assert!(section.contains("PASS"), "{section}");
        assert!(section.contains("DISABLED"), "{section}");
        assert!(section.contains("FAIL"), "{section}");
        // Models (when known).
        assert!(section.contains("anthropic/claude-x"), "{section}");
        assert!(section.contains("anthropic/claude-y"), "{section}");
        // The one-line summary appears for the FAIL row.
        assert!(section.contains("two gaps found"), "{section}");
    }

    #[test]
    fn render_pr_section_pass_is_visible_without_summary() {
        let mut l = GateLedger::new();
        l.set_in(GateVerdict::Pass, Some("m1".into()), None);
        l.set_canon(GateVerdict::Pass, Some("m2".into()), None);
        l.set_out(GateVerdict::Pass, Some("m3".into()), None);
        let section = l.render_pr_section();
        // A PASS is visible (not inferred from silence).
        assert_eq!(section.matches("PASS").count(), 3, "{section}");
    }

    #[test]
    fn render_failed_to_run_carries_cause_summary() {
        let mut l = GateLedger::new();
        l.set_in(
            GateVerdict::FailedToRun,
            Some("m".into()),
            Some("session\n  timed out".into()),
        );
        let section = l.render_pr_section();
        assert!(section.contains("FAILED TO RUN"), "{section}");
        // The multi-line cause is collapsed to one row.
        assert!(section.contains("session timed out"), "{section}");
    }

    #[test]
    fn write_then_read_ledger_roundtrips_under_git_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        let mut l = GateLedger::new();
        l.set_in(GateVerdict::Pass, Some("m1".into()), None);
        l.set_canon(GateVerdict::Disabled, None, None);
        write_ledger(ws, "my-change", &l).unwrap();
        // a16: the ledger lives under `.git/`, NOT the managed working tree.
        assert!(
            ws.join(".git/autocoder-gate-ledger/my-change.json").exists(),
            "ledger must persist under .git/"
        );
        assert!(
            !ws.join("openspec/changes/my-change").exists(),
            "ledger must NOT touch the working tree"
        );
        let back = read_ledger(ws, "my-change").expect("ledger reads back");
        assert_eq!(l, back);
    }

    #[test]
    fn read_ledger_absent_is_none() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_ledger(dir.path(), "never-written").is_none());
    }

    #[test]
    fn ledger_roundtrips_through_json() {
        let mut l = GateLedger::new();
        l.set_in(GateVerdict::Pass, Some("m1".into()), None);
        l.set_canon(GateVerdict::FailedToRun, Some("m2".into()), Some("boom".into()));
        let json = serde_json::to_string(&l).unwrap();
        let back: GateLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(l, back);
    }
}
