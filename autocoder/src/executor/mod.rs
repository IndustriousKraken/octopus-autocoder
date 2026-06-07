//! Backend-agnostic executor abstraction. autocoder invokes
//! implementations through this trait. The architecture-level spec lives at
//! `openspec/specs/executor/spec.md`; concrete backends are introduced by
//! per-change implementations (this phase: `claude_cli`).

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod acceptance_scan;
pub mod claude_cli;
pub mod event_log;
pub mod json_event;

#[async_trait]
pub trait Executor: Send + Sync {
    /// Run the agent against `change` in `workspace`.
    ///
    /// Returns `SpecNeedsRevision` when one or more tasks in tasks.md
    /// require capabilities outside the executor's sandbox. The agent
    /// flags upfront, before starting implementation.
    async fn run(&self, workspace: &Path, change: &str) -> Result<ExecutorOutcome>;
    async fn resume(&self, handle: ResumeHandle, answer: &str) -> Result<ExecutorOutcome>;
    /// Re-invoke the agent against an already-archived `change` in
    /// `workspace`, passing the operator's revision text and the
    /// current PR diff as context. The default implementation calls
    /// `run`, so backends that have not yet been taught about revision
    /// mode degrade to a plain re-run; the production
    /// `ClaudeCliExecutor` overrides this to build a revision-mode
    /// prompt.
    async fn run_revision(
        &self,
        workspace: &Path,
        change: &str,
        revision_context: &crate::revisions::RevisionContext,
    ) -> Result<ExecutorOutcome> {
        let _ = revision_context;
        self.run(workspace, change).await
    }

