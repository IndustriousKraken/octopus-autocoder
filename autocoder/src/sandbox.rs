//! OS-level sandbox around every `agentic_run` subprocess (a006).
//!
//! a003 closed the credential *key-flow* (no key reaches a subprocess); this
//! module is the *reach* half: a kernel-enforced jail around every CLI the
//! shared [`crate::agentic_run::agentic_run`] primitive spawns, so a model
//! cannot go *get* a credential that exists on the host (another CLI's config
//! store, `~/.ssh`, autocoder's own config) even though the wrapped CLI's own
//! sandbox would have permitted it. Enforcement is external to the CLI — the
//! kernel applies it around the subprocess regardless of the CLI's settings.
//!
//! ## Mechanisms (probed on the daemon host)
//!
//! Two mechanisms can apply the same view; [`detect_mechanism`] picks one at
//! startup, preferring `systemd-run`:
//!
//! - **`systemd-run` (transient *service* mode — NOT `--scope`).** PID 1
//!   applies the filesystem + namespace properties; stdout/stderr are captured
//!   with `--pipe --wait --collect`. The properties used
//!   (`man systemd.exec`): `ProtectSystem=strict` (whole fs read-only),
//!   `ProtectHome=tmpfs` (home replaced by empty tmpfs), `ReadWritePaths=`
//!   (rw allowlist for the executor's workspace), `BindReadOnlyPaths=` (ro
//!   allowlist — read-only workspace + the role's own CLI store), `PrivateTmp`,
//!   `PrivateDevices`, `ProtectProc=invisible` + `ProcSubset=pid` (no other
//!   process's `environ`/`mem`), `NoNewPrivileges`, `CapabilityBoundingSet=~…`
//!   (drop `CAP_NET_RAW`/`CAP_NET_ADMIN`/`CAP_SYS_PTRACE`),
//!   `RestrictAddressFamilies=~AF_PACKET`. Service mode is required for the
//!   filesystem properties to be applied by PID 1; `--scope` runs in the
//!   caller's unit and does not.
//! - **`bwrap` (bubblewrap) fallback** for unprivileged / non-systemd /
//!   in-container hosts: `--ro-bind / /` then `--tmpfs <home>` (hide home),
//!   `--bind`/`--ro-bind <workspace>` back, `--ro-bind <store>` for the self
//!   store, `--proc /proc`, `--dev /dev`, `--tmpfs /tmp`, `--unshare-*`,
//!   `--cap-drop`, `--die-with-parent`.
//!
//! Outbound network egress is **deliberately not restricted** here (no
//! `--unshare-net`, no `RestrictAddressFamilies` beyond `AF_PACKET`): egress
//! control belongs to the host firewall, and there is no maintainable in-app
//! allowlist for CDN'd API/forge hosts. The sandbox does filesystem and host
//! isolation, not a network allowlist.
//!
//! ## Credential-store layers
//!
//! Two complementary, independently-toggleable layers protect CLI config
//! stores (both ON by default; see [`crate::config`]):
//!
//! - **`os_hide`** — the filesystem allowlist above. The store of every CLI
//!   *other than the running role's own* is absent from the namespace
//!   (fail-closed; an unenumerated store is hidden by default). It cannot
//!   protect the running role's own store, which must stay readable for the
//!   CLI to authenticate.
//! - **`engine_deny`** — the per-invocation tool-use denylist the executor
//!   already supplies to the CLI (see [`crate::audits::write_sandbox_settings`])
//!   extended to deny the agent's `Read`/`Bash` tools on *every* registered
//!   CLI store, the self-store included. A string-pattern speed bump that
//!   covers the one store `os_hide` cannot; it deters, it does not bound.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::config::CliKind;

/// The kernel mechanism that applies the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMechanism {
    /// `systemd-run` in transient service mode (PID 1 applies the namespace).
    SystemdRun,
    /// `bwrap` (bubblewrap) — the unprivileged / non-systemd fallback.
    Bwrap,
}

impl SandboxMechanism {
    /// The binary this mechanism invokes.
    pub fn program(self) -> &'static str {
        match self {
            Self::SystemdRun => "systemd-run",
            Self::Bwrap => "bwrap",
        }
    }

    /// Operator-facing label for diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemdRun => "systemd-run",
            Self::Bwrap => "bwrap",
        }
    }
}

/// Capabilities dropped from the subprocess's bounding set: no raw-socket
/// sniffing (`CAP_NET_RAW`), no route/iptables hijack (`CAP_NET_ADMIN`), no
/// reading another process's memory (`CAP_SYS_PTRACE`).
pub const DROPPED_CAPS: [&str; 3] = ["CAP_NET_RAW", "CAP_NET_ADMIN", "CAP_SYS_PTRACE"];

/// The home directory the allowlist is built relative to. `$HOME`, falling
/// back to `/root` only if unset (the daemon always runs with `HOME` set).
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"))
}

/// The on-disk config/credential store directories for one CLI kind, under
/// `home`. These are the paths the OS allowlist admits (self) or hides
/// (others), AND that `engine_deny` denies the agent's read tools.
///
/// `claude` keeps its login + settings under `~/.claude`. `opencode` keeps
/// its credential store under `~/.local/share/opencode` and its config under
/// `~/.config/opencode`; both are protected.
pub fn config_stores_for(cli: CliKind, home: &Path) -> Vec<PathBuf> {
    match cli {
        CliKind::Claude => vec![home.join(".claude")],
        CliKind::Opencode => vec![
            home.join(".local/share/opencode"),
            home.join(".config/opencode"),
        ],
    }
}

