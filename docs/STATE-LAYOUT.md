# State layout

autocoder writes four categories of data, each with its own resolved
directory. The split exists so the daemon survives a host reboot
without losing operator-meaningful state (audit cadence, failure
counters, perma-stuck markers) while still letting transient
artefacts (control socket, in-progress pid locks) be cleared by the
reboot.

## Categories

| Category  | What lives here                                              | Survives reboot |
|-----------|--------------------------------------------------------------|------------------|
| `state`   | Audit cadence state, failure counters, revision state, alert throttles, audit-thread state | Yes |
| `cache`   | Per-repo cloned workspaces (`<cache>/workspaces/<sanitized-url>/`) and the in-tree marker files inside them | Yes |
| `logs`    | Per-change run logs (`<logs>/runs/<repo>/<change>.log`) and audit logs (`<logs>/runs/<repo>/audits/<type>-<ts>.log`) | Yes |
| `runtime` | Control socket (`<runtime>/control.sock`), per-workspace busy markers (`<runtime>/busy/<workspace>.json`), subprocess sidecar PIDs | No (by design) |

## Defaults by mode

| Category  | Server mode (systemd)    | Dev mode (XDG)                                       |
|-----------|--------------------------|------------------------------------------------------|
| `state`   | `/var/lib/autocoder`     | `${XDG_STATE_HOME:-$HOME/.local/state}/autocoder`    |
| `cache`   | `/var/cache/autocoder`   | `${XDG_CACHE_HOME:-$HOME/.cache}/autocoder`          |
| `logs`    | `/var/log/autocoder`     | `${XDG_STATE_HOME:-$HOME/.local/state}/autocoder/logs` |
| `runtime` | `/run/autocoder`         | `${XDG_RUNTIME_DIR:-/tmp/${UID}-runtime}/autocoder`  |

## Resolution priority

Each path is resolved at startup by this precedence (first non-empty
value wins):

1. `config.yaml`'s `paths.<field>` override.
2. The per-field environment variable: `AUTOCODER_STATE_DIR`,
   `AUTOCODER_CACHE_DIR`, `AUTOCODER_LOGS_DIR`, `AUTOCODER_RUNTIME_DIR`.
