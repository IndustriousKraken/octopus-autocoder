## Why

The path resolver trusts `XDG_RUNTIME_DIR` verbatim. After an environment-preserving user switch (`su autocoder` without `-`), the variable still points at the *original* user's private runtime directory (`/run/user/<other-uid>`, mode 0700), so `autocoder reload` resolves the control socket inside a directory the current user cannot even traverse and fails with a baffling "Permission denied" — while the same command under `sudo -u autocoder` (clean environment) works. This is a classic `su` footgun that other daemons (systemd itself, tmux) guard against by refusing a runtime dir the current user does not own.

## What Changes

- During path resolution, when `XDG_RUNTIME_DIR` is set but the directory it names is not owned by the current effective uid (or cannot be inspected), the resolver ignores the variable — logging the reason — and falls back to the next resolution step (the `runtime/` subdir of the state default), exactly as if the variable were unset.
- An owned, accessible `XDG_RUNTIME_DIR` behaves exactly as today.
- The reload CLI's connection-failure hint gains the third likely cause: a stale `XDG_RUNTIME_DIR` inherited from an env-preserving user switch, with the suggestion to retry from a clean environment (`sudo -u autocoder autocoder reload`).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the path-precedence requirement ("Daemon resolves four standard data-category paths with a defined precedence") gains an ownership guard on the XDG-derived runtime default; the "`autocoder reload` subcommand" requirement's connection-failure hint gains the stale-`XDG_RUNTIME_DIR` cause.

## Impact

- `autocoder/src/paths.rs`: `xdg_runtime_default` / `runtime_default_from` gain the ownership check (one `stat` + uid comparison, Unix-only — this daemon is Linux-only already).
- `autocoder/src/cli/reload.rs`: connection-error hint text.
- No config, API, or dependency changes. Deployments where the daemon itself runs with a legitimately-owned `XDG_RUNTIME_DIR` are unaffected; only foreign-owned (leaked) values change behavior, and they could never have been correct.