/// Every registered CLI strategy's config store, driven by [`CliKind::ALL`]
/// so the set grows automatically as strategies are added (task 5.2) — never
/// a hardcoded literal list.
pub fn all_config_stores(home: &Path) -> Vec<PathBuf> {
    CliKind::ALL
        .iter()
        .flat_map(|cli| config_stores_for(*cli, home))
        .collect()
}

/// The `engine_deny` read-deny glob patterns covering every registered CLI
/// store (the self-store included). Supplied per-invocation through the CLI's
/// own settings mechanism — never by mutating the operator's global config.
pub fn engine_deny_read_paths(home: &Path) -> Vec<String> {
    all_config_stores(home)
        .into_iter()
        .map(|p| format!("{}/**", p.display()))
        .collect()
}

/// The filesystem allowlist + role for one run, consumed by the argv
/// builders. Everything NOT named here is absent from the namespace.
#[derive(Debug, Clone)]
pub struct SandboxPlan {
    /// The run's workspace (always present in the namespace).
    pub workspace: PathBuf,
    /// `true` mounts the workspace read-write (the executor); `false` mounts
    /// it read-only (audits, agentic reviewer, contradiction checks).
    pub workspace_writable: bool,
    /// The running role's own CLI config store(s), admitted read-only so the
    /// wrapped CLI can authenticate.
    pub self_stores: Vec<PathBuf>,
    /// Other CLI stores admitted read-only — populated ONLY when `os_hide` is
    /// off (so a nested CLI of that kind could authenticate live). Empty under
    /// the secure default.
    pub extra_ro_stores: Vec<PathBuf>,
    /// The home directory replaced by tmpfs (then selectively re-bound above).
    pub home: PathBuf,
}

/// The program + args + explicit env of the strategy-built inner command,
/// extracted so it can be re-wrapped under a mechanism. The working directory
/// is applied by the wrapper (`--working-directory` / `--chdir <workspace>`),
/// so it is not carried here.
#[derive(Debug, Clone)]
pub struct InnerCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Env vars the strategy set explicitly (e.g. `ANTHROPIC_BASE_URL`).
    pub env: Vec<(OsString, OsString)>,
}

impl InnerCommand {
    /// Extract the inner invocation from a strategy-built [`tokio::process::Command`]
    /// before stdio/process-group are applied.
    pub fn from_command(cmd: &tokio::process::Command) -> Self {
        let std = cmd.as_std();
        let program = std.get_program().to_os_string();
        let args = std.get_args().map(|a| a.to_os_string()).collect();
        let env = std
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
            .collect();
        Self {
            program,
            args,
            env,
        }
    }
}

/// Env-var names/prefixes forwarded into the `systemd-run` service unit (which
/// does NOT inherit the caller's full environment). The wrapped CLI needs
/// `HOME`/`PATH`/`USER` to locate its store + binaries; the MCP child needs
/// the `ORCH_*` control-socket vars; the strategy's explicit `ANTHROPIC_*` /
/// model-selection env is forwarded separately from [`InnerCommand::env`].
const SYSTEMD_ENV_PASSTHROUGH: &[&str] = &["HOME", "PATH", "USER", "LOGNAME", "LANG", "TERM"];
const SYSTEMD_ENV_PASSTHROUGH_PREFIXES: &[&str] = &["ORCH_", "XDG_", "ANTHROPIC_"];

fn should_passthrough(name: &str) -> bool {
    SYSTEMD_ENV_PASSTHROUGH.contains(&name)
        || SYSTEMD_ENV_PASSTHROUGH_PREFIXES
            .iter()
            .any(|p| name.starts_with(p))
}

