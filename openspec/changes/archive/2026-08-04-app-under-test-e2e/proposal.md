## Why

Every check in the pipeline is a static reader. `[in]`, `[canon]`, `[rules]`, `[out]`, and the code reviewer all read text and judge whether it looks correct. `[out]` is the one nominally asking "does the code implement the spec," and it is an LLM reading a diff — precisely the judgment that misses "the component is defined, the handler is defined, and nothing imports the component." It is also advisory, so even when it notices, the PR still opens non-draft.

Nothing in the pipeline ever runs the software. The recurring production consequence is a class of change that implements a feature and never connects it to the UI — or assumes it is already connected — and passes every gate, because the defect is invisible in the text and only visible at runtime. The operator absorbs the cost: the same feature is re-requested repeatedly because autocoder has no way to verify it works, producing churn commits until a human drives an improvement loop by hand.

A canon requirement's `#### Scenario:` block is already a WHEN/THEN assertion — "WHEN the user clicks submit THEN a confirmation appears" maps onto an end-to-end test almost mechanically. The assertion grammar is not missing. What is missing is the ability to execute it.

## What Changes

- A new optional per-repository `app_under_test` config block declares how to start the application, how to know it is ready, and how to run its end-to-end suite. It is operator-owned, in autocoder's `config.yaml` — **not** in the target repository, so the agent cannot edit the thing that decides whether its own work passed.
- The daemon (not the agent) owns the application lifecycle: it allocates an ephemeral port per pass, starts the app, waits for the readiness probe, injects the resolved base URL into the agent session's environment, and tears the process tree down when the pass ends — including on timeout, failure, and shutdown. Concurrent repositories cannot collide on a fixed port, and no dev server outlives its pass.
- The implementer prompt gains an "Application under test" block when an app is running for the pass, naming the base URL, the e2e command, and the expectation that behavioral scenarios are verified by an executed test rather than asserted in prose.
- End-to-end results are reported in the PR body under `## End-to-end verification`, naming the command, its exit status, and the tests that ran.
- The toolchain is **provisioned by autocoder's own install wizard**, not by hand on the host: an optional end-to-end section installs the privileged system packages via the platform package manager and the browser binaries for the service account, reachable on an existing deployment via `--reconfigure`. Provisioning is operator-initiated and never implicit — autocoder does not download a browser runtime as a side effect of polling.
- New or modified end-to-end tests are **replayed against the pre-change tree**. A test that passes without the change is not evidence: it is either vacuous or the behavior already existed. Either way the PR opens as a draft with the finding named, reusing the existing reviewer-`Block` draft mechanism.

The binding verification signal is the e2e command's **exit code**, never the agent's visual judgment of a screenshot. Screenshots are a debugging aid for the agent's own loop. This keeps the oracle mechanical and keeps the feature working identically across all three supported CLIs, whose image-reading abilities differ.

No change to the tool allowlist or the bash denylist is required. The agent already has `Bash` to run the e2e command and `Read` to inspect artifacts; the daemon owns everything the agent would otherwise have needed a new capability for.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: gains the `app_under_test` per-repository config schema, the daemon-owned application lifecycle for a pass, the startup/`doctor` preflight for the e2e toolchain, wizard-driven toolchain provisioning, the PR-body end-to-end verification section, and the red-green replay that detects vacuous end-to-end tests.
- `executor`: the implementer prompt carries an "Application under test" block when the daemon has an application running for the pass.

## Impact

- `autocoder/src/config.rs`: new `AppUnderTestConfig` struct on `RepositoryConfig`, validation at `Config::load_from` (a declared e2e command requires a start command and a readiness probe; timeouts clamped like other numeric knobs).
- `autocoder/src/dependency_preflight.rs`: config-implied dependency probes when `app_under_test` is present, surfaced by `doctor` and at startup. A missing e2e toolchain disables the feature for that repository with a WARN rather than aborting the daemon — other repositories are unaffected.
- `autocoder/src/cli/install.rs`: an optional end-to-end testing section behind the existing `SystemActions` trait (which already drives `apt-get`/`dnf`/`pacman`/`zypper`/`brew` for `git`, `bubblewrap`, and `gh`), plus its `--reconfigure` and non-interactive wiring. Node is already a required dependency via `openspec`, so no new language runtime is introduced.
- `autocoder/src/polling_loop/`: pass-scoped application lifecycle (start, readiness wait, env injection, guaranteed teardown), the e2e run, and the red-green replay in a scratch worktree at the base commit.
- `autocoder/src/executor/claude_cli.rs` and `prompts/implementer.md`: the "Application under test" prompt block, following the existing "Prior iteration summary" precedent.
- `autocoder/src/polling_loop/pass.rs`: PR body gains `## End-to-end verification`; a vacuous-test finding drafts the PR via the existing draft path.
- `config.example.yaml`, `docs/CONFIG.md`, `docs/OPERATIONS.md`: the new block, its cost/latency expectations, and the failure modes.
- Operator-visible: repositories with no `app_under_test` block behave exactly as today. This is opt-in per repository, and absent config is not an error.