3. The systemd-set environment variable: `$STATE_DIRECTORY`,
   `$CACHE_DIRECTORY`, `$LOGS_DIRECTORY`, `$RUNTIME_DIRECTORY` (auto-
   populated by the rendered unit's `*Directory=autocoder` directives).
4. XDG-derived defaults under `$HOME` (dev mode).
5. Hard fallback to `/var/lib/autocoder` etc. — emits a WARN log on
   the way out because no override was found at all.

All four paths must resolve to absolute, distinct directories. A
relative path or a collision between two roles is a startup error.

## Path resolution rule

Every daemon-side state-file read AND write SHALL route through the
`DaemonPaths` resolver in `autocoder/src/paths.rs`. The resolver exposes
the four bare roots (`state`, `cache`, `logs`, `runtime`) plus a set of
per-state-shape helpers — `audit_threads_dir()`, `busy_markers_dir()`,
`proposal_requests_dir()`, `changelog_requests_dir()`,
`failure_state_dir()`, `revisions_dir()`, `audit_state_dir()`,
`run_logs_dir(<basename>)`, `audit_logs_dir(<basename>)`,
`workspaces_dir()`, `control_socket_path()` — that callers use instead
of constructing paths inline.

### State-dir subdirectories

| Subdirectory                | Contents                                                                                           |
|-----------------------------|----------------------------------------------------------------------------------------------------|
| `<state>/audit-state/`      | Per-audit-type cadence + last-run state, one `<audit-type>.json` per registered audit.             |
| `<state>/alert-state/`      | Per-workspace alert-throttle state, one `<workspace-basename>.json` per managed repository. Holds the daemon-wide `.migration-from-workspace-done` marker for the `a16` migration. |
| `<state>/failure-state/`    | Per-(repo, change) failure counters that drive perma-stuck detection.                              |
| `<state>/revisions/`        | Per-PR reviewer-revision state.                                                                    |
| `<state>/audit-threads/`    | Per-`thread_ts` state files for the `audit-reply-acts` (send-it) flow.                             |
| `<state>/proposal-requests/`| Per-`request_id` state files for the chat-request-triage (propose) flow.                           |
| `<state>/changelog-requests/`| Per-`request_id` state files for the changelog-stylist flow.                                      |

The rule exists to prevent a defect class where readers and writers
drift to different paths after the legacy-to-standard migration.
Operator-visible symptoms of the defect class (now fixed by `a09`):

- `send it` returning `?` for real audit threads, because the writer
  stamped state at `<state>/audit-threads/` while a stale reader looked
  under the legacy `/tmp/...` path and found only test fixtures.
- `@<bot> status` reporting `idle` while the busy marker existed,
  because the status reader and the busy-marker writer resolved their
  paths through different code paths.

The rule is **CI-enforced**. The integration test
`autocoder/tests/path_literals_audit.rs` greps every `*.rs` file under
`autocoder/src/` for the literal substring `/tmp/autocoder` and fails
the build on any hit outside a narrow allowlist (today: only
`src/migration.rs`, which references the legacy path on purpose so it
knows what to move). The failure message names the offending
`file:line:line-contents` AND points at the `DaemonPaths` resolver as
the correct fix.

**Adding a new state-file shape:** add a helper to `DaemonPaths`, use
it from the consumer side. The CI test passes automatically — no
allowlist edit needed unless the new code legitimately references the
legacy path for migration purposes.

## Migration from `/tmp/`

Pre-`state-paths-out-of-tmp`, autocoder wrote everything under `/tmp/`
which on most Linux server distributions is `tmpfs` — wiped on every
reboot. On the first daemon start after upgrade — detected by the
absence of `<state>/.migration-from-tmp-done` — a migration pass
scans these well-known legacy paths and moves their contents:

- `/tmp/workspaces/<entry>/` → `<cache>/workspaces/<entry>/`
- `/tmp/autocoder/audit-state/*.json` → `<state>/audit-state/`
- `/tmp/autocoder/failure-state/**/*.json` → `<state>/failure-state/`
- `/tmp/autocoder/revisions/**/*.json` → `<state>/revisions/`
- `/tmp/autocoder/logs/**/*.log` → `<logs>/runs/`

The migration is idempotent (the marker is what gates the scan),
per-entry error-tolerant (one failing entry does not abort the rest),
and writes the marker only when every entry completed without error.
Cross-partition moves (tmpfs → disk is the common case) fall back to
recursive copy + delete-on-success when `fs::rename` returns `EXDEV`.

Migration failures are LOGGED to `journalctl -u autocoder`; the
daemon does NOT refuse to start. Operators see per-entry ERROR lines
and can manually move or delete any orphan `/tmp` entries.

If the `<state>/.migration-from-tmp-done` marker is missing AFTER
the daemon has been up for a few minutes — and you see no migration
log line at startup — the daemon never ran the scan. Most common
cause: the daemon's runtime user does not have read access to the
legacy `/tmp/` paths (a known issue under `PrivateTmp=true` because
the systemd unit gets its own `/tmp` namespace). Migrate the data
manually with the same paths listed above.

To force a re-scan after restoring legacy data from backup, remove
`<state>/.migration-from-tmp-done` and restart the daemon.

## Alert-state migration from workspace (a16)

Pre-`a16`, alert-throttle state lived at `<workspace>/.alert-state.json`.
That daemon-written file appearing inside the managed repo's working
tree caused operator-visible failures: `git checkout` aborts on tracked-
but-modified `.alert-state.json`, `WritePolicy::None` audits saw the
daemon's own write as a violation, and the canonical spec was internally
contradictory about the file's location. The principled fix moved the
file to `<state>/alert-state/<workspace-basename>.json`.

On the first daemon start after upgrade — detected by the absence of
`<state>/alert-state/.migration-from-workspace-done` — the daemon walks
every configured repository and:

1. If the workspace has no `.alert-state.json`, no-op.
2. If both the workspace and state-dir versions exist, the state-dir
   version wins (more recently authoritative); the workspace copy is
   removed.
3. If only the workspace version exists, `fs::rename` moves it to
   `<state>/alert-state/<workspace-basename>.json` (cross-partition
   `EXDEV` falls back to copy + delete).
4. If the workspace file is tracked by git (`git ls-files --error-unmatch`
   succeeds), the daemon runs `git rm --cached`, commits with subject
   `chore: untrack .alert-state.json (now stored in daemon state dir per a16)`,
   and pushes to the base branch using the same token + auth path as
   normal autocoder pushes.

Per-repo failures (push rejected by branch protection, etc.) are logged
at ERROR with the suggested operator action (manual `git rm --cached`
+ PR) and the daemon continues to other repositories. The marker is
written only when every repo's outcome was clean; otherwise the next
startup retries every repo (idempotent reads on already-migrated repos).

The workspace-init step also enforces an invariant: if the migration
marker is present AND a `.alert-state.json` appears in the workspace
later (code drift, or a fresh re-clone of a repo whose history
transiently included it), the daemon logs WARN and removes the file.

To force a re-scan, remove `<state>/alert-state/.migration-from-workspace-done`
and restart the daemon.

## Workspace-local markers (stay in the workspace)

The workspace move to `<cache>/workspaces/` automatically preserves
every operator-meaningful in-tree file:

- `.perma-stuck.json` — operator action: delete to retry the change.
- `.needs-spec-revision.json` — operator action: edit the spec, then
  delete to retry.
- `.question.json` / `.answer.json` — askuser flow state.
- `.in-progress*` — per-iteration progress markers.

`.alert-state.json` is **no longer** a workspace-local file. Per
`a16-consolidate-workspace-bookkeeping-to-state-dir`, alert-throttle
state lives at `<state>/alert-state/<workspace-basename>.json` so the
daemon's writes never appear inside the managed repository's working
tree (and never trip `git checkout` / dirty-check). The first-startup
migration described below moves any pre-existing workspace-rooted
files automatically.

These continue to live inside the workspace directory because they
are operator-visible artefacts inside the change's directory (or at
the workspace root). The split between state-dir state (daemon-global
accounting indexed by repo+change) and workspace-local markers
(per-checkout, indexed by change directory) is deliberate.

## See also

- [`docs/CONFIG.md`](CONFIG.md) — full configuration reference,
  including the `paths:` block schema.
- [`docs/TROUBLESHOOTING.md`](TROUBLESHOOTING.md) — symptoms +
  remediation, including "audit storm after reboot" diagnosis.
- [`docs/DEPLOYMENT.md`](DEPLOYMENT.md) — production deployment
  guide.