/// Build the full `systemd-run` argv (program included) for `plan` wrapping
/// `inner`. Transient *service* mode with `--pipe --wait --collect` so the
/// existing streaming-JSON and capture output modes are preserved; the
/// filesystem allowlist, capability drops, and `/proc` restriction are applied
/// as `--property=` settings by PID 1.
pub fn systemd_run_argv(plan: &SandboxPlan, inner: &InnerCommand) -> Vec<OsString> {
    fn prop(argv: &mut Vec<OsString>, key: &str, val: &OsStr) {
        let mut s = OsString::from(format!("--property={key}="));
        s.push(val);
        argv.push(s);
    }

    let mut argv: Vec<OsString> = Vec::new();
    argv.push(OsString::from(SandboxMechanism::SystemdRun.program()));
    for flag in ["--quiet", "--pipe", "--wait", "--collect"] {
        argv.push(OsString::from(flag));
    }

    prop(&mut argv, "WorkingDirectory", plan.workspace.as_os_str());
    // Host isolation + capability drops + /proc restriction.
    prop(&mut argv, "NoNewPrivileges", OsStr::new("yes"));
    prop(&mut argv, "ProtectSystem", OsStr::new("strict"));
    prop(&mut argv, "ProtectHome", OsStr::new("tmpfs"));
    prop(&mut argv, "PrivateTmp", OsStr::new("yes"));
    prop(&mut argv, "PrivateDevices", OsStr::new("yes"));
    prop(&mut argv, "ProtectProc", OsStr::new("invisible"));
    prop(&mut argv, "ProcSubset", OsStr::new("pid"));
    prop(
        &mut argv,
        "CapabilityBoundingSet",
        OsStr::new(&format!("~{}", DROPPED_CAPS.join(" "))),
    );
    prop(&mut argv, "RestrictAddressFamilies", OsStr::new("~AF_PACKET"));

    // Filesystem allowlist: workspace rw/ro, then the read-only stores.
    if plan.workspace_writable {
        prop(&mut argv, "ReadWritePaths", plan.workspace.as_os_str());
    } else {
        prop(&mut argv, "BindReadOnlyPaths", plan.workspace.as_os_str());
    }
    for store in plan.self_stores.iter().chain(plan.extra_ro_stores.iter()) {
        prop(&mut argv, "BindReadOnlyPaths", store.as_os_str());
    }

    // Forward the strategy's explicit env + the curated passthrough set.
    for (k, v) in &inner.env {
        let mut s = OsString::from("--setenv=");
        s.push(k);
        s.push("=");
        s.push(v);
        argv.push(s);
    }
    let explicit: std::collections::HashSet<&OsStr> =
        inner.env.iter().map(|(k, _)| k.as_os_str()).collect();
    for (k, v) in std::env::vars_os() {
        if explicit.contains(k.as_os_str()) {
            continue;
        }
        if k.to_str().is_some_and(should_passthrough) {
            let mut s = OsString::from("--setenv=");
            s.push(&k);
            s.push("=");
            s.push(&v);
            argv.push(s);
        }
    }

    argv.push(OsString::from("--"));
    argv.push(inner.program.clone());
    argv.extend(inner.args.iter().cloned());
    argv
}

/// Build the full `bwrap` argv (program included) for `plan` wrapping `inner`.
/// `bwrap` inherits the caller's environment, so the strategy's explicit env
/// is applied onto the wrapper [`tokio::process::Command`] in
/// [`wrap_command`] rather than encoded here. Network namespaces are NOT
/// unshared (egress stays open by design).
pub fn bwrap_argv(plan: &SandboxPlan, inner: &InnerCommand) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    let push = |argv: &mut Vec<OsString>, s: &str| argv.push(OsString::from(s));

    push(&mut argv, SandboxMechanism::Bwrap.program());
    push(&mut argv, "--die-with-parent");
    push(&mut argv, "--new-session");
    // Isolate namespaces EXCEPT the network (egress stays open by design).
    push(&mut argv, "--unshare-user");
    push(&mut argv, "--unshare-ipc");
    push(&mut argv, "--unshare-pid");
    push(&mut argv, "--unshare-uts");
    push(&mut argv, "--unshare-cgroup");

    // Whole root read-only, then hide home, then re-bind the allowlist.
    push(&mut argv, "--ro-bind");
    push(&mut argv, "/");
    push(&mut argv, "/");
    push(&mut argv, "--tmpfs");
    argv.push(plan.home.as_os_str().to_os_string());

    push(&mut argv, if plan.workspace_writable { "--bind" } else { "--ro-bind" });
    argv.push(plan.workspace.as_os_str().to_os_string());
    argv.push(plan.workspace.as_os_str().to_os_string());

    for store in plan.self_stores.iter().chain(plan.extra_ro_stores.iter()) {
        push(&mut argv, "--ro-bind-try");
        argv.push(store.as_os_str().to_os_string());
        argv.push(store.as_os_str().to_os_string());
    }

    push(&mut argv, "--proc");
    push(&mut argv, "/proc");
    push(&mut argv, "--dev");
    push(&mut argv, "/dev");
    push(&mut argv, "--tmpfs");
    push(&mut argv, "/tmp");

    for cap in DROPPED_CAPS {
        push(&mut argv, "--cap-drop");
        push(&mut argv, cap);
    }

    push(&mut argv, "--chdir");
    argv.push(plan.workspace.as_os_str().to_os_string());

    push(&mut argv, "--");
    argv.push(inner.program.clone());
    argv.extend(inner.args.iter().cloned());
    argv
}

/// Build the wrapper [`tokio::process::Command`] for `mechanism`. The caller
/// applies stdio + `process_group(0)` + `current_dir` to the returned command
/// exactly as it would to an unwrapped spawn, so the timeout/kill/streaming
/// behavior is unchanged.
pub fn wrap_command(
    mechanism: SandboxMechanism,
    plan: &SandboxPlan,
    inner: &InnerCommand,
) -> tokio::process::Command {
    let argv = match mechanism {
        SandboxMechanism::SystemdRun => systemd_run_argv(plan, inner),
        SandboxMechanism::Bwrap => bwrap_argv(plan, inner),
    };
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    // bwrap inherits the env; systemd-run encodes it as --setenv (above) but
    // setting it on the wrapper too is harmless. Either way the strategy's
    // explicit env reaches the inner command.
    for (k, v) in &inner.env {
        cmd.env(k, v);
    }
    cmd
}

/// The spawn decision after the mechanism gate (tasks 4.1 / 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnPlan {
    /// Wrap the subprocess in the OS-level sandbox via this mechanism.
    Wrap(SandboxMechanism),
    /// No mechanism is available, but the operator opted into unsandboxed
    /// operation — spawn the bare subprocess (the loud WARN was emitted at
    /// startup).
    Unsandboxed,
}

