//! a011: comprehensive dependency preflight + the `autocoder doctor` report.
//!
//! One pass checks every REQUIRED dependency (`openspec`, `git`, a usable
//! platform sandbox mechanism) AND every dependency implied by the active
//! configuration (the agent-CLI binary for each configured strategy, a
//! forge/scout CLI when those features are on, an embedding backend when RAG
//! is on). All results are collected before reporting — the preflight never
//! stops at the first failure. The same report drives daemon startup (a
//! blocking dependency aborts startup with an actionable message) and the
//! on-demand `autocoder doctor` subcommand.
//!
//! Environment access is funnelled through the [`DepProbe`] trait so the
//! report-building logic is pure and unit-testable: production wires
//! [`RealProbe`] (PATH lookups + the `sandbox` module's usability probes);
//! tests inject a fake that returns canned facts.

use crate::config::{CliKind, Config, ReviewerKind};
use anyhow::{Result, anyhow};

/// Install hint for the openspec CLI. Mirrors the message the daemon has
/// historically printed so the canonical "openspec preflight" contract holds.
pub const OPENSPEC_MISSING_MSG: &str =
    "openspec preflight failed: `openspec` binary not found on PATH. \
     Install openspec and ensure the systemd unit's PATH covers its install directory.";

const GIT_HINT: &str =
    "Install git via your platform package manager (e.g. `apt-get install git`, \
     `brew install git`).";

const SANDBOX_HINT: &str =
    "Install a usable sandbox mechanism. On Linux: bubblewrap \
     (`apt-get install bubblewrap`) with unprivileged user namespaces enabled, \
     OR run the daemon under systemd so transient `systemd-run` service mode is \
     available. On macOS: `sandbox-exec` (ships with the OS).";

/// Status of one checked dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepStatus {
    /// Present AND usable. `detail` is an optional version/endpoint note.
    Satisfied { detail: Option<String> },
    /// Not present at all.
    Missing,
    /// Present but cannot be used (e.g. `bwrap` without user namespaces, or
    /// `openspec --version` exiting non-zero). `reason` explains why.
    Unusable { reason: String },
}

impl DepStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, DepStatus::Satisfied { .. })
    }
}

/// How important a dependency is to the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Importance {
    /// Missing/unusable aborts startup AND makes `doctor` exit non-zero.
    /// Used for `openspec`, `git`, a usable sandbox mechanism, AND the
    /// executor's agent CLI (without which the daemon cannot do any work).
    Required,
    /// Implied by an active configuration option. Reported AND warned when
    /// missing, never fatal — the daemon degrades (e.g. the agentic reviewer
    /// falls back to its HTTP path) or the feature simply produces nothing
    /// that run. A check is only added when its feature is on, so this
    /// honours "fatal only when the feature is active" as an upper bound:
    /// these never abort startup, so they can never abort it while inactive.
    Configured,
}

/// One dependency's full result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepCheck {
    pub name: String,
    pub importance: Importance,
    pub status: DepStatus,
    pub install_hint: String,
}

impl DepCheck {
    pub fn is_ok(&self) -> bool {
        self.status.is_ok()
    }

    /// A check blocks startup (and fails `doctor`) when it is `Required` AND
    /// not satisfied.
    pub fn is_blocking(&self) -> bool {
        matches!(self.importance, Importance::Required) && !self.is_ok()
    }

    /// A non-blocking problem worth a WARN (a configuration-implied
    /// dependency that is missing/unusable).
    pub fn is_warning(&self) -> bool {
        !self.is_ok() && !self.is_blocking()
    }

    fn status_tag(&self) -> &'static str {
        match self.status {
            DepStatus::Satisfied { .. } => "ok",
            DepStatus::Missing => "MISSING",
            DepStatus::Unusable { .. } => "UNUSABLE",
        }
    }
}

/// The full preflight report — one entry per dependency, in check order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepReport {
    pub checks: Vec<DepCheck>,
}

impl DepReport {
    /// True when at least one `Required` dependency is missing/unusable.
    pub fn has_blocking(&self) -> bool {
        self.checks.iter().any(|c| c.is_blocking())
    }

