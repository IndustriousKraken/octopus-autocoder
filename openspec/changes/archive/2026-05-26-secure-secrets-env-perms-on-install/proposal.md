## Why

`autocoder install` writes `secrets.env` (containing the GitHub PAT,
ChatOps bot token, and reviewer API key) to disk with the system's
default umask permissions, then later chmods it to `0o600`. The chmod
runs by spawning a `chmod` subprocess after the file is already on
disk with its contents.

Concretely in `autocoder/src/cli/install.rs:1188-1194`:

```rust
fs::write(&secrets_path, secrets.as_bytes())   // file created with default umask (typically 0o644)
    .await
    .with_context(|| format!("write {}", secrets_path.display()))?;

let config_mode = if mode == InstallMode::Server { 0o640 } else { 0o600 };
actions.chmod(&config_path, config_mode).await?;
actions.chmod(&secrets_path, 0o600).await?;     // chmod runs only after fs::write completes
```

`tokio::fs::write` (which delegates to `std::fs::write`) opens the file
with `O_CREAT | O_WRONLY | O_TRUNC` and no explicit mode, so the
kernel uses `0o666 & ~umask`. With the typical root umask of `022`,
`/etc/autocoder/secrets.env` is created with mode `0o644`
(world-readable). The `chmod` subprocess at `install.rs:1194` is then
spawned via `tokio::process::Command::new("chmod")` (see
`install.rs:335-346`), which takes additional time to fork+exec
before the permissions get tightened.

During this window — bounded by the chmod-subprocess fork+exec
latency — any unprivileged local user on the host can `cat
/etc/autocoder/secrets.env` and read every secret the wizard just
collected:

- the GitHub PAT (full repo-write scope)
- the Slack / Discord / Teams / Mattermost / Matrix bot token
- the reviewer LLM API key

The server-mode install runs as root and the file lives in
`/etc/autocoder/`, whose parent directory is also created with
default umask (`0o755`, world-traversable), so any local user can
race the chmod. Harm: full disclosure of repository-write credentials
to local users on the install host.

## What Changes

Create `secrets.env` with `0o600` from the start, so the file is
never observable to other users. Two acceptable shapes:

- Use `std::fs::OpenOptions::new().mode(0o600).create_new(true).write(true).open(...)`
  followed by `write_all` — opens with the restrictive mode in one
  syscall.
- Or pre-create the file via `OpenOptions` with mode `0o600`, then
  call the existing `fs::write` path.

The same race exists for the `config.yaml` file at
`install.rs:1183-1185` when `mode == InstallMode::Dev` (target mode
`0o600`); fix it the same way. The server-mode `config.yaml` target
is `0o640` (group-readable), so its disclosure window is narrower
but the fix should still create it with `0o640` from the start
rather than relying on a post-write chmod.

## Impact

- `autocoder/src/cli/install.rs` (the `execute_inner` function around
  lines 1183-1194).
- Tests under `autocoder/src/cli/install.rs` that exercise the
  install flow (the `RecordingActions` test double records `Chmod`
  calls — those assertions may need to be updated to reflect that
  the file is born with the right mode and the post-write `chmod` is
  no longer required).