    /// Triage-mode invocation for the `audit-reply-acts` flow: the
    /// operator typed `@<bot> send it` in an audit thread, so the
    /// daemon spawns the executor against the audit's findings to
    /// classify each finding as quick-fix vs spec-worthy, apply the
    /// quick fixes directly, and create `openspec/changes/<slug>/`
    /// dirs for the spec-worthy ones.
    ///
    /// Default impl returns `Failed { reason: "triage mode not
    /// supported" }` so a backend that hasn't been taught about
    /// triage degrades to a polite refusal instead of a panic.
    async fn run_triage(
        &self,
        workspace: &Path,
        ctx: &TriageContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "triage mode not supported by this executor backend".to_string(),
        })
    }

    /// Chat-driven triage for the `chat-request-triage` flow: the
    /// operator typed `@<bot> propose <repo> <text>` in chat. The
    /// executor classifies the request as DIRECTIVE / QUESTION /
    /// AMBIGUOUS (per the `prompts/chat-request-triage.md` template),
    /// and for DIRECTIVE applies code fixes and/or creates
    /// `openspec/changes/<chat-derived-slug>/`; for QUESTION writes a
    /// `<workspace>/.chat-reply.md` and finishes; for AMBIGUOUS calls
    /// the `ask_user` MCP tool.
    ///
    /// Default impl returns `Failed { reason: "chat-triage mode not
    /// supported" }`.
    async fn run_chat_triage(
        &self,
        workspace: &Path,
        ctx: &ChatTriageContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "chat-triage mode not supported by this executor backend".to_string(),
        })
    }

    /// Brownfield-draft mode for the `brownfield` chatops verb (a23).
    /// The polling iteration's brownfield handler resolves the prompt
    /// template (embedded default OR a workspace-relative override
    /// from `features.brownfield.prompt_path`), substitutes the
    /// `BrownfieldDraftContext` fields into the template, AND passes
    /// the rendered prompt here. The backend's job is to invoke the
    /// wrapped CLI with the rendered prompt under a read-only
    /// sandbox (the polling layer verifies the resulting diff stays
    /// under `openspec/`).
    ///
    /// Default impl returns `Failed { reason: "brownfield-draft mode
    /// not supported" }`.
    async fn run_brownfield_draft(
        &self,
        workspace: &Path,
        ctx: &BrownfieldDraftContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "brownfield-draft mode not supported by this executor backend"
                .to_string(),
        })
    }

    /// Scout-mode invocation for the `scout` chatops verb (a25). The
    /// polling layer has substituted the scout prompt template; this
    /// method passes the rendered prompt verbatim to the wrapped CLI
    /// under a read-only sandbox AND returns the executor's final
    /// answer (expected to be a JSON array of opportunity items). The
    /// scout polling handler parses the JSON itself.
    ///
    /// Default impl returns `Failed { reason: "scout mode not
    /// supported" }`.
    async fn run_scout(
        &self,
        workspace: &Path,
        ctx: &ScoutContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "scout mode not supported by this executor backend".to_string(),
        })
    }

    /// Chat-driven changelog stylist for the `changelog` chatops verb.
    /// The deterministic extractor has already produced the JSON payload
    /// in `ctx.changelog_json`; this method asks the wrapped CLI to read
    /// any existing `CHANGELOG.md`, match its style (or create a new file
    /// in Keep a Changelog format), and write the polished release notes.
    ///
    /// Default impl returns `Failed { reason: "changelog stylist not
    /// supported" }` so a backend that hasn't been taught about the
    /// changelog flow degrades to a polite refusal instead of a panic.
    async fn run_changelog(
        &self,
        workspace: &Path,
        ctx: &ChangelogContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "changelog stylist not supported by this executor backend".to_string(),
        })
    }

    /// Issue-mode invocation for the issues lane (a009). The issues
    /// walker has already rendered the issue-flavored implementer prompt
    /// (fix-to-EXISTING-spec framing, NOT the change implementer prompt)
    /// AND passes it here verbatim. The backend runs the wrapped CLI with
    /// the MCP outcome tools wired (so the agent can call
    /// `outcome_success` / `outcome_spec_needs_revision`), keyed by the
    /// issue slug for run-log + outcome-store purposes. Acceptance is
    /// against the EXISTING canon — there is no spec delta to apply.
    ///
    /// Default impl returns `Failed { reason: "issue mode not supported"
    /// }` so a backend that hasn't been taught about the issues lane
    /// degrades to a polite refusal instead of a panic.
    async fn run_issue(
        &self,
        workspace: &Path,
        ctx: &IssueContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "issue mode not supported by this executor backend".to_string(),
        })
    }

    /// Read-only triage of a reported GitHub issue for the a010 hybrid
    /// issues-lane ingestion. The ingestion layer has already rendered the
    /// `prompts/issue-report-triage.md` template (reported body embedded as
    /// untrusted DATA) AND passes it here verbatim. The backend runs the
    /// wrapped CLI read-only AND returns the agent's final answer (expected
    /// to carry a `CLASSIFICATION:` verdict block, parsed by the ingestion
    /// layer). The agent writes nothing — classification is advice, not an
    /// action.
    ///
    /// Default impl returns `Failed { reason: "issue-triage mode not
    /// supported" }` so a backend that hasn't been taught about ingestion
    /// degrades to a polite refusal instead of a panic.
    async fn run_issue_triage(
        &self,
        workspace: &Path,
        ctx: &IssueReportTriageContext,
    ) -> Result<ExecutorOutcome> {
        let _ = workspace;
        let _ = ctx;
        Ok(ExecutorOutcome::Failed {
            reason: "issue-triage mode not supported by this executor backend".to_string(),
        })
    }
}

/// Context handed to `Executor::run_triage`. Plumbed in from the
/// dispatcher (which constructs it from the `AuditThreadState` plus the
/// workspace's canonical-specs index). Carried verbatim through the
/// prompt template's `{{...}}` substitutions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageContext {
    /// The full findings excerpt (capped at 35,000 chars) the operator
    /// saw in the audit's reply thread.
    pub findings: String,
    /// The audit's slug (e.g. `architecture_brightline`,
    /// `drift_audit`, `security_bug_audit`).
    pub audit_type: String,
    /// The repository URL the audit ran against.
    pub repo_url: String,
    /// A brief listing of which canonical specs live in
    /// `openspec/specs/`. The triage prompt instructs the LLM to read
    /// the relevant subset before classifying findings.
    pub canonical_specs_index: String,
}

/// Context handed to `Executor::run_changelog`. Plumbed in from the
/// chat-driven `changelog` flow. Carried verbatim through the
/// `prompts/changelog-stylist.md` template's `{{...}}` substitutions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogContext {
    /// JSON payload produced by the deterministic `autocoder changelog`
    /// extractor, wrapped in a `{ "sections": [ … ] }` envelope (a72):
    /// one section per `--format json` shape. A flagless gap-fill run
    /// carries one section per undocumented stable release tag,
    /// oldest-first; an explicit `--since`/`--to` run carries a single
    /// section. The stylist gets this as its primary input.
    pub changelog_json: String,
    /// Repository URL the changelog targets (for the prompt's context
    /// banner line).
    pub repo_url: String,
    /// The operator's revision text — populated only when this
    /// invocation is a revision of a prior changelog PR; empty for the
    /// first stylist run.
    pub revision_text: String,
}

