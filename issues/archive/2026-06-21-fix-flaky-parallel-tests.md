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

- [x] Reproduce: run `cargo test --bin autocoder` several times in a row and
  confirm sporadic failures that each pass in isolation
  (`cargo test --bin autocoder <name> -- --exact`). DONE: run 1 = 16 failed
  (13 `polling_loop` + 1 `control_socket` rollback + 1 known-environmental
  `sandbox`); the rollback test passes 5/5 in isolation. The 13 `polling_loop`
  failures were a `PoisonError` CASCADE, not 13 independent failures.
- [x] Inventory the test sites that mutate the process-global `ENV_CONTROL_SOCKET`
  (search `set_var`/`remove_var` for `ENV_CONTROL_SOCKET` across
  `mcp_askuser_server.rs`, `executor/claude_cli.rs`, `control_socket.rs`,
  `cli/audit.rs`, `cli/verify.rs`, and any test using
  `control_socket::spawn_submission_listener`), plus tests that READ it.
  DONE — and the finding is that `ENV_CONTROL_SOCKET` is ALREADY serialized:
  every test that touches it already holds `crate::testing::ENV_LOCK` (a
  process-wide test mutex that already exists — the report's "no shared test
  mutex" claim was stale). The ACTUAL flake source is a DIFFERENT process-global:
  the PR-creation API-base override `polling_loop::test_hooks::github_api_base()`
  (guarded by `test_hooks::lock()`). See the fix below.
- [x] Make those tests parallel-safe. DONE via the "acceptable alternative" —
  serialize on the EXISTING process-wide test mutexes rather than introducing
  `serial_test`. `serial_test` was deliberately NOT used: it would create a
  SECOND, independent lock that does not serialize against the crate's existing
  `crate::testing::ENV_LOCK` / `polling_loop::test_hooks::lock()`, so a
  `#[serial]` test and a lock-holding test could still run concurrently and
  clobber the shared global — i.e. it would not fix the bug. (`serial_test` was
  added then reverted; `Cargo.toml` is unchanged.) The real fixes:
    1. `polling_loop::mod.rs` `test_hooks`: made `lock()` / `github_api_base()` /
       `set_github_api_base()` POISON-TOLERANT (`unwrap_or_else(|e| e.into_inner())`).
       These guard only a unit token + a per-test override string, so recovering on
       poison is safe. This stops ONE failing test from cascading `PoisonError`
       into the ~13 other tests that share the lock (the bulk of the 39/16-failure
       runs were this cascade, not 13 real failures).
    2. Added `test_hooks::lock()` to the 4 tests that drive production PR-open code
       (reading the `github_api_base` override) WITHOUT serializing:
       `control_socket::tests::defer_with_auto_submit_pr_takes_pr_path` (the
       confirmed contaminator of the rollback e2e test — same `owner/repo` path),
       `t05::cancellation_during_sleep_exits`,
       `t05::failure_alert_posted_then_suppressed_within_24h`,
       `t06::failure_alert_cleared_on_subsequent_success`.
- [ ] Re-run the full suite ≥5 times under default parallelism; confirm all green.
- [x] If `cli` fake-server tests still flake after the env-var fix, harden them.
  N/A — the `cli` fake-server tests did NOT flake. The report's predicted flaky
  tests (`cli::survives::...error_response_surfaces`,
  `cli::rollback::...confirm_accepted_acts_and_reports_pr`) passed in every run;
  their `fake_server` helpers already bind synchronously before returning and the
  client takes the socket path explicitly (no global, no bind/connect race). The
  ACTUAL flaky tests were `polling_loop::*` (poison cascade) and the
  `control_socket` rollback e2e test (cross-test override leak) — both fixed above.
- [x] Do NOT change production behavior, config, or specs. HELD — every edit is
  test-only: the `test_hooks` module is entirely `#[cfg(test)]`, and the other
  three edits add a lock acquisition inside test functions. No production code,
  config, or spec changed; `Cargo.toml` is unchanged. No spec delta.
- [ ] Activate the staged decomposition issues (FINAL step, only once the suite is
  reliably green): `git mv deferred-issues/*.md issues/` so the five
  `architecture_advisor` decomposition issues enter the active lane, then remove the
  now-empty `deferred-issues/` directory. They were staged in `deferred-issues/`
  (outside the lane) specifically to gate them behind this fix — they are
  behavior-preserving refactors that need a trustworthy test suite to verify
  against. This deliberately adds five issue files to this PR; say so in the PR
  description so the reviewer reads it as intentional.
