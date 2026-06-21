# Issue: the test suite is flaky under parallel execution (shared process-global state)

## Report

`cargo test --bin autocoder` is not reliably green under its default (parallel)
execution. Three back-to-back runs of an identical working tree produced **39
failed, then 1 failed, then 0 failed (3031 passed)**. Every test that failed in a
parallel run PASSES when run in isolation (e.g.
`cargo test --bin autocoder cli::survives::tests::error_response_surfaces -- --exact`
passes 3/3). Different tests fail on each run. This is parallel-contention
flakiness in the tests, NOT a product-logic regression — the code is correct, but
the suite cannot be trusted as a backstop while it flakes this way.

This is behavior-preserving (test-only) and carries NO spec delta: the fix makes
the tests parallel-safe; product behavior, configuration, and specs are unchanged.

## Diagnosis (test isolation, not product logic)

The primary, confirmed cause is **shared process-global state mutated by tests
without serialization**:

- The control-socket env var `ENV_CONTROL_SOCKET`
  (`crate::mcp_askuser_server::ENV_CONTROL_SOCKET`) is **process-global** and is
  `std::env::set_var`/`remove_var`'d by many tests: set across
  `autocoder/src/mcp_askuser_server.rs` (e.g. ~lines 1978, 2025, 2090, 2306, and
  the `/nonexistent/control.sock` cases ~2635–2917), removed across
  `autocoder/src/executor/claude_cli.rs` (~3862, 3912, 4231, 4285, …), and now
  also set by the `verify` subcommand and standalone-audit listener tests via
  `control_socket::spawn_submission_listener`. There are ~237 process-global
  `env::set_var`/`remove_var` sites in `autocoder/src` overall.
- There is **no test serialization** in the crate: no `serial_test` dependency,
  no `#[serial]`, no shared test mutex (confirmed — `serial_test` is absent from
  `autocoder/Cargo.toml`). So any two tests that set/remove `ENV_CONTROL_SOCKET`
  (or read it while another test mutates it) clobber each other when run
  concurrently. A test that reads a value another test just overwrote — or that
  another test just removed — fails non-deterministically. The 39-failure run is
  consistent with a burst of this clobbering under load.

A likely **secondary** cause to confirm after the primary fix: some `cli` tests
(`cli::survives`, `cli::blame`, `cli::rollback`, on-demand review) stand up an
async fake server — e.g. `cli/survives.rs` `fake_server` (~line 148) returns a
unix-socket path the command connects to. Such single-shot async fake servers can
miss accept/timing windows under heavy parallel load. These may be amplified by
the env clobbering and overall machine load rather than independently broken;
confirm whether they still flake once the env-var contention is removed.

## Desired end state (acceptance criteria)

- No test depends on, or is corrupted by, another test's mutation of a
  process-global env var. Every test that sets/removes `ENV_CONTROL_SOCKET` (and
  any other process-global env it touches) is either serialized against the others
  that touch the same global, OR no longer mutates the global at all.
- `cargo test --bin autocoder` passes on **at least 5 consecutive full runs**
  under default parallelism, with no test failing that passes in isolation.
- The previously-observed flaky tests pass under parallel load:
  `cli::survives::tests::error_response_surfaces`,
  `cli::rollback::tests::confirm_accepted_acts_and_reports_pr`, and the
  `mcp_askuser_server` / `verify` / standalone-audit tests that touch
  `ENV_CONTROL_SOCKET`.
- Production code paths that set `ENV_CONTROL_SOCKET` (daemon startup in
  `cli/run.rs`, the listener helper) are unchanged — this is a TEST-isolation fix
  only.

## Tasks

- [ ] Reproduce: run `cargo test --bin autocoder` several times in a row and
  confirm sporadic failures that each pass in isolation
  (`cargo test --bin autocoder <name> -- --exact`).
- [ ] Inventory the test sites that mutate the process-global `ENV_CONTROL_SOCKET`
  (search `set_var`/`remove_var` for `ENV_CONTROL_SOCKET` across
  `mcp_askuser_server.rs`, `executor/claude_cli.rs`, `control_socket.rs`,
  `cli/audit.rs`, `cli/verify.rs`, and any test using
  `control_socket::spawn_submission_listener`), plus tests that READ it.
- [ ] Make those tests parallel-safe. Preferred: add the `serial_test`
  dev-dependency (check crates.io for the current version; do not pin from memory)
  and mark every test that sets/removes/reads `ENV_CONTROL_SOCKET` with a NAMED
  serial group, e.g. `#[serial(control_socket)]`, so they run one-at-a-time
  relative to each other but still parallel with unrelated tests. Acceptable
  alternative where practical: remove the global-env dependency from the test path
  (thread the socket path explicitly instead of via the process-global env var) so
  no serialization is needed.
- [ ] Re-run the full suite ≥5 times under default parallelism; confirm all green.
- [ ] If `cli` fake-server tests still flake after the env-var fix, harden them:
  ensure each binds its own ephemeral socket/port and isolated state, and that the
  fake server is ready before the client connects (await readiness rather than
  racing). Re-run ≥5 times to confirm.
- [ ] Do NOT change production behavior, config, or specs. Keep the test fix
  itself test-only and behavior-preserving (no spec delta).
- [ ] Activate the staged decomposition issues (FINAL step, only once the suite is
  reliably green): `git mv deferred-issues/*.md issues/` so the five
  `architecture_advisor` decomposition issues enter the active lane, then remove the
  now-empty `deferred-issues/` directory. They were staged in `deferred-issues/`
  (outside the lane) specifically to gate them behind this fix — they are
  behavior-preserving refactors that need a trustworthy test suite to verify
  against. This deliberately adds five issue files to this PR; say so in the PR
  description so the reviewer reads it as intentional.
