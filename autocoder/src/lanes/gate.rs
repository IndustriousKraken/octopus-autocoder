//! Task-local feature gate for the issues lane (a009).
//!
//! The issues lane is gated by `features.issues`, off by default. Rather
//! than thread a boolean through the deep polling-loop call chain, the
//! lane follows the established verifier-gate pattern
//! (`crate::preflight::canon_contradiction`, `crate::code_implements_spec`):
//! a `tokio::task_local!` context, set ONCE at the top of each polling
//! task's future by the daemon when `features.issues.enabled`, AND read
//! by the polling pass via [`current`]. A task that never called [`scope`]
//! — every test that does not opt in, AND the default-off operator — sees
//! `None`, so `issues/<slug>/` directories are simply not worked.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

/// Process-scoped context for the issues lane. Its presence (`Some`) is
/// the on-switch; its fields carry the lane's resolved configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssuesLaneContext {
    /// Optional override path for the issue-flavored implementer prompt
    /// (`features.issues.prompt_path`). `None` → embedded default
    /// (`prompts/implementer-issue.md`).
    pub prompt_path: Option<PathBuf>,
    /// Whether the hybrid PUBLIC ingestion path (a010) is active for this
    /// task: the bot triages reported GitHub issues read-only AND posts
    /// candidates to chatops. Gated behind the existing scout issue-read
    /// opt-in (`features.scout.include_issues`); `false` → only the curated
    /// (a009) path is active. Default `false`.
    pub ingest: bool,
}

tokio::task_local! {
    /// Per-task issues-lane context. `None` represents the disabled
    /// state; the polling pass's [`current`] reader returns `None` AND
    /// the lane is a no-op. Production callers (one per polling task)
    /// wrap the top-level future once at startup.
    static CTX: Option<Arc<IssuesLaneContext>>;
}

/// Run `fut` with the given issues-lane context bound for the duration of
/// the future. `None` represents the disabled state.
pub fn scope<F>(ctx: Option<Arc<IssuesLaneContext>>, fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    CTX.scope(ctx, fut)
}

/// Snapshot of the current task's issues-lane context. `None` when the
/// operator did not opt in OR the surrounding task did not call [`scope`].
/// Cheap clone of an `Arc`.
pub fn current() -> Option<Arc<IssuesLaneContext>> {
    CTX.try_with(|c| c.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_is_none_without_scope() {
        // No `scope` wrapping → the lane is inactive.
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_is_none_when_scoped_none() {
        let seen = scope(None, async { current() }).await;
        assert!(seen.is_none(), "scoped None must read back as None");
    }

    #[tokio::test]
    async fn current_returns_scoped_context() {
        let ctx = Arc::new(IssuesLaneContext {
            prompt_path: Some(PathBuf::from("prompts/custom-issue.md")),
            ingest: true,
        });
        let seen = scope(Some(ctx.clone()), async { current() }).await;
        let seen = seen.expect("scoped Some must read back as Some");
        assert_eq!(
            seen.prompt_path.as_deref(),
            Some(std::path::Path::new("prompts/custom-issue.md"))
        );
        assert!(seen.ingest, "scoped ingest flag must read back");
    }
}
