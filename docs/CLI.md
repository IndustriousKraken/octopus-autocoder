# CLI Reference

```
autocoder <COMMAND>
```

## `run`

Start the polling daemon.

```bash
autocoder run --config <path-to-config.yaml>
```

The daemon polls every configured repository on its interval, processes ready OpenSpec changes, and opens monolithic PRs. Terminates only on SIGINT, SIGTERM, or a fatal initialization error. Logs go to stderr; control verbosity with `RUST_LOG=info` (default), `RUST_LOG=debug`, etc.

## `reload`

Ask a running daemon to re-read its YAML config and hot-apply the `github`, `reviewer`, and `chatops` sections.

```bash
sudo -u autocoder autocoder reload
```

The CLI connects to the daemon's Unix-domain control socket at `/tmp/autocoder/control/control.sock`, sends `{"action":"reload"}`, and prints the daemon's pretty-printed JSON response to stdout (exit 0) or stderr (exit non-zero). The socket file is mode `0600` and owned by the user the daemon runs as, so the CLI must run as the same user — hence `sudo -u autocoder`. If the daemon is not running, the CLI prints an error naming the expected socket path and exits non-zero. See [Runtime control: live config reload](OPERATIONS.md#runtime-control-live-config-reload) for the full behavior.

## `audit run`

Trigger an audit on-demand for one workspace, complementing the cadence-based scheduling that `autocoder run` does in the background.

```bash
# With the daemon running: enqueue the audit for the next polling iteration.
autocoder audit run --workspace /tmp/workspaces/github_com_acme_myrepo --audit security_bug_audit

# Without the daemon: invoke the audit module directly against the workspace
# and print findings to stdout. Useful for prompt-template iteration.
autocoder audit run --workspace /path/to/checkout --audit architecture_brightline
```

**`--audit`** is the exact `audit_type` slug (e.g. `security_bug_audit`, `drift_audit`, `architecture_brightline`). The chatops verb does substring matching against the operator's argument; the CLI does NOT — typing `--audit sec` is rejected with an `unknown audit` error listing the registered names. This is a deliberate asymmetry: a CLI call may be running inside a script where a substring match that suddenly resolves differently after a registry change would be surprising.

**Daemon-present path.** The CLI probes for the control socket at `/tmp/autocoder/control/control.sock`. When the socket is reachable, the CLI sends a `queue_audit` action with the workspace path; the daemon resolves the workspace to a managed repo and appends the audit-type to that repo's `pending_audit_runs` queue. The CLI prints the daemon's ack (`✓ Queued <audit> for <url>. Will run on the next polling iteration (~Nm).`) and exits 0. When the workspace is NOT in the daemon's repo list, the CLI prints an error naming the workspace and the daemon's known repos and exits non-zero — the CLI does NOT fall back to standalone mode in that case, because the daemon owns the workspace's lifecycle when present and a standalone invocation would race the daemon.

**Daemon-absent path.** When the socket is missing or refusing connections, the CLI builds a minimal audit registry, looks up the audit by name, constructs an in-memory `RepositoryConfig` whose `local_path` is `--workspace`, and invokes the audit's `run` directly. Findings (and any other outcome variant) are printed to stdout. This path skips the post-hoc write-policy enforcement the scheduler does, so use it only against a workspace you control and intend to inspect by hand.

Exit codes: 0 on success (queue ack OR standalone success), non-zero on any error (unknown audit, daemon refused the request, audit `run` errored, …).

## `rewind`

Discard the in-flight agent branch and re-queue one or more archived changes. Use this when an agent produced unusable work or a PR was rejected and you want the daemon to try again.

```bash
# Soft rewind (single-repo config): prompt for confirmation, then delete
# the local agent branch and unarchive one change.
autocoder rewind my-broken-change --config config.yaml

# Hard rewind: skip the prompt, delete local AND remote agent branch,
# then unarchive two changes.
autocoder rewind change-A change-B --config config.yaml --hard

# Multi-repo config: --repo is REQUIRED. The selector matches either the
# full URL or the short-name (basename minus .git).
autocoder rewind my-change --config config.yaml --repo my-repo
```

**Soft vs hard semantics:**

| Mode     | Confirmation prompt | Local agent branch | Remote agent branch                       |
|----------|---------------------|--------------------|-------------------------------------------|
| soft     | y/N, defaults no    | deleted            | left intact                                |
| `--hard` | skipped             | deleted            | deleted (failures logged but non-blocking) |

The confirmation prompt for soft rewind looks like:

```
This will delete branch 'agent-q' (local) and unarchive 1 change(s) (my-broken-change). Proceed? [y/N]
```

Bare Enter, `n`, or any input other than `y`/`Y` declines and exits without modifying any state.

**`--repo` selector:**

With **one** configured repository, `--repo` is optional and defaults to that repo. With **two or more** configured repositories, `--repo` is required. autocoder matches the selector against each repository's full URL (exact equality) AND against the URL's short-name (basename with any trailing `.git` stripped). Zero matches or multiple matches exit non-zero with an error listing the available selectors.

**Unarchiving multiple changes:**

If you pass multiple change names and one of them fails to unarchive (typo, no matching archive entry, destination collision), the remaining names are still attempted. The process exits non-zero at the end with a summary naming both the succeeded and failed changes.

**Recovering from an accidental rewind:**

Archived directories are **not** deleted by archive — they are renamed under `openspec/changes/archive/<YYYY-MM-DD>-<name>/`. To reverse an accidental rewind, move the directory back into the archive manually. The canonical date-prefix format is preserved by autocoder's `archive` step, so a manual `mv` restores the queue's view of state.

---
