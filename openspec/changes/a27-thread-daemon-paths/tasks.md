## 1. paths.rs API surgery

- [ ] 1.1 Remove from `autocoder/src/paths.rs`:
  - The `OnceLock<DaemonPaths>` static (whatever its name).
  - `pub fn current() -> DaemonPaths` AND its body.
  - `pub fn install_global(paths: DaemonPaths)`.
  - `pub fn install_global_for_tests(paths: DaemonPaths)`.
  - `pub fn get_global() -> Option<&'static DaemonPaths>`.
  - `fn test_fallback() -> DaemonPaths`.
  - Any helper functions used only by the removed surface.
- [ ] 1.2 Retain in `autocoder/src/paths.rs`:
  - The `DaemonPaths` struct AND its `Clone` derive.
  - The constructor used at daemon startup (the env-driven `resolve_from_env`-equivalent OR `from_systemd_dirs` OR however the existing entrypoint produces it).
  - All helper methods on `DaemonPaths` itself (e.g., `alert_state_path`, `audit_logs_dir`, `control_socket_path`, `workspaces_dir`, etc.) — these stay; the refactor changes WHO calls them, not WHAT they do.
- [ ] 1.3 Decide on Arc vs Clone-by-value: pick ONE convention AND document it in a `//!` doc-comment at the top of `paths.rs`. Recommendation: `Arc<DaemonPaths>` everywhere — `DaemonPaths` has 4 `PathBuf` fields, each ~24 bytes, but cloning still hits the heap allocator per field. `Arc` is one allocator hit AND every consumer holds a cheap ref-counted handle. Either choice is acceptable provided the spec's no-globals invariant holds.

## 2. Daemon entrypoint plumbing

- [ ] 2.1 In `autocoder/src/main.rs` (OR the top-level daemon-startup module), construct one `DaemonPaths` value via the existing env-driven resolution at the same point in the startup sequence that `install_global` was previously called.
- [ ] 2.2 Wrap in `Arc` (if that's the chosen convention) AND pass to whatever top-level type the daemon constructs (e.g., `DaemonOrchestrator::new(paths, ...)` OR equivalent).
- [ ] 2.3 The orchestrator's `Drop` AND graceful-shutdown paths drop the `Arc<DaemonPaths>` naturally; no special cleanup needed.

## 3. Per-module refactors

For each of the 11 modules currently calling `paths::current()`, apply the appropriate pattern:

- [ ] 3.1 `autocoder/src/alert_state.rs` (1 call site at line 116) — accept `paths: &Arc<DaemonPaths>` on every public API; update internal callers in this module.
- [ ] 3.2 `autocoder/src/proposal_requests.rs` (1 call site at line 110) — same pattern.
- [ ] 3.3 `autocoder/src/audits/mod.rs` (1 call site at line 306) — likely a struct holding `Arc<DaemonPaths>`; the function at line 306 becomes a method.
- [ ] 3.4 `autocoder/src/executor/claude_cli.rs` (1 call site at line 1397) — likely embedded in a struct; field-based.
- [ ] 3.5 `autocoder/src/control_socket.rs` (1 call site at line 217) — field-based.
- [ ] 3.6 `autocoder/src/revisions.rs` (1 call site at line 95) — likely free-function-with-param.
- [ ] 3.7 `autocoder/src/failure_state.rs` (1 call site at line 57) — same as alert_state.
- [ ] 3.8 `autocoder/src/changelog_requests.rs` (1 call site at line 91) — same pattern.
- [ ] 3.9 `autocoder/src/busy_marker.rs` (4 call sites at lines 135, 151, 288, 1138) — likely a struct (busy-marker manager) holding `Arc<DaemonPaths>`.
- [ ] 3.10 `autocoder/src/workspace.rs` (2 call sites at lines 20, 217) — refactor the workspace-resolution functions to take `&Arc<DaemonPaths>`.
- [ ] 3.11 `autocoder/src/audits/scheduler.rs` (2 call sites at lines 1449, 1464) — scheduler struct holds `Arc<DaemonPaths>`.
- [ ] 3.12 `autocoder/src/audits/threads.rs` (1 call site at line 85) — free-function-with-param.
- [ ] 3.13 Update every CALLER in `src/` that previously didn't need to pass `DaemonPaths` AND now does. This will ripple — chain calls upward until the caller has a `DaemonPaths` (or `Arc<DaemonPaths>`) in scope (typically because it's a method of a struct that holds one, OR because main.rs hands it down).

