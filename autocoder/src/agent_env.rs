//! a014: capture the operator's activated login-shell environment and inject
//! it — credential-filtered — into every agentic subprocess.
//!
//! `a013` makes the host toolchains *visible* to the executor (home exposed),
//! but visible is not *active*: systemd does not source `~/.bashrc`, so the
//! shell init that *activates* `pyenv` / `rbenv` / `poetry` / `nvm` (the shims
//! on `PATH`, `PYENV_ROOT` / `NVM_DIR`, `eval "$(pyenv init -)"`, the
//! virtualenv) never runs in the daemon's own environment. The agent finds the
//! files but `python` resolves to the wrong interpreter (or none).
//!
//! This module captures the operator's *activated* login-shell environment
//! (the toolchain-activated `PATH` plus `PYENV_ROOT` / `NVM_DIR` / `CARGO_HOME`
//! / `GOPATH` / `POETRY_*` style variables) and provides it to every agentic
//! subprocess through [`crate::agentic_run::agentic_run`], so those toolchains
//! are *usable*, not merely present.
//!
//! ## Best-effort capture
//!
//! [`capture_login_shell`] dumps a login + interactive shell's environment
//! (`bash -lic env`, time-bounded) so `~/.bashrc`-guarded init runs. It
//! degrades gracefully: a failed, timed-out, or partial capture yields an
//! empty [`CapturedEnv`] and the agentic run proceeds against the base
//! environment rather than failing (a014 task 1.2 / 5.4).
//!
//! ## Credential filter
//!
//! The captured environment is **credential-filtered** ([`CredentialFilter`]):
//! variables whose names contain `TOKEN` / `SECRET` / `KEY` / `PASSWORD`, or
//! carry a provider prefix such as `AWS_` / `ANTHROPIC_`, are dropped so the
//! operator's shell-exported secrets never reach the model — including provider
//! API keys, which as an env value would also bill the wrapped CLI off the
//! operator's subscription (per `a003`). The exclusion set ships with defaults
//! and is operator-editable (`exclude_add` / `exclude_remove`), mirroring
//! `a013`'s mask-list.
//!
//! ## Composition + precedence
//!
//! [`apply_captured_env`] layers the captured env onto the strategy-built
//! subprocess command, but **never overrides a variable the run itself set**
//! (the sandbox / strategy env, e.g. `ANTHROPIC_BASE_URL`): the run-set value
//! takes precedence (a014 task 3.1 / 5.3). Applying it to the strategy's inner
//! command before the OS-sandbox wrapper means the captured env flows through
//! every sandbox mechanism uniformly (`systemd-run --setenv`, `bwrap` env
//! inheritance) with no per-mechanism wiring.

use std::collections::HashSet;
use std::ffi::OsString;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Substrings (case-insensitive) that mark a variable name as credential-bearing.
pub const DEFAULT_CREDENTIAL_SUBSTRINGS: &[&str] = &["TOKEN", "SECRET", "KEY", "PASSWORD"];

/// Provider/credential prefixes (case-insensitive) that mark a variable name as
/// credential-bearing. `ANTHROPIC_` keeps provider API keys out of the
/// subprocess; the strategy's own non-credential `ANTHROPIC_BASE_URL` /
/// `ANTHROPIC_MODEL` are set as run-set env and so are never sourced from the
/// captured environment.
pub const DEFAULT_CREDENTIAL_PREFIXES: &[&str] = &["AWS_", "ANTHROPIC_"];

/// Interactive-shell bookkeeping variables that carry no toolchain activation
/// and would only inject noise (or stale state) into the subprocess. Dropped
/// from the captured environment regardless of the credential filter.
const NON_PROPAGATED: &[&str] = &[
    "_", "SHLVL", "PWD", "OLDPWD", "PS1", "PS2", "PS4", "LINES", "COLUMNS",
];

/// Name prefixes excluded from the captured environment because they belong to
/// autocoder's own control plane, not the operator's toolchains. `ORCH_*` (the
/// MCP / control-socket variables) are governed by the daemon's per-run config
/// and curated sandbox passthrough; a startup login-shell snapshot must never
/// shadow them.
const NON_PROPAGATED_PREFIXES: &[&str] = &["ORCH_"];

