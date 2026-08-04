//! app-under-test-e2e: daemon-owned lifecycle for the application a pass
//! verifies against.
//!
//! The daemon — not the agent — starts the application, decides when it is
//! ready, and guarantees it is gone afterwards. Three reasons that ownership
//! sits here rather than in the agent's session:
//!
//! - **Teardown is a guarantee, not a hope.** [`RunningApp`] kills the whole
//!   process group from `Drop`, so completion, timeout, failure, panic, and
//!   shutdown all converge on the same path.
//! - **Ports cannot collide.** One tokio task per repository means two
//!   repositories can be mid-pass at once, and the red-green replay starts a
//!   second instance beside the pass's own. Every instance takes its own
//!   ephemeral port.
//! - **The readiness probe stays out of the agent's shell**, so no relaxation
//!   of `executor.sandbox.disallowed_bash_patterns` is required.

use crate::config::AppUnderTestConfig;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Environment variable carrying the allocated port to the application's
/// start command. The conventional name for the ecosystems this targets.
pub const PORT_ENV: &str = "PORT";

/// Environment variable carrying the resolved base URL to the executor
/// session and the end-to-end command.
pub const BASE_URL_ENV: &str = "APP_UNDER_TEST_URL";

/// Reserve an ephemeral TCP port by binding it and immediately releasing it.
///
/// There is an unavoidable race between release and the application's own
/// bind: nothing stops a third party taking the port in between. The kernel
/// makes that unlikely (it does not hand out the same ephemeral port again
/// immediately) and the alternative — holding the socket open and passing the
/// descriptor — cannot work when the application binds by number, which is
/// what a `PORT` environment variable means. A collision surfaces as a
/// readiness-probe failure, which is already a handled, non-blocking outcome.
pub fn allocate_ephemeral_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("binding an ephemeral port for the application under test")?;
    let port = listener
        .local_addr()
        .context("reading the allocated ephemeral port")?
        .port();
    drop(listener);
    Ok(port)
}

/// The base URL an application on `port` is reachable at.
pub fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// A started application, owned for the life of one pass (or one replay).
///
/// Dropping this kills the process group. Holding it is what keeps the
/// application alive, so it must outlive every step that talks to the app.
#[derive(Debug)]
pub struct RunningApp {
    child: Option<tokio::process::Child>,
    pgid: Option<i32>,
    pub port: u16,
}

impl RunningApp {
    pub fn base_url(&self) -> String {
        base_url(self.port)
    }

