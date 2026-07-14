## MODIFIED Requirements

### Requirement: Daemon resolves four standard data-category paths with a defined precedence
The daemon SHALL resolve four data-category paths at startup: `state` (persistent state — audit cadence, failure counters, alert throttles, revisions), `cache` (re-creatable but kept — repo workspaces), `logs` (per-change run logs), and `runtime` (control socket, transient locks). Each path is resolved by this precedence: (1) an explicit `paths.<field>` value in `config.yaml`, (2) the per-field environment variable `AUTOCODER_STATE_DIR` / `AUTOCODER_CACHE_DIR` / `AUTOCODER_LOGS_DIR` / `AUTOCODER_RUNTIME_DIR`, (3) the systemd-set environment variable `$STATE_DIRECTORY` / `$CACHE_DIRECTORY` / `$LOGS_DIRECTORY` / `$RUNTIME_DIRECTORY`, (4) XDG-derived defaults (dev mode), (5) a hard fallback to `/var/lib/autocoder` and siblings. All four paths SHALL be absolute. No two paths may resolve to the same directory.

At step (4), the runtime path's XDG-derived default SHALL trust `XDG_RUNTIME_DIR` only when the directory it names is owned by the process's current effective uid. When the variable is set but the directory is owned by a different uid, or cannot be inspected at all, the resolver SHALL ignore the variable — logging that it was ignored and why — and derive the runtime default as if the variable were unset (the `runtime/` subdirectory of the XDG state default). A foreign-owned `XDG_RUNTIME_DIR` is a leaked environment from an env-preserving user switch (`su <user>` without `-`), and per the XDG Base Directory specification the runtime directory MUST be owned by the user; resolving another user's private `/run/user/<uid>` can only produce permission failures or a wrong socket path. This ownership guard applies ONLY to the implicit XDG inference — explicit overrides at steps (1)–(3) are honored verbatim as today.

#### Scenario: Config explicit value wins over all env vars
- **WHEN** `config.yaml` sets `paths.state_dir: /custom/state` AND `AUTOCODER_STATE_DIR=/env/state` is set AND `$STATE_DIRECTORY=/var/lib/autocoder` is set
- **THEN** the resolved state path is `/custom/state`

#### Scenario: Env var wins over systemd-set var
- **WHEN** no config override AND `AUTOCODER_STATE_DIR=/env/state` AND `$STATE_DIRECTORY=/var/lib/autocoder`
- **THEN** the resolved state path is `/env/state`

#### Scenario: systemd-set var used when no config or env override
- **WHEN** no config override AND no env var AND `$STATE_DIRECTORY=/var/lib/autocoder`
- **THEN** the resolved state path is `/var/lib/autocoder`

#### Scenario: XDG defaults used in dev mode
- **WHEN** no config override AND no env var AND no systemd-set var AND `$HOME=/home/dev`
- **THEN** the resolved state path is `/home/dev/.local/state/autocoder` (or `$XDG_STATE_HOME/autocoder` when set)

#### Scenario: Owned XDG_RUNTIME_DIR is used verbatim for the runtime default
- **WHEN** no config, env, or systemd override applies to the runtime path AND `XDG_RUNTIME_DIR` names an existing directory owned by the current effective uid
- **THEN** the resolved runtime path is `$XDG_RUNTIME_DIR/autocoder`

#### Scenario: Foreign-owned XDG_RUNTIME_DIR is ignored
- **WHEN** no config, env, or systemd override applies to the runtime path AND `XDG_RUNTIME_DIR` names a directory owned by a different uid (e.g. the variable leaked through an env-preserving `su` from another user's session)
- **THEN** the resolved runtime path is the `runtime/` subdirectory of the XDG state default, exactly as if `XDG_RUNTIME_DIR` were unset
- **AND** the resolver logs that `XDG_RUNTIME_DIR` was ignored, naming the directory and the ownership mismatch

#### Scenario: Uninspectable XDG_RUNTIME_DIR is ignored
- **WHEN** no config, env, or systemd override applies to the runtime path AND `XDG_RUNTIME_DIR` names a path whose metadata cannot be read (missing, or traversal denied)
- **THEN** the resolved runtime path is the `runtime/` subdirectory of the XDG state default
- **AND** the resolver logs that `XDG_RUNTIME_DIR` was ignored, naming the path and the inspection error

#### Scenario: Explicit runtime overrides are never ownership-checked
- **WHEN** `AUTOCODER_RUNTIME_DIR` (or a `config.yaml` paths value, or systemd's `$RUNTIME_DIRECTORY`) names a directory regardless of its owner
- **THEN** the resolved runtime path honors that override verbatim — the ownership guard applies only to the implicit `XDG_RUNTIME_DIR` inference

#### Scenario: Relative-path config is rejected at startup
- **WHEN** `config.yaml` sets `paths.state_dir: relative/path`
- **THEN** the daemon fails to start with a clear error naming the field and requiring an absolute path

#### Scenario: Same path for two roles is rejected
- **WHEN** the resolution yields the same directory for two of the four roles
- **THEN** the daemon fails to start with an error naming both roles and the conflicting path

### Requirement: `autocoder reload` subcommand
autocoder SHALL provide a `reload` CLI subcommand that connects to the running daemon's control socket, sends `{"action":"reload"}`, prints the response, and exits 0 on success or non-zero on failure. The subcommand SHALL NOT require the daemon's `--config` path as an argument; the daemon already knows its config path and re-reads it from there.

#### Scenario: Successful reload
- **WHEN** the operator runs `autocoder reload`
- **THEN** the CLI connects to
  `<system-temp>/autocoder/control/control.sock`, sends the request,
  reads the response, prints it (pretty-printed JSON) to stdout,
  and exits 0 IF the response's `ok` field is `true`

#### Scenario: Reload rejected
- **WHEN** the daemon's reload handler returns `{"ok": false, ...}`
  (validation failure, IO error reading config, etc.)
- **THEN** the CLI prints the response to stderr and exits with
  a non-zero status

#### Scenario: Daemon not running
- **WHEN** `autocoder reload` is invoked and the control socket
  does not exist OR the connection is refused
- **THEN** the CLI prints an error message naming the expected
  socket path and exits non-zero
- **AND** the message hints at the likely causes: the daemon is
  not running, it is running under a different user, or a stale
  `XDG_RUNTIME_DIR` inherited from an env-preserving user switch is
  pointing the CLI at another user's runtime directory
- **AND** the message suggests retrying from a clean environment
  via `sudo -u autocoder autocoder reload`
