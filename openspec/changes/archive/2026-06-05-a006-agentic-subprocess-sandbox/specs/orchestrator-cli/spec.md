# orchestrator-cli — delta for a006-agentic-subprocess-sandbox

## ADDED Requirements

### Requirement: Sandbox credential-protection config — toggles, precedence, and relaxed-posture logging
autocoder SHALL expose the sandbox's credential-protection layers as two boolean toggles, `os_hide` AND `engine_deny`, configurable both globally (under the `executor.sandbox` block) AND per-repository, each defaulting to ON. A per-repository value SHALL override the global value for that repository; absent both, the default applies. Loosening either toggle SHALL be explicit — there is no implicit downgrade — AND the daemon SHALL emit a startup WARN, per repository, naming each toggle that is OFF for that repository.

The named presets (both on — the default; `os_hide` off with `engine_deny` on, for a repository that develops CLI wrappers and needs a nested CLI to authenticate live; both off, for a repository whose purpose is testing credential-grab behavior) are documentation over these two switches; the switches are the contract.

Separately, when no sandbox mechanism is available on the host (neither `systemd-run` nor `bwrap` can apply the sandbox), agentic runs SHALL fail closed with a clear error naming the missing mechanism, UNLESS the operator has explicitly set a config flag opting into unsandboxed operation — in which case the daemon emits a loud startup WARN AND proceeds.

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