## 4. Test refactors

- [ ] 4.1 Every test currently invoking production code that previously called `paths::current()` SHALL be updated to construct a `DaemonPaths` via `test_daemon_paths()` AND pass it explicitly to the production API.
- [ ] 4.2 Test signature pattern: replace `let result = production_fn(...)` with
  ```rust
  let (_tempdir, paths) = test_daemon_paths();
  let result = production_fn(&paths, ...);
  ```
  Keep `_tempdir` in scope so the tempdir lives for the duration of the test.
- [ ] 4.3 Each test that previously asserted against the SHARED `<system-temp>/autocoder/...` paths (via the removed `test_fallback`) SHALL be updated to assert against its OWN tempdir-scoped path.
- [ ] 4.4 Where the test_daemon_paths-call AND the production-API-call are deeply nested (e.g., the test constructs `Daemon::new()` which internally creates dozens of subcomponents), update the test to pass the `Arc<DaemonPaths>` through the daemon constructor.
- [ ] 4.5 Tests:
  - Every existing test passes after the refactor.
  - One new test verifies that two concurrent production-fn invocations (via `std::thread::spawn`) with DIFFERENT `DaemonPaths` values do NOT collide on disk — confirming the per-test isolation property.

## 5. CI guard against regression

- [ ] 5.1 Extend `autocoder/tests/path_literals_audit.rs` (added by `a10`) with a second scanner pass:
  - Walk `autocoder/src/**/*.rs`.
  - Match the literal strings `paths::current`, `paths::install_global`, `paths::test_fallback`, `paths::get_global`.
  - Fail with a list of offending file:line locations.
  - The audit's own constants are constructed from fragments at runtime so the scanner does not match itself (same pattern `a10` used for the `/tmp/autocoder/` scan).
- [ ] 5.2 Allowlist: empty. After the refactor, none of these references should appear in `src/` at all.
- [ ] 5.3 Tests:
  - The scanner's own self-match test passes (the scanner file is not flagged).
  - The scanner fails when given a synthetic test file containing one of the banned strings.

## 6. Docs

- [ ] 6.1 Update `docs/STATE-LAYOUT.md`'s "Path resolution rule" section: the rule changes from "every daemon state-file read AND write routes through the `DaemonPaths` resolver" to "every daemon state-file read AND write goes through a `DaemonPaths` value threaded into the consumer via constructor OR function parameter; there is no process-global `paths::current()`."
- [ ] 6.2 Add a paragraph in `docs/STATE-LAYOUT.md` describing the per-test isolation property: each test owns its own `DaemonPaths` via `test_daemon_paths()`; concurrent tests cannot collide on disk because each test's fixtures live under its own tempdir.
- [ ] 6.3 Update `docs/test-reliability.md` (created by `a10`)'s disposition-table row for the test-mode-fallback issue: mark it `fixed-in-a27` AND describe the resolution.

## 7. Spec deltas

- [ ] 7.1 `openspec/changes/a27-thread-daemon-paths/specs/orchestrator-cli/spec.md` ADDs the threading-invariant requirement.
- [ ] 7.2 `openspec/changes/a27-thread-daemon-paths/specs/project-documentation/spec.md` ADDs the docs requirement covering the updated "Path resolution rule" AND test-isolation paragraph.

## 8. Verification

- [ ] 8.1 `cargo test` passes — every existing test plus the new concurrent-isolation test AND the extended path-literals scanner.
- [ ] 8.2 `openspec validate a27-thread-daemon-paths --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 8.4 Manual verification:
  - `grep -rn "paths::current\b" autocoder/src/` returns no results.
  - `grep -rn "paths::install_global\b\|paths::test_fallback\b\|paths::get_global\b" autocoder/src/` returns no results.
  - The daemon starts AND runs against a real workspace AS BEFORE (smoke verification that the refactor didn't break startup).
