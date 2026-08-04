//! app-under-test-e2e: the pass's end-to-end verification step.
//!
//! Bridges the daemon-owned application lifecycle, the suite run, and the
//! red-green replay into the two things `execute_one_pass` needs: a PR-body
//! section and a draft decision.
//!
//! Every failure here is REPORTED, never fatal. End-to-end verification is
//! opt-in per repository and degrades by design — an unprovisioned host or a
//! dev server that will not boot must not hold the queue or discard committed
//! work. What it must never do is let an unverified change look verified, so
//! the section is emitted in every case, including the ones where nothing ran.

use crate::app_under_test::{self, AppSessionRecord, RunningApp};
use crate::config::RepositoryConfig;
use crate::e2e_replay::{self, E2eVerification};
use crate::paths::DaemonPaths;
use std::path::Path;
use std::time::Duration;

/// The application started for a pass, plus why it was not started when it
/// was not. Held for the whole pass: dropping it tears the process group down.
pub(crate) struct PassApp {
    pub(crate) app: Option<RunningApp>,
    /// Populated only when an application was configured but could not be
    /// established, so the PR can say what happened rather than staying silent.
    pub(crate) failure: Option<String>,
}

impl PassApp {
    fn none(failure: Option<String>) -> Self {
        Self { app: None, failure }
    }
}

/// Clamp a configured timeout. The operator-facing WARN for a zero value
/// already fired at config load, so this is the silent floor.
fn secs(configured: u64) -> Duration {
    Duration::from_secs(configured.max(1))
}

/// Start the application for this pass, when the repository declares one.
///
/// Called BEFORE the executor so the implementer prompt can carry the
/// "Application under test" block; the returned value must be held for the
/// rest of the pass.
pub(crate) async fn start_app_for_pass(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
) -> PassApp {
    let Some(cfg) = repo.app_under_test.as_ref() else {
        // Not configured: no application, no record, no section. This
        // repository behaves exactly as it did before the feature existed.
        return PassApp::none(None);
    };
    let basename = crate::workspace::basename(workspace);
    // Clear any record left by an interrupted previous pass BEFORE starting,
    // so a stale one can never make the prompt advertise a dead application.
    app_under_test::clear_session_record(paths, basename);

    match app_under_test::start_and_wait_ready(cfg, workspace, secs(cfg.ready_timeout_secs)).await {
        Ok(app) => {
            let record = AppSessionRecord {
                base_url: app.base_url(),
                e2e_command: cfg.e2e_command.clone(),
            };
            if let Err(e) = app_under_test::write_session_record(paths, basename, &record) {
                // The application is up but the prompt cannot learn about it.
                // Tear it down rather than run a pass whose agent believes no
                // application exists while one holds a port.
                tracing::warn!(url = %repo.url, "could not publish the app-under-test record: {e:#}");
                drop(app);
                return PassApp::none(Some(format!(
                    "the application started but its session record could not be written: {e}"
                )));
            }
            tracing::info!(
                url = %repo.url,
                base_url = %record.base_url,
                "application under test is ready for this pass"
            );
            PassApp { app: Some(app), failure: None }
        }
        Err(e) => {
            // A readiness failure never holds the queue: the pass proceeds
            // without an application and the PR says verification did not run.
            tracing::warn!(
                url = %repo.url,
                "application under test did not become ready; proceeding without it: {e}"
            );
            PassApp::none(Some(e.to_string()))
        }
    }
}

/// Run the suite and the replay for this pass.
///
/// `base` is the commit the agent branch was recreated from. Returns the
/// PR-body section and whether the PR must open as a draft.
pub(crate) async fn verify_pass(
    paths: &DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    pass_app: &PassApp,
    base: Option<&str>,
) -> Option<E2eVerification> {
    let cfg = repo.app_under_test.as_ref()?;

    let Some(app) = pass_app.app.as_ref() else {
        let reason = pass_app
            .failure
            .clone()
            .unwrap_or_else(|| "no application was started for this pass".to_string());
        return Some(E2eVerification::not_run(reason));
    };
    let Some(base) = base else {
        // Without the base commit the replay cannot run; the suite still can,
        // but reporting a suite result while silently skipping the vacuous-test
        // check would overstate what was verified.
        return Some(E2eVerification::not_run(
            "the pass's base commit could not be resolved, so the suite and replay were skipped",
        ));
    };

    let scratch_root = paths.cache.join("e2e-replay");
    Some(
        e2e_replay::run_e2e_verification(
            cfg,
            workspace,
            app.port,
            base,
            &scratch_root,
            secs(cfg.ready_timeout_secs),
            secs(cfg.e2e_timeout_secs),
        )
        .await,
    )
}

