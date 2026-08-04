## 1. Config schema

- [x] 1.1 In `autocoder/src/config.rs`, add an `AppUnderTestConfig` struct (`start_command`, `ready_check`, `e2e_command`, optional `working_dir`, `ready_timeout_secs`, `e2e_timeout_secs`) as an optional field on `RepositoryConfig`, with `#[serde(deny_unknown_fields)]` consistent with the surrounding structs.
- [x] 1.2 In `Config::load_from`, validate the block: a present block requires all three commands; clamp the two timeouts to their documented minimums with exactly one WARN per clamped field naming the field path, retaining the raw configured value on the loaded `Config`.
- [x] 1.3 Confirm `check-config` surfaces the same validation errors with no side effects (it shares the load path; add coverage rather than new logic if so).

## 2. Dependency preflight

- [x] 2.1 In `autocoder/src/dependency_preflight.rs`, add a config-implied probe for the end-to-end toolchain, run once per declaring repository, behind the existing `DepProbe` trait so the report logic stays pure.
- [x] 2.2 A missing toolchain disables end-to-end verification for that repository only: one startup WARN naming the repository, the missing dependency, and the provisioning command that remediates it. Daemon startup and other repositories are unaffected.
- [x] 2.3 Surface the probe result in `doctor`, collecting all probes before reporting (never stop at the first failure).

## 3. Toolchain provisioning

- [x] 3.1 Add an optional end-to-end testing section to the install wizard in `autocoder/src/cli/install.rs`, behind the existing `SystemActions` trait so `cargo test` drives it with the recording mock exactly as the `git` / `bubblewrap` / `gh` package installs are driven today.
- [x] 3.2 The section installs the privileged system packages via the detected platform package manager, reusing the existing multi-package-manager detection (`apt-get`/`dnf`/`pacman`/`zypper`/`brew`).
- [x] 3.3 The section installs the browser binaries for the service account into a daemon-owned path derived from the resolved cache directory (the systemd unit already provides `CacheDirectory=autocoder`), not the service account's default user cache.
- [x] 3.4 Export the browser-binary location to the executor session and the end-to-end command, and confirm the path is readable from inside the OS sandbox for both read-only and read-write roles.
- [x] 3.5 Wire the section into `--reconfigure` so an existing installation can add end-to-end support without a reinstall or manual package commands, and into the wizard's non-interactive mode with a corresponding flag.
- [x] 3.6 Make the section idempotent, and on a host with no supported package manager report the unprovisioned components plus the manual commands without aborting the rest of the installation.
- [x] 3.7 Assert no provisioning path is reachable from a pass or from daemon startup — provisioning is operator-initiated only.

## 4. Application lifecycle

- [x] 4.1 Add a pass-scoped application lifecycle helper: allocate an ephemeral TCP port per application instance, start `start_command` in the resolved `working_dir` with the port in the environment, poll `ready_check` until success or `ready_timeout_secs`.
- [x] 4.2 Guarantee teardown of the started process tree on every exit path — completion, executor timeout, pass failure, panic, daemon shutdown — using a guard type in the style of the existing `IterationGuard`.
- [x] 4.3 On readiness failure: terminate anything started, WARN naming the repository and the failed probe, proceed with the pass without an application. Do not hold the change, write `.needs-spec-revision.json`, or increment the perma-stuck counter.
- [x] 4.4 Inject the resolved base URL into the executor session environment; confirm no change to `executor.sandbox.disallowed_bash_patterns` is needed (the readiness probe runs in daemon code, not the agent's shell).

## 5. Implementer prompt block

- [x] 5.1 In `prompts/implementer.md` and its builder in `autocoder/src/executor/claude_cli.rs`, add the conditional "Application under test" block following the "Prior iteration summary" precedent: base URL, e2e command, exit-code-is-authoritative, screenshots-are-debugging-aids.
- [x] 5.2 Omit the block entirely when no application is running, and assert the rendered prompt is byte-identical to the pre-change rendering for the same inputs.
- [x] 5.3 Render identical block content through every registered `CliStrategy`; no part of the contract may depend on the CLI's image-reading ability.

## 6. End-to-end run and PR reporting

- [x] 6.1 After the executor completes and an application was running, run `e2e_command` bounded by `e2e_timeout_secs`, capturing summary output and terminating the process tree on timeout.
- [x] 6.2 Emit a `## End-to-end verification` PR-body section naming the command, exit status, and captured summary; when no application ran, state that verification did not run and why — never omit the section silently and never render absence as a pass.
- [x] 6.3 A non-zero exit or a timeout opens the PR as a draft, reusing the existing reviewer-`Block` draft path in `autocoder/src/polling_loop/pass.rs`.

## 7. Red-green replay

- [x] 7.1 Detect end-to-end test files added or modified by the pass; skip the entire replay when there are none.
- [x] 7.2 Create a scratch git worktree at the pass's base commit, overlay only the new or modified end-to-end test files, start the application from that tree on its OWN ephemeral port distinct from the pass's instance, and run `e2e_command`.
- [x] 7.3 Require the replayed tests to fail. On a pass-against-base, open the PR as a draft and name each test that passed against base; retain the committed work, hold nothing, and do not increment the perma-stuck counter.
- [x] 7.4 Remove the scratch worktree and any application it started on every exit path; assert the agent branch and its working tree are unmodified by the replay.

## 8. Tests

- [x] 8.1 Config tests: absent block is a no-op; each missing required field errors naming the repository and field; timeout defaults; zero clamps with exactly one WARN and a retained raw value.
- [x] 8.2 Provisioning tests, driven by the recording `SystemActions` mock: the section issues the expected package-manager and browser-install calls; `--reconfigure` runs the section without touching other sections; a host with no package manager reports the manual commands and does not abort; a re-run is idempotent; no provisioning call is reachable from a pass or from startup.
- [x] 8.3 Lifecycle tests: two concurrent repositories receive different ports; a pass instance and a replay instance receive different ports; teardown occurs on completion, timeout, failure, and simulated shutdown with no socket left bound; readiness timeout proceeds without an application and holds nothing.
- [x] 8.4 Prompt tests: block present with a running application and absent otherwise; byte-identical rendering in the absent case; identical content across strategies.
- [x] 8.5 Reporting tests: pass, fail, and timeout each produce the correct section and draft state; a pass with no application states verification did not run and is never rendered as a pass.
- [x] 8.6 Replay tests: a vacuous test that passes against base drafts the PR, names the test, and retains the work; a genuine red-green test produces no finding; no e2e test change skips the replay; the agent branch is unmodified and the scratch worktree is removed on both the success and failure paths.
- [x] 8.7 Run the full `cargo test --release --all-features` suite and confirm every pre-existing executor, prompt, install-wizard, and PR-body scenario passes unchanged.

## 9. Documentation

- [x] 9.1 Add the `app_under_test` block to `config.example.yaml` with commented field documentation (it is `include_str!`'d as the install wizard's template, so it must deserialize cleanly).
- [x] 9.2 Document the block in `docs/CONFIG.md`, the provisioning path in `docs/INSTALL.md`, and the operational behavior in `docs/OPERATIONS.md`: opt-in per repository, provisioning via `--reconfigure`, degradation posture, wall-clock and token cost expectations, and how to read the PR verification section.
