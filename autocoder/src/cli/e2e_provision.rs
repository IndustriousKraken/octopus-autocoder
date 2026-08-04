//! app-under-test-e2e: provisioning of the end-to-end browser runtime.
//!
//! Two tiers, only one of which needs root:
//!
//! 1. **System packages** — the shared libraries the browser links against.
//!    Requires elevated privileges, which is why this runs from the install
//!    wizard (where root is already expected) and never from a pass or from
//!    daemon startup.
//! 2. **Browser binaries** — a user-space download, placed in the
//!    daemon-owned location ([`crate::paths::DaemonPaths::e2e_browsers_dir`])
//!    rather than the service account's default user cache, so the runtime
//!    resolves identically for the daemon, the executor session, and the
//!    end-to-end command.
//!
//! The repository's own test-runner dependency is deliberately NOT provisioned
//! here: it belongs to the target repository's manifest and its working
//! sessions.
//!
//! Both steps delegate to the browser tool's own dependency resolver rather
//! than a package list carried in this repository. That is a deliberate
//! trade, measured on Ubuntu 24.04: the resolver installs ~45 packages where
//! `ldd` shows only [`MINIMAL_SYSTEM_LIBS`] (10) are strictly required to
//! launch. The resolver wins anyway because package names are
//! distro-VERSION-specific (noble's 64-bit `time_t` transition renames
//! `libatk1.0-0` to `libatk1.0-0t64`), which the installer's
//! `OsPackageDep::pkg_name` table — keyed by package MANAGER — structurally
//! cannot express. Roughly a third of the extra weight is fonts, which
//! materially improve rendering fidelity for screenshot-based debugging.

use super::install::{SystemActions, WizardIo};
use anyhow::Result;
use std::path::Path;

/// The strictly-required shared-library packages on Ubuntu 24.04, derived by
/// running `ldd` against the downloaded browser binary and verified by an
/// actual headless launch. NOT used for provisioning (see the module docs on
/// why the vendor resolver is preferred) — it is the documented lean set for
/// an operator provisioning a constrained host by hand.
pub const MINIMAL_SYSTEM_LIBS: &[&str] = &[
    "libnss3",
    "libnspr4",
    "libatk1.0-0t64",
    "libatk-bridge2.0-0t64",
    "libatspi2.0-0t64",
    "libgbm1",
    "libxcomposite1",
    "libxdamage1",
    "libxfixes3",
    "libxrandr2",
];

/// Environment variable the browser tool reads to place AND resolve its
/// binaries. Pinning it is the whole point of the env-carrying install action:
/// dropping it silently would install to the user cache and leave a
/// "successful" provision the daemon cannot find.
pub const BROWSERS_PATH_ENV: &str = "PLAYWRIGHT_BROWSERS_PATH";

/// What one provisioning step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The step ran and succeeded.
    Installed,
    /// The operator declined this step.
    Declined,
    /// The step could not run here; carries the manual remediation.
    Unavailable { reason: String, manual: String },
    /// The step ran and failed; carries the failure detail.
    Failed { detail: String },
}

impl StepOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, StepOutcome::Installed)
    }
}

/// Result of the whole section, so the caller can summarize without
/// re-deriving anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eProvisionReport {
    pub system_deps: StepOutcome,
    pub browsers: StepOutcome,
}

impl E2eProvisionReport {
    /// True only when BOTH tiers are in place — the browser runtime is
    /// unusable if either is missing.
    pub fn fully_provisioned(&self) -> bool {
        self.system_deps.is_ok() && self.browsers.is_ok()
    }

