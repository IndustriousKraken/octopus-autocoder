# Post-executor `git status --porcelain` spawn fails with a bare ENOENT

## Symptom

A repo's `status` reply showed, under `last iteration`:

```
outcome: last failure: post-executor error: spawning git status --porcelain: No such file or directory
```

The message is the post-executor error wrapper (`polling_loop/queue_walk.rs:329`,
`post-executor error: {e:#}`) carrying a `run_git` spawn failure (`git.rs`
`run_git`, context `spawning \`git status --porcelain\``). The underlying io error
is `ENOENT` ("No such file or directory") raised by `Command::output()` itself —
i.e. the SPAWN failed, not git returning a non-zero status.

## Why this is confusing

A bare `ENOENT` on spawning a subprocess has two distinct causes and the current
message does not distinguish them:

1. **The `git` binary is not on the daemon's `PATH`.**
2. **The `current_dir(workspace)` passed to `Command` no longer exists** — the
   workspace directory was removed/recreated between the executor run and the
   post-executor `git status --porcelain` call (e.g. workspace-cache eviction,
   a rebuild, or fork recreation), so the cwd is gone.

In this deployment other git operations in the same status (branch/commit/PR
lookups) succeeded, so `git` is on `PATH` — pointing at cause (2): a missing
workspace cwd at post-executor time. But the operator cannot tell that from the
message, and the condition surfaces as a terminal "last failure" rather than a
recoverable transient.

## Tasks

- [ ] Make the spawn error actionable: when `run_git` (or its post-executor
  caller) hits an `ENOENT` on spawn, distinguish "`git` not found on PATH" from
  "workspace directory missing" — check whether the `current_dir` exists and
  whether `git` resolves — and report which, instead of a bare
  `No such file or directory`.
- [ ] Treat a missing workspace cwd at post-executor time as a TRANSIENT,
  recoverable condition (classify it like the workspace-init/dirty recovery path
  so the workspace is re-initialized on the next iteration) rather than a terminal
  post-executor failure that lingers as `last failure` in status.
- [ ] Confirm the daemon validates `git` availability at startup (dependency
  preflight, `dependency_preflight.rs`); if it does not, add it, so a genuinely
  missing `git` binary fails fast and loudly at startup instead of mid-pass.

## Tests

- [ ] `run_git` / `status_entries` invoked with a non-existent `current_dir`
  returns an error that names the missing-workspace cause (not a bare ENOENT).
- [ ] The post-executor path, given a vanished workspace, classifies the failure
  as transient/recoverable (re-init next iteration), asserted via the recovery
  classification rather than message text.