/// The default expected-toolchain set the `doctor` runnability check probes
/// when the operator has not configured one (a014 task 4.2). The common
/// version-manager-managed runtimes; absent ones are simply not reported.
pub const DEFAULT_EXPECTED_TOOLCHAINS: &[&str] = &["python3", "node", "ruby", "go"];

/// The default timeout for the one-shot login-shell capture at startup. The
/// capture runs once; a hung interactive init must not block the daemon, so it
/// is time-bounded and degrades to an empty capture.
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// A credential-pattern matcher: the default substrings/prefixes resolved with
/// the operator's `exclude_add` / `exclude_remove` edits (a014 task 2.2),
/// mirroring `a013`'s mask-list editability. All matching is case-insensitive.
#[derive(Debug, Clone)]
pub struct CredentialFilter {
    /// Uppercased substrings; a name CONTAINING any marks it credential-bearing.
    substrings: Vec<String>,
    /// Uppercased prefixes; a name STARTING WITH any marks it credential-bearing.
    prefixes: Vec<String>,
}

impl Default for CredentialFilter {
    fn default() -> Self {
        Self::new(&[], &[])
    }
}

impl CredentialFilter {
    /// Build the filter from the default set plus the operator's edits. An
    /// `exclude_add` entry ending in `_` is treated as a PREFIX (e.g. `GCP_`),
    /// otherwise as a substring (e.g. `APIKEY`). An `exclude_remove` entry drops
    /// the matching default token (case-insensitive exact) so a name that would
    /// otherwise match can pass (e.g. removing `KEY` to admit a `*_KEY` toolchain
    /// variable). Both lists match case-insensitively.
    pub fn new(exclude_add: &[String], exclude_remove: &[String]) -> Self {
        let removed: HashSet<String> = exclude_remove
            .iter()
            .map(|s| s.trim().to_ascii_uppercase())
            .collect();
        let mut substrings: Vec<String> = DEFAULT_CREDENTIAL_SUBSTRINGS
            .iter()
            .map(|s| s.to_ascii_uppercase())
            .filter(|s| !removed.contains(s))
            .collect();
        let mut prefixes: Vec<String> = DEFAULT_CREDENTIAL_PREFIXES
            .iter()
            .map(|s| s.to_ascii_uppercase())
            .filter(|s| !removed.contains(s))
            .collect();
        for entry in exclude_add {
            let up = entry.trim().to_ascii_uppercase();
            if up.is_empty() || removed.contains(&up) {
                continue;
            }
            if up.ends_with('_') {
                if !prefixes.contains(&up) {
                    prefixes.push(up);
                }
            } else if !substrings.contains(&up) {
                substrings.push(up);
            }
        }
        Self {
            substrings,
            prefixes,
        }
    }

    /// Resolve the filter from an [`AgentEnvConfig`]-style pair of optional edit
    /// lists (the executor `agent_env` block).
    pub fn from_edits(exclude_add: Option<&Vec<String>>, exclude_remove: Option<&Vec<String>>) -> Self {
        let empty: Vec<String> = Vec::new();
        Self::new(
            exclude_add.unwrap_or(&empty),
            exclude_remove.unwrap_or(&empty),
        )
    }

    /// Whether `name` matches a credential pattern and so must NOT be propagated
    /// to the agentic subprocess (a014 task 2.1).
    pub fn is_credential(&self, name: &str) -> bool {
        let up = name.to_ascii_uppercase();
        self.substrings.iter().any(|s| up.contains(s.as_str()))
            || self.prefixes.iter().any(|p| up.starts_with(p.as_str()))
    }
}

/// The captured, credential-filtered login-shell environment provided to every
/// agentic subprocess. Constructed once at daemon startup; an empty instance
/// (the degraded / not-yet-captured case) is a no-op when applied.
#[derive(Debug, Clone, Default)]
pub struct CapturedEnv {
    /// The propagated `(name, value)` pairs — credential- AND noise-filtered.
    vars: Vec<(String, String)>,
    /// How many variables the credential filter excluded (for the startup log).
    excluded: usize,
}

