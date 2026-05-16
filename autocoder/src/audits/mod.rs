//! Periodic-audit framework types.
//!
//! NOTE: This module is a minimal scaffold introduced by the
//! `architecture-consultative-audit` change so the consultative audit
//! can compile and ship its prompt + parser without waiting on the full
//! `periodic-audits-foundation` change. When the foundation lands, the
//! types here (`Audit`, `WritePolicy`, `AuditOutcome`, `Finding`,
//! `Severity`, `AuditContext`, `AuditSettings`) are the contract its
//! scheduler / state-file / registry will integrate against. They are
//! intentionally free-standing: no scheduler, no state persistence, no
//! `cli/run.rs` registry wiring lives here. The foundation change owns
//! all of that.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod architecture_consultative;

/// Audit-execution sandbox policy. Future scheduler will enforce this
/// via post-hoc `git status --porcelain` checks; for now it's a
/// declaration that travels with each audit impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// No writes allowed. Sandbox blocks `Write`/`Edit`; post-hoc diff
    /// must be empty.
    None,
    /// Writes allowed only under `openspec/changes/`.
    OpenSpecOnly,
    /// Full write access. Reserved for future audits.
    #[allow(dead_code)]
    Approved,
}

/// Severity of an individual `Finding`. The consultative audit uses
/// `Low` and `Medium` only; `High` is reserved for bright-line audits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
}

/// One observation produced by an audit. Renders as a chatops bullet
/// (subject + anchor) plus a per-invocation log entry (full body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub subject: String,
    pub body: String,
    pub anchor: Option<String>,
}

/// What an audit's `run` returned. The framework dispatches on the
/// variant: `Reported` posts to chatops, `SpecsWritten` lets the
/// iteration's `list_pending` pick up the new directories, `NoFindings`
/// is silent (subset of `Reported(vec![])`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditOutcome {
    #[allow(dead_code)]
    NoFindings,
    Reported(Vec<Finding>),
    #[allow(dead_code)]
    SpecsWritten(Vec<String>),
}

/// Per-audit knobs read from `audits.<type>.*` config. The
/// consultative audit honors `prompt_path` (override the embedded
/// default) and `notify_on_clean` (kept for parity with
/// foundation-defined behavior; the audit itself does not act on it).
/// `extra` is the foundation's escape hatch for per-audit numeric /
/// string knobs (e.g. brightline thresholds) without bloating the
/// top-level schema.
#[derive(Debug, Clone, Default)]
pub struct AuditSettings {
    pub prompt_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub notify_on_clean: bool,
    #[allow(dead_code)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// Runtime context passed to each audit's `run`. The foundation will
/// extend this with chatops + log-writer handles; until then the
/// consultative audit only needs `workspace`.
pub struct AuditContext<'a> {
    pub workspace: &'a Path,
}

#[async_trait]
pub trait Audit: Send + Sync {
    fn audit_type(&self) -> &'static str;
    fn requires_head_change(&self) -> bool;
    fn write_policy(&self) -> WritePolicy;
    async fn run(&self, ctx: &AuditContext<'_>) -> Result<AuditOutcome>;
}
