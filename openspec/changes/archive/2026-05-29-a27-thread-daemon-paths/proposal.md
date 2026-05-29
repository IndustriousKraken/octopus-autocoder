## Why

`a10` swept hard-coded `/tmp/autocoder/` literals out of test code AND added the `test_daemon_paths()` helper, but called out a residual gap: production code routes path lookups through `crate::paths::current()`, a process-global `OnceLock<DaemonPaths>` with a test-mode fallback returning a SHARED `<system-temp>/autocoder/` root. When a test invokes production code that reads this global, the test's fixtures land in that shared root regardless of how carefully the test's own code avoids hard-coded literals.

The current state of the codebase:

- 17 call sites of `paths::current()` across 11 modules in `autocoder/src/`.
- 0 call sites in `autocoder/tests/`. Tests reach the global exclusively through production code.
- Tests pass today because they happen to use distinct basenames (sanitized repo URLs, audit names) that don't collide on disk.
- The brittleness is structural: a future test author who picks a colliding basename gets a flaky test, with the root cause hidden behind a process-global no one expects.

The proper fix is the one the caveat names: thread `DaemonPaths` through APIs as a parameter (or constructor argument) instead of reading it from a process-global at runtime. This eliminates the global AND its fallback entirely. Each module that needs path information takes a `DaemonPaths` value (or `Arc<DaemonPaths>`) explicitly; the daemon constructs one at startup AND threads it down through its call graph; tests construct their own via the existing `test_daemon_paths()` helper.

Beyond test-isolation correctness, removing the global yields three compounding benefits:

1. **Clearer dependency graph.** Every module's path dependencies are visible at its constructor site instead of hidden inside `current()` calls.
2. **Easier testability.** Each test can supply its own isolated `DaemonPaths` without worrying about cross-test global state.
3. **Foundation for future per-workspace path overrides.** When future changes (e.g., the spec-storage path in `a26`) introduce per-repo path customization, threading is the natural mechanism. The global obscures this kind of customization.

## What Changes

**Eliminate `crate::paths::current()` AND `crate::paths::test_fallback()` from production code paths.** Production modules SHALL receive a `DaemonPaths` value (by ownership, reference, OR `Arc`) at construction OR as a parameter, NOT by reading a process-global at runtime. The 17 existing call sites of `paths::current()` SHALL be refactored to consume their `DaemonPaths` from the call chain.

**Daemon entrypoint is the single construction site.** `autocoder/src/main.rs` (or the equivalent daemon-startup module) SHALL construct one `DaemonPaths` value via the existing systemd/env-driven resolution AND thread it into the top-level orchestrator. Every other module receives its `DaemonPaths` via its constructor OR through a function parameter on the call path that needs it.

**Tests construct their own `DaemonPaths` per test.** The `test_daemon_paths()` helper (added by `a10`) becomes the canonical way for tests to obtain a `DaemonPaths`. Each test creates its own isolated tempdir-scoped instance AND passes it explicitly into the production APIs it calls. No test relies on a process-global.

**Removal scope:**

- `crate::paths::current()` — removed.
- `crate::paths::test_fallback()` — removed.
- `crate::paths::install_global()` — removed (no global to install into).
- `crate::paths::install_global_for_tests()` — removed (tests construct their own).
- `crate::paths::get_global()` — removed.
- The `OnceLock<DaemonPaths>` static — removed.
- `DaemonPaths::resolve_from_env()` (OR the constructor used by the daemon entrypoint) — retained AND becomes the single entrypoint for production construction.
- `test_daemon_paths()` — retained, becomes the canonical test-side construction helper.

After the refactor, the compiler enforces the invariant: any code attempting to call a removed function fails to build.

**Per-module refactor pattern.** Modules currently calling `paths::current()` fall into two shapes:

1. **Stateful modules with a struct type.** Add a `DaemonPaths` field (OR `Arc<DaemonPaths>`); accept it in the constructor; use the field at the current call sites. Example: `audits::scheduler::Scheduler` gains a `paths: Arc<DaemonPaths>` field.
2. **Free-function utilities.** Add `paths: &DaemonPaths` as a function parameter; update every caller to pass it. Example: `audits::threads::store_thread_state(paths, …)`.

For modules called from many places (e.g., `control_socket`, `executor/claude_cli`), the refactor ripples up the call chain. The implementer SHALL prefer threading the value through call signatures over reintroducing any form of global state (thread-locals, lazy-statics, etc.).

**Arc-or-clone is the implementer's choice.** `DaemonPaths` is 4 `PathBuf` fields, cheap to clone. Modules MAY take ownership AND clone, OR hold an `Arc<DaemonPaths>` for shared reference. The spec does NOT prescribe one; both are acceptable provided no global state results.

**No new env vars OR config knobs.** The daemon's existing path-resolution (from `$STATE_DIRECTORY` / `$CACHE_DIRECTORY` / etc. per `orchestrator-cli` canonical) is unchanged. This change is internal-implementation only — operator-visible behavior is identical.

**CI guard against regression.** A new test SHALL grep `autocoder/src/` for `paths::current()` AND `paths::install_global` AND `paths::test_fallback` AND `paths::get_global` references. The test SHALL fail if any of those names appear anywhere in `src/`. (Effectively, the removed APIs cannot be reintroduced via copy-paste from old code.)

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED requirement: `Production paths are threaded through APIs, not read from a process-global`. This codifies the architectural invariant the refactor establishes.
  - `project-documentation` — ADDED requirement: `docs/STATE-LAYOUT.md "Path resolution rule" describes DaemonPaths threading; legacy paths::current() references are removed`. The existing "Path resolution rule" section gains updated guidance per the threading model.
- **Affected code (broad strokes — every consumer of `paths::current()` plus its callers):**
  - `autocoder/src/paths.rs` — remove the global cell, `current()`, `test_fallback()`, `install_global()`, `install_global_for_tests()`, `get_global()`. Retain `DaemonPaths` struct AND its resolution-from-env constructor.
  - `autocoder/src/main.rs` — construct one `DaemonPaths` at startup AND pass it to the top-level orchestrator.
  - `autocoder/src/alert_state.rs`, `proposal_requests.rs`, `audits/mod.rs`, `executor/claude_cli.rs`, `control_socket.rs`, `revisions.rs`, `failure_state.rs`, `changelog_requests.rs`, `busy_marker.rs`, `workspace.rs`, `audits/scheduler.rs`, `audits/threads.rs` — refactor per the per-module pattern above. Each module's tests adjust to pass an explicit `DaemonPaths` via `test_daemon_paths()`.
  - `autocoder/src/testing.rs` — `test_daemon_paths()` helper retained; its callers grow to include every test that exercises production code paths.
  - `autocoder/tests/path_literals_audit.rs` — extend with the new ban on `paths::current` / `paths::install_global` / `paths::test_fallback` / `paths::get_global` references in `src/`.
- **Operator-visible behavior:** none. The change is internal-implementation only.
- **Test-suite behavior:** each test now constructs its own `DaemonPaths`. Test concurrency is no longer a fixture-collision risk because each test's fixtures live under its own tempdir.
- **Breaking:** no for operators. Yes for downstream Rust code that might have called the removed functions, but autocoder is not consumed as a library — no external consumers exist.
- **Acceptance:** `cargo test` passes (every existing test plus the new path-literals scanner extension). `openspec validate a27-thread-daemon-paths --strict` passes. `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings. Manual verification: `grep -r "paths::current\b" autocoder/src/ autocoder/tests/` returns no results.