impl CapturedEnv {
    /// The empty / degraded capture: applying it changes nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The propagated variables.
    pub fn vars(&self) -> &[(String, String)] {
        &self.vars
    }

    /// Number of propagated variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Number of variables withheld by the credential filter.
    pub fn excluded_count(&self) -> usize {
        self.excluded
    }

    /// Lookup a propagated variable's value (the last one wins on duplicate
    /// names, matching shell `env` semantics).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Build a [`CapturedEnv`] from raw `(name, value)` pairs by applying the
    /// credential + noise filters. Pure, so the filtering is unit-testable.
    pub fn from_pairs(pairs: Vec<(String, String)>, filter: &CredentialFilter) -> Self {
        let mut vars = Vec::with_capacity(pairs.len());
        let mut excluded = 0usize;
        for (k, v) in pairs {
            if NON_PROPAGATED.contains(&k.as_str())
                || NON_PROPAGATED_PREFIXES.iter().any(|p| k.starts_with(p))
            {
                continue;
            }
            if filter.is_credential(&k) {
                excluded += 1;
                continue;
            }
            vars.push((k, v));
        }
        Self { vars, excluded }
    }
}

/// Parse the output of `env` (one `NAME=VALUE` per line) into `(name, value)`
/// pairs. A line without `=` (or with an empty name) is skipped; the value may
/// itself contain `=`. Values spanning multiple lines are not reconstructed
/// (best-effort), so a multi-line value's continuation lines are dropped as
/// no-`=` lines.
pub fn parse_env_dump(dump: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in dump.lines() {
        let Some(eq) = line.find('=') else {
            continue;
        };
        let name = &line[..eq];
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), line[eq + 1..].to_string()));
    }
    out
}

/// Capture the operator's activated login + interactive shell environment
/// (`bash -lic env`) and credential-filter it. Best-effort and time-bounded:
/// any failure (shell missing, non-zero exit, timeout, unparsable output)
/// yields an empty [`CapturedEnv`] so the run proceeds with the base
/// environment (a014 task 1.2 / 5.4).
pub async fn capture_login_shell(filter: &CredentialFilter) -> CapturedEnv {
    // `-l` (login) sources `~/.bash_profile` / `~/.profile`; `-i` (interactive)
    // sources `~/.bashrc`, where version-manager init (`pyenv init`, `nvm`) is
    // commonly guarded. `-c 'env'` dumps the activated environment.
    capture_with("bash", &["-lic", "env"], filter, DEFAULT_CAPTURE_TIMEOUT).await
}

/// [`capture_login_shell`] with the shell program / args / timeout injectable,
/// so the capture + degradation behavior is unit-testable without depending on
/// the host's real login shell.
pub async fn capture_with(
    program: &str,
    args: &[&str],
    filter: &CredentialFilter,
    timeout: Duration,
) -> CapturedEnv {
    use tokio::process::Command;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // If the timeout fires the child is dropped; kill it rather than leak it.
        .kill_on_drop(true);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "a014: login-shell env capture could not spawn `{program}` \
                 (proceeding with base environment): {e}"
            );
            return CapturedEnv::empty();
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => out,
        Ok(Ok(out)) => {
            tracing::warn!(
                "a014: login-shell env capture exited {:?}; proceeding with base \
                 environment",
                out.status.code()
            );
            return CapturedEnv::empty();
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "a014: login-shell env capture failed ({e}); proceeding with base \
                 environment"
            );
            return CapturedEnv::empty();
        }
        Err(_) => {
            tracing::warn!(
                "a014: login-shell env capture timed out after {}s; proceeding with \
                 base environment",
                timeout.as_secs()
            );
            return CapturedEnv::empty();
        }
    };

    let dump = String::from_utf8_lossy(&output.stdout);
    CapturedEnv::from_pairs(parse_env_dump(&dump), filter)
}

