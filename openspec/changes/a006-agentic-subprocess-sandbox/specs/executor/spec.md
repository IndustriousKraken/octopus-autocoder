# executor — delta for a006-agentic-subprocess-sandbox

## ADDED Requirements

### Requirement: Every agentic subprocess runs inside an OS-level sandbox
Every role that spawns a CLI through the shared `agentic_run` primitive — the executor, every audit, AND any agentic role added by other changes (e.g. an agentic reviewer) — SHALL have that subprocess wrapped in an OS-level sandbox enforced by the kernel, NOT by the wrapped CLI's own sandbox. The wrap is a property of the single `agentic_run` spawn seam, so no role can opt out. The in-process HTTP roles (the non-agentic `oneshot` reviewer AND the contradiction-check LLM block) spawn no subprocess and are out of scope. This requirement governs the OS-level sandbox; it does not change the canonical tool-use-sandbox scoping (the CLI permission layer), which sits beside it.

The sandbox SHALL be applied via `systemd-run` in transient-service mode (so PID 1 applies the filesystem and namespace properties; stdout captured with `--pipe --wait --collect`), with a bubblewrap (`bwrap`) fallback for hosts without a usable system manager (unprivileged or non-systemd / in-container). It SHALL enforce, identically for every role:

- **Filesystem allowlist (default-deny).** The subprocess sees only: the workspace, the running role's own CLI config store (read-only, so the CLI can authenticate), AND the minimal runtime (binaries/libraries, a private `/tmp`, a restricted `/dev`). The home directory, every other CLI's config store, autocoder's own config and state, `~/.ssh`, AND the rest of the host are NOT in the namespace. The workspace SHALL be mounted read-write for the executor AND read-only for read-only roles (audits, agentic reviewer).
- **Capability drops.** At minimum `CAP_NET_RAW` (no raw-socket sniffing), `CAP_NET_ADMIN` (no route/iptables hijack), AND `CAP_SYS_PTRACE` (no reading another process's memory); `NoNewPrivileges`; address families restricted to exclude `AF_PACKET`.
- **Process-table restriction.** `/proc` mounted so the subprocess cannot read another process's `environ` or `mem`.

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

### Requirement: CLI config stores are protected by OS-hide and engine-deny
A model running as one CLI SHALL NOT be able to read another CLI's credential/config store, AND SHALL be deterred from reading its own. Two complementary layers enforce this, each independently toggleable (`os_hide`, `engine_deny`), both ON by default:

- **`os_hide` (filesystem allowlist; fail-closed).** Under the OS-level sandbox, the config store of every CLI OTHER than the running role's own is absent from the subprocess namespace. It cannot protect the running role's OWN store, which must stay readable for the CLI to authenticate; it protects every other store. Because it is an allowlist, a store no one enumerated is hidden by default.
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
