# executor — delta for a013-sandbox-denylist-model

## MODIFIED Requirements

### Requirement: Every agentic subprocess runs inside an OS-level sandbox
Every role that spawns a CLI through the shared `agentic_run` primitive — the executor, every audit, AND any agentic role added by other changes (e.g. an agentic reviewer) — SHALL have that subprocess wrapped in an OS-level sandbox enforced by the kernel, NOT by the wrapped CLI's own sandbox. The wrap is a property of the single `agentic_run` spawn seam, so no role can opt out. The in-process HTTP roles (the non-agentic `oneshot` reviewer AND the contradiction-check LLM block) spawn no subprocess and are out of scope. This requirement governs the OS-level sandbox; it does not change the canonical tool-use-sandbox scoping (the CLI permission layer), which sits beside it.

The sandbox SHALL be applied by a **platform-appropriate mechanism**: on Linux via `systemd-run` in transient-service mode (so PID 1 applies the filesystem and namespace properties; stdout captured with `--pipe --wait --collect`), with a bubblewrap (`bwrap`) fallback for hosts without a usable system manager; on macOS via `sandbox-exec` (the Seatbelt sandbox) with a generated profile.

**The filesystem policy is role-dependent**, because the executor must run the project's build toolchain while read-only roles only read:

- **Executor — exposed home, default-deny mask-list (denylist).** The home directory SHALL be present AND writable, so build toolchains installed under `$HOME` (`~/.cargo`, `~/.rustup`, `~/.nvm`, `~/.pyenv`, `~/.rbenv`, caches, …) work without enumeration — EXCEPT a default-deny **mask-list** of sensitive paths which SHALL be masked (replaced with empty or inaccessible mounts). The mask-list covers **credential paths** (read-protection: `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.netrc`, cloud-token dirs, other CLIs' config stores, package-manager credential files such as `~/.cargo/credentials.toml` / `~/.npmrc`) AND **shell-init/persistence paths** (write-protection: `~/.bashrc`, `~/.profile`, `~/.ssh/authorized_keys`, autostart/cron). It ships with defaults AND is operator-editable (see the orchestrator-cli config requirement). System paths outside `$HOME` are visible read-only.
- **Read-only roles (audits, agentic reviewer) — allowlist.** The home directory SHALL be masked; the subprocess sees only the workspace (read-only), the running role's own CLI config store (read-only, for authentication), the **resolved CLI binary AND its runtime dependency closure** (following symlinks, even when installed under `~/.local/bin`), AND the minimal runtime.
- **Strict mode — opt-in allowlist for the executor.** An operator MAY opt the executor into the read-only-style allowlist (home masked; only the workspace read-write, the role's own store, the resolved CLI binary + toolchain, and the minimal runtime bound) for high-compliance hosts. This is NOT the default.

The workspace SHALL be read-write for the executor AND read-only for read-only roles, in every policy.

- **Capability / operation restriction.** On Linux: drop `CAP_NET_RAW` (no raw-socket sniffing), `CAP_NET_ADMIN` (no route/iptables hijack), AND `CAP_SYS_PTRACE` (no reading another process's memory); `NoNewPrivileges`; address families restricted to exclude `AF_PACKET`. On macOS the generated Seatbelt profile SHALL deny the equivalents where the platform exposes them — raw/packet networking, inspection of other processes, AND privilege elevation.
- **Process-table restriction.** On Linux, `/proc` mounted so the subprocess cannot read another process's `environ` or `mem`. On macOS (which has no `/proc`), the Seatbelt profile SHALL deny process-information access to other processes.

Outbound network egress SHALL NOT be restricted by this sandbox: network egress control belongs to the host firewall, not the daemon. The sandbox does filesystem and host isolation, not a network allowlist.

#### Scenario: The executor sees the host toolchains under an exposed home
- **WHEN** the executor spawns through `agentic_run` under the default (denylist) policy
- **THEN** the home directory and its build toolchains (e.g. `~/.cargo`, `~/.pyenv`, `~/.nvm`) are readable AND tool caches are writable
- **AND** the workspace is read-write

#### Scenario: A masked credential is unreadable even via Bash
- **WHEN** the spawned agent attempts to read a mask-listed credential (e.g. `~/.ssh/id_ed25519`, another CLI's store, or `~/.cargo/credentials.toml`) through any tool, including a `Bash` command such as `cat`, `head`, or `python -c open()`
- **THEN** the read fails because the path is masked
- **AND** the failure does not depend on the wrapped CLI's own permission rules

#### Scenario: A masked persistence file cannot be written
- **WHEN** the spawned agent attempts to write a mask-listed persistence file (e.g. `~/.bashrc` or `~/.ssh/authorized_keys`)
- **THEN** the write does not persist to the real file because the path is masked

#### Scenario: Read-only roles get a home-masked allowlist with the CLI binary bound
- **WHEN** a read-only role (an audit or an agentic reviewer) spawns through `agentic_run`
- **THEN** the home directory is masked AND the role sees only the read-only workspace, its own CLI store, the resolved CLI binary (following symlinks, even under `~/.local/bin`) plus its dependency closure, and the minimal runtime
- **AND** an attempt by that role to modify a workspace file fails

#### Scenario: The CLI binary is reachable regardless of policy
- **WHEN** the running role's CLI binary is installed under the home directory (e.g. `~/.local/bin/<cli>`)
- **THEN** under the executor's exposed-home policy it is simply visible (home is present)
- **AND** under an allowlist policy (a read-only role or strict mode) it is bound — following symlinks — with its dependency closure, read-only and executable, so the wrapped CLI execs

#### Scenario: Capability drops block sniffing and cross-process reads
- **WHEN** the spawned agent attempts to open a raw/packet socket OR to ptrace or read another process's memory
- **THEN** the operation fails because the capability is not in the subprocess's bounding set

#### Scenario: Enforcement is external to the CLI
- **WHEN** the wrapped CLI's own sandbox configuration would otherwise permit a masked or out-of-allowlist read
- **THEN** the read still fails, because the OS-level policy is enforced by the kernel around the subprocess regardless of the CLI's settings

#### Scenario: Fallback on a host without a usable system manager
- **WHEN** the daemon runs where `systemd-run` cannot apply the sandbox (unprivileged or non-systemd environment) AND `bwrap` is available
- **THEN** `agentic_run` applies the equivalent policy via the `bwrap` fallback
- **AND** no unsandboxed subprocess is spawned

#### Scenario: macOS applies the sandbox via sandbox-exec
- **WHEN** the daemon runs on macOS
- **THEN** `agentic_run` applies the OS-level policy via `sandbox-exec` with a generated Seatbelt profile (exposed-home-minus-mask-list for the executor; the allowlist for read-only roles)
- **AND** no unsandboxed subprocess is spawned

#### Scenario: Strict mode masks all of home for the executor
- **WHEN** the operator opts the executor into strict mode
- **THEN** the executor runs under the allowlist (home masked; only the workspace read-write, the role's own store, the resolved CLI binary + toolchain, and the minimal runtime bound)

### Requirement: CLI config stores are protected by OS-hide and engine-deny
A model running as one CLI SHALL NOT be able to read another CLI's credential/config store, AND SHALL be deterred from reading its own. Two complementary layers enforce this, each independently toggleable (`os_hide`, `engine_deny`), both ON by default:

- **`os_hide` (mask-list membership).** The config store of every CLI OTHER than the running role's own is in the sandbox **mask-list**, so it is masked (absent) from the subprocess regardless of the role's filesystem policy. It cannot protect the running role's OWN store, which must stay readable for the CLI to authenticate; it protects every other store. Turning `os_hide` off removes the other CLI stores from the mask-list (exposing them, for the wrapper-development case).
- **`engine_deny` (the wrapped CLI's own permission denylist; fail-open).** The per-invocation tool-use settings the executor already supplies to the CLI (the canonical "Tool-use sandbox is applied at every spawn" mechanism) SHALL deny the agent's file-reading tools (`Read`, AND the corresponding `Bash` patterns) on the config store of EVERY registered CLI strategy — the running role's own included. This covers the self-store that `os_hide` cannot, but is a string-pattern speed bump that determined shell indirection can evade: it deters, it does not bound.

The engine-deny rules SHALL be supplied per-invocation through each CLI's own settings mechanism (as the existing tool-use sandbox already does for `claude`), NOT by mutating the operator's global CLI configuration.

The running role's own CLI store stays readable by that same-uid subprocess because the CLI must read it to authenticate; disclosure of that one store means a model could impersonate that CLI to its own provider, never reach another provider's credential or another secret. This residual is NOT network-contained (egress is out of scope); it is bounded by the single-store blast radius AND by `engine_deny` deterrence.

#### Scenario: Under the default, another CLI's store is unreadable
- **WHEN** a role running as one CLI attempts to read a different registered CLI's config store under the default (`os_hide` on)
- **THEN** that store is absent from the namespace AND the read fails

#### Scenario: With os_hide off, other stores are still engine-denied
- **WHEN** `os_hide` is off for the run AND `engine_deny` is on
- **THEN** another CLI's config store is present in the namespace (so a nested CLI of that kind could authenticate)
- **AND** the agent's `Read`/`Bash` tools are denied that store's paths at the CLI permission layer

#### Scenario: The self-store authenticates but is engine-denied to the agent
- **WHEN** a role runs as a CLI whose own config store is in the namespace read-only for authentication
- **THEN** the CLI authenticates from that store
- **AND** the agent's `Read`/`Bash` tools are denied that store's paths at the CLI permission layer

#### Scenario: Deny rules are per-invocation, not global mutation
- **WHEN** the engine-deny rules are applied for a run
- **THEN** they are delivered via the per-invocation settings mechanism (e.g. the temp Claude Code settings file)
- **AND** the operator's global CLI configuration is not modified