/// Layer the captured environment onto a strategy-built subprocess command
/// WITHOUT overriding any variable the run already set explicitly (the
/// sandbox / strategy env): the run-set value takes precedence (a014 task
/// 3.1 / 5.3). A no-op for an empty capture, preserving the base environment.
pub fn apply_captured_env(cmd: &mut tokio::process::Command, captured: &CapturedEnv) {
    if captured.is_empty() {
        return;
    }
    // The keys the run set explicitly (e.g. `ANTHROPIC_BASE_URL`) win on
    // conflict, so they are skipped when applying the captured environment.
    let run_set: HashSet<OsString> = cmd
        .as_std()
        .get_envs()
        .map(|(k, _)| k.to_os_string())
        .collect();
    for (k, v) in captured.vars() {
        if run_set.contains(&OsString::from(k.as_str())) {
            continue;
        }
        cmd.env(k, v);
    }
}

// ---------------------------------------------------------------------------
// Process-global captured environment.
//
// The capture is genuinely daemon-wide (one operator, one activated shell), so
// it lives in a process-global set once at startup and is read by every
// `agentic_run` spawn. Before it is set (unit tests, non-daemon binaries),
// `current_captured_env` returns the empty capture so existing behavior is
// unchanged.
// ---------------------------------------------------------------------------

static GLOBAL_CAPTURED: OnceLock<Arc<CapturedEnv>> = OnceLock::new();

/// Seed the daemon-global captured environment once at startup (idempotent — a
/// second call is ignored).
pub fn init_captured_env(env: CapturedEnv) {
    let _ = GLOBAL_CAPTURED.set(Arc::new(env));
}