/// Decide how to spawn given the detected mechanism + the unsandboxed opt-in.
/// Fail-closed: with no mechanism AND no opt-in, return an error naming the
/// missing mechanisms so NO unsandboxed subprocess is spawned (task 4.1).
pub fn decide_spawn(
    mechanism: Option<SandboxMechanism>,
    allow_unsandboxed: bool,
) -> anyhow::Result<SpawnPlan> {
    match (mechanism, allow_unsandboxed) {
        (Some(m), _) => Ok(SpawnPlan::Wrap(m)),
        (None, true) => Ok(SpawnPlan::Unsandboxed),
        (None, false) => Err(anyhow::anyhow!(
            "no OS sandbox mechanism is available on this host: neither \
             `systemd-run` (transient service mode) nor `bwrap` can apply the \
             sandbox. Refusing to spawn an unsandboxed agentic subprocess. \
             Install/enable one of them, or set \
             `executor.sandbox.allow_unsandboxed: true` to override (NOT \
             recommended — the model could then reach host credentials)."
        )),
    }
}

/// The loud startup WARN emitted once when the daemon will run agentic
/// subprocesses unsandboxed (no mechanism available AND the operator opted
/// in). `None` when a mechanism exists or no opt-in was given. Separated from
/// the logging site so it can be asserted without a daemon (task 8.7).
pub fn startup_unsandboxed_warning(
    mechanism: Option<SandboxMechanism>,
    allow_unsandboxed: bool,
) -> Option<String> {
    (mechanism.is_none() && allow_unsandboxed).then(|| {
        "no OS sandbox mechanism (systemd-run / bwrap) is available AND \
         `executor.sandbox.allow_unsandboxed` is set: agentic subprocesses \
         are running UNSANDBOXED. A wrapped CLI's model can reach host \
         credentials (other CLIs' stores, ~/.ssh, autocoder config). Install \
         systemd-run or bwrap, or unset the opt-in, to restore the sandbox."
            .to_string()
    })
}

/// Everything one `agentic_run` call needs to apply (or skip) the OS-level
/// sandbox. Constructed per-run by the daemon from the detected mechanism, the
/// resolved per-repo toggles, and the role's read/write posture + CLI kind.
///
/// `enforce == false` (the [`Default`]) skips the OS layer entirely — used by
/// test fixtures AND any not-yet-wired path so existing behavior is unchanged.
/// Production sets `enforce == true` via [`RunSandbox::for_role`], which makes
/// the mechanism gate fail-closed when no mechanism is available.
#[derive(Debug, Clone)]
pub struct RunSandbox {
    pub enforce: bool,
    pub mechanism: Option<SandboxMechanism>,
    pub allow_unsandboxed: bool,
    pub workspace_writable: bool,
    /// The CLI the running role drives — selects its own store (admitted
    /// read-only for auth) vs the other stores (hidden under `os_hide`).
    pub cli: CliKind,
    pub os_hide: bool,
    pub engine_deny: bool,
}

impl Default for RunSandbox {
    fn default() -> Self {
        Self {
            enforce: false,
            mechanism: None,
            allow_unsandboxed: false,
            workspace_writable: false,
            cli: CliKind::Claude,
            os_hide: true,
            engine_deny: true,
        }
    }
}

impl RunSandbox {
    /// Build the enforced sandbox for one role. `workspace_writable` is `true`
    /// for the executor and `false` for read-only roles (audits, agentic
    /// reviewer, contradiction checks). `cli` is the role's resolved
    /// [`CliKind`] (task 2.5: the self-store is derived from it).
    pub fn for_role(
        mechanism: Option<SandboxMechanism>,
        allow_unsandboxed: bool,
        cli: CliKind,
        workspace_writable: bool,
        toggles: crate::config::SandboxToggles,
    ) -> Self {
        Self {
            enforce: true,
            mechanism,
            allow_unsandboxed,
            workspace_writable,
            cli,
            os_hide: toggles.os_hide,
            engine_deny: toggles.engine_deny,
        }
    }

    /// The filesystem allowlist for this run: workspace (rw/ro), the role's
    /// own CLI store(s) read-only, and — only when `os_hide` is off — every
    /// OTHER registered CLI's store read-only. Non-existent store paths are
    /// dropped so the mechanism does not fail binding a missing directory.
    pub fn build_plan(&self, workspace: &Path) -> SandboxPlan {
        self.build_plan_with_home(workspace, &home_dir())
    }

    /// [`build_plan`](Self::build_plan) against an explicit `home` (so the
    /// allowlist construction is testable without mutating `$HOME`).
    pub fn build_plan_with_home(&self, workspace: &Path, home: &Path) -> SandboxPlan {
        let self_stores = config_stores_for(self.cli, home)
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        let extra_ro_stores = if self.os_hide {
            Vec::new()
        } else {
            CliKind::ALL
                .iter()
                .filter(|c| **c != self.cli)
                .flat_map(|c| config_stores_for(*c, home))
                .filter(|p| p.exists())
                .collect()
        };
        SandboxPlan {
            workspace: workspace.to_path_buf(),
            workspace_writable: self.workspace_writable,
            self_stores,
            extra_ro_stores,
            home: home.to_path_buf(),
        }
    }

