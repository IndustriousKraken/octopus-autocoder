# Local `verify` subcommand: run the pre-executor gates before pushing

## Why

The `[in]` (change-internal), `[canon]` (change-vs-canonical), and `[rules]`
(global-rules) gates only run server-side, inside the polling loop, at implement
time. So the only way to learn whether a change passes them is to push it and wait
for the daemon to run it — and when it fails, the daemon writes
`.needs-spec-revision.json`, the operator relays the marker, fixes the spec, pushes
again, and clears the marker. That round-trip has eaten most of several sessions:
every kickback (a duplicate-add audit task, a shared-label conflation, a
top-level-vs-thread mismatch) was a multi-minute remote loop for a check that is a
single agentic session.

The check logic already exists and is exercised every iteration; what is missing is
a way to run it locally, on the working tree, before pushing. A `verify` subcommand
that invokes the SAME checks and reports findings turns the remote round-trip into a
local, seconds-long loop — and because it reuses the server's exact logic, model
config, and prompts, its verdict matches what the server will enforce (closing the
gap that local adversarial reviewers leave open: they approximate the gate, this IS
the gate).

There is a load-bearing precondition. The gate-check functions capture the agent's
verdict through an MCP `submit_*` tool, relayed over a Unix control socket to a
`SubmissionStore`; the runner drains it via `try_consume_submission`. That env var
(`ENV_CONTROL_SOCKET`) is set ONLY at daemon startup. With no daemon,
`try_consume_submission` returns `None` and every gate fails closed — so a naive
`verify` would NEVER pass. The same gap already makes `cli/audit.rs::run_standalone`
non-functional daemon-absent: advisory audits that capture findings via
`submit_findings` always error "no submit_findings submission" (fail-closed but
unusable). Both need the submission transport stood up in-process per invocation.
The abstraction-first fix is to extract that bootstrap once and use it from all
three callers (daemon, `verify`, standalone audit).

## What Changes

- Extract a shared `control_socket::spawn_submission_listener(paths)` helper that
  stands up the submission transport in-process for a single invocation: it
  constructs `crate::submission_store::SubmissionStore::new()`, registers the gate
  submission schemas
  (`preflight::change_contradiction::register_contradiction_submission_schema`,
  `preflight::canon_contradiction::register_canon_contradiction_submission_schema`,
  `preflight::global_rules::register_rule_violations_submission_schema`) plus the
  audit schemas (`audits::register_submission_schemas`), binds the socket
  (`control_socket::bind_at`), sets `ENV_CONTROL_SOCKET`
  (`mcp_askuser_server::ENV_CONTROL_SOCKET`), spawns `control_socket::serve(...)`,
  and tears down via a `CancellationToken` on drop (serve removes the socket file).
  This is the same sequence `cli/run.rs` already does inline at daemon startup; the
  helper makes it a reusable primitive (three callers: daemon, `verify`, standalone
  audit).
- A `verify <change-slug>` subcommand stands up `spawn_submission_listener` as a
  hard precondition, then runs the enabled pre-executor verifier-gate checks
  (`[in]`, `[canon]`, `[rules]`, and any other enabled spec-checking gate) against a
  change in the LOCAL working tree, reusing the same check entry points
  (`preflight::change_contradiction::run_agentic_contradiction_check`,
  `preflight::canon_contradiction::run_agentic_canon_contradiction_check`,
  `preflight::global_rules::run_agentic_global_rules_check`), prompts, model config
  (`executor.change_internal_contradiction_check_llm`,
  `executor.change_canonical_contradiction_check_llm`,
  `executor.global_rules_check_llm`), and submission schemas the server uses.
- `verify` runs in the repository's working directory, operating on
  `openspec/changes/<change-slug>/` and the local `openspec/specs/`. It reports
  findings to stdout, grouped and labeled by gate. It does NOT run the executor,
  write `.needs-spec-revision.json`, or make spec/source edits; the only files it
  touches are transient run artifacts (`.mcp.json`, the control socket) which it
  cleans up on exit.
- Exit code is CI-usable, conforming to `gatekeepers-fail-closed`: `0` only when
  every gate that ran is clean; non-zero when any gate finds a contradiction; AND
  (fail-closed) non-zero when an enabled gate cannot run — it never reports clean
  for a gate that did not actually run. When the config enables NO spec-checking
  gate, `verify` does NOT exit 0 silently: it reports that no gate evaluated the
  change and exits non-zero (code never manufactures a clean pass when nothing was
  evaluated — `gatekeepers-contain-no-judgment`).
- `verify` resolves its session timeout from `executor.agentic_session_timeout_secs`
  via the same `ExecutorConfig::agentic_session_timeout()` path the daemon uses
  (default `3600` if omitted) — not a verify-local literal.
- By default `verify` runs the gates ENABLED in config (matching server
  enforcement); a selector overrides (`--all`, or `--gate in,canon`).
- `cli/audit.rs::run_standalone` is wired through `spawn_submission_listener` so
  daemon-absent advisory audits (architecture_advisor, drift, documentation,
  canon_contradiction) capture their `submit_findings` verdict instead of failing
  closed.
- A check-only install (a prebuilt binary plus a minimal config carrying only the
  gate model blocks and corpus locations) lets `verify` run on a low-powered
  spec-authoring machine without building from source or running the daemon.

## Impact

- Affected specs: `orchestrator-cli` (the new `verify` subcommand AND the
  standalone-audit submission-capture fix; CLI subcommands live here alongside
  `rewind`, `audit run`, `reload`).
- Affected code:
  - `control_socket.rs`: new `spawn_submission_listener(paths)` helper (extracted
    from the inline bootstrap in `cli/run.rs`).
  - `cli/run.rs`: optionally refactored to call the helper at startup (single
    source of the bootstrap sequence).
  - `cli/verify.rs` (new) + `Command::Verify` in `cli/mod.rs`: a thin driver that
    stands up the listener, invokes the existing gate-check entry points against
    working-tree paths, and renders findings.
  - `cli/audit.rs::run_standalone`: wired through the helper.
  - The check-only install script (fetch prebuilt binary, place on PATH, drop a
    minimal config).
- Reuses, does not redefine, the verifier-gate framework — `verify` is a new
  invocation surface for the existing `[in]`/`[canon]`/`[rules]` checks, outside the
  executor lifecycle. (`verifier_gate.rs` is only the registry/labels/retry helper
  and `llm.rs` only resolves the model; neither is the check function.)
- One helper, three callers: the same primitive fixes the daemon-absent standalone
  audit gap.
- This is the local accelerator; the server gates remain the fail-closed
  enforcement (they run against fresher canon and cover all contributors).