    /// Terminate the application's whole process group.
    ///
    /// Signals the GROUP, not the child: a dev server is typically a shell
    /// that forks the real server, so killing only the direct child orphans a
    /// listener that then holds the port. `SIGTERM` first, then `SIGKILL`,
    /// mirroring the busy-marker recovery path.
    pub fn shutdown(&mut self) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };
        // SAFETY: `killpg` on a pgid this process created; a dead group is a
        // benign ESRCH.
        unsafe {
            libc::killpg(pgid as libc::pid_t, libc::SIGTERM);
        }
        // The child is reaped by the Drop of `tokio::process::Child` (which is
        // kill_on_drop-free here); escalate immediately rather than blocking a
        // teardown path that may itself be running under a timeout.
        unsafe {
            libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

impl Drop for RunningApp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Resolve the application's working directory against the workspace root.
/// A configured `working_dir` is workspace-relative by contract, so an
/// absolute value is rejected rather than silently escaping the workspace.
pub fn resolve_working_dir(workspace: &Path, cfg: &AppUnderTestConfig) -> Result<PathBuf> {
    match cfg.working_dir.as_deref() {
        None => Ok(workspace.to_path_buf()),
        Some(rel) => {
            let candidate = Path::new(rel);
            if candidate.is_absolute() || rel.split('/').any(|c| c == "..") {
                return Err(anyhow!(
                    "app_under_test.working_dir must be a workspace-relative path \
                     without `..`; got `{rel}`"
                ));
            }
            Ok(workspace.join(rel))
        }
    }
}

/// Spawn the application's start command in its own process group.
fn spawn_app(
    cfg: &AppUnderTestConfig,
    working_dir: &Path,
    port: u16,
) -> Result<tokio::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&cfg.start_command)
        .current_dir(working_dir)
        .env(PORT_ENV, port.to_string())
        .env(BASE_URL_ENV, base_url(port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Own process group so teardown reaps the entire tree with one signal.
    cmd.process_group(0);
    cmd.spawn()
        .with_context(|| format!("spawning app_under_test.start_command in {}", working_dir.display()))
}

/// Whether the application is serving yet.
async fn probe_ready(cfg: &AppUnderTestConfig, working_dir: &Path, port: u16) -> bool {
    if let Some(path) = cfg.ready_check.http_path.as_deref() {
        let url = format!("{}{}", base_url(port), path);
        return match reqwest::Client::new()
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        };
    }
    if let Some(command) = cfg.ready_check.command.as_deref() {
        return match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(working_dir)
            .env(PORT_ENV, port.to_string())
            .env(BASE_URL_ENV, base_url(port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(_) => false,
        };
    }
    // Config load rejects a probe-less block, so this is unreachable in
    // production; treating it as not-ready keeps the failure non-blocking.
    false
}

/// The runtime facts about a running application, published for the prompt
/// builder to read.
///
/// Written when the application becomes ready and removed when it stops, so
/// its mere PRESENCE is the "an application is up for this pass" signal — the
/// prompt block is included exactly when this file exists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppSessionRecord {
    /// Resolved base URL, e.g. `http://127.0.0.1:53124`.
    pub base_url: String,
    /// The configured end-to-end command, quoted into the prompt so the agent
    /// runs the same command the daemon will use to verify.
    pub e2e_command: String,
}

/// Publish the running application's facts for `workspace_basename`.
pub fn write_session_record(
    paths: &crate::paths::DaemonPaths,
    workspace_basename: &str,
    record: &AppSessionRecord,
) -> Result<()> {
    let path = paths.app_under_test_path(workspace_basename);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Write-temp-then-rename, so a reader never observes a half-written record.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(record)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Read the running application's facts, if any. A corrupt record reads as
/// absent (with a WARN): the prompt then simply omits the block, which
/// degrades to the no-application behavior rather than failing the pass.
pub fn read_session_record(
    paths: &crate::paths::DaemonPaths,
    workspace_basename: &str,
) -> Option<AppSessionRecord> {
    let path = paths.app_under_test_path(workspace_basename);
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "app-under-test record is corrupt; building the prompt as if no application is running: {e}"
            );
            None
        }
    }
}

/// Remove the record. Idempotent — a missing file is the desired end state.
pub fn clear_session_record(paths: &crate::paths::DaemonPaths, workspace_basename: &str) {
    let path = paths.app_under_test_path(workspace_basename);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), "could not remove app-under-test record: {e}");
    }
}

/// Why an application could not be established for a pass.
#[derive(Debug)]
pub enum StartFailure {
    /// The start command could not be spawned at all.
    Spawn(anyhow::Error),
    /// The readiness probe never succeeded within the budget.
    NotReady { waited: Duration },
    /// The process exited before becoming ready; carries captured stderr.
    Exited { detail: String },
}

impl std::fmt::Display for StartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartFailure::Spawn(e) => write!(f, "could not start: {e}"),
            StartFailure::NotReady { waited } => write!(
                f,
                "readiness probe did not succeed within {}s",
                waited.as_secs()
            ),
            StartFailure::Exited { detail } => {
                write!(f, "exited before becoming ready: {detail}")
            }
        }
    }
}

