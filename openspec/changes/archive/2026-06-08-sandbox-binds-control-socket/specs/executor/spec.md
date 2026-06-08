# executor — delta for sandbox-binds-control-socket

## ADDED Requirements

### Requirement: OS sandbox exposes the daemon control socket to the sandboxed relay
Every agentic role relays its structured result to the daemon over the Unix-domain control socket via the per-execution MCP child, which runs INSIDE the OS-level sandbox. The OS sandbox SHALL therefore bind the daemon's control socket into the child's mount namespace, read-only, so the relay can `connect()` to it. (A read-only bind is sufficient: connecting to a Unix-domain socket is a socket operation, not a filesystem write.) The bind SHALL be applied in every mechanism — `systemd-run`, `bwrap`, AND `sandbox-exec` — AND under every filesystem policy — the executor's denylist AND the read-only roles' allowlist.

The bind SHALL be applied so that it survives the policy's masking steps: the private `/tmp` (systemd `PrivateTmp=yes` / bwrap `--tmpfs /tmp`) AND the masked home (allowlist `ProtectHome=tmpfs` / `--tmpfs <home>`). A control socket residing under `/tmp` or under a masked home SHALL remain connectable from inside the sandbox.

When no control socket is configured for the run (the relay env var is unset), no such bind SHALL be added.

This does not widen the sandbox trust boundary: the control socket is the intended, already-authorized relay channel for these roles, and the daemon validates every request it receives — exposing the socket only lets the sanctioned relay succeed.

#### Scenario: Control socket is bound under the executor (denylist) policy
- **WHEN** the executor spawns under the OS sandbox AND a control socket is configured for the run
- **THEN** the constructed sandbox invocation binds the control-socket path into the namespace read-only
- **AND** the relay's `connect()` to the socket succeeds from inside the sandbox

#### Scenario: Control socket is bound under a read-only role (allowlist) policy
- **WHEN** a read-only role (an audit or an agentic reviewer) spawns under the OS sandbox AND a control socket is configured
- **THEN** the constructed sandbox invocation binds the control-socket path into the namespace read-only, even though the home directory is masked

#### Scenario: A control socket under /tmp survives the private-tmp masking
- **WHEN** the control socket resides under `/tmp` (the runtime directory fell back to the per-uid temp location) AND the sandbox applies a private `/tmp`
- **THEN** the control-socket bind is applied AFTER the private-`/tmp` masking
- **AND** the socket remains present AND connectable inside the namespace

#### Scenario: No control socket configured adds no bind
- **WHEN** no control socket is configured for the run (the relay env var is unset)
- **THEN** the constructed sandbox invocation adds no control-socket bind
