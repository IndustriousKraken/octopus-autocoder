# Tasks

OpenSpec: implements the two ADDED requirements in
`specs/orchestrator-cli/spec.md` (session-log persistence; no-submission
diagnosability).

## 1. Shared session-log writer

- [x] 1.1 Add a small helper that persists a captured `AgenticRunOutcome`
  (`agentic_run.rs`: `final_answer`, `stdout`, `stderr`, `exit_status`,
  `timed_out`) to a discoverable per-session log under the run-logs directory,
  named by (category, name, timestamp), mirroring the audits' run-log writer
  (`autocoder/src/audits/mod.rs` `AuditLogWriter` / `persist_run_log`). Prefer
  reusing/generalizing the audit writer over a third bespoke writer (leave a clear
  seam so the planned reviewer session log can share it). Return the written
  path so callers can name it in alerts.

## 2. Persist each gate session's output (every outcome)

- [x] 2.1 In the `[in]` gate runner (`autocoder/src/preflight/change_contradiction.rs`,
  `CliContradictionSessionRunner::run_session`), after `agentic_run_with_session`
  returns, persist the captured outcome via the helper — for ALL outcomes (clean,
  findings, no-submission, timeout, error), BEFORE the no-submission/timeout
  branches form their result. Use a path keyed by gate label + change slug +
  timestamp (e.g. `gates/in-<change>-<timestamp>.log`).
- [x] 2.2 Do the same for the `[canon]` gate
  (`autocoder/src/preflight/canon_contradiction.rs` / `corpus_check.rs`), the
  `[rules]` gate (`autocoder/src/preflight/global_rules.rs`), and the `[out]` gate
  (`autocoder/src/code_implements_spec.rs` / `verifier_gate.rs`). If they share a
  session runner, persist once at the shared layer so all four gates are covered.
- [x] 2.3 Thread the written log path into the fail-closed outcome so the WARN and
  the chatops alert can name it.

## 3. Name the log path in the operator-facing surfaces

- [x] 3.1 In the fail-closed "gate FAILED TO RUN — change held" alert rendering
  (`autocoder/src/polling_loop/alerts_throttle.rs` and/or the marker `gate_error`
  surfacing) AND the WARN line, include the persisted log-file path (as the
  executor's failure reason names its log).

## 4. No-submission diagnosability (daemon-side records)

- [x] 4.1 In the MCP submission server (`autocoder/src/mcp_askuser_server.rs`),
  record which submission tool it advertised for the session's `ORCH_MCP_ROLE`
  (or that no tool matched the role) — emit it where it lands in the session's
  captured output and/or a discoverable record cross-referenceable by role. This
  makes "tool never advertised" (mode a) determinable.
- [x] 4.2 In the daemon submission path (the relay write — `relay_submission` —
  and `consume_submission` in `autocoder/src/audits/mod.rs`), record whether a
  submission was relayed for the session/role, so a consume that finds none can be
  distinguished as "relayed-but-not-consumed" vs "never relayed" (mode c vs b).
  Log at relay-time and at consume-time, keyed by role.
- [x] 4.3 Keep these as records/logging only — no control-flow change; the
  fail-closed disposition is unchanged.

## 5. Tests

- [x] 5.1 Session-log writer: given a captured outcome (non-empty stdout/stderr,
  an exit status, timed-out true/false), the helper writes a file under the
  run-logs dir whose content includes that captured output. Assert file existence
  + captured content presence (data flow), not exact formatting.
- [x] 5.2 Per-outcome persistence: drive a gate runner via its existing injected
  seam for (a) no-submission, (b) clean, (c) findings, (d) timeout/error — assert
  a log file is written in each case, and that the fail-closed outcome carries the
  log path. Use the existing test seams (e.g. the `[in]` gate's `test_submission`
  injection); do not spawn a real subprocess.
- [x] 5.3 Alert names the path: the fail-closed alert/WARN for a held gate
  includes the persisted log path. Assert the path is present (data flow), not the
  surrounding wording.
- [x] 5.4 Diagnosability records: the MCP server records the advertised tool for a
  given role (e.g. role `contradiction_check` → `submit_contradictions`; an
  unknown role → none); the submission path distinguishes relayed-but-not-consumed
  from never-relayed. Assert the recorded facts, not message wording.

## 6. Validation

- [x] 6.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [x] 6.2 `openspec validate verifier-gates-persist-session-log --strict` from the
  repo root.
