# Tasks

## 1. Extract the shared submission-listener helper (load-bearing precondition)

- [x] 1.1 Add `control_socket::spawn_submission_listener(paths: &crate::paths::DaemonPaths) -> SubmissionListenerGuard` (or equivalent guard type) that, in order: constructs `crate::submission_store::SubmissionStore::new()`; registers the gate submission schemas via `preflight::change_contradiction::register_contradiction_submission_schema`, `preflight::canon_contradiction::register_canon_contradiction_submission_schema`, `preflight::global_rules::register_rule_violations_submission_schema`, AND the audit schemas via `audits::register_submission_schemas`; binds the socket with `control_socket::bind_at(&control_socket::socket_path(paths))`; sets `mcp_askuser_server::ENV_CONTROL_SOCKET` to the socket path; spawns `control_socket::serve(listener, path, state, cancel)` on a minimal `ControlState` (most fields default cheaply); and returns a guard holding the `CancellationToken` so `Drop` cancels `serve` (which removes the socket file). All referenced symbols are `pub` and reachable from the binary crate.
- [x] 1.2 Refactor the daemon's inline schema registration in `cli/run.rs` to call the shared `register_gate_and_audit_submission_schemas`, so the submission schema set has a single source across the daemon, `verify`, and the standalone audit path. The daemon keeps its own `ENV_CONTROL_SOCKET` set + `bind_at` + `control_socket::listen`/`serve` bootstrap because it serves a FULL `ControlState` (github/reviewer/chatops/reload handlers) that the submission-only `spawn_submission_listener` does not build; only the two daemon-absent callers (`verify`, `audit run`) use `spawn_submission_listener`.

## 2. The `verify` subcommand

- [x] 2.1 Add `Command::Verify { change_slug, gate selector, config path }` to `cli/mod.rs` (alongside `Rewind`, `CheckConfig`, `SyncSpecs`, `Reload`). Runs in the cwd repo; no `--repo` selector.
- [x] 2.2 Add a `cli/verify.rs` driver that: resolves the change directory (`openspec/changes/<slug>/`) and the local canon (`openspec/specs/`) from the cwd repo; loads the (minimal) config; calls `control_socket::spawn_submission_listener(paths)` as a hard precondition (holding the guard for the duration of the run); resolves the session timeout from `executor.agentic_session_timeout()` (which reads `executor.agentic_session_timeout_secs`, default `3600`) — NOT a verify-local literal; then invokes the enabled pre-executor gate checks against those working-tree paths.

## 3. Reuse the gate checks (no reimplementation)

- [x] 3.1 Invoke the SAME check entry points the verifier-gate framework uses — `preflight::change_contradiction::run_agentic_contradiction_check` (`[in]`), `preflight::canon_contradiction::run_agentic_canon_contradiction_check` (`[canon]`), and `preflight::global_rules::run_agentic_global_rules_check` (`[rules]`) (shared core in `preflight::corpus_check`) — passing the working-tree change + local canon, the same prompts, and the same per-gate model config: `executor.change_internal_contradiction_check_llm` (`[in]`), `executor.change_canonical_contradiction_check_llm` (`[canon]`), `executor.global_rules_check_llm` (`[rules]`). Do NOT fork the logic. (`verifier_gate.rs` is the registry/labels/retry helper and `llm.rs` resolves the model — neither is the check function.)
- [x] 3.2 Default to the gates ENABLED in config; honor `--all` (every realized spec-checking gate) and `--gate <list>` (named subset). Run the gates generically so any later corpus-parameterized gate is picked up without changing `verify`.

## 4. Output + exit semantics (fail-closed, no manufactured pass)

- [x] 4.1 Render findings to stdout grouped by gate, each labeled with the gate identifier (`[in]` / `[canon]` / `[rules]`) and carrying the same narrative the server marker's `revision_suggestion` would.
- [x] 4.2 Exit `0` only when every gate that ran is clean; non-zero on any finding; non-zero (fail-closed, per `gatekeepers-fail-closed`) when an enabled gate cannot run (model unconfigured / transport error / unregistered strategy / no submission captured), reporting "gate could not run" — never report clean for a gate that did not evaluate.
- [x] 4.3 When the resolved gate set is EMPTY (no spec-checking gate enabled, and no selector forces one), `verify` does NOT exit 0 silently: it reports that no gate evaluated the change and exits non-zero (per `gatekeepers-contain-no-judgment` — code never manufactures a clean pass when nothing was evaluated).
- [x] 4.4 Read-only with respect to spec/source: assert no `.needs-spec-revision.json` is written, no executor is invoked, and no spec/source files are edited by `verify`. Transient run artifacts (`.mcp.json`, the control socket) MAY be created and MUST be cleaned up on exit (the listener guard's `CancellationToken` removes the socket on drop).

## 5. Wire the standalone-audit path through the same helper (the fold)

- [x] 5.1 In `cli/audit.rs::run_standalone`, call `control_socket::spawn_submission_listener(paths)` (holding the guard for the duration) BEFORE `audit_arc.run(&mut ctx)`, so submission-based advisory audits (architecture_advisor, drift, documentation, canon_contradiction) capture their `submit_findings` verdict via `try_consume_submission` instead of getting `None` and erroring "no submit_findings submission". Remove/correct the stale comment at `cli/audit.rs` that claims findings are printed to stdout for submission-based audits.

## 6. Check-only install

- [x] 6.1 Add a check-only install path (script) that fetches the PREBUILT binary (built in CI / on the server — never compiled on the spec-box), places it on the interactive `PATH`, and writes a minimal config containing only the gate model blocks needed by the gates that run — `executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, `executor.global_rules_check_llm` — plus their `enabled` flags and corpus locations; no repos/chatops/reviewer/daemon config.
- [x] 6.2 Ensure CI publishes the prebuilt binary artifact the install script consumes.

## 7. Tests

- [x] 7.1 Without `spawn_submission_listener` standing up the transport (env var absent), every gate is `Errored`/fail-closed — proving the helper is a hard precondition, not an optimization.
- [x] 7.2 A clean change → exit 0, no marker, no executor, no spec/source edits; transient artifacts cleaned up (assert behavior/state).
- [x] 7.3 A change with a seeded contradiction → the finding is printed gate-labeled AND exit is non-zero.
- [x] 7.4 An enabled gate that cannot run (e.g. model unconfigured) → fail-closed: "could not run" + non-zero, NOT clean.
- [x] 7.5 An empty enabled-gate set (no spec-checking gate enabled, no selector) → non-zero with "no gate evaluated the change", NOT a silent exit 0.
- [x] 7.6 Default runs only enabled gates; `--all` / `--gate` override; an unknown gate name is an error, not a silent skip.
- [x] 7.7 The verify driver invokes the same check entry points as the server path (assert via the shared `run_agentic_*` calls / no duplicated logic), so verdicts cannot drift from the server gate.
- [x] 7.8 `verify` resolves its timeout from `executor.agentic_session_timeout()` (e.g. a configured `agentic_session_timeout_secs` is honored; absent → `3600`), not a local literal.
- [x] 7.9 Standalone audit through the helper: a daemon-absent `autocoder audit run <advisory>` captures a `submit_findings` submission and returns findings instead of erroring "no submit_findings submission".

## 8. Docs

- [x] 8.1 Document `verify` in `docs/` (and the spec-box setup): run it in a repo before pushing; check-only install on the spec-authoring machine; it is the local accelerator, the server gates remain the enforcement.
