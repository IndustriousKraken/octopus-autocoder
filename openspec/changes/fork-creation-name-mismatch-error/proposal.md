## Why

When an upstream repository is renamed, GitHub does not rename an existing fork. Startup fork setup then fails in the worst possible way: the fork-creation POST returns 2xx (GitHub idempotently returns the *existing* fork, still under its old name), the daemon ignores the response body, polls the derived fork URL for 60 seconds, and finally emits a vague "fork creation succeeded but the fork was not reachable within 60s" alert. The operator gets a misleading timeout message for a deterministic, permanently-failing condition, and the alert's remedy hint says bare `reload`, which is not an actual command.

## What Changes

- After a 2xx fork-creation response, the daemon reads the returned fork's identity (`full_name` / clone URL) from the response body and compares it against the derived expected fork URL.
- On mismatch (the existing-fork idempotent case where the fork's name differs — e.g. upstream renamed after forking), the repository fails fork setup **immediately** with a precise cause naming the actual fork and the expected name (so the operator knows to rename the fork), skipping the 60-second reachability poll that can never succeed.
- On match, behavior is unchanged: poll reachability up to 60s as today.
- The fork-setup failure alert's remedy hint names the real command: restart the daemon or run `autocoder reload` on the daemon host (instead of bare `reload`).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "Startup verification of fork existence" requirement gains fork-identity verification of the creation response (fail fast with a rename-remedy cause on name mismatch) and a concrete `autocoder reload` remedy hint in the fork-setup failure alert.

## Impact

- `autocoder/src/forge/github.rs`: `create_fork` / `create_fork_at` parse the 2xx response body and return the created/existing fork's identity instead of discarding it.
- `autocoder/src/cli/run.rs`: `ensure_forks_exist_with` (and the `ForkOps` trait it drives) compares the returned fork identity against the derived fork URL before entering the reachability poll; `fork_setup_failure_alert_message` remedy text updated.
- No config, API, or dependency changes. Fork-PR mode operators see earlier, more precise failure alerts; matching-fork behavior is byte-for-byte unchanged.