/// Tear down the pass's application and remove its published record.
///
/// Idempotent, and safe to call when nothing was started. Dropping `PassApp`
/// alone kills the process group; this additionally clears the record so a
/// later pass cannot read a stale one.
pub(crate) fn finish_pass_app(paths: &DaemonPaths, workspace: &Path, pass_app: PassApp) {
    drop(pass_app);
    app_under_test::clear_session_record(paths, crate::workspace::basename(workspace));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppUnderTestConfig, ReadyCheck};

    fn repo_with(app: Option<AppUnderTestConfig>) -> RepositoryConfig {
        let mut r: RepositoryConfig = serde_yml::from_str(
            "url: \"git@github.com:o/r.git\"\nbase_branch: main\nagent_branch: agent-q\npoll_interval_sec: 300\n",
        )
        .unwrap();
        r.app_under_test = app;
        r
    }

    fn app_cfg(start: &str, ready: &str) -> AppUnderTestConfig {
        AppUnderTestConfig {
            start_command: start.to_string(),
            ready_check: ReadyCheck { http_path: None, command: Some(ready.to_string()) },
            e2e_command: "true".to_string(),
            working_dir: None,
            ready_timeout_secs: 2,
            e2e_timeout_secs: 30,
            e2e_test_paths: None,
        }
    }

    #[tokio::test]
    async fn unconfigured_repo_starts_nothing_and_reports_nothing() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with(None);

        let pass_app = start_app_for_pass(&paths, tmp.path(), &repo).await;
        assert!(pass_app.app.is_none());
        assert!(pass_app.failure.is_none(), "not configured is not a failure");
        // No section at all: the PR looks exactly as it did before the feature.
        assert!(verify_pass(&paths, tmp.path(), &repo, &pass_app, Some("abc")).await.is_none());
        // And no record was published for the prompt builder to find.
        assert!(
            app_under_test::read_session_record(&paths, crate::workspace::basename(tmp.path()))
                .is_none()
        );
    }

    #[tokio::test]
    async fn ready_app_publishes_a_record_and_clears_it_on_finish() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with(Some(app_cfg("sleep 30", "true")));
        let basename = crate::workspace::basename(tmp.path());

        let pass_app = start_app_for_pass(&paths, tmp.path(), &repo).await;
        assert!(pass_app.app.is_some(), "a ready app is established");
        let record = app_under_test::read_session_record(&paths, basename)
            .expect("record published for the prompt builder");
        assert!(record.base_url.starts_with("http://127.0.0.1:"));
        assert_eq!(record.e2e_command, "true");

        finish_pass_app(&paths, tmp.path(), pass_app);
        assert!(
            app_under_test::read_session_record(&paths, basename).is_none(),
            "the record is cleared so a later pass cannot read a stale one"
        );
    }

    #[tokio::test]
    async fn app_that_never_becomes_ready_reports_but_does_not_hold_the_pass() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with(Some(app_cfg("sleep 30", "false")));

        let pass_app = start_app_for_pass(&paths, tmp.path(), &repo).await;
        assert!(pass_app.app.is_none());
        assert!(pass_app.failure.is_some(), "the reason is carried for the PR");
        // No record → the implementer prompt omits the block.
        assert!(
            app_under_test::read_session_record(&paths, crate::workspace::basename(tmp.path()))
                .is_none()
        );

        let v = verify_pass(&paths, tmp.path(), &repo, &pass_app, Some("abc"))
            .await
            .expect("a configured repo always reports");
        assert!(!v.drafts_pr, "an unstartable app must not draft every PR");
        assert!(v.section.contains("did NOT run"), "but it is visibly unverified");
    }

    #[tokio::test]
    async fn a_stale_record_is_cleared_before_the_pass_starts() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let tmp = tempfile::TempDir::new().unwrap();
        let basename = crate::workspace::basename(tmp.path());
        // Simulate a record left by a pass that was killed mid-flight.
        app_under_test::write_session_record(
            &paths,
            basename,
            &AppSessionRecord {
                base_url: "http://127.0.0.1:1".into(),
                e2e_command: "stale".into(),
            },
        )
        .unwrap();

        let repo = repo_with(Some(app_cfg("sleep 30", "false"))); // never ready
        let pass_app = start_app_for_pass(&paths, tmp.path(), &repo).await;
        assert!(pass_app.app.is_none());
        assert!(
            app_under_test::read_session_record(&paths, basename).is_none(),
            "the stale record must not survive to advertise a dead application"
        );
    }

    #[tokio::test]
    async fn missing_base_commit_reports_rather_than_overstating() {
        let (_td, paths) = crate::testing::test_daemon_paths();
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with(Some(app_cfg("sleep 30", "true")));
        let pass_app = start_app_for_pass(&paths, tmp.path(), &repo).await;

        let v = verify_pass(&paths, tmp.path(), &repo, &pass_app, None)
            .await
            .expect("reports");
        assert!(!v.drafts_pr);
        assert!(v.section.contains("did NOT run"));
        finish_pass_app(&paths, tmp.path(), pass_app);
    }
}