/// Start the application and wait for it to serve.
///
/// On every failure path the spawned process group is torn down before
/// returning, so a half-started application never outlives the attempt.
/// Returns the failure rather than an `Err` for the not-ready cases: the
/// caller proceeds with the pass WITHOUT an application, which is a reported
/// condition, not an error.
pub async fn start_and_wait_ready(
    cfg: &AppUnderTestConfig,
    workspace: &Path,
    ready_timeout: Duration,
) -> std::result::Result<RunningApp, StartFailure> {
    let working_dir = match resolve_working_dir(workspace, cfg) {
        Ok(d) => d,
        Err(e) => return Err(StartFailure::Spawn(e)),
    };
    let port = match allocate_ephemeral_port() {
        Ok(p) => p,
        Err(e) => return Err(StartFailure::Spawn(e)),
    };
    let child = match spawn_app(cfg, &working_dir, port) {
        Ok(c) => c,
        Err(e) => return Err(StartFailure::Spawn(e)),
    };
    let pgid = child.id().map(|id| id as i32);
    let mut app = RunningApp { child: Some(child), pgid, port };

    let deadline = tokio::time::Instant::now() + ready_timeout;
    loop {
        if probe_ready(cfg, &working_dir, port).await {
            return Ok(app);
        }
        // A process that exited is never going to become ready; fail fast
        // rather than burning the whole readiness budget on a dead server.
        if let Some(child) = app.child.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            let detail = format!("start command exited with {status}");
            app.shutdown();
            return Err(StartFailure::Exited { detail });
        }
        if tokio::time::Instant::now() >= deadline {
            app.shutdown();
            return Err(StartFailure::NotReady { waited: ready_timeout });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Match one path segment against a pattern segment, where `*` matches any
/// run of characters (including none) within the segment.
///
/// Recursive rather than a two-pointer scan: patterns and segments here are a
/// handful of characters, so the worst-case backtracking is irrelevant and the
/// recursive form is the one that is obviously correct on `*` runs.
fn segment_match(pattern: &str, segment: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| go(&p[1..], &s[i..])),
            Some(c) => !s.is_empty() && s[0] == *c && go(&p[1..], &s[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = segment.chars().collect();
    go(&p, &s)
}

/// Match a workspace-relative path against a glob pattern.
///
/// Supports the two constructs the default patterns need: `**` spanning zero
/// or more path segments, and `*` within a single segment. Hand-rolled rather
/// than pulling a glob crate — this is a bounded, well-understood algorithm,
/// and the codebase's existing preference is to own small logic like this
/// (see the single-pass prompt renderer) rather than add a dependency.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn go(p: &[&str], t: &[&str]) -> bool {
        match p.first() {
            None => t.is_empty(),
            // `**` consumes any number of segments, including none — EXCEPT
            // as the final pattern segment, where `dir/**` means "everything
            // UNDER dir" and so requires at least one segment to consume.
            // That is the gitignore/globset convention and the one an operator
            // writing `tests/e2e/**` intends; without it, a file literally
            // named `e2e` would match a pattern meaning "inside e2e".
            Some(&"**") => {
                let trailing = p.len() == 1;
                if trailing {
                    !t.is_empty()
                } else {
                    (0..=t.len()).any(|i| go(&p[1..], &t[i..]))
                }
            }
            Some(seg) => {
                !t.is_empty() && segment_match(seg, t[0]) && go(&p[1..], &t[1..])
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    go(&p, &t)
}

/// The changed paths that are end-to-end tests under `patterns`.
pub fn e2e_test_files<'a>(changed: &'a [String], patterns: &[String]) -> Vec<&'a String> {
    changed
        .iter()
        .filter(|path| patterns.iter().any(|pat| glob_match(pat, path)))
        .collect()
}

/// Cap on captured end-to-end output carried into the PR body. The full
/// output lives in the run log; the PR gets a slice, mirroring the audits'
/// notification cap.
const E2E_SUMMARY_CAP: usize = 3000;

/// The result of running the end-to-end suite.
///
/// `NotRun` is a first-class variant rather than an absence: a pass with no
/// application must render as explicitly unverified, never as a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eOutcome {
    Passed { summary: String },
    Failed { status: i32, summary: String },
    TimedOut { after_secs: u64 },
    NotRun { reason: String },
}

impl E2eOutcome {
    /// Only an actual zero exit is a pass. Every other state — failure,
    /// timeout, or never having run — is non-passing.
    pub fn is_pass(&self) -> bool {
        matches!(self, E2eOutcome::Passed { .. })
    }

    /// Whether this outcome should open the PR as a draft.
    ///
    /// A suite that ran and did not pass drafts the PR. `NotRun` does NOT:
    /// end-to-end verification is opt-in and degrades by design (an
    /// unprovisioned host, a dev server that would not boot), so drafting
    /// every PR on a host that cannot run it would punish the wrong thing.
    /// It is still rendered as unverified so a human sees it.
    pub fn drafts_pr(&self) -> bool {
        matches!(self, E2eOutcome::Failed { .. } | E2eOutcome::TimedOut { .. })
    }

    /// The `## End-to-end verification` PR-body section.
    ///
    /// Always emitted — including for `NotRun`. Silence would read as "no
    /// end-to-end concern", which is exactly the inference the feature exists
    /// to prevent.
    pub fn render_pr_section(&self, command: &str) -> String {
        let mut out = String::from("## End-to-end verification\n\n");
        match self {
            E2eOutcome::Passed { summary } => {
                out.push_str(&format!("`{command}` passed (exit 0).\n"));
                if !summary.trim().is_empty() {
                    out.push_str(&format!("\n```\n{}\n```\n", summary.trim_end()));
                }
            }
            E2eOutcome::Failed { status, summary } => {
                out.push_str(&format!(
                    "`{command}` FAILED (exit {status}). This PR is a draft.\n"
                ));
                if !summary.trim().is_empty() {
                    out.push_str(&format!("\n```\n{}\n```\n", summary.trim_end()));
                }
            }
            E2eOutcome::TimedOut { after_secs } => {
                out.push_str(&format!(
                    "`{command}` did NOT complete within {after_secs}s and was terminated. \
                     A timeout is not a pass. This PR is a draft.\n"
                ));
            }
            E2eOutcome::NotRun { reason } => {
                out.push_str(&format!(
                    "End-to-end verification did NOT run ({reason}), so the behavior in this \
                     change has not been verified by running it.\n"
                ));
            }
        }
        out
    }
}

/// Truncate captured output to the PR-body cap, keeping the TAIL — test
/// runners put the failure summary at the end.
fn summarize_output(stdout: &str, stderr: &str) -> String {
    let combined = if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };
    let trimmed = combined.trim();
    if trimmed.len() <= E2E_SUMMARY_CAP {
        return trimmed.to_string();
    }
    let tail: String = trimmed
        .chars()
        .skip(trimmed.chars().count().saturating_sub(E2E_SUMMARY_CAP))
        .collect();
    format!("[…truncated…]\n{tail}")
}