/// Context handed to `Executor::run_brownfield_draft`. Built by the
/// polling iteration's brownfield handler from the operator's request
/// AND the workspace's surface (README, docs filenames, code-symbol
/// overview). The `rendered_prompt` field holds the final prompt after
/// the polling layer has substituted these inputs into the resolved
/// template (embedded default OR `features.brownfield.prompt_path`
/// override).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrownfieldDraftContext {
    /// Canonical capability slug (matches `^[a-z][a-z0-9-]*$`). Used
    /// to derive the change directory name AND the spec path.
    pub capability_name: String,
    /// Fully rendered prompt: template + interpolated context. The
    /// executor passes this verbatim to the wrapped CLI.
    pub rendered_prompt: String,
}

/// Context handed to `Executor::run_scout` (a25). The polling layer
/// renders the scout prompt template AND passes the result here. The
/// executor backend should run the wrapped CLI under a read-only
/// sandbox (Read, Glob, Grep, Bash including `gh`) AND return the
/// model's final answer as `ExecutorOutcome::Completed { final_answer }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoutContext {
    /// Fully rendered prompt: template + interpolated context.
    pub rendered_prompt: String,
}

/// Context handed to `Executor::run_issue` (a009). The issues walker
/// renders the issue-flavored implementer prompt (template + the issue's
/// `issue.md` + `tasks.md` body) AND passes the result here. The slug
/// keys the per-run log + the MCP outcome store (so the agent's
/// `outcome_*` tool calls are consumed for this issue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueContext {
    /// The issue's directory slug under `openspec/issues/`.
    pub slug: String,
    /// Fully rendered issue-flavored prompt: template + issue body.
    pub rendered_prompt: String,
}

/// Context handed to `Executor::run_issue_triage` (a010). The hybrid
/// ingestion layer renders the `prompts/issue-report-triage.md` template
/// (with the reported body embedded as untrusted DATA via single-pass
/// substitution) AND passes the result here. The executor runs it
/// read-only AND returns the agent's final answer; the ingestion layer
/// parses the `CLASSIFICATION:` verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueReportTriageContext {
    /// Fully rendered triage prompt: template + interpolated context.
    pub rendered_prompt: String,
}

