## ADDED Requirements

### Requirement: Production paths SHALL be threaded through APIs, NOT read from a process-global
The daemon SHALL construct exactly one `DaemonPaths` value at startup (in `main.rs` OR the equivalent entrypoint module) via the existing env-driven resolution AND SHALL thread that value into the rest of the codebase as an explicit constructor argument OR function parameter. Modules requiring path information SHALL accept a `DaemonPaths` value (by ownership, reference, OR `Arc<DaemonPaths>`) at their construction site OR on the function call path. No module SHALL read paths from a process-global cell, lazy-static, OR thread-local at runtime.

The following APIs that previously enabled global-state access SHALL be removed from `autocoder/src/paths.rs` AND SHALL NOT be reintroduced:

- `crate::paths::current()`
- `crate::paths::install_global(_)`
- `crate::paths::install_global_for_tests(_)`
- `crate::paths::test_fallback()`
- `crate::paths::get_global()`
- The underlying `OnceLock<DaemonPaths>` static.

The `DaemonPaths` struct itself, its helper methods (`alert_state_path`, `audit_logs_dir`, `control_socket_path`, `workspaces_dir`, etc.), AND its env-driven constructor SHALL be retained. The change is to WHO calls those helpers, NOT to what the struct provides.

Tests SHALL construct their own `DaemonPaths` via the existing `test_daemon_paths()` helper (which returns a tempdir-scoped instance) AND pass it explicitly into the production APIs they exercise. The test-suite invariant becomes: each test's fixtures live exclusively under its own tempdir, with no shared `<system-temp>/autocoder/...` location.

A CI scanner (an extension of the `a10` path-literals audit) SHALL fail the build if any of the removed function names reappears in `autocoder/src/` source files. The allowlist for this second-pass scanner SHALL be empty.

#### Scenario: Daemon entrypoint constructs the single instance
- **WHEN** the daemon starts up
- **THEN** `autocoder/src/main.rs` (OR the equivalent entrypoint module) constructs ONE `DaemonPaths` value via the env-driven resolution
- **AND** that value is handed (by ownership, reference, OR `Arc`) to the top-level orchestrator
- **AND** no other code path constructs an additional `DaemonPaths` for production use

#### Scenario: Module constructor accepts paths
- **WHEN** a module that requires path information is constructed (e.g., the audits scheduler, the busy-marker manager, the control-socket handler)
- **THEN** its constructor signature includes a `DaemonPaths` parameter (by ownership, reference, OR `Arc`)
- **AND** the module stores the value as a field for use by its methods
- **AND** the module does NOT call any removed global accessor

#### Scenario: Free function accepts paths as parameter
- **WHEN** a free function in `autocoder/src/` needs path information (e.g., a helper in `audits/threads.rs` OR `proposal_requests.rs`)
- **THEN** the function's signature includes a `paths: &DaemonPaths` (OR equivalent) parameter
- **AND** every caller passes the paths explicitly
- **AND** the function does NOT call any removed global accessor

#### Scenario: Test constructs its own DaemonPaths
- **WHEN** a test exercises production code that previously read from the global
- **THEN** the test calls `test_daemon_paths()` to obtain a `(TempDir, DaemonPaths)` pair
- **AND** passes the `DaemonPaths` explicitly into the production API
- **AND** the test's fixtures land under the tempdir, NOT a shared `<system-temp>/autocoder/...` location

#### Scenario: Concurrent tests do not collide on disk
- **WHEN** two tests run concurrently (cargo's default per-test thread) AND both invoke the same production API
- **THEN** each test's invocation uses ITS OWN `DaemonPaths` constructed via `test_daemon_paths()`
- **AND** the two tests' fixtures live under DISJOINT tempdir roots
- **AND** no fixture write OR read crosses between tests

#### Scenario: CI scanner blocks reintroduction
- **WHEN** the path-literals audit (extended per this requirement) runs against `autocoder/src/`
- **THEN** the scanner fails the build if it finds any reference to `paths::current`, `paths::install_global`, `paths::test_fallback`, OR `paths::get_global` in any `src/**/*.rs` file
- **AND** the scanner's own constants are constructed at runtime from fragments so it does not match itself
