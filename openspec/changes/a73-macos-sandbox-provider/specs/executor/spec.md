# executor — delta for a73-macos-sandbox-provider

## MODIFIED Requirements

### Requirement: Every agentic subprocess runs inside an OS-level sandbox
Every role that spawns a CLI through the shared `agentic_run` primitive — the executor, every audit, AND any agentic role added by other changes (e.g. an agentic reviewer) — SHALL have that subprocess wrapped in an OS-level sandbox enforced by the kernel, NOT by the wrapped CLI's own sandbox. The wrap is a property of the single `agentic_run` spawn seam, so no role can opt out. The in-process HTTP roles (the non-agentic `oneshot` reviewer AND the contradiction-check LLM block) spawn no subprocess and are out of scope. This requirement governs the OS-level sandbox; it does not change the canonical tool-use-sandbox scoping (the CLI permission layer), which sits beside it.

The sandbox SHALL be applied by a **platform-appropriate mechanism**: on Linux via `systemd-run` in transient-service mode (so PID 1 applies the filesystem and namespace properties; stdout captured with `--pipe --wait --collect`), with a bubblewrap (`bwrap`) fallback for hosts without a usable system manager (unprivileged or non-systemd / in-container); on macOS via `sandbox-exec` (the Seatbelt sandbox) applied with a generated profile. On every platform the mechanism SHALL enforce, identically for every role:

- **Filesystem allowlist (default-deny).** The subprocess sees only: the workspace, the running role's own CLI config store (read-only, so the CLI can authenticate), AND the minimal runtime (binaries/libraries, a private `/tmp`, a restricted `/dev`). The home directory, every other CLI's config store, autocoder's own config and state, `~/.ssh`, AND the rest of the host are NOT reachable. The workspace SHALL be read-write for the executor AND read-only for read-only roles (audits, agentic reviewer).
- **Capability / operation restriction.** On Linux: drop `CAP_NET_RAW` (no raw-socket sniffing), `CAP_NET_ADMIN` (no route/iptables hijack), AND `CAP_SYS_PTRACE` (no reading another process's memory); `NoNewPrivileges`; address families restricted to exclude `AF_PACKET`. On macOS the generated Seatbelt profile SHALL deny the equivalents where the platform exposes them — raw/packet networking, inspection of other processes, AND privilege elevation.
- **Process-table restriction.** On Linux, `/proc` mounted so the subprocess cannot read another process's `environ` or `mem`. On macOS (which has no `/proc`), the Seatbelt profile SHALL deny process-information access to other processes — the platform analog.

Outbound network egress SHALL NOT be restricted by this sandbox: network egress control belongs to the host firewall, not the daemon, AND no maintainable in-app allowlist exists for CDN'd API/forge hosts. The sandbox does filesystem and host isolation, not a network allowlist.

#### Scenario: Workspace is present, the rest of the host is not
- **WHEN** any role spawns through `agentic_run` under the default sandbox
- **THEN** the subprocess can read — and, for the executor, write — the workspace
- **AND** paths outside the allowlist (the home directory, `~/.ssh`, autocoder's config/state directory) are absent from the subprocess's filesystem namespace

#### Scenario: A credential outside the allowlist is unreadable even via Bash
- **WHEN** the spawned agent attempts to read a credential outside the allowlist (e.g. `~/.ssh/id_ed25519` or autocoder's config) through any tool, including a `Bash` command such as `cat`, `head`, or `python -c open()`
- **THEN** the read fails because the path is not in the namespace
- **AND** the failure does not depend on the wrapped CLI's own permission rules

#### Scenario: Read-only roles get a read-only workspace
- **WHEN** a read-only role (an audit or an agentic reviewer) spawns through `agentic_run`
- **THEN** the workspace is mounted read-only
- **AND** an attempt by that role to modify a workspace file fails

#### Scenario: Capability drops block sniffing and cross-process reads
- **WHEN** the spawned agent attempts to open a raw/packet socket OR to ptrace or read another process's memory
- **THEN** the operation fails because the capability is not in the subprocess's bounding set

#### Scenario: Enforcement is external to the CLI
- **WHEN** the wrapped CLI's own sandbox configuration would otherwise permit an out-of-allowlist read
- **THEN** the read still fails, because the OS-level allowlist is enforced by the kernel around the subprocess regardless of the CLI's settings

#### Scenario: Fallback on a host without a usable system manager
- **WHEN** the daemon runs where `systemd-run` cannot apply the sandbox (unprivileged or non-systemd environment) AND `bwrap` is available
- **THEN** `agentic_run` applies the equivalent allowlist, capability drops, AND `/proc` restriction via the `bwrap` fallback
- **AND** no unsandboxed subprocess is spawned

#### Scenario: macOS applies the sandbox via sandbox-exec
- **WHEN** the daemon runs on macOS
- **THEN** `agentic_run` applies the OS-level sandbox via `sandbox-exec` with a generated Seatbelt profile enforcing the filesystem allowlist (including the read-only-workspace rule for read-only roles)
- **AND** no unsandboxed subprocess is spawned

#### Scenario: An out-of-allowlist read is denied on macOS
- **WHEN** on macOS the spawned agent attempts to read a path outside the allowlist (e.g. `~/.ssh/id_ed25519` or autocoder's config), including via a `Bash` command
- **THEN** the read is denied by the Seatbelt profile
- **AND** the failure does not depend on the wrapped CLI's own permission rules
