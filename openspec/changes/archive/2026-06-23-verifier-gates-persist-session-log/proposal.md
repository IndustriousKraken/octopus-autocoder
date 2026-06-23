# Verifier gate sessions persist a discoverable run log

## Why

When a verifier gate (`[in]`, `[canon]`, `[rules]`, `[out]`) holds a change
because its agentic session produced no submission, an operator cannot tell WHY.
The gate session runs in capture mode and the daemon keeps only a ~200-character
excerpt of the captured output in a WARN / chatops line; the full session output
is discarded. So a "gate FAILED TO RUN — change held" on, for example, a Kimi /
opencode session that read the delta files and then ended without calling
`submit_contradictions` is indistinguishable between several very different causes:

- the submission MCP tool was never advertised to the session,
- it was advertised but the model ended its turn without calling it,
- the model called it but no submission reached the daemon (a relay / control-socket failure),
- the session errored or the upstream API failed.

These have different fixes, and the operator currently has to guess. The daemon
already holds the full captured output of the session — it simply throws it away.
Persisting it (and the two facts the session's own stdout may not reliably show —
what the MCP server advertised, and whether a submission was relayed) turns a
held gate from an opaque dead end into a diagnosable event.

## What Changes

- Each verifier gate persists its agentic session's full captured output to a
  discoverable per-session log file, for EVERY outcome (clean, findings,
  fail-closed no-submission, timeout, error) — not only failures — so held
  changes are diagnosable and surprising clean/finding results are auditable.
- The log is stored under the run-logs directory, uniquely named by gate, change,
  and timestamp, mirroring the existing audit-run-log pattern. The fail-closed
  "gate FAILED TO RUN — change held" alert and the WARN name the log path.
- For the no-submission case specifically, the persisted log together with the
  daemon's submission-listener recording makes the failure mode determinable:
  what the MCP submission server advertised for the session's role, and whether a
  submission was received by the daemon before the consume found none.
- This is observability only. It does NOT change any gate prompt, the gate
  dispositions, or the fail-closed posture. It is provider-agnostic: it persists
  raw captured output and records facts; it never parses output for a decision.

## Impact

- Affected capability: `orchestrator-cli` — adds a session-log requirement to the
  verifier-gate framework and a no-submission-diagnosability requirement. The
  existing gate requirements (their fail-closed disposition) are unchanged; this
  layers logging on top.
- Affected code: the gate session runner(s)
  (`autocoder/src/preflight/change_contradiction.rs`,
  `autocoder/src/preflight/canon_contradiction.rs` / `corpus_check.rs`,
  `autocoder/src/preflight/global_rules.rs`, the `[out]` runner), reusing or
  mirroring the audits' run-log writer (`autocoder/src/audits/mod.rs`); the MCP
  submission server (`autocoder/src/mcp_askuser_server.rs`) to record the
  advertised tool for a role; and the daemon submission listener
  (`consume_submission` / the relay path) to record received-vs-none. The
  fail-closed alert rendering (`autocoder/src/polling_loop/alerts_throttle.rs`)
  to name the log path.
- Same observability principle as the reviewer-no-submission log in
  `executor-outcome-legibility-and-retry`; ideally both reuse one session-log
  helper rather than a third writer.
