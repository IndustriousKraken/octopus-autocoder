## ADDED Requirements

### Requirement: Shared in-process submission listener for daemon-absent gate and audit runs

autocoder SHALL provide a shared `control_socket::spawn_submission_listener(paths)` helper that stands up the submission transport in-process for a single invocation, so an agentic gate or audit that captures its verdict via an MCP `submit_*` tool can run without a daemon. The helper SHALL, in order: construct a `crate::submission_store::SubmissionStore`; register the gate submission schemas (`register_contradiction_submission_schema`, `register_canon_contradiction_submission_schema`, `register_rule_violations_submission_schema`) AND the audit submission schemas (`register_submission_schemas`); bind the control socket (`control_socket::bind_at`); set the control-socket env var (`mcp_askuser_server::ENV_CONTROL_SOCKET`) to the bound path; spawn `control_socket::serve` on a `ControlState`; AND return a guard whose drop cancels the listener's `CancellationToken` (stopping `serve`, which removes the socket file).

The submission SCHEMA SET — the gate AND audit submission schemas — SHALL be registered from a SINGLE shared function (`register_gate_and_audit_submission_schemas`) used by ALL THREE submission-capturing entry points (the daemon at startup, the `verify` subcommand, AND the standalone audit path), so the set cannot drift between them: a gate or audit whose schema is registered in one path but not another would silently fail to capture its verdict. The `spawn_submission_listener` helper performs the full bind → env-var → `serve` bootstrap on a submission-only `ControlState` AND SHALL be the shared bootstrap for the two DAEMON-ABSENT callers (`verify` AND the standalone audit path). The daemon retains its own bootstrap because it serves a FULL `ControlState` (github, reviewer, chatops, reload handlers) that the submission-only helper does not build; it shares the schema-set registration, not the listener. Without a listener (or, for the daemon, its running control socket), an agentic gate or submission-based audit drains `None` from `try_consume_submission` and fails closed; with it, verdicts are captured exactly as under the daemon.

#### Scenario: Listener is a precondition — gates fail closed without it
- **WHEN** an agentic gate's verdict is drained while the control-socket env var is unset (no `spawn_submission_listener` active)
- **THEN** the drain returns no submission AND the gate result is `Errored` (fail-closed)
- **AND** no gate reports clean for a run whose verdict was never captured

#### Scenario: Listener stands up the transport for an in-process run
- **WHEN** `spawn_submission_listener(paths)` is held for the duration of a gate or audit run
- **THEN** the control-socket env var points at a live bound socket AND the gate/audit submission schemas are registered on the store
- **AND** the run captures its `submit_*` verdict via `try_consume_submission` as it would under a daemon

#### Scenario: Listener tears down its transient socket on drop
- **WHEN** the listener guard is dropped at the end of an invocation
- **THEN** its `CancellationToken` fires, `serve` stops, AND the socket file is removed
- **AND** no daemon control socket is left behind by the invocation

### Requirement: `verify` subcommand runs the pre-executor gate checks locally on a working-tree change