    /// Operator-facing summary naming anything that did not get provisioned
    /// AND how to do it by hand.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (label, step) in [
            ("browser system packages", &self.system_deps),
            ("browser binaries", &self.browsers),
        ] {
            match step {
                StepOutcome::Installed => out.push_str(&format!("  {label}: installed\n")),
                StepOutcome::Declined => out.push_str(&format!(
                    "  {label}: declined — end-to-end verification stays disabled\n"
                )),
                StepOutcome::Unavailable { reason, manual } => out.push_str(&format!(
                    "  {label}: NOT provisioned ({reason})\n    Provision manually with: {manual}\n"
                )),
                StepOutcome::Failed { detail } => out.push_str(&format!(
                    "  {label}: FAILED ({detail})\n"
                )),
            }
        }
        out
    }
}

/// Extract the operator-useful part of a failed command's stderr.
///
/// Deliberately the LAST non-empty lines, not the first: the tools involved
/// emit progress and dependency notices to stderr before doing any work, so a
/// first-line heuristic reports something like "npm warn exec: the following
/// package will be installed" as though it were the cause. Observed on a real
/// provisioning run; the actual failure is always at the tail.
fn failure_detail(status: i32, stderr: &str) -> String {
    let tail: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("npm warn") && !l.starts_with("npm notice"))
        .collect();
    let msg = tail
        .iter()
        .rev()
        .take(2)
        .rev()
        .copied()
        .collect::<Vec<&str>>()
        .join(" | ");
    if msg.is_empty() {
        format!("exit {status} (no diagnostic output)")
    } else {
        format!("exit {status}: {msg}")
    }
}

/// Confirm `dir` is actually reachable AND readable *as the service account*,
/// by attempting it rather than reasoning about mode bits.
///
/// Chowning the leaf directory proves nothing about the path to it: every
/// ancestor must also be traversable. Rather than re-implement that walk (and
/// the owner/group/other precedence that goes with it), this performs the real
/// access as the target user — the same thing the daemon will do.
async fn verify_readable_as(
    actions: &dyn SystemActions,
    dir: &Path,
    user: &str,
) -> StepOutcome {
    let probe = format!("test -r {d} && test -x {d}", d = dir.display());
    match actions
        .run_install_command("su", &["-s", "/bin/sh", "-c", &probe, user])
        .await
    {
        Ok(out) if out.status == 0 => StepOutcome::Installed,
        Ok(_) => StepOutcome::Failed {
            detail: format!(
                "browsers installed to {} but the `{user}` account cannot read it — \
                 check that every parent directory is traversable by `{user}` \
                 (a cache_dir beneath a private home directory is the usual cause)",
                dir.display()
            ),
        },
        // The probe itself could not run; report it rather than claiming
        // either outcome.
        Err(e) => StepOutcome::Failed {
            detail: format!(
                "browsers installed to {} but readability as `{user}` could not be \
                 verified: {e}",
                dir.display()
            ),
        },
    }
}

/// The manual command for the system-package tier, shown when the automated
/// path is unavailable.
fn manual_system_deps_hint() -> String {
    format!(
        "npx playwright install-deps chromium  (or, minimally: apt-get install -y {})",
        MINIMAL_SYSTEM_LIBS.join(" ")
    )
}

/// The manual command for the browser-binary tier.
fn manual_browsers_hint(browsers_dir: &Path) -> String {
    format!(
        "{BROWSERS_PATH_ENV}={} npx playwright install chromium",
        browsers_dir.display()
    )
}