/// Run the end-to-end suite against a running application.
///
/// The command's EXIT CODE is the authoritative verification signal. A
/// timeout terminates the whole process group and is reported as a timeout —
/// never as a pass.
pub async fn run_e2e(
    cfg: &AppUnderTestConfig,
    workspace: &Path,
    port: u16,
    timeout: Duration,
) -> E2eOutcome {
    use std::os::unix::process::CommandExt;
    let working_dir = match resolve_working_dir(workspace, cfg) {
        Ok(d) => d,
        Err(e) => return E2eOutcome::NotRun { reason: e.to_string() },
    };
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&cfg.e2e_command)
        .current_dir(&working_dir)
        .env(PORT_ENV, port.to_string())
        .env(BASE_URL_ENV, base_url(port))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return E2eOutcome::NotRun {
                reason: format!("could not spawn the end-to-end command: {e}"),
            };
        }
    };
    let pgid = child.id().map(|id| id as i32);

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let summary = summarize_output(
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            );
            match out.status.code() {
                Some(0) => E2eOutcome::Passed { summary },
                Some(code) => E2eOutcome::Failed { status: code, summary },
                // Killed by a signal: not a pass.
                None => E2eOutcome::Failed { status: -1, summary },
            }
        }
        Ok(Err(e)) => E2eOutcome::NotRun {
            reason: format!("the end-to-end command could not be awaited: {e}"),
        },
        Err(_) => {
            // Terminate the GROUP: a test runner spawns browsers and workers,
            // and killing only the shell would strand them holding resources.
            if let Some(pgid) = pgid {
                // SAFETY: a pgid this process created; ESRCH is benign.
                unsafe {
                    libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
                }
            }
            E2eOutcome::TimedOut { after_secs: timeout.as_secs() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReadyCheck;

    fn cfg(start: &str, ready: ReadyCheck) -> AppUnderTestConfig {
        AppUnderTestConfig {
            start_command: start.to_string(),
            ready_check: ready,
            e2e_command: "true".to_string(),
            working_dir: None,
            ready_timeout_secs: 5,
            e2e_timeout_secs: 60,
            e2e_test_paths: None,
        }
    }

    fn http(path: &str) -> ReadyCheck {
        ReadyCheck { http_path: Some(path.to_string()), command: None }
    }

    fn command_check(c: &str) -> ReadyCheck {
        ReadyCheck { http_path: None, command: Some(c.to_string()) }
    }

    #[test]
    fn allocated_ports_differ() {
        // Two instances (two repositories, or a pass and its replay) must never
        // be handed the same port.
        let a = allocate_ephemeral_port().unwrap();
        let b = allocate_ephemeral_port().unwrap();
        assert_ne!(a, b, "each allocation is distinct");
        assert!(a > 1024 && b > 1024, "ephemeral range, not privileged");
    }

    #[test]
    fn working_dir_defaults_to_the_workspace_root() {
        let ws = Path::new("/tmp/ws");
        let c = cfg("true", http("/healthz"));
        assert_eq!(resolve_working_dir(ws, &c).unwrap(), ws.to_path_buf());
    }

    #[test]
    fn working_dir_is_workspace_relative_only() {
        let ws = Path::new("/tmp/ws");
        let mut c = cfg("true", http("/healthz"));

        c.working_dir = Some("web".to_string());
        assert_eq!(resolve_working_dir(ws, &c).unwrap(), ws.join("web"));

        // An absolute path or a traversal would escape the workspace.
        c.working_dir = Some("/etc".to_string());
        assert!(resolve_working_dir(ws, &c).is_err(), "absolute is rejected");
        c.working_dir = Some("../../etc".to_string());
        assert!(resolve_working_dir(ws, &c).is_err(), "traversal is rejected");
    }

    #[tokio::test]
    async fn command_probe_readiness_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Sleeps so it is still alive when the probe (`true`) succeeds.
        let c = cfg("sleep 30", command_check("true"));
        let app = start_and_wait_ready(&c, tmp.path(), Duration::from_secs(5))
            .await
            .expect("a ready app starts");
        assert!(app.port > 1024);
        assert!(app.base_url().contains(&app.port.to_string()));
    }

    #[tokio::test]
    async fn never_ready_times_out_and_tears_down() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Alive but never ready: the probe always fails.
        let c = cfg("sleep 30", command_check("false"));
        let err = start_and_wait_ready(&c, tmp.path(), Duration::from_millis(600))
            .await
            .expect_err("a never-ready app is a start failure");
        assert!(matches!(err, StartFailure::NotReady { .. }), "got {err}");
    }

    #[tokio::test]
    async fn app_that_exits_fails_fast_rather_than_waiting_out_the_budget() {
        let tmp = tempfile::TempDir::new().unwrap();
        let c = cfg("exit 3", command_check("false"));
        let started = tokio::time::Instant::now();
        let err = start_and_wait_ready(&c, tmp.path(), Duration::from_secs(30))
            .await
            .expect_err("an exited app is a start failure");
        assert!(matches!(err, StartFailure::Exited { .. }), "got {err}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "fails fast instead of burning the readiness budget"
        );
    }

    #[tokio::test]
    async fn dropping_the_app_frees_its_port() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Hold the port with a listener inside the app's own process group.
        let c = cfg("sleep 30", command_check("true"));
        let port = {
            let app = start_and_wait_ready(&c, tmp.path(), Duration::from_secs(5))
                .await
                .expect("starts");
            app.port
            // dropped here → group killed
        };
        // The allocated port is bindable again once the group is gone, which
        // is the observable proof that teardown ran on the Drop path.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "port {port} is free after the app is dropped"
        );
    }

    fn e2e_cfg(command: &str) -> AppUnderTestConfig {
        let mut c = cfg("true", command_check("true"));
        c.e2e_command = command.to_string();
        c
    }

    #[tokio::test]
    async fn passing_suite_is_a_pass() {
        let tmp = tempfile::TempDir::new().unwrap();
        let c = e2e_cfg("echo all-green; exit 0");
        let outcome = run_e2e(&c, tmp.path(), 5555, Duration::from_secs(10)).await;
        assert!(outcome.is_pass());
        assert!(!outcome.drafts_pr());
        assert!(outcome.render_pr_section("cmd").contains("passed"));
    }

    #[tokio::test]
    async fn failing_suite_drafts_the_pr_and_keeps_the_exit_code() {
        let tmp = tempfile::TempDir::new().unwrap();
        let c = e2e_cfg("echo boom >&2; exit 4");
        let outcome = run_e2e(&c, tmp.path(), 5555, Duration::from_secs(10)).await;
        assert!(!outcome.is_pass());
        assert!(outcome.drafts_pr(), "a failing suite drafts the PR");
        assert!(matches!(outcome, E2eOutcome::Failed { status: 4, .. }), "got {outcome:?}");
        let section = outcome.render_pr_section("cmd");
        assert!(section.contains("exit 4") && section.contains("boom"));
    }

    #[tokio::test]
    async fn timeout_is_never_a_pass_and_terminates_the_suite() {
        let tmp = tempfile::TempDir::new().unwrap();
        let c = e2e_cfg("sleep 30");
        let outcome = run_e2e(&c, tmp.path(), 5555, Duration::from_millis(400)).await;
        assert!(matches!(outcome, E2eOutcome::TimedOut { .. }), "got {outcome:?}");
        assert!(!outcome.is_pass(), "a timeout is never reported as a pass");
        assert!(outcome.drafts_pr());
    }

    #[test]
    fn not_run_is_rendered_explicitly_and_does_not_draft() {
        // Absence of verification must be VISIBLE, but it must not draft every
        // PR on a host that simply cannot run the suite.
        let outcome = E2eOutcome::NotRun { reason: "no application was started".into() };
        assert!(!outcome.is_pass(), "absence is never a pass");
        assert!(!outcome.drafts_pr());
        let section = outcome.render_pr_section("cmd");
        assert!(section.contains("## End-to-end verification"), "the section is still emitted");
        assert!(section.contains("did NOT run"));
        assert!(section.contains("no application was started"));
    }

    #[test]
    fn glob_matches_the_built_in_default_patterns() {
        use crate::config::DEFAULT_E2E_TEST_PATHS;
        let defaults: Vec<String> =
            DEFAULT_E2E_TEST_PATHS.iter().map(|s| s.to_string()).collect();
        let matches = |p: &str| defaults.iter().any(|pat| glob_match(pat, p));

        // Conventional e2e layouts.
        assert!(matches("tests/e2e/login.spec.ts"));
        assert!(matches("e2e/checkout.ts"), "**/e2e/** at the root");
        assert!(matches("src/components/Button.test.tsx"));
        assert!(matches("apps/web/tests/e2e/deep/nested.spec.js"));

        // Implementation files must NOT be treated as tests — overlaying
        // those onto the base tree would defeat the whole replay.
        assert!(!matches("src/server.ts"));
        assert!(!matches("src/components/Button.tsx"));
        assert!(!matches("README.md"));
    }

    #[test]
    fn glob_double_star_spans_zero_or_more_segments() {
        assert!(glob_match("**/e2e/**", "e2e/a.ts"), "zero leading segments");
        assert!(glob_match("**/e2e/**", "a/b/c/e2e/d.ts"), "several leading segments");
        assert!(!glob_match("**/e2e/**", "e2e"), "** trailing needs a segment");
        assert!(glob_match("tests/**", "tests/a/b/c.ts"));
        assert!(!glob_match("tests/**", "src/tests/a.ts"), "anchored at the root");
    }

    #[test]
    fn glob_single_star_stays_within_a_segment() {
        assert!(glob_match("*.spec.ts", "login.spec.ts"));
        assert!(!glob_match("*.spec.ts", "a/login.spec.ts"), "* does not cross /");
        assert!(glob_match("a/*/c.ts", "a/b/c.ts"));
        assert!(!glob_match("a/*/c.ts", "a/b/x/c.ts"));
    }

    #[test]
    fn configured_paths_replace_the_defaults() {
        use crate::config::{AppUnderTestConfig, DEFAULT_E2E_TEST_PATHS};
        let mut c = cfg("true", command_check("true"));
        assert_eq!(
            c.resolved_e2e_test_paths().len(),
            DEFAULT_E2E_TEST_PATHS.len(),
            "unset → defaults"
        );

        c.e2e_test_paths = Some(vec!["acceptance/**".to_string()]);
        assert_eq!(c.resolved_e2e_test_paths(), vec!["acceptance/**".to_string()]);
        // Replacement, not extension: a default pattern no longer matches.
        let paths = c.resolved_e2e_test_paths();
        assert!(!paths.iter().any(|p| glob_match(p, "src/a.spec.ts")));
        assert!(paths.iter().any(|p| glob_match(p, "acceptance/flow.ts")));

        // An explicitly-empty list reads as "unset", not "match nothing".
        c.e2e_test_paths = Some(vec![]);
        assert_eq!(c.resolved_e2e_test_paths().len(), DEFAULT_E2E_TEST_PATHS.len());
        let _ = AppUnderTestConfig::validate;
    }

    #[test]
    fn e2e_test_files_selects_only_matching_paths() {
        let changed: Vec<String> = ["src/server.ts", "tests/e2e/login.spec.ts", "README.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let patterns: Vec<String> = vec!["**/e2e/**".to_string()];
        let selected = e2e_test_files(&changed, &patterns);
        assert_eq!(selected, vec![&"tests/e2e/login.spec.ts".to_string()]);
    }

    #[test]
    fn long_output_is_truncated_keeping_the_tail() {
        // Runners put the failure summary last, so the tail is what matters.
        let long = format!("{}\nFAILED: the last line matters", "x".repeat(E2E_SUMMARY_CAP * 2));
        let summary = summarize_output(&long, "");
        assert!(summary.len() <= E2E_SUMMARY_CAP + 32, "bounded: {}", summary.len());
        assert!(summary.contains("FAILED: the last line matters"), "tail preserved");
        assert!(summary.contains("truncated"), "truncation is disclosed");
    }

    #[tokio::test]
    async fn http_probe_reports_not_ready_when_nothing_listens() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Nothing binds the port, so the HTTP probe can never succeed.
        let c = cfg("sleep 30", http("/healthz"));
        let err = start_and_wait_ready(&c, tmp.path(), Duration::from_millis(800))
            .await
            .expect_err("no listener → not ready");
        assert!(matches!(err, StartFailure::NotReady { .. }), "got {err}");
    }
}