autocoder SHALL provide a `verify <change-slug>` subcommand that runs the pre-executor verifier-gate checks — `[in]` (change-internal), `[canon]` (change-vs-canonical), `[rules]` (global-rules), AND any other realized spec-checking gate that is enabled — against a change in the LOCAL working tree, so an operator can learn whether a change would pass the server gates BEFORE pushing it. It is a new invocation surface for the existing checks, NOT a redefinition of the verifier-gate framework: it invokes the same check entry points (`preflight::change_contradiction::run_agentic_contradiction_check`, `preflight::canon_contradiction::run_agentic_canon_contradiction_check`, `preflight::global_rules::run_agentic_global_rules_check`; shared core in `preflight::corpus_check`), the same prompts, the same per-gate model configuration (`executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, `executor.global_rules_check_llm`), AND the same submission schemas the server uses, so its verdict matches what the server will enforce.

`verify` SHALL stand up the submission transport in-process via `control_socket::spawn_submission_listener(paths)` as a hard precondition for the duration of the run; without it the gates fail closed and `verify` cannot pass. `verify` SHALL resolve its agentic-session timeout from `ExecutorConfig::agentic_session_timeout()` (reading `executor.agentic_session_timeout_secs`, default `3600` when omitted) — NOT a verify-local literal.

The subcommand SHALL run in the repository's working directory, reading `openspec/changes/<change-slug>/specs/**` (the deltas) and the local `openspec/specs/**` (canon) — the working copy, before any push. It SHALL NOT run the executor, SHALL NOT write `.needs-spec-revision.json`, AND SHALL NOT make spec or source edits. It MAY create transient run artifacts (`.mcp.json`, the control socket) AND SHALL clean them up on exit. It reports findings to stdout, grouped by gate AND labeled with the gate identifier, each carrying the finding narrative the server marker's `revision_suggestion` would carry.

By default `verify` SHALL run the gates ENABLED in config (so its verdict matches server enforcement); a selector MAY override (`--all` for every realized spec-checking gate, `--gate <list>` for a named subset). Exit code SHALL be CI-usable, conforming to the `gatekeepers-fail-closed` standard: `0` ONLY when every gate that ran returned no findings; non-zero when any gate finds a contradiction; AND non-zero when an enabled gate CANNOT run (model unconfigured, transport error, unregistered strategy, no submission captured) — `verify` SHALL report "gate could not run" AND fail, never reporting clean for a gate that did not actually evaluate. When the resolved gate set is EMPTY (no spec-checking gate enabled AND no selector forcing one), `verify` SHALL NOT exit `0` silently: it SHALL report that no gate evaluated the change AND exit non-zero, conforming to the `gatekeepers-contain-no-judgment` standard (code never manufactures a clean pass when nothing was evaluated).

`verify` is a subcommand of the autocoder binary (so it ships the identical check logic the server runs). A check-only install SHALL be supported: it fetches a PREBUILT binary, places it on the interactive `PATH`, AND drops a minimal config carrying only what `verify` needs (the `executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, AND `executor.global_rules_check_llm` model blocks with their `enabled` flags, plus corpus locations) — so it runs on a low-powered spec-authoring machine without building from source OR running the daemon.

#### Scenario: A clean change passes verify
- **WHEN** an operator runs `verify <slug>` in a repo against a change whose deltas contradict neither themselves nor canon AND the relevant gates are enabled and configured
- **THEN** each run gate reports clean AND the command exits `0`
- **AND** no marker is written, no executor runs, AND no spec or source files are edited; transient run artifacts are cleaned up

#### Scenario: A contradicting change is reported with a non-zero exit
- **WHEN** `verify <slug>` runs against a change whose deltas contradict canon (or each other)
- **THEN** the command prints the finding(s), each labeled with the gate that produced it (`[in]` / `[canon]` / `[rules]`)
- **AND** it exits non-zero
- **AND** the finding narrative matches what the server's `.needs-spec-revision.json` would carry

#### Scenario: verify's verdict matches the server gate
- **WHEN** `verify` runs the same enabled gate against the same change the server would
- **THEN** it uses the same check entry point, prompts, model config, and submission schema as the server
- **AND** a change `verify` reports clean is not subsequently kicked back by that same server gate (absent canon drift since the local run)

#### Scenario: A gate that cannot run fails closed, not clean
- **WHEN** an enabled gate cannot run during `verify` (its model is unconfigured, the agentic session errors, its strategy is unregistered, or no submission is captured)
- **THEN** `verify` reports that the gate could not run AND exits non-zero
- **AND** it does NOT report the change as clean

#### Scenario: Without the submission listener every gate fails closed
- **WHEN** `verify` runs but the in-process submission listener was not stood up (the control-socket env var is unset)
- **THEN** every gate drains no submission AND is reported as unable to run (fail-closed) with a non-zero exit
- **AND** no gate reports clean — confirming the listener is a hard precondition

#### Scenario: An empty enabled-gate set is loud, not a silent pass
- **WHEN** `verify <slug>` runs with a config in which NO spec-checking gate is enabled AND no selector forces one
- **THEN** `verify` reports that no gate evaluated the change AND exits non-zero
- **AND** it does NOT exit `0` — code never manufactures a clean pass for a change nothing checked

#### Scenario: verify honors the unified agentic-session timeout
- **WHEN** `verify` runs with `executor.agentic_session_timeout_secs` configured (or omitted)
- **THEN** the gate sessions use the value resolved from `ExecutorConfig::agentic_session_timeout()` (the configured value, or `3600` when omitted)
- **AND** `verify` does NOT use a verify-local timeout literal

#### Scenario: Default runs enabled gates; selector overrides
- **WHEN** `verify <slug>` is run with no gate selector
- **THEN** it runs exactly the spec-checking gates enabled in config
- **WHEN** `verify <slug> --all` or `verify <slug> --gate in,canon` is run
- **THEN** it runs the selected gates regardless of their enabled state (reporting any that cannot run as fail-closed)
- **AND** an unknown gate name in `--gate` is an error, not a silent skip

#### Scenario: Check-only install runs without a daemon or a source build
- **WHEN** an operator runs the check-only install on a spec-authoring machine
- **THEN** a prebuilt `verify`-capable binary is placed on the interactive `PATH` AND a minimal config with the `executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, and `executor.global_rules_check_llm` model blocks is written
- **AND** `verify` runs against a local repo with no daemon running and without compiling from source

### Requirement: Standalone `audit run` captures submission-based audit verdicts without a daemon

The standalone audit path (`audit run`) SHALL stand up the in-process submission transport via `control_socket::spawn_submission_listener(paths)` before invoking an audit, so submission-based advisory audits (architecture_advisor, drift, documentation, canon_contradiction) capture their `submit_findings` verdict through `try_consume_submission` instead of draining `None` and failing closed. Without the listener these audits are non-functional daemon-absent (they error with "no submit_findings submission"); with it, a standalone `audit run` returns findings exactly as the daemon would.

#### Scenario: A daemon-absent standalone advisory audit returns findings
- **WHEN** an operator runs `audit run <submission-based-advisory-audit>` with no daemon running
- **THEN** the standalone path stands up the submission listener for the run AND the audit's `submit_findings` verdict is captured
- **AND** the audit returns its findings rather than erroring "no submit_findings submission"

#### Scenario: Standalone audit listener is torn down after the run
- **WHEN** a standalone `audit run` completes
- **THEN** the submission listener guard is dropped AND its control socket is removed
- **AND** no daemon control socket is left behind by the standalone run
