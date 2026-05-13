//! Backend-agnostic executor abstraction. autocoder invokes
//! implementations through this trait. The architecture-level spec lives at
//! `openspec/specs/executor/spec.md`; concrete backends are introduced by
//! per-change implementations (this phase: `claude_cli`).

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod claude_cli;

#[async_trait]
pub trait Executor: Send + Sync {
    async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome>;
    async fn resume(&self, handle: ResumeHandle, answer: &str) -> Result<ExecutorOutcome>;
}

#[derive(Debug, Clone)]
pub enum ExecutorOutcome {
    /// The underlying agent reported successful completion of the change.
    /// autocoder decides what to do with a no-diff `Completed`.
    Completed,
    /// The agent has signaled ambiguity. autocoder persists the
    /// `resume_handle` to `.question.json`, posts the question to ChatOps,
    /// and unlocks the change.
    AskUser {
        question: String,
        resume_handle: ResumeHandle,
    },
    /// Unrecoverable failure. autocoder unlocks the change and does
    /// NOT archive it.
    Failed { reason: String },
}

/// Opaque payload passed between `run` and `resume`. JSON-serializable so
/// autocoder can persist it across daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeHandle(pub serde_json::Value);