    /// The `engine_deny` read-deny patterns to fold into the per-invocation
    /// tool-use denylist (empty when the toggle is off). Covers EVERY
    /// registered CLI store, the self-store included.
    pub fn engine_deny_paths(&self) -> Vec<String> {
        if self.engine_deny {
            engine_deny_read_paths(&home_dir())
        } else {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon-global sandbox context.
//
// The detected mechanism + the unsandboxed opt-in are genuinely daemon-wide,
// so they live in a process-global set once at startup. The *active* per-repo
// toggle override is set for the duration of one change's pipeline via
// [`enter_repo`] (the daemon processes one iteration at a time under the
// busy-marker model), so the executor AND the in-iteration pre-flight /
// review roles all see that repository's resolved toggles.
// ---------------------------------------------------------------------------

use std::sync::{Mutex, OnceLock};

use crate::config::SandboxToggles;

struct GlobalSandbox {
    mechanism: Option<SandboxMechanism>,
    allow_unsandboxed: bool,
    /// The global (`executor.sandbox`) resolved toggles — the fallback when
    /// no per-repo override is active.
    global_toggles: SandboxToggles,
}

static GLOBAL: OnceLock<GlobalSandbox> = OnceLock::new();
static ACTIVE_TOGGLES: Mutex<Option<SandboxToggles>> = Mutex::new(None);

/// Initialize the daemon-global sandbox context once at startup (idempotent —
/// a second call is ignored). After this, [`current_run_sandbox`] returns an
/// *enforced* [`RunSandbox`] so every `agentic_run` spawn is gated + wrapped.
/// Before it (unit tests, non-daemon binaries), `current_run_sandbox` returns
/// the unenforced default so existing behavior is unchanged.
pub fn init_global(
    mechanism: Option<SandboxMechanism>,
    allow_unsandboxed: bool,
    global_toggles: SandboxToggles,
) {
    let _ = GLOBAL.set(GlobalSandbox {
        mechanism,
        allow_unsandboxed,
        global_toggles,
    });
}

/// RAII guard returned by [`enter_repo`]; clears the active per-repo toggle
/// override when dropped so the next iteration starts from the global default.
#[must_use]
pub struct RepoToggleGuard(());

impl Drop for RepoToggleGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_TOGGLES.lock() {
            *active = None;
        }
    }
}

/// Set the active per-repository toggle override for the duration of one
/// change's pipeline. Resolves the repo's `sandbox` block over the global
/// toggles (per-repo overrides global, per field), so the executor AND the
/// in-iteration pre-flight/review roles wrap with this repository's effective
/// posture. A no-op (returns a guard anyway) before [`init_global`].
pub fn enter_repo(repo: Option<&crate::config::RepoSandboxConfig>) -> RepoToggleGuard {
    if let Some(g) = GLOBAL.get() {
        let toggles = g.global_toggles.with_repo_override(repo);
        if let Ok(mut active) = ACTIVE_TOGGLES.lock() {
            *active = Some(toggles);
        }
    }
    RepoToggleGuard(())
}

/// Build the [`RunSandbox`] for one spawn from the daemon-global context.
/// `cli` is the running role's resolved CLI (selects its own store); `writable`
/// is `true` for the executor and `false` for read-only roles. Before
/// [`init_global`] (tests / non-daemon paths) the unenforced default is
/// returned so the OS layer is skipped.
pub fn current_run_sandbox(cli: CliKind, workspace_writable: bool) -> RunSandbox {
    match GLOBAL.get() {
        None => RunSandbox::default(),
        Some(g) => {
            let toggles = ACTIVE_TOGGLES
                .lock()
                .ok()
                .and_then(|a| *a)
                .unwrap_or(g.global_toggles);
            RunSandbox::for_role(g.mechanism, g.allow_unsandboxed, cli, workspace_writable, toggles)
        }
    }
}

/// Probe whether `systemd-run` can apply the sandbox in transient service
/// mode on this host. Runs a trivial `true` unit; success means PID 1 will
/// accept our property set. Unprivileged hosts without polkit/session-bus
/// access fail this probe (→ fall back to `bwrap`).
fn systemd_run_usable() -> bool {
    which("systemd-run")
        && std::process::Command::new("systemd-run")
            .args(["--quiet", "--pipe", "--wait", "--collect"])
            .arg("--")
            .arg("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Probe whether `bwrap` can apply the sandbox (it needs unprivileged user
/// namespaces; some hosts disable them).
fn bwrap_usable() -> bool {
    which("bwrap")
        && std::process::Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "--proc", "/proc", "--", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Whether `bin` is resolvable on `$PATH`.
fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
        })
        .unwrap_or(false)
}

/// Detect the usable sandbox mechanism at daemon startup, preferring
/// `systemd-run` service mode, else `bwrap`, else `None` (task 1.3). The
/// `None` case drives the fail-closed gate ([`decide_spawn`]).
pub fn detect_mechanism() -> Option<SandboxMechanism> {
    if systemd_run_usable() {
        Some(SandboxMechanism::SystemdRun)
    } else if bwrap_usable() {
        Some(SandboxMechanism::Bwrap)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osv(items: &[OsString]) -> Vec<String> {
        items.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    fn plan(writable: bool, os_hide: bool) -> SandboxPlan {
        let home = PathBuf::from("/home/u");
        SandboxPlan {
            workspace: PathBuf::from("/home/u/.cache/ws"),
            workspace_writable: writable,
            self_stores: vec![home.join(".claude")],
            extra_ro_stores: if os_hide {
                Vec::new()
            } else {
                vec![home.join(".local/share/opencode")]
            },
            home,
        }
    }

    fn inner() -> InnerCommand {
        InnerCommand {
            program: OsString::from("claude"),
            args: vec![OsString::from("--settings"), OsString::from("/tmp/s.json")],
            env: vec![(
                OsString::from("ANTHROPIC_BASE_URL"),
                OsString::from("https://example.invalid"),
            )],
        }
    }

    #[test]
    fn config_stores_cover_each_cli_kind() {
        let home = Path::new("/home/u");
        assert_eq!(config_stores_for(CliKind::Claude, home), vec![home.join(".claude")]);
        let oc = config_stores_for(CliKind::Opencode, home);
        assert!(oc.iter().any(|p| p.ends_with("opencode")));
        // all_config_stores spans every registered CLI kind (driven by ALL).
        let all = all_config_stores(home);
        for cli in CliKind::ALL {
            for store in config_stores_for(cli, home) {
                assert!(all.contains(&store), "all_config_stores must include {store:?}");
            }
        }
    }

    #[test]
    fn engine_deny_paths_cover_every_store_recursively() {
        let home = Path::new("/home/u");
        let pats = engine_deny_read_paths(home);
        // The self (claude) AND another CLI's (opencode) store are both denied.
        assert!(pats.iter().any(|p| p.contains("/.claude/**")));
        assert!(pats.iter().any(|p| p.contains("opencode") && p.ends_with("/**")));
    }

    #[test]
    fn systemd_argv_is_service_mode_with_pipe_and_allowlist() {
        let a = osv(&systemd_run_argv(&plan(true, true), &inner()));
        // Transient service mode, NOT --scope; --pipe --wait --collect.
        assert_eq!(a[0], "systemd-run");
        assert!(a.contains(&"--pipe".to_string()));
        assert!(a.contains(&"--wait".to_string()));
        assert!(a.contains(&"--collect".to_string()));
        assert!(!a.iter().any(|x| x == "--scope"), "must NOT be scope mode");
        // Capability drops + NoNewPrivileges + AF_PACKET restriction + /proc.
        assert!(a.iter().any(|x| x
            == "--property=CapabilityBoundingSet=~CAP_NET_RAW CAP_NET_ADMIN CAP_SYS_PTRACE"));
        assert!(a.iter().any(|x| x == "--property=NoNewPrivileges=yes"));
        assert!(a.iter().any(|x| x == "--property=RestrictAddressFamilies=~AF_PACKET"));
        assert!(a.iter().any(|x| x == "--property=ProtectProc=invisible"));
        assert!(a.iter().any(|x| x == "--property=ProcSubset=pid"));
        // Executor workspace is read-write.
        assert!(a.iter().any(|x| x == "--property=ReadWritePaths=/home/u/.cache/ws"));
        // The self store is admitted read-only.
        assert!(a.iter().any(|x| x == "--property=BindReadOnlyPaths=/home/u/.claude"));
        // Strategy env is forwarded.
        assert!(a
            .iter()
            .any(|x| x == "--setenv=ANTHROPIC_BASE_URL=https://example.invalid"));
        // The inner command is after `--`.
        let dd = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(a[dd + 1], "claude");
    }

    #[test]
    fn systemd_argv_read_only_workspace_uses_bind_read_only() {
        let a = osv(&systemd_run_argv(&plan(false, true), &inner()));
        assert!(a.iter().any(|x| x == "--property=BindReadOnlyPaths=/home/u/.cache/ws"));
        assert!(
            !a.iter().any(|x| x == "--property=ReadWritePaths=/home/u/.cache/ws"),
            "a read-only role must NOT get the workspace read-write"
        );
    }

    #[test]
    fn systemd_argv_os_hide_off_admits_other_store() {
        // os_hide off → the opencode store is admitted read-only.
        let a = osv(&systemd_run_argv(&plan(true, false), &inner()));
        assert!(a
            .iter()
            .any(|x| x.starts_with("--property=BindReadOnlyPaths=") && x.contains("opencode")));
        // os_hide on → it is absent.
        let b = osv(&systemd_run_argv(&plan(true, true), &inner()));
        assert!(!b.iter().any(|x| x.contains("opencode")));
    }

    #[test]
    fn bwrap_argv_hides_home_binds_workspace_and_drops_caps() {
        let a = osv(&bwrap_argv(&plan(true, true), &inner()));
        assert_eq!(a[0], "bwrap");
        // home replaced by tmpfs, whole root read-only.
        let ro_root = a.windows(3).position(|w| w == ["--ro-bind", "/", "/"]);
        assert!(ro_root.is_some(), "whole root is bound read-only");
        assert!(a.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/home/u"));
        // workspace read-write for the executor.
        assert!(a
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == "/home/u/.cache/ws" && w[2] == "/home/u/.cache/ws"));
        // self store re-bound read-only (try variant: absent path is skipped).
        assert!(a.windows(2).any(|w| w[0] == "--ro-bind-try" && w[1] == "/home/u/.claude"));
        // /proc restricted, caps dropped, egress NOT unshared.
        assert!(a.windows(2).any(|w| w[0] == "--proc" && w[1] == "/proc"));
        for cap in DROPPED_CAPS {
            assert!(a.windows(2).any(|w| w[0] == "--cap-drop" && w[1] == cap));
        }
        assert!(!a.iter().any(|x| x == "--unshare-net"), "egress must stay open");
        assert!(a.iter().any(|x| x == "--die-with-parent"));
    }

    #[test]
    fn bwrap_argv_read_only_workspace_uses_ro_bind() {
        let a = osv(&bwrap_argv(&plan(false, true), &inner()));
        assert!(a
            .windows(3)
            .any(|w| w[0] == "--ro-bind" && w[1] == "/home/u/.cache/ws" && w[2] == "/home/u/.cache/ws"));
        assert!(
            !a.windows(3)
                .any(|w| w[0] == "--bind" && w[1] == "/home/u/.cache/ws"),
            "a read-only role must NOT bind the workspace read-write"
        );
    }

    // task 4.1 / 8.7: no mechanism + no opt-in fails closed; opt-in proceeds.
    #[test]
    fn gate_fails_closed_without_mechanism_or_opt_in() {
        let err = decide_spawn(None, false).unwrap_err().to_string();
        assert!(err.contains("systemd-run") && err.contains("bwrap"));
        assert!(err.to_lowercase().contains("refus"));
    }

    #[test]
    fn gate_opt_in_proceeds_unsandboxed() {
        assert_eq!(decide_spawn(None, true).unwrap(), SpawnPlan::Unsandboxed);
    }

    #[test]
    fn gate_wraps_when_mechanism_available() {
        assert_eq!(
            decide_spawn(Some(SandboxMechanism::SystemdRun), false).unwrap(),
            SpawnPlan::Wrap(SandboxMechanism::SystemdRun)
        );
        assert_eq!(
            decide_spawn(Some(SandboxMechanism::Bwrap), true).unwrap(),
            SpawnPlan::Wrap(SandboxMechanism::Bwrap)
        );
    }

    #[test]
    fn unsandboxed_warning_only_when_no_mechanism_and_opt_in() {
        assert!(startup_unsandboxed_warning(None, true).is_some());
        assert!(startup_unsandboxed_warning(None, false).is_none());
        assert!(startup_unsandboxed_warning(Some(SandboxMechanism::Bwrap), true).is_none());
    }

    #[test]
    fn inner_command_extracts_program_args_env() {
        let mut cmd = tokio::process::Command::new("claude");
        cmd.arg("--settings").arg("/tmp/s.json");
        cmd.env("ANTHROPIC_MODEL", "claude-opus-4-8");
        let inner = InnerCommand::from_command(&cmd);
        assert_eq!(inner.program, OsString::from("claude"));
        assert_eq!(inner.args, vec![OsString::from("--settings"), OsString::from("/tmp/s.json")]);
        assert!(inner
            .env
            .iter()
            .any(|(k, v)| k == "ANTHROPIC_MODEL" && v == "claude-opus-4-8"));
    }

    // a006 / task 8.4: under the default (os_hide on) the other CLI's store is
    // absent from the allowlist; with os_hide off it is admitted read-only,
    // while engine_deny still denies it at the CLI layer.
    #[test]
    fn os_hide_controls_other_store_presence_in_allowlist() {
        // A temp HOME with BOTH a claude and an opencode store present.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir_all(home.path().join(".local/share/opencode")).unwrap();
        std::fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
        let ws = home.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();

        let toggles_on = crate::config::SandboxToggles { os_hide: true, engine_deny: true };
        let run_on = RunSandbox::for_role(
            Some(SandboxMechanism::SystemdRun),
            false,
            CliKind::Claude,
            false,
            toggles_on,
        );
        let plan_on = run_on.build_plan_with_home(&ws, home.path());
        // The running role's own (claude) store is admitted; the other
        // (opencode) store is absent.
        assert!(plan_on.self_stores.iter().any(|p| p.ends_with(".claude")));
        assert!(
            plan_on.extra_ro_stores.is_empty(),
            "os_hide on: no other CLI store is admitted: {:?}",
            plan_on.extra_ro_stores
        );

        let toggles_off = crate::config::SandboxToggles { os_hide: false, engine_deny: true };
        let run_off = RunSandbox::for_role(
            Some(SandboxMechanism::SystemdRun),
            false,
            CliKind::Claude,
            false,
            toggles_off,
        );
        let plan_off = run_off.build_plan_with_home(&ws, home.path());
        assert!(
            plan_off.extra_ro_stores.iter().any(|p| p.to_string_lossy().contains("opencode")),
            "os_hide off: the other CLI store is admitted read-only: {:?}",
            plan_off.extra_ro_stores
        );
        // engine_deny still covers every store (self + others) at the CLI layer.
        let deny = run_off.engine_deny_paths();
        assert!(deny.iter().any(|p| p.contains("/.claude/**")));
        assert!(deny.iter().any(|p| p.contains("opencode") && p.ends_with("/**")));
    }

    // a006: engine_deny off contributes no read-deny patterns.
    #[test]
    fn engine_deny_off_yields_no_patterns() {
        let toggles = crate::config::SandboxToggles { os_hide: true, engine_deny: false };
        let run = RunSandbox::for_role(None, true, CliKind::Claude, true, toggles);
        assert!(run.engine_deny_paths().is_empty());
    }

    // a006: the unenforced default skips the OS layer (existing behavior).
    #[test]
    fn default_run_sandbox_is_unenforced() {
        let run = RunSandbox::default();
        assert!(!run.enforce);
        assert!(run.engine_deny_paths().is_empty() || !run.enforce);
    }

    // ----- Gated enforcement integration tests (tasks 8.1–8.3) -----
    // These exercise REAL kernel enforcement and so run only where a mechanism
    // is usable; elsewhere (e.g. unprivileged CI) they skip so `cargo test`
    // stays green. They use std::process directly with the argv builders.

    fn run_wrapped(plan: &SandboxPlan, program: &str, args: &[&str]) -> std::process::Output {
        let mech = detect_mechanism().expect("caller checked a mechanism is available");
        let inner = InnerCommand {
            program: OsString::from(program),
            args: args.iter().map(OsString::from).collect(),
            env: Vec::new(),
        };
        let argv = match mech {
            SandboxMechanism::SystemdRun => systemd_run_argv(plan, &inner),
            SandboxMechanism::Bwrap => bwrap_argv(plan, &inner),
        };
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .expect("wrapped command spawns")
    }

    // task 8.1: a path outside the allowlist is unreadable from inside the
    // sandbox even via a Bash `cat` (the read fails because the path is not in
    // the namespace — not the wrapped CLI's deny rule).
    #[test]
    fn enforced_out_of_allowlist_read_fails() {
        if detect_mechanism().is_none() {
            eprintln!("skipping enforced_out_of_allowlist_read_fails: no sandbox mechanism");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let ws = home.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        // A secret under HOME (hidden by ProtectHome=tmpfs / --tmpfs <home>),
        // NOT inside the workspace nor any admitted store.
        let secret = home.path().join("secret.txt");
        std::fs::write(&secret, "TOPSECRET").unwrap();

        let run = RunSandbox::for_role(
            detect_mechanism(),
            false,
            CliKind::Claude,
            true,
            crate::config::SandboxToggles::default(),
        );
        let plan = run.build_plan_with_home(&ws, home.path());
        let out = run_wrapped(&plan, "cat", &[secret.to_str().unwrap()]);
        assert!(
            !out.status.success() && !String::from_utf8_lossy(&out.stdout).contains("TOPSECRET"),
            "a secret outside the allowlist must be unreadable inside the sandbox"
        );
    }

    // task 8.2: the executor's workspace is writable; a read-only role's
    // workspace write fails.
    #[test]
    fn enforced_workspace_write_posture() {
        if detect_mechanism().is_none() {
            eprintln!("skipping enforced_workspace_write_posture: no sandbox mechanism");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let ws = home.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();

        // Executor (writable): touching a workspace file succeeds.
        let rw = RunSandbox::for_role(
            detect_mechanism(),
            false,
            CliKind::Claude,
            true,
            crate::config::SandboxToggles::default(),
        );
        let plan_rw = rw.build_plan_with_home(&ws, home.path());
        let out_rw = run_wrapped(&plan_rw, "touch", &[ws.join("rw.txt").to_str().unwrap()]);
        assert!(out_rw.status.success(), "executor workspace must be writable");

        // Read-only role: the same write fails.
        let ro = RunSandbox::for_role(
            detect_mechanism(),
            false,
            CliKind::Claude,
            false,
            crate::config::SandboxToggles::default(),
        );
        let plan_ro = ro.build_plan_with_home(&ws, home.path());
        let out_ro = run_wrapped(&plan_ro, "touch", &[ws.join("ro.txt").to_str().unwrap()]);
        assert!(
            !out_ro.status.success(),
            "a read-only role must NOT be able to write the workspace"
        );
    }

    // task 8.3: a capability-gated operation (raw/packet socket open) fails
    // inside the sandbox because CAP_NET_RAW is not in the bounding set.
    #[test]
    fn enforced_raw_socket_open_fails() {
        if detect_mechanism().is_none() {
            eprintln!("skipping enforced_raw_socket_open_fails: no sandbox mechanism");
            return;
        }
        if which("python3") {
            // ok
        } else {
            eprintln!("skipping enforced_raw_socket_open_fails: python3 absent");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let ws = home.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let run = RunSandbox::for_role(
            detect_mechanism(),
            false,
            CliKind::Claude,
            true,
            crate::config::SandboxToggles::default(),
        );
        let plan = run.build_plan_with_home(&ws, home.path());
        // Exit 0 only if a raw packet socket opened (which the dropped
        // CAP_NET_RAW must prevent).
        let prog = "import socket,sys\n\
                    try:\n  s=socket.socket(socket.AF_PACKET, socket.SOCK_RAW)\n  s.close()\n  sys.exit(0)\n\
                    except Exception:\n  sys.exit(3)\n";
        let out = run_wrapped(&plan, "python3", &["-c", prog]);
        assert!(
            !out.status.success(),
            "opening a raw packet socket must fail with CAP_NET_RAW dropped"
        );
    }
}