/// Context handed to `Executor::run_chat_triage`. Plumbed in from the
/// chat-driven `propose` flow. Carried verbatim through the
/// `prompts/chat-request-triage.md` template's `{{...}}` substitutions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTriageContext {
    /// The operator's free-form request text (trimmed, capped at 10,000
    /// chars by the parser). Internal whitespace + line breaks are
    /// preserved.
    pub request_text: String,
    /// The repository URL the request targets (for the prompt's
    /// context-banner line, not for git operations).
    pub repo_url: String,
    /// A brief listing of which canonical specs live in
    /// `openspec/specs/`. The triage prompt instructs the LLM to read
    /// the relevant subset before deciding how to act on the directive.
    pub canonical_specs_index: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutorOutcome {
    /// The underlying agent reported successful completion of the change.
    /// autocoder decides what to do with a no-diff `Completed`. The
    /// optional `final_answer` carries the agent's conversational
    /// summary captured from the JSON-event stream's terminal `result`
    /// event; `None` when streaming-JSON mode is off (legacy text mode)
    /// OR when no `result` event was emitted before the child exited.
    Completed {
        #[doc(hidden)]
        final_answer: Option<String>,
    },
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
    /// a74: the agentic run could not START because a required precondition
    /// was unmet — the agent subprocess never spawned. The motivating case is
    /// the a006 OS-sandbox-mechanism gate refusing to spawn when no usable
    /// mechanism is available AND the operator has not opted into unsandboxed
    /// operation. DISTINCT from `Failed` (where the subprocess ran and THEN
    /// the task failed): no revision/implementation work was attempted. The
    /// distinction is carried by THIS variant (the outcome kind), NOT a
    /// message substring, so callers branch reliably. The revise dispatcher
    /// posts a guiding failure reply AND consumes the trigger (manual
    /// re-trigger — an unmet precondition will not heal between polls) but does
    /// NOT charge a revision slot.
    PreconditionUnmet { reason: String },
    /// The agent inspected `tasks.md` and identified one or more tasks
    /// that require capabilities outside its sandbox. autocoder writes
    /// a `.needs-spec-revision.json` marker, posts a chatops alert under
    /// `AlertCategory::SpecNeedsRevision`, and halts the queue walk. The
    /// change is excluded from future `list_pending` calls until the
    /// operator deletes the marker.
    SpecNeedsRevision {
        unimplementable_tasks: Vec<UnimplementableTask>,
        revision_suggestion: String,
    },
    /// The agent completed some tasks but wants another iteration to
    /// finish the rest. autocoder commits + force-pushes the WIP to the
    /// agent branch, writes `.iteration-pending.json` with the cumulative
    /// state, drops `.in-progress`, and continues polling — the next
    /// iteration on this repo picks up the iteration-pending change with
    /// a continuation block prepended to its prompt. The `iteration_number`
    /// is the upcoming iteration's number (computed by the classifier as
    /// `prior_iteration_number + 1`); the cap is 5 (a 6th request is
    /// overridden to `Failed`).
    IterationRequested {
        completed_tasks: Vec<String>,
        remaining_tasks: Vec<String>,
        reason: String,
        iteration_number: u32,
    },
    /// Subprocess killed by signal during operator-initiated daemon
    /// shutdown; should NOT count against `consecutive_failures`. The
    /// daemon's SIGTERM cascaded to the executor's process group, the
    /// wrapped CLI child was killed by SIGTERM (the reaped status reports
    /// `signal() == Some(15)`; the "128 + 15 = 143" exit-code form is
    /// accepted defensively), AND the process-wide `SHUTDOWN_REQUESTED`
    /// flag was true at classification time. The polling loop drops
    /// `.in-progress`, leaves `.iteration-pending.json` untouched, AND
    /// skips the failure-counter + perma-stuck + chatops-alert paths.
    Aborted { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnimplementableTask {
    /// Task identifier from tasks.md, e.g. "5.2" or "13.1".
    pub task_id: String,
    /// The literal task text, quoted from tasks.md for the alert body.
    pub task_text: String,
    /// One-line reason the task is outside the agent's sandbox.
    pub reason: String,
}

impl PartialEq for ResumeHandle {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// Opaque payload passed between `run` and `resume`. JSON-serializable so
/// autocoder can persist it across daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeHandle(pub serde_json::Value);

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 1.2: `ExecutorOutcome::Aborted` round-trips through Debug
    /// AND PartialEq match arms cleanly. The structural shape is
    /// `Aborted { reason: String }`, matching `Failed`'s shape — this
    /// test pins the variant's structure AND ensures any future enum
    /// changes that break it light up.
    #[test]
    fn aborted_variant_round_trips_through_debug_and_match() {
        let v = ExecutorOutcome::Aborted {
            reason: "daemon shutdown (SIGTERM cascade)".to_string(),
        };
        // Debug round-trip — produces a parseable shape.
        let dbg = format!("{v:?}");
        assert!(
            dbg.contains("Aborted") && dbg.contains("daemon shutdown"),
            "Debug must surface variant name AND reason: {dbg}"
        );
        // PartialEq round-trip — Aborted equals itself but not other
        // variants carrying the same reason text.
        let other = ExecutorOutcome::Aborted {
            reason: "daemon shutdown (SIGTERM cascade)".to_string(),
        };
        assert_eq!(v, other, "Aborted must compare equal by reason");
        let failed = ExecutorOutcome::Failed {
            reason: "daemon shutdown (SIGTERM cascade)".to_string(),
        };
        assert_ne!(
            v, failed,
            "Aborted must NOT equal Failed even when reason matches — distinct variants"
        );
        // Pattern-match arms compile AND extract `reason` cleanly.
        let reason = match v {
            ExecutorOutcome::Aborted { reason } => reason,
            other => panic!("expected Aborted; got {other:?}"),
        };
        assert_eq!(reason, "daemon shutdown (SIGTERM cascade)");
    }
}
