# orchestrator-cli delta: app-under-test-e2e

## ADDED Requirements

### Requirement: `app_under_test` per-repository config schema
The `RepositoryConfig` schema SHALL accept an optional `app_under_test` block declaring how to start the repository's application, how to determine it is ready, and how to run its end-to-end suite. The block is operator-owned: it lives in autocoder's own configuration, and autocoder SHALL NOT read it from the target repository's working tree, so a working session cannot alter the terms on which its own work is verified.

The block SHALL carry: `start_command` (shell command, required), `ready_check` (an HTTP path or a shell command, required), `e2e_command` (shell command, required), `working_dir` (optional, relative to the workspace root, defaulting to the workspace root), `ready_timeout_secs` (optional, default 60), `e2e_timeout_secs` (optional, default 600), and `e2e_test_paths` (optional list of workspace-relative glob patterns identifying the repository's end-to-end tests).

When `e2e_test_paths` is absent, autocoder SHALL use a documented built-in default set of patterns. When present, the configured list SHALL REPLACE the defaults rather than extend them, so an operator whose layout the defaults misidentify can state the truth for their repository without fighting an inherited pattern.

A repository with no `app_under_test` block SHALL behave exactly as it does today: no application is started, no end-to-end verification runs, no prompt block is added, and no PR section is emitted. Absent config is never an error.

#### Scenario: Absent block leaves behavior unchanged
- **WHEN** a repository entry has no `app_under_test` block
- **THEN** no application lifecycle, end-to-end run, or red-green replay is attempted for that repository
- **AND** the implementer prompt and PR body are byte-identical to what they would have been before this change

#### Scenario: Incomplete block is rejected at config load
- **WHEN** an `app_under_test` block is present but omits `start_command`, `ready_check`, OR `e2e_command`
- **THEN** `Config::load_from` fails with an error naming the repository entry AND the missing field
- **AND** `check-config` reports the same error without side effects

#### Scenario: `e2e_test_paths` defaults and override
- **WHEN** `e2e_test_paths` is absent
- **THEN** the built-in default patterns identify the repository's end-to-end tests
- **AND** when the field IS present, the configured patterns are used INSTEAD of the defaults, not in addition to them

#### Scenario: Timeout defaults and clamping
- **WHEN** `ready_timeout_secs` or `e2e_timeout_secs` is absent
- **THEN** the effective values are `60` and `600` seconds respectively
- **AND** when a configured value is `0`, autocoder clamps the effective value to the documented minimum AND emits exactly one WARN at startup naming the field path, matching the existing numeric-knob clamp precedent
- **AND** the loaded `Config` retains the raw configured value so operator-visible diagnostics show what was configured

#### Scenario: Config is not sourced from the target repository
- **WHEN** the target repository's working tree contains a file that would declare an application under test
- **THEN** autocoder ignores it entirely and uses only the operator's `app_under_test` block

### Requirement: Application under test is daemon-owned for the duration of a pass
When a repository declares `app_under_test` AND its end-to-end toolchain is available, autocoder SHALL own the application's lifecycle for the pass: it allocates an ephemeral TCP port, starts `start_command` with that port supplied through the environment, waits for `ready_check` to succeed within `ready_timeout_secs`, and tears the started process tree down when the pass ends.

Teardown SHALL occur on every exit path — normal completion, executor timeout, pass failure, panic, and daemon shutdown — so no application process outlives the pass that started it. A port SHALL be allocated per application instance rather than fixed, so neither concurrently-polling repositories nor a pass's application and a concurrent red-green replay instance can collide.

Establishing the application for a pass — port allocation, start, readiness, and teardown — SHALL be the daemon's responsibility and never the agent's. An agent-initiated restart of an already-established application during its own session is permitted: the restarted process is inside the session's process group and is reaped with it. No relaxation of `executor.sandbox.disallowed_bash_patterns` is required or permitted by this requirement: the readiness probe runs in daemon code, not through the agent's shell.

#### Scenario: Application is started and its URL injected
- **WHEN** a pass begins for a repository with a usable `app_under_test` block
- **THEN** autocoder allocates an ephemeral port, starts `start_command` in the configured `working_dir`, AND waits for `ready_check` to succeed
- **AND** on readiness, the resolved base URL is injected into the executor session's environment
- **AND** the port differs between two repositories polling concurrently
- **AND** the port differs from any port allocated to a concurrent red-green replay instance

#### Scenario: Teardown is guaranteed
- **WHEN** the pass ends for any reason — completion, executor timeout, failure, panic, or daemon shutdown
- **THEN** the started process tree is terminated
- **AND** no listening socket from that pass remains bound afterward

#### Scenario: Readiness timeout does not hold the queue
- **WHEN** `ready_check` does not succeed within `ready_timeout_secs`
- **THEN** autocoder terminates any started process, logs a WARN naming the repository AND the probe that failed, AND proceeds with the pass WITHOUT an application
- **AND** the implementer prompt omits the application block
- **AND** the PR body records that end-to-end verification did not run
- **AND** no change is held, no `.needs-spec-revision.json` is written, AND the perma-stuck counter is not incremented

### Requirement: End-to-end toolchain is probed by startup preflight and `doctor`
When any repository declares `app_under_test`, the dependency preflight SHALL probe for that repository's end-to-end toolchain and report the result through both daemon startup logging AND the `doctor` subcommand, following the existing config-implied-dependency pattern.

A missing toolchain SHALL disable end-to-end verification for that repository only, with a WARN naming the repository, the missing dependency, AND the provisioning command that would remediate it. It SHALL NOT abort daemon startup and SHALL NOT affect any other repository.

#### Scenario: Missing toolchain degrades one repository
- **WHEN** a repository declares `app_under_test` AND the end-to-end toolchain is unavailable
- **THEN** the daemon starts normally
- **AND** exactly one WARN names the repository AND the missing dependency
- **AND** that repository runs passes with no application and no end-to-end verification
- **AND** other repositories are unaffected

#### Scenario: `doctor` reports the probe result
- **WHEN** `doctor` runs against a config declaring `app_under_test`
- **THEN** its report includes the end-to-end toolchain probe result per declaring repository
- **AND** all probes are collected before reporting; the run does not stop at the first failure

### Requirement: End-to-end toolchain provisioning is operator-initiated, not manual host setup
autocoder SHALL provision the end-to-end toolchain through its own install wizard rather than requiring the operator to perform host package work by hand. The wizard SHALL offer an optional end-to-end testing section that provisions both tiers of the dependency: the privileged system packages the browser runtime requires, installed via the platform package manager; AND the browser binaries themselves, installed for the service account into a daemon-owned path.

The section SHALL be reachable on an existing installation via the `--reconfigure` flag, so adding end-to-end verification to a running deployment requires no reinstall and no manual package commands. It SHALL be drivable non-interactively for automated provisioning, consistent with the wizard's existing non-interactive support.

The browser-binary location SHALL be a daemon-owned path derived from the resolved cache directory rather than the service account's default user cache, AND autocoder SHALL export that location to both the executor session and the end-to-end command so the runtime resolves it identically inside the OS sandbox.

Provisioning SHALL NOT report success it has not confirmed. When the daemon runs as a service account, autocoder SHALL verify the provisioned location is readable AS that account before recording the browser tier as provisioned; a location that installed successfully but cannot be read is reported as NOT provisioned, naming the path and the account. Transferring ownership of the location does not by itself establish the account can reach it — every parent directory must also be traversable — so this is an attempted access, not an inspection of the location alone.

When a provisioning step fails, the reported diagnostic SHALL name the CAUSE of the failure. Tooling in this path emits progress and dependency notices before doing work, so the leading output line is not the diagnostic.

Provisioning SHALL NOT occur implicitly: never during a pass, never at daemon startup. A missing toolchain is reported and the feature degrades per the preflight requirement; autocoder does not silently download a browser runtime as a side effect of polling.

Project-level test dependencies — the end-to-end runner declared in the target repository's own manifest — are OUT of scope for autocoder provisioning; they belong to the repository and its working sessions.

#### Scenario: Wizard section provisions both dependency tiers
- **WHEN** the operator accepts the end-to-end testing section during `install`
- **THEN** autocoder installs the required system packages via the detected platform package manager
- **AND** installs the browser binaries for the service account into the daemon-owned path
- **AND** both steps run through the same mockable system-actions seam the wizard's existing package installation uses, so they are covered by `cargo test`

#### Scenario: Existing installation adds end-to-end support without a reinstall
- **WHEN** the operator runs the install wizard with `--reconfigure` and selects the end-to-end testing section on a host where autocoder is already installed
- **THEN** the section runs against the existing installation
- **AND** no other configuration section is modified

#### Scenario: Browser location is daemon-owned and resolvable inside the sandbox
- **WHEN** an end-to-end command or executor session runs for a repository with a usable `app_under_test` block
- **THEN** the browser-binary location is exported to that process
- **AND** the location is derived from the resolved cache directory, not the service account's default user cache
- **AND** the location is readable from inside the OS sandbox

#### Scenario: No implicit provisioning
- **WHEN** a pass runs, or the daemon starts, and the end-to-end toolchain is absent
- **THEN** autocoder does NOT download or install any component
- **AND** the absence is reported per the preflight requirement

#### Scenario: Unsupported platform reports rather than aborts
- **WHEN** the end-to-end section runs on a host with no supported package manager
- **THEN** autocoder reports which components it could not provision AND the manual commands that would provision them
- **AND** the remainder of the installation completes normally

#### Scenario: An installed-but-unreadable location is not reported as provisioned
- **WHEN** the browser binaries install successfully AND the service account cannot read the resulting location
- **THEN** the browser tier is reported as NOT provisioned, naming the path AND the account
- **AND** the overall result is not fully provisioned, so end-to-end verification stays disabled rather than failing later at pass time

#### Scenario: A failed step names the cause
- **WHEN** a provisioning step fails after its tooling emitted leading progress or dependency notices
- **THEN** the reported diagnostic names the actual failure, not a leading notice

#### Scenario: Re-running the section is idempotent
- **WHEN** the end-to-end section runs on a host where the toolchain is already provisioned
- **THEN** the run completes without error AND without duplicating installed state

### Requirement: End-to-end verification results are reported in the PR body
When an application was running for a pass, autocoder SHALL run `e2e_command` after the executor completes and SHALL record the outcome in the pull request body under a `## End-to-end verification` section naming the command, its exit status, and its captured summary output.

The **exit code of `e2e_command` is the authoritative verification signal**. No agent-produced narrative, screenshot interpretation, or self-report SHALL substitute for it. When no application was running, the section SHALL state that verification did not run rather than being omitted silently.

#### Scenario: Passing suite is reported
- **WHEN** `e2e_command` exits zero within `e2e_timeout_secs`
- **THEN** the PR body carries `## End-to-end verification` naming the command AND its passing status

#### Scenario: Failing suite is reported and drafts the PR
- **WHEN** `e2e_command` exits non-zero
- **THEN** the PR body records the failure AND the captured output
- **AND** the PR is opened as a draft

#### Scenario: Timeout is a non-passing outcome
- **WHEN** `e2e_command` does not complete within `e2e_timeout_secs`
- **THEN** the process tree is terminated, the section records the timeout, AND the PR is opened as a draft
- **AND** the timeout is never reported as a pass

#### Scenario: Verification did not run
- **WHEN** no application was running for the pass (unconfigured, toolchain missing, or readiness failed)
- **THEN** the section states that end-to-end verification did not run AND why
- **AND** the absence of verification is never rendered as a pass

### Requirement: Red-green replay detects vacuous end-to-end tests
When a pass adds or modifies end-to-end test files AND an application was running, autocoder SHALL replay those tests against the pre-change tree: a scratch git worktree at the pass's base commit, overlaid with only the new or modified end-to-end test files, with the application started from that tree.

The replay's application instance SHALL receive its own ephemeral port allocation, distinct from the port bound by the pass's application instance, so the two never contend for the same port. The replay instance carries the same teardown guarantee as the pass's: it is terminated on every exit path.

The replayed tests SHALL be required to **fail**. A test that passes without the change under test is not evidence that the change works — it is either vacuous or the behavior already existed. On a pass-against-base, autocoder SHALL open the PR as a draft AND name the specific tests that passed against the base commit, so a human resolves the ambiguity. The pass's committed work SHALL NOT be discarded, no change SHALL be held, and the perma-stuck counter SHALL NOT be incremented.

End-to-end test files are identified by matching the pass's changed paths against `e2e_test_paths` (or the built-in defaults when unset). The replay SHALL be skipped when the pass added or modified no file matching those patterns, AND the pull request SHALL say the replay did not run rather than implying it passed.

The replay SHALL also be skipped when the suite did not pass against the change itself. Its question — do these tests detect the change? — is only meaningful once the tests are known to pass WITH the change; replaying tests that fail in both trees would report a red result that means nothing. A skip for this reason SHALL state it, and the failing suite already drafts the pull request on its own.

#### Scenario: Vacuous test drafts the PR
- **WHEN** a new end-to-end test passes when replayed against the base commit
- **THEN** the PR is opened as a draft
- **AND** the PR body names each test that passed against base
- **AND** the committed work is retained on the agent branch

#### Scenario: Genuine test replays red
- **WHEN** every new or modified end-to-end test fails when replayed against the base commit AND passes on the agent branch
- **THEN** no vacuous-test finding is recorded AND the draft state is not forced by this requirement

#### Scenario: Replay is skipped when the suite did not pass
- **WHEN** the end-to-end suite did not pass against the change
- **THEN** the replay does not run AND the pull request states why
- **AND** the pull request is already a draft on account of the failing suite

#### Scenario: Replay is skipped when no e2e test changed
- **WHEN** a pass modifies no end-to-end test file
- **THEN** no scratch worktree is created AND no replay runs
- **AND** the PR body's end-to-end section reports the suite result without a replay finding

#### Scenario: Replay does not disturb the agent branch
- **WHEN** the replay runs
- **THEN** it operates in a scratch worktree at the base commit
- **AND** the agent branch, its working tree, and its committed content are unmodified by the replay
- **AND** its application instance binds a port distinct from the pass's application instance
- **AND** the scratch worktree AND any application it started are removed when the replay ends, on every exit path
