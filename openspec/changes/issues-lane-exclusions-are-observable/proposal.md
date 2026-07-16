## Why

The issues lane's ready-list silently skips excluded units: an `.in-progress` lock or `.perma-stuck.json` park marker drops the issue from selection with no log line, the markers are `.git/info/exclude`d so they never reach an operator's clone, and neither `repo_status` nor the chatops `status` reply carries any issues-lane fields (they report the changes lane only). In production this left an issue invisible-stuck for three weeks: alphabetically-later issues ran while it sat, and the operator had no way — short of SSHing into the server workspace — to learn it was excluded, let alone why. A stale `.in-progress` lock (e.g. a crash leftover) is worse than a park marker: parking at least alerts once when it happens; a stale lock excludes forever and never alerts at all.

## What Changes

- Every excluded issue unit is logged per pass with its reason (locked, with lock age; parked, with marked-at), so the exclusion state is continuously visible in the journal instead of only at the moment of parking.
- Stale `.in-progress` locks are recovered: a lock older than the existing busy-marker stale threshold is removed with a WARN and a chatops alert, so a crash leftover costs bounded time instead of excluding the issue for the process's remaining life(times).
- The `status` verb's reply (and the `repo_status` control-socket action feeding it) gains an issues-lane section: ready units, locked units with lock age, parked units with marked-at and last reason — mirroring the changes lane's queue/marker sections, so operators can diagnose from chat without server access.
- `clear-perma-stuck` covers both lanes: the wildcard sweep enumerates issue-lane park markers (both unit forms) alongside change markers, and the exact-target form falls back to issue units when no change matches — observed in production: a parked issue's own alert said "remove `.perma-stuck.json`" while `clear-perma-stuck <repo> *` replied "nothing to clear" because the sweep only enumerated `openspec/changes/`. `clear-revision` stays changes-only (issues carry no spec delta).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: new requirement — issues-lane exclusions are logged per pass and stale locks are recovered; the "Marker-clear operator commands accept wildcard targets" requirement is modified so `clear-perma-stuck` covers both lanes.
- `chatops-manager`: new requirement — the status reply surfaces issues-lane state.

## Impact

- `autocoder/src/lanes/issues.rs`: `list_ready` returns (or logs) exclusion reasons instead of bare `continue`; stale-lock recovery against `executor.busy_marker_stale_threshold_secs`.
- `autocoder/src/control_socket/handlers.rs` (`build_repo_status`, `sweep_marker_clear`, `handle_clear_perma_stuck`) and `autocoder/src/chatops/operator_commands.rs` (`RepoStatusResponse` + `format_status_reply`): issues-lane fields, rendering, and marker-clear coverage.
- No new configuration; the stale threshold reuses the existing busy-marker value.
