# Control socket has no authorization; a sandboxed agent can drive operator actions and forge gate verdicts

## Symptom

The daemon control socket exposes the full operator action surface AND the
gate-verdict submission surface with no authentication, no session binding, and
no per-connection identity. The socket is deliberately bridged into every
sandboxed agent's namespace, so a prompt-injected agent (untrusted repo content
driving the executor) can invoke any action against any configured repo.

## Why

**Socket file mode is correct but irrelevant to the real threat.** The socket is
`0600` in a `0750` runtime dir (`control_socket.rs:684`, `paths.rs:539`) — that
blocks *other* local uids. But `agentic_run.rs:1447-1448` ro-binds the same
socket into every agent namespace and `ORCH_DAEMON_CONTROL_SOCKET` is set in the
agent env; a read-only bind does not block `connect()`. The agent runs at the
daemon's uid. So for the sandboxed agent, connect == full trust.

**Dispatch does no auth.** `dispatch_request` (`control_socket.rs:884-901`) parses
one JSON line, reads `action`, and routes to the handler — no token, no
`SO_PEERCRED`, no session check. Each handler resolves its target repo from the
attacker-supplied `url` via `find_repo` (`control_socket.rs:909`), not from any
session identity. So an agent spawned for repo A can act on repo B by passing
B's url.

Reachable state-mutating actions from the `DISPATCH` table include
`wipe_workspace` (deletes any repo's workspace), `rollback_recovery` /
`ignore_for_queue_marker` (commit + force-push to the agent branch, open a PR),
`clear_perma_stuck_marker` / `clear_revision_marker` (clear safety markers, incl.
a `*` wildcard sweep), and the `queue_*` / `prioritize` / `trigger_audit` /
`reload` scheduling mutators.

**Gate-verdict forgery.** `handle_record_submission` (`handlers.rs`) stores a
payload keyed by `(workspace_basename, change)` taken from the REQUEST, with no
session binding; `submission_store::record` is last-writer-wins and most gate
roles have no registered schema ("a role with no registered schema accepts any
payload"). The daemon's reviewer/contradiction/canon/rule gates drain this via
`consume_submission` and treat the payload as that role's verdict. So the
executor (running first, same workspace/change) can submit a "clean/approve"
payload for `role:"reviewer"` (and the contradiction/canon/rule roles) BEFORE the
gate runs; the daemon consumes the forged verdict and the malicious change passes
review → gets pushed/PR'd. This closes the loop from "prompt-inject the agent" to
"merge attacker-authored code".

Impact: sandbox escape into the daemon control plane; cross-repo unauthorized
actions; CI/review-gate bypass. Server/pipeline-compromise severity.

Chosen approach: PER-SESSION RELAY SOCKET. Each spawned agent gets its own
relay-only socket bound to its `(workspace_basename, change, role)`, sharing the
daemon's stores. Identity is intrinsic to the socket, so forgery is impossible
by construction and the operator surface is unreachable — one mechanism fixes
both holes.

## Tasks

- [x] Relay-only dispatch: `RELAY_ACTIONS` allowlist (`record_submission`,
  `record_outcome`, `record_advertised_tool`, `query_canonical_specs`); when
  `ControlState.relay_identity` is `Some`, `dispatch_request` refuses every other
  action (operator surface + daemon-side `consume_*`). Done in `control_socket.rs`.
- [x] Per-session identity stamping: `relay_identity_field` makes the daemon-bound
  `(workspace_basename, change, role)` authoritative in the four relay handlers,
  ignoring the request's fields — kills cross-repo targeting AND verdict forgery.
  Done in `control_socket.rs` + `control_socket/handlers.rs`.
- [x] ACTIVATION: `spawn_session_relay` + a startup-registered `SessionRelayDeps`
  (`cli/run.rs`, sharing the daemon's stores) stand up a per-session relay
  listener bound to the run's identity (`workspace.file_name()`,
  `change`==`role`==`opts.change` — matching what the daemon consumes by). At
  agent spawn, `agentic_run` binds the per-session relay socket OVER the shared
  control-socket path inside the sandbox (new `SandboxPlan.control_socket_remap`,
  honored by bwrap `--ro-bind-try SRC DEST` and systemd `BindReadOnlyPaths=src:dest`)
  — so `ENV_CONTROL_SOCKET` stays unchanged and the MCP child transparently
  reaches its own relay socket. No per-strategy MCP-config threading needed. The
  daemon still consumes over its own full socket; both hit the shared store. The
  guard is held for the run and drops (tearing the socket down) on return.
  Documented dev-only gap: sandbox-exec (macOS) cannot remap a path, so there the
  agent reaches the shared socket — the production deployment is Linux.
- [ ] Register a real schema for every gate role so `record_submission` cannot
  accept an arbitrary "approve" blob (defense-in-depth on top of identity stamping).
- [ ] Consider an `SO_PEERCRED`/token check as defense-in-depth on the operator
  socket even though same-uid limits its value.

## Tests

- [x] A relay socket (`relay_identity` set) refuses `wipe_workspace` /
  `rollback_recovery` / `clear_revision_marker` / `reload` / `consume_submission`
  and still accepts a genuine relay action
  (`control_socket::tests::relay_socket_refuses_non_relay_actions`).
- [x] A `record_submission` forging `role: reviewer` from an executor-bound relay
  socket is stamped to the executor's own `(workspace, change)` — the reviewer
  slot the daemon consumes stays empty
  (`control_socket::tests::relay_socket_stamps_identity_blocking_verdict_forgery`).
- [ ] End-to-end (after activation): a spawned agent's MCP child reaches only its
  per-session relay socket; an operator action over it is refused.
- [ ] A gate role with a registered schema rejects a payload that doesn't match
  the schema (forged "approve" blob is refused).
