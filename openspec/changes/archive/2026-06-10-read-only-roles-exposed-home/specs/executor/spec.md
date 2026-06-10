# executor — delta for read-only-roles-exposed-home

## MODIFIED Requirements

### Requirement: Every agentic subprocess runs inside an OS-level sandbox
Every role that spawns a CLI through the shared `agentic_run` primitive — the executor, every audit, AND any agentic role added by other changes (e.g. an agentic reviewer) — SHALL have that subprocess wrapped in an OS-level sandbox enforced by the kernel, NOT by the wrapped CLI's own sandbox. The wrap is a property of the single `agentic_run` spawn seam, so no role can opt out. The in-process HTTP roles (the non-agentic `oneshot` reviewer AND the contradiction-check LLM block) spawn no subprocess and are out of scope. This requirement governs the OS-level sandbox; it does not change the canonical tool-use-sandbox scoping (the CLI permission layer), which sits beside it.

The sandbox SHALL be applied by a **platform-appropriate mechanism**: on Linux via `systemd-run` in transient-service mode (so PID 1 applies the filesystem and namespace properties; stdout captured with `--pipe --wait --collect`), with a bubblewrap (`bwrap`) fallback for hosts without a usable system manager; on macOS via `sandbox-exec` (the Seatbelt sandbox) with a generated profile.

**The default filesystem policy is the exposed-home denylist for every role**, because a wrapped CLI and the toolchains it drives live under `$HOME` (node/pyenv/rbenv/cargo, and the CLI's own install + session + caches). Roles differ only in **workspace** writability:

- **Exposed home, default-deny mask-list (denylist) — executor AND read-only roles.** The home directory SHALL be present AND writable, so toolchains installed under `$HOME` (`~/.cargo`, `~/.rustup`, `~/.nvm`, `~/.pyenv`, `~/.rbenv`, the CLI's own install + session, caches, …) work without enumeration — EXCEPT a default-deny **mask-list** of sensitive paths which SHALL be masked (replaced with empty or inaccessible mounts). The mask-list covers **credential paths** (read-protection: `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.netrc`, cloud-token dirs, other CLIs' config stores, package-manager credential files such as `~/.cargo/credentials.toml` / `~/.npmrc`) AND **shell-init/persistence paths** (write-protection: `~/.bashrc`, `~/.profile`, `~/.ssh/authorized_keys`, autostart/cron). It ships with defaults AND is operator-editable (see the orchestrator-cli config requirement). System paths outside `$HOME` are visible read-only.
- **Strict mode — opt-in masked-home allowlist.** An operator MAY opt into the masked-home allowlist for high-compliance hosts: the home directory SHALL be masked; the subprocess sees only the workspace, the running role's own CLI config store (read-only, for authentication), the **resolved CLI binary AND its runtime dependency closure** (following symlinks, even when installed under `~/.local/bin`), AND the minimal runtime. This is NOT the default, and it accepts that a toolchain-heavy CLI (e.g. a Node app whose runtime sprawls under `$HOME`) may be unable to start under the mask.

The workspace SHALL be read-write for the executor AND read-only for read-only roles, in every policy — EXCEPT that a read-only role's workspace SHALL expose a writable, ephemeral project-scratch subtree where the running CLI requires one (e.g. opencode writes `<workspace>/.opencode/` and crashes if it cannot). That subtree SHALL be overlaid writable (a tmpfs, discarded after the run, on the Linux mechanisms) so the CLI's project scratch works while the repo files stay read-only; it SHALL be derived from the role's resolved CLI, NOT operator-supplied. The home directory's read-WRITE exposure under the denylist applies to read-only roles too: their "read-only" is the workspace's tracked files, not the home — a read-only role may read the home AND write its own caches/session there, but SHALL NOT modify the repo.

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

#### Scenario: Read-only roles get the exposed home with a read-only workspace
- **WHEN** a read-only role (an audit, an agentic reviewer, or a verifier gate) spawns through `agentic_run` under the default policy
- **THEN** the home directory is present — readable so the CLI finds its toolchain runtime, AND writable so the CLI can write its own session/cache — with the credential mask-list still masked
- **AND** the workspace's tracked files are read-only: an attempt by that role to modify a repo file fails
- **AND** an attempt to read a mask-listed credential still fails

#### Scenario: A read-only role's CLI writes its project scratch
- **WHEN** a read-only role runs a CLI that writes a project-local scratch directory in its working directory (e.g. opencode writing `<workspace>/.opencode/`)
- **THEN** that scratch subtree is writable (overlaid on the read-only workspace), so the CLI does not crash on the write
- **AND** the rest of the workspace stays read-only (a repo-file write still fails)
- **AND** the scratch is ephemeral on the tmpfs mechanisms (its writes are discarded after the run, not persisted to the host workspace)

#### Scenario: The CLI binary is reachable regardless of policy
- **WHEN** the running role's CLI binary is installed under the home directory (e.g. `~/.local/bin/<cli>` or `~/.opencode/bin/<cli>`)
- **THEN** under the default denylist policy (the executor OR a read-only role) it is simply visible, with its runtime, because the home is present
- **AND** under the strict-mode allowlist it is bound — following symlinks — with its dependency closure, read-only and executable, so the wrapped CLI execs

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
- **THEN** `agentic_run` applies the OS-level policy via `sandbox-exec` with a generated Seatbelt profile (exposed-home-minus-mask-list for the executor AND read-only roles, with the workspace write-denied for read-only roles; the masked-home allowlist for strict mode)
- **AND** no unsandboxed subprocess is spawned

#### Scenario: Strict mode masks all of home for the executor
- **WHEN** the operator opts the executor into strict mode
- **THEN** the executor runs under the allowlist (home masked; only the workspace read-write, the role's own store, the resolved CLI binary + toolchain, and the minimal runtime bound)
