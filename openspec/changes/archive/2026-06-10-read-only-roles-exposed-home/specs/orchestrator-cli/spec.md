# orchestrator-cli — delta for read-only-roles-exposed-home

## MODIFIED Requirements

### Requirement: Sandbox credential-protection config — toggles, precedence, and relaxed-posture logging
autocoder SHALL expose the sandbox's credential-protection layers as two boolean toggles, `os_hide` AND `engine_deny`, configurable both globally (under the `executor.sandbox` block) AND per-repository, each defaulting to ON. A per-repository value SHALL override the global value for that repository; absent both, the default applies. Loosening either toggle SHALL be explicit — there is no implicit downgrade — AND the daemon SHALL emit a startup WARN, per repository, naming each toggle that is OFF for that repository.

The named presets (both on — the default; `os_hide` off with `engine_deny` on, for a repository that develops CLI wrappers and needs a nested CLI to authenticate live; both off, for a repository whose purpose is testing credential-grab behavior) are documentation over these two switches; the switches are the contract.

The filesystem **mask-list** — the default-deny set of paths masked under the exposed-home denylist (credential paths AND shell-init/persistence paths) — SHALL ship with sensible defaults AND be operator-editable both globally and per-repository: an operator MAY add a path to mask OR remove a default entry to expose it (e.g. open `~/.ssh` to develop an SSH tool). Removing a default mask entry SHALL be explicit AND logged at startup as a relaxed posture, since egress is unrestricted. `os_hide` governs the other-CLI-store subset of the mask-list as a named convenience toggle. The exposed-home denylist (home present read-write, mask-list masked) is the default for the executor AND read-only roles alike; they differ only in workspace writability (read-write for the executor, read-only for read-only roles). The executor MAY additionally be opted into **strict mode** — the masked-home allowlist (mask all of home; bind only the workspace, the role's own store, the resolved CLI binary + toolchain, and the minimal runtime) — for high-compliance hosts; strict mode is NOT the default, AND it is the ONLY policy that uses the masked-home allowlist.

Separately, when no **platform-appropriate** sandbox mechanism is available on the host — on Linux, neither `systemd-run` nor `bwrap` can apply the sandbox; on macOS, `sandbox-exec` is unavailable — agentic runs SHALL fail closed with a clear error naming the missing mechanism, UNLESS the operator has explicitly set a config flag opting into unsandboxed operation — in which case the daemon emits a loud startup WARN AND proceeds. On macOS, `sandbox-exec` ships with the operating system, so the gate is normally satisfied without any install.

#### Scenario: Secure default when unset
- **WHEN** neither the global nor the per-repository config sets `os_hide` or `engine_deny`
- **THEN** both are ON for every repository
- **AND** no relaxed-posture WARN is emitted

#### Scenario: Per-repo overrides global
- **WHEN** the global config sets `os_hide` on AND a repository sets `os_hide` off
- **THEN** that repository runs with `os_hide` off
- **AND** repositories without a per-repo value run with `os_hide` on

#### Scenario: Relaxed posture is logged per repository
- **WHEN** a repository runs with `os_hide` OR `engine_deny` off (by per-repo or global config)
- **THEN** the daemon emits a startup WARN for that repository naming each toggle that is off

#### Scenario: No sandbox mechanism fails closed by default
- **WHEN** neither `systemd-run` nor `bwrap` can apply the sandbox AND the operator has not opted into unsandboxed operation
- **THEN** an agentic run fails with an error naming the missing mechanism
- **AND** no unsandboxed subprocess is spawned

#### Scenario: Explicit unsandboxed opt-in proceeds with a loud warning
- **WHEN** no sandbox mechanism is available AND the operator has explicitly opted into unsandboxed operation in config
- **THEN** agentic runs proceed
- **AND** the daemon emits a loud startup WARN that subprocesses are running unsandboxed

#### Scenario: macOS satisfies the gate via sandbox-exec
- **WHEN** the daemon runs on macOS AND `sandbox-exec` is available
- **THEN** the mechanism gate is satisfied AND agentic runs proceed sandboxed
- **AND** the run does not fail closed

#### Scenario: Editing the mask-list adds or exposes a path
- **WHEN** an operator adds a path to the mask-list
- **THEN** that path is masked for the executor
- **AND** WHEN an operator removes a default mask entry (e.g. `~/.ssh`), that path is exposed AND the daemon emits a startup relaxed-posture WARN naming it

#### Scenario: Strict mode masks all of home
- **WHEN** the executor is opted into strict mode
- **THEN** it runs under the allowlist (home masked; only the workspace, the role's own store, the resolved CLI binary + toolchain, and the minimal runtime bound)