/// Provision the end-to-end browser runtime, with a consent step per tier.
///
/// Never aborts the caller: a declined, unavailable, or failed step is
/// reported in the returned [`E2eProvisionReport`] and the installation
/// continues. End-to-end verification is opt-in per repository, so a host that
/// cannot provision it must still complete its installation — the preflight
/// warns per declaring repository at startup.
///
/// `service_account` (when the daemon runs as a system user) receives
/// ownership of the browser directory, since the download runs privileged but
/// the daemon reads it unprivileged.
pub async fn provision_e2e_toolchain(
    io: &mut dyn WizardIo,
    actions: &dyn SystemActions,
    browsers_dir: &Path,
    service_account: Option<&str>,
    non_interactive: bool,
) -> Result<E2eProvisionReport> {
    io.print(
        "\nEnd-to-end testing (optional)\n\
         Installs the browser runtime so autocoder can verify a change by RUNNING\n\
         the application, not only by reading the diff. Required for any repository\n\
         configured with an `app_under_test` block.\n",
    );

    // `npx` is the entry point for both tiers. Without it neither can run, and
    // that is a report-and-continue condition, not an error: the operator may
    // simply not want this feature on this host.
    if actions.which("npx").await.is_none() {
        let reason = "`npx` not found on PATH (install Node.js)".to_string();
        io.print(&format!("  Skipping: {reason}\n"));
        return Ok(E2eProvisionReport {
            system_deps: StepOutcome::Unavailable {
                reason: reason.clone(),
                manual: manual_system_deps_hint(),
            },
            browsers: StepOutcome::Unavailable {
                reason,
                manual: manual_browsers_hint(browsers_dir),
            },
        });
    }

    // ----- Tier 1: privileged system packages -----
    let deps_cmd = "npx playwright install-deps chromium";
    io.print(&format!(
        "\n  Step 1/2 — browser system packages.\n    Command: {deps_cmd}  (requires elevated privileges)\n"
    ));
    let consent = if non_interactive {
        true
    } else {
        io.confirm("Install the browser system packages now?", true).await?
    };
    let system_deps = if !consent {
        StepOutcome::Declined
    } else {
        match actions
            .run_install_command("npx", &["playwright", "install-deps", "chromium"])
            .await
        {
            Ok(out) if out.status == 0 => StepOutcome::Installed,
            Ok(out) => StepOutcome::Failed {
                detail: failure_detail(out.status, &out.stderr),
            },
            Err(e) => StepOutcome::Failed { detail: e.to_string() },
        }
    };

    // ----- Tier 2: browser binaries into the daemon-owned path -----
    // Attempted even when tier 1 did not complete: the download is
    // independent, and a partially-provisioned host is more useful (and more
    // legible in the report) than one that stopped at the first problem.
    let browsers_display = browsers_dir.display().to_string();
    io.print(&format!(
        "\n  Step 2/2 — browser binaries.\n    Command: {}\n",
        manual_browsers_hint(browsers_dir)
    ));
    let consent = if non_interactive {
        true
    } else {
        io.confirm("Download the browser binaries now?", true).await?
    };
    let browsers = if !consent {
        StepOutcome::Declined
    } else {
        // Re-running is idempotent: an already-downloaded revision is left
        // alone by the tool itself.
        match actions
            .run_install_command_env(
                "npx",
                &["playwright", "install", "chromium"],
                &[(BROWSERS_PATH_ENV, browsers_display.as_str())],
            )
            .await
        {
            Ok(out) if out.status == 0 => {
                // The download ran privileged; the daemon reads it as the
                // service account.
                if let Some(user) = service_account {
                    actions.chown(browsers_dir, user, user).await?;
                    // VERIFY rather than assume. Chowning the leaf says
                    // nothing about whether the service account can traverse
                    // to it: an operator who points `paths.cache_dir` beneath
                    // a restrictive parent (a home directory, say) gets a
                    // provision that reports success and a daemon that cannot
                    // read a byte of it. Observed on a real run.
                    verify_readable_as(actions, browsers_dir, user).await
                } else {
                    StepOutcome::Installed
                }
            }
            Ok(out) => StepOutcome::Failed {
                detail: failure_detail(out.status, &out.stderr),
            },
            Err(e) => StepOutcome::Failed { detail: e.to_string() },
        }
    };

    let report = E2eProvisionReport { system_deps, browsers };
    io.print(&format!("\n  End-to-end provisioning summary:\n{}", report.render()));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::install::{RecordedCall, RecordingActions};
    use std::path::PathBuf;

    /// Minimal scripted IO: canned confirm answers, captured output.
    struct FakeIo {
        answers: Vec<bool>,
        out: String,
    }

    impl FakeIo {
        fn new(answers: Vec<bool>) -> Self {
            Self { answers, out: String::new() }
        }
    }

    #[async_trait::async_trait]
    impl WizardIo for FakeIo {
        async fn read_line(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn read_password(&mut self) -> Result<String> {
            Ok(String::new())
        }
        fn print(&mut self, s: &str) {
            self.out.push_str(s);
        }
        async fn confirm(&mut self, _prompt: &str, default: bool) -> Result<bool> {
            Ok(if self.answers.is_empty() {
                default
            } else {
                self.answers.remove(0)
            })
        }
        async fn choose(
            &mut self,
            _prompt: &str,
            _options: &[&str],
            default_idx: usize,
        ) -> Result<usize> {
            Ok(default_idx)
        }
    }

    fn browsers_dir() -> PathBuf {
        PathBuf::from("/var/cache/autocoder/e2e-browsers")
    }

    fn subprocess_calls(actions: &RecordingActions) -> Vec<(String, Vec<String>, Vec<(String, String)>)> {
        actions
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                RecordedCall::RunSubprocess { cmd, args, env } => Some((cmd, args, env)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn provisions_both_tiers_and_pins_the_browsers_path() {
        let actions = RecordingActions::new()
            .with_which("npx", Some(PathBuf::from("/usr/bin/npx")));
        let mut io = FakeIo::new(vec![true, true]);
        let report = provision_e2e_toolchain(
            &mut io,
            &actions,
            &browsers_dir(),
            Some("autocoder"),
            false,
        )
        .await
        .unwrap();

        assert!(report.fully_provisioned());
        let calls = subprocess_calls(&actions);
        let installs: Vec<_> = calls.iter().filter(|(cmd, _, _)| cmd == "npx").collect();
        assert_eq!(installs.len(), 2, "one install per tier: {calls:?}");

        // Tier 1 runs WITHOUT the browsers-path env (it installs system libs).
        assert_eq!(installs[0].1, vec!["playwright", "install-deps", "chromium"]);
        assert!(installs[0].2.is_empty());

        // Tier 2 MUST carry the pinned path, else the browsers land in the
        // service account's user cache where the daemon will not find them.
        assert_eq!(installs[1].1, vec!["playwright", "install", "chromium"]);
        assert_eq!(
            installs[1].2,
            vec![(
                BROWSERS_PATH_ENV.to_string(),
                browsers_dir().display().to_string()
            )]
        );

        // The privileged download is handed to the unprivileged reader.
        assert!(
            actions.calls().iter().any(|c| matches!(
                c,
                RecordedCall::Chown { path, owner, .. }
                    if path == &browsers_dir() && owner == "autocoder"
            )),
            "browser dir is chowned to the service account: {:?}",
            actions.calls()
        );
        // ...AND the handoff is verified, not assumed.
        assert!(
            calls.iter().any(|(cmd, args, _)| cmd == "su"
                && args.iter().any(|a| a.contains("test -r"))
                && args.contains(&"autocoder".to_string())),
            "readability is probed as the service account: {calls:?}"
        );
    }

    /// A chown that "succeeds" while the path stays unreachable must not be
    /// reported as provisioned — the daemon would find nothing there.
    #[tokio::test]
    async fn unreadable_browser_dir_is_not_reported_as_provisioned() {
        let actions = RecordingActions::new()
            .with_which("npx", Some(PathBuf::from("/usr/bin/npx")))
            // Every command reports failure, including the readability probe.
            .with_install_status(1);
        let mut io = FakeIo::new(vec![]);
        let report = provision_e2e_toolchain(
            &mut io,
            &actions,
            &browsers_dir(),
            Some("autocoder"),
            true,
        )
        .await
        .unwrap();
        assert!(!report.fully_provisioned());
        assert!(matches!(report.browsers, StepOutcome::Failed { .. }));
    }

    #[tokio::test]
    async fn missing_npx_reports_manual_commands_without_installing() {
        // `npx` absent → neither tier can run.
        let actions = RecordingActions::new().with_which("npx", None);
        let mut io = FakeIo::new(vec![]);
        let report =
            provision_e2e_toolchain(&mut io, &actions, &browsers_dir(), None, false)
                .await
                .unwrap();

        assert!(!report.fully_provisioned());
        assert!(subprocess_calls(&actions).is_empty(), "nothing is installed");
        // The operator is told how to do it by hand, including the lean set.
        let rendered = report.render();
        assert!(rendered.contains("install-deps"), "got: {rendered}");
        assert!(rendered.contains("libnss3"), "names the minimal set: {rendered}");
    }

    #[tokio::test]
    async fn declining_a_tier_installs_nothing_for_it() {
        let actions = RecordingActions::new()
            .with_which("npx", Some(PathBuf::from("/usr/bin/npx")));
        // Decline tier 1, accept tier 2.
        let mut io = FakeIo::new(vec![false, true]);
        let report =
            provision_e2e_toolchain(&mut io, &actions, &browsers_dir(), None, false)
                .await
                .unwrap();

        assert_eq!(report.system_deps, StepOutcome::Declined);
        assert!(report.browsers.is_ok());
        assert!(!report.fully_provisioned(), "one tier missing → not usable");
        let calls = subprocess_calls(&actions);
        assert_eq!(calls.len(), 1, "only the accepted tier ran: {calls:?}");
        assert_eq!(calls[0].1, vec!["playwright", "install", "chromium"]);
    }

    #[tokio::test]
    async fn non_interactive_consents_to_both_tiers() {
        let actions = RecordingActions::new()
            .with_which("npx", Some(PathBuf::from("/usr/bin/npx")));
        let mut io = FakeIo::new(vec![]);
        let report =
            provision_e2e_toolchain(&mut io, &actions, &browsers_dir(), None, true)
                .await
                .unwrap();
        assert!(report.fully_provisioned());
        assert_eq!(subprocess_calls(&actions).len(), 2);
        // The commands are still SHOWN even when consent is implied by the flag.
        assert!(io.out.contains("install-deps"), "got: {}", io.out);
    }

    /// Regression for a real provisioning run on Ubuntu 24.04: the tool wrote
    /// npm notices to stderr BEFORE the real error, so a first-line heuristic
    /// reported "the following package will be installed" as the cause.
    #[test]
    fn failure_detail_reports_the_cause_not_the_leading_npm_notice() {
        let stderr = "npm warn exec The following package was not found and will be installed: playwright@1.62.1\n\
                      \n\
                      Installing dependencies...\n\
                      E: Could not open lock file /var/lib/dpkg/lock-frontend - open (13: Permission denied)\n";
        let detail = failure_detail(1, stderr);
        assert!(detail.contains("Permission denied"), "names the real cause: {detail}");
        assert!(
            !detail.contains("npm warn"),
            "the leading npm notice is not the diagnostic: {detail}"
        );
    }

    #[test]
    fn failure_detail_survives_empty_stderr() {
        let detail = failure_detail(2, "\n  \n");
        assert!(detail.contains("exit 2"), "still names the status: {detail}");
        assert!(detail.contains("no diagnostic"), "says so plainly: {detail}");
    }

    #[tokio::test]
    async fn a_failed_tier_is_reported_not_fatal() {
        // `chown` is only reached on a successful download, so a failing
        // install must not panic or abort — it reports.
        let actions = RecordingActions::new()
            .with_which("npx", Some(PathBuf::from("/usr/bin/npx")))
            .with_install_status(1);
        let mut io = FakeIo::new(vec![]);
        let report =
            provision_e2e_toolchain(&mut io, &actions, &browsers_dir(), None, true)
                .await
                .expect("a failing step is reported, never an Err");
        assert!(matches!(report.system_deps, StepOutcome::Failed { .. }));
        assert!(matches!(report.browsers, StepOutcome::Failed { .. }));
        assert!(report.render().contains("FAILED"));
    }
}