    pub fn blocking(&self) -> impl Iterator<Item = &DepCheck> {
        self.checks.iter().filter(|c| c.is_blocking())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &DepCheck> {
        self.checks.iter().filter(|c| c.is_warning())
    }

    /// A human-readable, multi-line report (used by `autocoder doctor` AND
    /// logged at startup). Required and configuration-implied checks are
    /// grouped so an operator sees the whole picture at once.
    pub fn render(&self) -> String {
        let mut out = String::from("autocoder dependency report\n");
        self.render_group(&mut out, "REQUIRED", true);
        self.render_group(&mut out, "CONFIGURATION-IMPLIED", false);

        out.push('\n');
        let blocking = self.blocking().count();
        let warnings = self.warnings().count();
        if blocking > 0 {
            out.push_str(&format!(
                "Summary: {blocking} required dependency(ies) missing or unusable; \
                 {warnings} warning(s).\n"
            ));
        } else if warnings > 0 {
            out.push_str(&format!(
                "Summary: all required dependencies satisfied; {warnings} warning(s).\n"
            ));
        } else {
            out.push_str("Summary: all dependencies satisfied.\n");
        }
        out
    }

    /// Append one report group (REQUIRED vs CONFIGURATION-IMPLIED) to `out`.
    fn render_group(&self, out: &mut String, title: &str, want_required: bool) {
        let group: Vec<&DepCheck> = self
            .checks
            .iter()
            .filter(|c| matches!(c.importance, Importance::Required) == want_required)
            .collect();
        if group.is_empty() {
            return;
        }
        out.push('\n');
        out.push_str(title);
        out.push('\n');
        for c in group {
            out.push_str(&format!("  [{:<8}] {}", c.status_tag(), c.name));
            match &c.status {
                DepStatus::Satisfied { detail: Some(d) } => out.push_str(&format!("  ({d})")),
                DepStatus::Satisfied { detail: None } => {}
                DepStatus::Unusable { reason } => {
                    out.push_str(&format!("\n              reason: {reason}"));
                    out.push_str(&format!("\n              fix:    {}", c.install_hint));
                }
                DepStatus::Missing => {
                    out.push_str(&format!("\n              fix:    {}", c.install_hint));
                }
            }
            out.push('\n');
        }
    }

    /// The actionable error message used to abort daemon startup, naming each
    /// blocking dependency AND how to install it.
    pub fn blocking_error_message(&self) -> String {
        let mut msg = String::from(
            "dependency preflight failed: required dependency(ies) missing or unusable:",
        );
        for c in self.blocking() {
            let detail = match &c.status {
                DepStatus::Unusable { reason } => format!(" (unusable: {reason})"),
                _ => String::new(),
            };
            msg.push_str(&format!("\n  - {}{}: {}", c.name, detail, c.install_hint));
        }
        msg
    }

    /// Log the whole report: INFO for each satisfied check, WARN for each
    /// configuration-implied problem. Blocking checks are surfaced via the
    /// returned error at the call site, not logged here.
    pub fn log(&self) {
        for c in &self.checks {
            match &c.status {
                DepStatus::Satisfied { detail } => tracing::info!(
                    dependency = %c.name,
                    detail = detail.as_deref().unwrap_or(""),
                    "dependency satisfied"
                ),
                _ if c.is_warning() => tracing::warn!(
                    dependency = %c.name,
                    status = c.status_tag(),
                    "configuration-implied dependency missing or unusable; {}",
                    c.install_hint
                ),
                _ => {} // blocking checks are reported by the startup error
            }
        }
    }
}

/// The process exit code `autocoder doctor` returns: non-zero when a required
/// dependency is missing/unusable.
pub fn doctor_exit_code(report: &DepReport) -> i32 {
    if report.has_blocking() { 1 } else { 0 }
}

/// Environment facts the report builder depends on. Real impl probes the host;
/// tests inject canned facts so the decision logic is exercised deterministically.
pub trait DepProbe {
    /// Status of the `openspec` CLI (`openspec --version`).
    fn openspec(&self) -> DepStatus;
    /// Status of the platform sandbox mechanism.
    fn sandbox(&self) -> crate::sandbox::SandboxAvailability;
    /// Whether a binary is resolvable (on PATH, or as an existing path).
    fn binary_present(&self, bin: &str) -> bool;
}

/// Production probe: PATH lookups + the `sandbox` module's usability probes.
pub struct RealProbe;

impl DepProbe for RealProbe {
    fn openspec(&self) -> DepStatus {
        probe_openspec("openspec")
    }
    fn sandbox(&self) -> crate::sandbox::SandboxAvailability {
        crate::sandbox::sandbox_availability()
    }
    fn binary_present(&self, bin: &str) -> bool {
        if bin.contains('/') {
            std::path::Path::new(bin).is_file()
        } else {
            crate::sandbox::which(bin)
        }
    }
}

/// Run `<bin> --version` and classify the outcome. Distinguishes a missing
/// binary (NotFound) from a present-but-broken one (non-zero exit), preserving
/// the historical openspec-preflight messages.
pub fn probe_openspec(bin: &str) -> DepStatus {
    match std::process::Command::new(bin).arg("--version").output() {
        Ok(out) if out.status.success() => DepStatus::Satisfied {
            detail: Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        },
        Ok(out) => {
            let stderr_tail: String =
                String::from_utf8_lossy(&out.stderr).chars().take(200).collect();
            DepStatus::Unusable {
                reason: format!(
                    "openspec preflight failed: `{bin} --version` exited {code:?}. stderr: {stderr_tail}",
                    code = out.status.code(),
                ),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DepStatus::Missing,
        Err(e) => DepStatus::Unusable {
            reason: format!("spawning `{bin} --version` errored: {e}"),
        },
    }
}

/// Install hint for an agent CLI by name. The agent CLIs install themselves
/// and have an interactive login, so the installer prints these rather than
/// running them (a011 task 2.3).
fn agent_cli_hint(bin: &str) -> String {
    match bin {
        "claude" => "Install the Claude Code CLI: `curl -fsSL https://claude.ai/install.sh | bash`, \
                     then authenticate once with `claude` (interactive login)."
            .to_string(),
        "opencode" => "Install opencode per https://opencode.ai (its own installer), \
                       then authenticate it for your provider."
            .to_string(),
        other => format!(
            "Install the `{other}` agent CLI and ensure it is on the daemon's PATH and authenticated."
        ),
    }
}

/// Build the comprehensive report from a config + a probe. Pure: every
/// environment fact comes from `probe`, so this is unit-testable.
pub fn build_report(cfg: &Config, probe: &dyn DepProbe) -> DepReport {
    let mut checks = Vec::new();

    // --- Required ----------------------------------------------------------
    checks.push(DepCheck {
        name: "openspec".to_string(),
        importance: Importance::Required,
        status: probe.openspec(),
        install_hint: OPENSPEC_MISSING_MSG.to_string(),
    });
    checks.push(DepCheck {
        name: "git".to_string(),
        importance: Importance::Required,
        status: if probe.binary_present("git") {
            DepStatus::Satisfied { detail: None }
        } else {
            DepStatus::Missing
        },
        install_hint: GIT_HINT.to_string(),
    });
    checks.push(sandbox_check(probe));

    // --- Configured agent-CLI strategies -----------------------------------
    for req in configured_cli_binaries(cfg) {
        let present = probe.binary_present(&req.bin);
        checks.push(DepCheck {
            name: format!("agent CLI `{}` ({})", req.bin, req.used_by.join(", ")),
            importance: if req.required {
                Importance::Required
            } else {
                Importance::Configured
            },
            status: if present {
                DepStatus::Satisfied { detail: None }
            } else {
                DepStatus::Missing
            },
            install_hint: agent_cli_hint(&req.bin),
        });
    }

    // --- Forge / scout CLI -------------------------------------------------
    // The forge layer talks to GitHub/GitLab over REST, but the scout feature
    // (default on) shells out to `gh`. Report it when scout is enabled.
    if cfg.features.scout.enabled {
        checks.push(DepCheck {
            name: "forge/scout CLI `gh` (scout)".to_string(),
            importance: Importance::Configured,
            status: if probe.binary_present("gh") {
                DepStatus::Satisfied { detail: None }
            } else {
                DepStatus::Missing
            },
            install_hint:
                "Install the GitHub CLI `gh` (https://cli.github.com) and authenticate \
                 with `gh auth login`, OR disable the scout feature (`features.scout.enabled: false`)."
                    .to_string(),
        });
    }

    // --- Embedding backend (RAG) -------------------------------------------
    if let Some(rag) = cfg.canonical_rag.as_ref().filter(|r| r.is_active()) {
        checks.push(rag_backend_check(rag, probe));
    }

    DepReport { checks }
}

fn sandbox_check(probe: &dyn DepProbe) -> DepCheck {
    use crate::sandbox::SandboxAvailability;
    let status = match probe.sandbox() {
        SandboxAvailability::Usable { mechanism } => DepStatus::Satisfied {
            detail: Some(mechanism.to_string()),
        },
        SandboxAvailability::PresentButUnusable { present } => DepStatus::Unusable {
            reason: format!(
                "{} present but cannot apply the sandbox (e.g. unprivileged user \
                 namespaces disabled)",
                present.join(", ")
            ),
        },
        SandboxAvailability::Absent => DepStatus::Missing,
    };
    DepCheck {
        name: "sandbox mechanism".to_string(),
        importance: Importance::Required,
        status,
        install_hint: SANDBOX_HINT.to_string(),
    }
}

fn rag_backend_check(
    rag: &crate::config::CanonicalRagConfig,
    probe: &dyn DepProbe,
) -> DepCheck {
    use crate::config::LlmProvider;
    let provider = rag.provider.unwrap_or(LlmProvider::Ollama);
    let (status, hint) = match provider {
        LlmProvider::Ollama => {
            // Ollama may be a local binary OR a remote/containerised endpoint,
            // so a missing `ollama` binary is a warning, not a hard failure.
            if probe.binary_present("ollama") {
                (
                    DepStatus::Satisfied {
                        detail: Some(format!("ollama, model `{}`", rag.model)),
                    },
                    String::new(),
                )
            } else {
                (
                    DepStatus::Missing,
                    format!(
                        "Install Ollama (https://ollama.com) and pull `{}`, OR ensure your \
                         Ollama endpoint at {} is reachable (the `ollama` CLI is optional if \
                         you run it as a container/remote).",
                        rag.model, rag.api_base_url
                    ),
                )
            }
        }
        LlmProvider::OpenAiCompatible => (
            // A remote HTTP endpoint — nothing local to probe.
            DepStatus::Satisfied {
                detail: Some(format!("openai-compatible endpoint {}", rag.api_base_url)),
            },
            String::new(),
        ),
        LlmProvider::Anthropic => (
            DepStatus::Unusable {
                reason: "anthropic does not support embeddings".to_string(),
            },
            "Set canonical_rag.provider to `ollama` or `openai_compatible`.".to_string(),
        ),
    };
    DepCheck {
        name: "embedding backend (RAG)".to_string(),
        importance: Importance::Configured,
        status,
        install_hint: hint,
    }
}

/// A distinct agent-CLI binary the config requires, with the role(s) that use
/// it. `required` is true when the binary is the executor's (indispensable).
struct CliRequirement {
    bin: String,
    required: bool,
    used_by: Vec<String>,
}

/// Enumerate the distinct agent-CLI binaries the *configured* roles use: the
/// executor command (always), the agentic reviewer command (when enabled), and
/// every `models:` registry entry's resolved CLI. Strategies that are not
/// configured are not included — so their CLIs are never required.
fn configured_cli_binaries(cfg: &Config) -> Vec<CliRequirement> {
    let mut reqs: Vec<CliRequirement> = Vec::new();
    let mut add = |bin: String, required: bool, used_by: String| {
        if let Some(existing) = reqs.iter_mut().find(|r| r.bin == bin) {
            existing.required = existing.required || required;
            if !existing.used_by.contains(&used_by) {
                existing.used_by.push(used_by);
            }
        } else {
            reqs.push(CliRequirement {
                bin,
                required,
                used_by: vec![used_by],
            });
        }
    };

    // The executor's CLI is indispensable — without it nothing runs.
    add(cfg.executor.command.clone(), true, "executor".to_string());

    // The agentic reviewer wraps its own CLI; it degrades to a oneshot HTTP
    // path when the CLI is absent, so it is configuration-implied, not required.
    if let Some(rev) = cfg.reviewer.as_ref()
        && rev.enabled
        && matches!(rev.kind, ReviewerKind::Agentic)
    {
        add(rev.command.clone(), false, "reviewer".to_string());
    }

    // Every model-registry entry resolves to a driving CLI.
    if let Some(models) = cfg.models.as_ref() {
        for (nick, entry) in models {
            let cli: CliKind = entry.resolved_cli();
            add(
                cli.as_str().to_string(),
                false,
                format!("model: {nick}"),
            );
        }
    }

    reqs
}

/// The comprehensive startup preflight: build the report against the real
/// host, log it, and abort startup (with an actionable, all-at-once message)
/// when a required dependency is missing or unusable. Extends the historical
/// openspec-only preflight to the full dependency set (a011 task 1.3).
pub fn run_startup_preflight(cfg: &Config) -> Result<()> {
    let report = build_report(cfg, &RealProbe);
    report.log();
    if report.has_blocking() {
        return Err(anyhow!(report.blocking_error_message()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxAvailability;

    /// A fully-controllable probe for the report-building logic.
    struct FakeProbe {
        openspec: DepStatus,
        sandbox: SandboxAvailability,
        present: Vec<String>,
    }

    impl FakeProbe {
        fn all_present() -> Self {
            Self {
                openspec: DepStatus::Satisfied { detail: Some("1.3.1".into()) },
                sandbox: SandboxAvailability::Usable { mechanism: "bwrap" },
                present: vec!["git".into(), "claude".into(), "gh".into(), "ollama".into()],
            }
        }
    }

    impl DepProbe for FakeProbe {
        fn openspec(&self) -> DepStatus {
            self.openspec.clone()
        }
        fn sandbox(&self) -> SandboxAvailability {
            self.sandbox.clone()
        }
        fn binary_present(&self, bin: &str) -> bool {
            self.present.iter().any(|b| b == bin)
        }
    }

    /// A config with the executor on `claude`, scout disabled, no RAG — the
    /// minimal surface for targeted dependency assertions.
    fn base_cfg() -> Config {
        const YAML: &str = r#"
repositories:
  - url: "git@github.com:owner/repo.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 300
executor:
  kind: claude_cli
  command: claude
  timeout_secs: 1800
github:
  token_env: GITHUB_TOKEN
"#;
        let mut cfg: Config = serde_yml::from_str(YAML).expect("minimal config parses");
        // scout defaults to on; turn it off so base_cfg checks only the core set.
        cfg.features.scout.enabled = false;
        cfg
    }

    fn find<'a>(r: &'a DepReport, needle: &str) -> Option<&'a DepCheck> {
        r.checks.iter().find(|c| c.name.contains(needle))
    }

    #[test]
    fn all_satisfied_has_no_blocking() {
        let report = build_report(&base_cfg(), &FakeProbe::all_present());
        assert!(!report.has_blocking(), "report: {}", report.render());
        assert_eq!(doctor_exit_code(&report), 0);
    }

    #[test]
    fn reports_multiple_missing_together_not_just_the_first() {
        // openspec missing AND git missing AND sandbox absent — all three must
        // appear in one report (do not stop at the first).
        let probe = FakeProbe {
            openspec: DepStatus::Missing,
            sandbox: SandboxAvailability::Absent,
            present: vec!["claude".into()],
        };
        let report = build_report(&base_cfg(), &probe);
        assert_eq!(find(&report, "openspec").unwrap().status, DepStatus::Missing);
        assert_eq!(find(&report, "git").unwrap().status, DepStatus::Missing);
        assert_eq!(find(&report, "sandbox").unwrap().status, DepStatus::Missing);
        // All three are blocking → reported together.
        assert_eq!(report.blocking().count(), 3, "{}", report.render());
    }

    #[test]
    fn missing_required_dependency_blocks_with_actionable_message() {
        let probe = FakeProbe {
            openspec: DepStatus::Missing,
            ..FakeProbe::all_present()
        };
        let report = build_report(&base_cfg(), &probe);
        assert!(report.has_blocking());
        assert_eq!(doctor_exit_code(&report), 1);
        let msg = report.blocking_error_message();
        // Names the dependency AND how to install it (the canonical openspec text).
        assert!(msg.contains("openspec"), "{msg}");
        assert!(msg.contains("not found on PATH"), "{msg}");
        assert!(msg.contains("Install openspec"), "{msg}");
    }

    #[test]
    fn configured_strategy_cli_is_checked_unconfigured_is_not() {
        // Executor configured on `opencode`; `opencode` is absent.
        let mut cfg = base_cfg();
        cfg.executor.command = "opencode".to_string();
        let probe = FakeProbe {
            present: vec!["git".into(), "claude".into()], // opencode absent, claude present
            ..FakeProbe::all_present()
        };
        let report = build_report(&cfg, &probe);
        // The configured strategy (opencode) is reported missing.
        let oc = find(&report, "opencode").expect("opencode strategy must be checked");
        assert_eq!(oc.status, DepStatus::Missing);
        // The unconfigured strategy (claude) is NOT a check of its own —
        // only the configured executor strategy appears.
        assert!(
            report.checks.iter().all(|c| !c.name.contains("`claude`")),
            "unconfigured claude must not be required: {}",
            report.render()
        );
    }

    #[test]
    fn present_but_unusable_sandbox_is_unusable_not_satisfied() {
        let probe = FakeProbe {
            sandbox: SandboxAvailability::PresentButUnusable { present: vec!["bwrap"] },
            ..FakeProbe::all_present()
        };
        let report = build_report(&base_cfg(), &probe);
        let sb = find(&report, "sandbox").unwrap();
        assert!(matches!(sb.status, DepStatus::Unusable { .. }), "{:?}", sb.status);
        assert!(!sb.is_ok());
        // Unusable required dep blocks, just like missing.
        assert!(sb.is_blocking());
        assert_eq!(doctor_exit_code(&report), 1);
    }

    #[test]
    fn scout_gh_reported_when_enabled_and_missing_but_not_blocking() {
        let mut cfg = base_cfg();
        cfg.features.scout.enabled = true;
        let probe = FakeProbe {
            present: vec!["git".into(), "claude".into()], // gh absent
            ..FakeProbe::all_present()
        };
        let report = build_report(&cfg, &probe);
        let gh = find(&report, "gh").expect("gh must be checked when scout enabled");
        assert_eq!(gh.status, DepStatus::Missing);
        assert!(gh.is_warning(), "gh missing is a warning, not blocking");
        assert!(!report.has_blocking(), "scout gh must not block startup");
    }

    #[test]
    fn scout_gh_not_checked_when_disabled() {
        let mut cfg = base_cfg();
        cfg.features.scout.enabled = false;
        let probe = FakeProbe {
            present: vec!["git".into(), "claude".into()],
            ..FakeProbe::all_present()
        };
        let report = build_report(&cfg, &probe);
        assert!(find(&report, "gh").is_none(), "gh must not be checked when scout off");
    }

    #[test]
    fn executor_cli_missing_blocks() {
        let probe = FakeProbe {
            present: vec!["git".into(), "gh".into(), "ollama".into()], // claude (executor) absent
            ..FakeProbe::all_present()
        };
        let report = build_report(&base_cfg(), &probe);
        let ex = find(&report, "executor").expect("executor CLI checked");
        assert_eq!(ex.status, DepStatus::Missing);
        assert!(ex.is_blocking(), "executor CLI is required");
    }

    #[test]
    fn probe_openspec_missing_binary_is_missing() {
        let st = probe_openspec("openspec-definitely-not-on-this-host-xyz");
        assert_eq!(st, DepStatus::Missing);
    }

    #[test]
    fn probe_openspec_nonzero_exit_is_unusable() {
        // `false` exits non-zero → present but broken.
        let bin = if std::path::Path::new("/bin/false").exists() {
            "/bin/false"
        } else {
            "/usr/bin/false"
        };
        let st = probe_openspec(bin);
        assert!(matches!(st, DepStatus::Unusable { .. }), "{st:?}");
    }
}
