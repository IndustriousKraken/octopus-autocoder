# Implementation tasks

## 1. Probe the host sandbox mechanisms

- [ ] 1.1 Confirm the available `systemd-run` sandbox properties on the target by reading `systemd-run --help` and `systemd.exec(5)` on the host (the binary is installed where the daemon runs): `ProtectSystem=strict`, `ReadWritePaths=`, `ReadOnlyPaths=`, `ProtectHome=tmpfs`, `BindReadOnlyPaths=`, `InaccessiblePaths=`, `ProtectProc=invisible`/`ProcSubset=pid`, `CapabilityBoundingSet=`, `RestrictAddressFamilies=`, `NoNewPrivileges=`, `PrivateTmp=`, `PrivateDevices=`. Record which require service mode (PID 1 exec) vs scope.
- [ ] 1.2 Confirm the `bwrap` flags for the same view (`--ro-bind`, `--bind`, `--tmpfs`, `--proc`, `--dev`, `--unshare-*`, `--cap-drop`, `--die-with-parent`) on the host.
- [ ] 1.3 Determine, at daemon startup, whether `systemd-run` (service mode) is usable, else `bwrap`, else neither — for the mechanism-selection + fail-closed gate (section 4).

## 2. Wrap the `agentic_run` spawn in the OS-level sandbox (`agentic_run.rs`)

- [ ] 2.1 Spawn every `agentic_run` child through `systemd-run` in **transient service mode** (NOT `--scope`), capturing stdout/stderr with `--pipe --wait --collect`, so the existing streaming-JSON and simple-capture output modes are preserved unchanged. Keep the existing process-group + timeout + kill behavior around the wrapped command.
- [ ] 2.2 Build the per-role **filesystem allowlist**: workspace read-write for the executor and read-only for read-only roles (audits / agentic reviewer); the running role's own CLI config store read-only (for authentication); the minimal runtime (binaries/libraries, private `/tmp`, restricted `/dev`). Everything else — home, other CLIs' stores, autocoder config/state, `~/.ssh` — absent from the namespace.
- [ ] 2.3 Apply capability drops (`CAP_NET_RAW`, `CAP_NET_ADMIN`, `CAP_SYS_PTRACE`), `NoNewPrivileges`, an address-family restriction excluding `AF_PACKET`, and a `/proc` mount that hides other processes' `environ`/`mem`.
- [ ] 2.4 Do NOT add any egress/network allowlist — outbound is unrestricted at this layer by design.
- [ ] 2.5 Resolve the running role's "own CLI config store" path from the role's resolved `CliStrategy` so the allowlist admits exactly that one store.

## 3. `bwrap` fallback

- [ ] 3.1 Implement the equivalent allowlist, capability drops, and `/proc` restriction via `bwrap` for hosts where `systemd-run` cannot apply the sandbox, selected per the section 1.3 detection.

## 4. Mechanism gate (fail closed) + unsandboxed opt-in

- [ ] 4.1 When neither `systemd-run` nor `bwrap` is usable AND the operator has not opted into unsandboxed operation, fail an agentic run with a clear error naming the missing mechanism; spawn no unsandboxed subprocess.
- [ ] 4.2 Add an explicit config flag opting into unsandboxed operation; when set, proceed AND emit one loud startup WARN that subprocesses run unsandboxed.

## 5. Config-credential layers — `os_hide` + `engine_deny`

- [ ] 5.1 `os_hide`: when on (default), exclude every CLI strategy's config store EXCEPT the running role's own from the allowlist (section 2.2); when off, admit the other stores read-only so a nested CLI of that kind can authenticate.
- [ ] 5.2 `engine_deny`: extend the per-invocation tool-use deny set (the existing temp Claude Code settings / each CLI's equivalent permission mechanism) so the agent's `Read`/`Bash` tools are denied the config store of EVERY registered CLI strategy, the self-store included. Drive the path set from the registered strategies (so it grows as strategies are added), NOT a hardcoded list. Supply per-invocation; never mutate the operator's global CLI config.

## 6. Toggles, precedence, and logging (config + startup)

- [ ] 6.1 Add `os_hide` and `engine_deny` booleans to the global `executor.sandbox` config and to the per-repository config schema, each defaulting to ON.
- [ ] 6.2 Resolve effective values per repository: per-repo overrides global; absent both, the default. No implicit downgrade.
- [ ] 6.3 At startup, emit a per-repository WARN naming each toggle that is OFF for that repository.

## 7. Docs

- [ ] 7.1 Document the two toggles, the secure default, the presets, precedence, and the no-mechanism fail-closed behavior + opt-in (`docs/CONFIG.md` / `docs/DEPLOYMENT.md`).
- [ ] 7.2 Note that this repository wraps CLIs and so needs `os_hide` off (the `os_hide` off / `engine_deny` on preset) under the secure default, or its live cross-CLI development breaks.

## 8. Tests

- [ ] 8.1 A path outside the allowlist (`~/.ssh/...`, autocoder config) is unreadable from inside the sandbox even via a `Bash` `cat` (assert the read fails, not the wrapped CLI's deny rule).
- [ ] 8.2 The executor's workspace is writable; a read-only role's workspace write fails.
- [ ] 8.3 A capability-gated operation (raw socket open / ptrace) fails inside the sandbox.
- [ ] 8.4 Under the default (`os_hide` on), another registered CLI's config store is absent from the namespace; with `os_hide` off and `engine_deny` on, that store is present but the agent's `Read`/`Bash` tools are denied its paths.
- [ ] 8.5 Effective-toggle resolution: per-repo overrides global; the secure default applies when unset.
- [ ] 8.6 A repository running with a toggle off emits a per-repository startup WARN naming it (assert a WARN fires for the relaxed setting, not its exact wording).
- [ ] 8.7 With no available mechanism and no opt-in, an agentic run fails closed; with the explicit opt-in, it proceeds and emits the unsandboxed WARN.

## 9. Acceptance gate

- [ ] 9.1 `cargo test` passes for the autocoder crate.
- [ ] 9.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 9.3 `openspec validate a006-agentic-subprocess-sandbox --strict` passes.
