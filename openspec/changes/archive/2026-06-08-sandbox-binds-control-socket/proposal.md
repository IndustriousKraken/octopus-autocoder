## Why

Every agentic role relays its structured result to the daemon over the Unix-domain control socket at `<runtime>/control.sock` — the implementer's `outcome_success` / `outcome_request_iteration` / `outcome_spec_needs_revision`, the audits' `submit_findings`, the reviewer's `submit_review`, and `query_canonical_specs`. The relay runs in the per-execution MCP child, which is a descendant of the wrapped CLI and therefore runs **inside the OS-level sandbox** (a006).

But the sandbox never binds the control socket into the child's mount namespace — it binds only the workspace, the role's CLI stores, and the CLI binary. The socket path is forwarded as an env var (`ORCH_DAEMON_CONTROL_SOCKET`), but the socket file itself is absent from the namespace whenever it lives in a masked location: under `/tmp` (the sandbox sets systemd `PrivateTmp=yes` / bwrap `--tmpfs /tmp`) or under a masked `$HOME` (read-only roles get `ProtectHome=tmpfs` / `--tmpfs <home>`). `<runtime>` defaults to `/tmp/<uid>-runtime/autocoder` whenever the daemon runs without a systemd `RuntimeDirectory=` (no `$RUNTIME_DIRECTORY`) and `runtime_dir` is unset — a common deployment — so the socket sits under `/tmp` and the sandbox hides it.

The symptom: the implementer finishes the change, then its `connect()` to the socket fails. The agent reads it as a "control-socket outage," retries for the whole window, and the executor subprocess **times out** — the completed work is never recorded and is discarded on the next dirty-recovery. This is the same root cause as the audit-settings-file `/tmp` bug, one layer over.

## What Changes

The OS sandbox SHALL bind the daemon's control socket into the child's mount namespace, read-only, in every mechanism (systemd-run, bwrap, sandbox-exec) AND under every filesystem policy (denylist, allowlist). A read-only bind is sufficient — connecting to a Unix-domain socket is a socket operation, not a filesystem write. The bind is applied AFTER the policy's masking steps (the private `/tmp`, the masked home), so a socket residing under `/tmp` or a masked location is re-exposed and remains connectable. When no control socket is configured (the env var is unset — tests, or a daemon run without the relay), no bind is added.

This does not widen the trust boundary: the control socket is the *intended*, already-authorized relay channel for these roles (the MCP child connects to it by design), and the daemon validates every request it receives. Making the socket reachable only lets the sanctioned relay succeed.

## Impact

- **Affected specs:** `executor` — ADD `OS sandbox exposes the daemon control socket to the sandboxed relay`.
- **Affected code:** `sandbox.rs` — a generic `extra_ro_paths` on `SandboxPlan` bound read-only by `systemd_run_argv`, `bwrap_argv`, AND `seatbelt_profile` after the masking steps; `agentic_run` threads the resolved control-socket path (`ORCH_DAEMON_CONTROL_SOCKET`) into the plan when set.
- **Operator-visible behavior:** agentic runs under the OS sandbox can reach the control socket regardless of where `<runtime>` resolves (including `/tmp`); the implementer's success outcome is recorded instead of timing out.
- **Related (out of scope here):** the daemon's `runtime` defaulting to `/tmp` when neither `runtime_dir` nor `$RUNTIME_DIRECTORY` is set is fragile independent of the sandbox; this change makes the relay correct regardless of that default, but a follow-up MAY prefer a non-`/tmp` runtime default / surface a warning.
- **Dependencies:** builds on `Every agentic subprocess runs inside an OS-level sandbox` AND `Per-execution MCP child exposes outcome tools via control-socket relay`. No unmerged dependencies.
- **Acceptance:** `cargo test` passes; `openspec validate sandbox-binds-control-socket --strict` passes. Tests: the control socket appears as a read-only bind in the systemd-run argv, the bwrap argv, AND the seatbelt profile, under BOTH the denylist and allowlist policies; the bind is placed after the `/tmp` / home masking; an empty `extra_ro_paths` adds no bind; `agentic_run` threads the socket path into the plan when `ORCH_DAEMON_CONTROL_SOCKET` is set AND omits it when unset.