/// The daemon-global captured environment, or the empty capture before
/// [`init_captured_env`] (tests / non-daemon paths), so the OS-env layer is a
/// no-op there.
pub fn current_captured_env() -> Arc<CapturedEnv> {
    GLOBAL_CAPTURED
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(CapturedEnv::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Credential filter (a014 task 2.1 / 5.2) ---------------------------

    #[test]
    fn credential_filter_excludes_default_patterns() {
        let f = CredentialFilter::default();
        // Names containing the credential substrings (any case).
        assert!(f.is_credential("FOO_TOKEN"));
        assert!(f.is_credential("github_token"));
        assert!(f.is_credential("MY_SECRET"));
        assert!(f.is_credential("AWS_SECRET_ACCESS_KEY"));
        assert!(f.is_credential("API_KEY"));
        assert!(f.is_credential("db_password"));
        // Provider prefixes.
        assert!(f.is_credential("ANTHROPIC_API_KEY"));
        assert!(f.is_credential("AWS_ACCESS_KEY_ID"));
        // Toolchain-activation variables are NOT credentials.
        for keep in [
            "PATH",
            "PYENV_ROOT",
            "RBENV_ROOT",
            "NVM_DIR",
            "CARGO_HOME",
            "GOPATH",
            "POETRY_HOME",
            "VIRTUAL_ENV",
            "HOME",
        ] {
            assert!(!f.is_credential(keep), "{keep} must propagate");
        }
    }

    #[test]
    fn credential_filter_operator_add_and_remove() {
        // Add a company prefix (ends in `_`) AND a bare substring; remove the
        // default `KEY` token so `*_KEY` toolchain vars can pass (a014 task 2.2).
        let f = CredentialFilter::new(
            &["GCP_".to_string(), "APIKEY".to_string()],
            &["KEY".to_string()],
        );
        assert!(f.is_credential("GCP_SERVICE_JSON"), "added prefix excludes");
        assert!(f.is_credential("MY_APIKEY"), "added substring excludes");
        // `KEY` removed → a name matching only `KEY` now passes.
        assert!(!f.is_credential("SIGNING_KEY"), "removed token admits *_KEY");
        // But `SECRET` (still a default) keeps catching secrets.
        assert!(f.is_credential("APP_SECRET"));
    }

    // --- env dump parsing --------------------------------------------------

    #[test]
    fn parse_env_dump_handles_values_with_equals_and_skips_junk() {
        let dump = "PATH=/usr/bin:/bin\nKEYVAL=a=b=c\nnoequals\n=emptyname\nX=\n";
        let pairs = parse_env_dump(dump);
        assert_eq!(pairs.len(), 3, "{pairs:?}");
        assert_eq!(pairs[0], ("PATH".into(), "/usr/bin:/bin".into()));
        assert_eq!(pairs[1], ("KEYVAL".into(), "a=b=c".into()));
        assert_eq!(pairs[2], ("X".into(), String::new()));
    }

    // --- from_pairs filtering (a014 task 5.2) ------------------------------

    #[test]
    fn from_pairs_drops_credentials_and_noise_keeps_toolchain() {
        let pairs = vec![
            ("PATH".to_string(), "/opt/pyenv/shims:/usr/bin".to_string()),
            ("PYENV_ROOT".to_string(), "/home/op/.pyenv".to_string()),
            ("FOO_TOKEN".to_string(), "supersecret".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-live".to_string()),
            ("SHLVL".to_string(), "3".to_string()),
            ("_".to_string(), "/usr/bin/env".to_string()),
            // autocoder's own control-plane var — must not be snapshotted.
            ("ORCH_DAEMON_CONTROL_SOCKET".to_string(), "/run/orch.sock".to_string()),
        ];
        let env = CapturedEnv::from_pairs(pairs, &CredentialFilter::default());
        assert_eq!(env.get("PATH"), Some("/opt/pyenv/shims:/usr/bin"));
        assert_eq!(env.get("PYENV_ROOT"), Some("/home/op/.pyenv"));
        // Credentials withheld.
        assert_eq!(env.get("FOO_TOKEN"), None);
        assert_eq!(env.get("ANTHROPIC_API_KEY"), None);
        assert_eq!(env.excluded_count(), 2, "two credential vars withheld");
        // Interactive-shell noise dropped (but not counted as a credential).
        assert_eq!(env.get("SHLVL"), None);
        assert_eq!(env.get("_"), None);
        // autocoder control-plane var dropped (governed by the daemon, not the
        // login-shell snapshot).
        assert_eq!(env.get("ORCH_DAEMON_CONTROL_SOCKET"), None);
    }

    // --- capture degradation (a014 task 1.2 / 5.4) -------------------------

    #[tokio::test]
    async fn capture_degrades_to_empty_when_shell_missing() {
        // A program that cannot be spawned → empty capture, no panic.
        let env = capture_with(
            "definitely-not-a-real-shell-binary-xyz",
            &["-lic", "env"],
            &CredentialFilter::default(),
            DEFAULT_CAPTURE_TIMEOUT,
        )
        .await;
        assert!(env.is_empty(), "missing shell yields the empty capture");
        assert_eq!(env.excluded_count(), 0);
    }

    #[tokio::test]
    async fn capture_degrades_to_empty_on_nonzero_exit() {
        // `false` exits non-zero → degraded empty capture (partial/failed).
        let env = capture_with(
            "false",
            &[],
            &CredentialFilter::default(),
            DEFAULT_CAPTURE_TIMEOUT,
        )
        .await;
        assert!(env.is_empty());
    }

    #[tokio::test]
    async fn capture_parses_and_filters_a_real_dump() {
        // Drive the capture against a deterministic stub that prints an env
        // dump including a toolchain var AND a credential var.
        let script = "printf 'PATH=/stub/bin\\nPYENV_ROOT=/stub/.pyenv\\nFOO_TOKEN=leak\\n'";
        let env = capture_with(
            "sh",
            &["-c", script],
            &CredentialFilter::default(),
            DEFAULT_CAPTURE_TIMEOUT,
        )
        .await;
        assert_eq!(env.get("PATH"), Some("/stub/bin"));
        assert_eq!(env.get("PYENV_ROOT"), Some("/stub/.pyenv"));
        assert_eq!(env.get("FOO_TOKEN"), None, "credential withheld at capture");
        assert_eq!(env.excluded_count(), 1);
    }

    // --- apply precedence (a014 task 3.1 / 5.3) ----------------------------

    fn env_of(cmd: &tokio::process::Command) -> std::collections::HashMap<String, String> {
        cmd.as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect()
    }

    #[test]
    fn apply_captured_env_run_set_wins_over_captured() {
        let mut cmd = tokio::process::Command::new("true");
        // The run sets ANTHROPIC_BASE_URL explicitly (strategy env).
        cmd.env("ANTHROPIC_BASE_URL", "https://run.example/api");
        let captured = CapturedEnv::from_pairs(
            vec![
                // Conflicts with the run-set var — the run's value must win.
                ("ANTHROPIC_BASE_URL".to_string(), "https://captured.example".to_string()),
                // No conflict — applied.
                ("PATH".to_string(), "/captured/bin".to_string()),
                ("PYENV_ROOT".to_string(), "/home/op/.pyenv".to_string()),
            ],
            // Disable the credential filter's ANTHROPIC_ prefix for this test so
            // the conflict variable survives capture; use a removal.
            &CredentialFilter::new(&[], &["ANTHROPIC_".to_string()]),
        );
        apply_captured_env(&mut cmd, &captured);
        let e = env_of(&cmd);
        assert_eq!(
            e.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://run.example/api"),
            "run-set value wins over the captured one"
        );
        assert_eq!(e.get("PATH").map(String::as_str), Some("/captured/bin"));
        assert_eq!(e.get("PYENV_ROOT").map(String::as_str), Some("/home/op/.pyenv"));
    }

    #[test]
    fn apply_empty_capture_is_a_noop() {
        let mut cmd = tokio::process::Command::new("true");
        cmd.env("ANTHROPIC_MODEL", "claude-opus-4-8");
        apply_captured_env(&mut cmd, &CapturedEnv::empty());
        let e = env_of(&cmd);
        // Only the originally-set var; nothing injected.
        assert_eq!(e.get("ANTHROPIC_MODEL").map(String::as_str), Some("claude-opus-4-8"));
        assert_eq!(e.len(), 1);
    }

    // --- a shell-init-activated toolchain becomes runnable (a014 task 5.1) --

    #[tokio::test]
    async fn captured_path_makes_a_shell_activated_toolchain_runnable() {
        use std::os::unix::fs::PermissionsExt;
        // A "managed" interpreter that exists ONLY under a directory the base
        // PATH does not include — standing in for a pyenv/nvm shim activated
        // solely by the operator's shell init.
        let tmp = tempfile::tempdir().unwrap();
        let bindir = tmp.path().join("managed-bin");
        std::fs::create_dir(&bindir).unwrap();
        let tool = bindir.join("managedtool");
        std::fs::write(&tool, "#!/bin/sh\necho MANAGED-OK\n").unwrap();
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();

        // Baseline: WITHOUT the captured env, the tool does not resolve in the
        // subprocess (the shell cannot find it on the inherited PATH).
        let baseline = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("managedtool")
            .env("PATH", "/usr/bin:/bin")
            .output()
            .await
            .unwrap();
        assert!(
            !baseline.status.success(),
            "control: the managed tool must NOT resolve without the captured env"
        );

        // The operator's shell init activated the toolchain by putting its shim
        // dir on PATH; the captured env carries that activated PATH.
        let captured = CapturedEnv::from_pairs(
            vec![(
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", bindir.display()),
            )],
            &CredentialFilter::default(),
        );
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg("managedtool");
        apply_captured_env(&mut cmd, &captured);
        let out = cmd.output().await.unwrap();
        assert!(
            out.status.success(),
            "the managed tool resolves once the captured env is applied"
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("MANAGED-OK"),
            "the managed (not the bare fallback) tool ran"
        );
    }

    #[test]
    fn global_captured_env_defaults_to_empty_before_init() {
        // In the unit-test process `init_captured_env` is never called, so the
        // global read is the empty (no-op) capture — agentic runs are unchanged.
        let env = current_captured_env();
        assert!(env.is_empty());
    }
}
