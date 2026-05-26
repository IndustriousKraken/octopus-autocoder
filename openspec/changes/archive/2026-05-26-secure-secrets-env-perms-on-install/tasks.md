## 1. Create secrets.env with restrictive permissions from the start

- [x] 1.1 In `autocoder/src/cli/install.rs::execute_inner`, replace
  the `tokio::fs::write(&secrets_path, ...)` at line ~1188 with code
  that opens the file using
  `std::fs::OpenOptions::new().mode(0o600).create_new(true).write(true).open(&secrets_path)`
  (via `std::os::unix::fs::OpenOptionsExt`), then writes the secrets
  bytes. The file MUST never exist on disk with a mode wider than
  `0o600`. If the file already exists (re-install scenario), remove
  it first or use `truncate(true)` AFTER chmod'ing the existing file
  to `0o600` so the truncated-but-not-yet-rewritten state is also
  not world-readable.
- [x] 1.2 In the same function, do the same for `config_path`:
  open with `mode(0o600)` for dev mode and `mode(0o640)` for server
  mode, so the config file is also never created world-readable.
  The subsequent `actions.chmod(&config_path, config_mode).await?`
  call may be left in place as a defensive no-op or removed once
  the post-write mode matches the create mode.
- [x] 1.3 After both files are written, the existing `chown` calls
  (only run in server mode at lines ~1196-1197) MUST still execute;
  do not remove them. Only the `chmod` race is being fixed, not the
  ownership step.

## 2. Update install tests to assert the secure-from-birth invariant

- [x] 2.1 Add a test
  `secrets_env_is_created_with_0600_before_any_chmod` under
  `autocoder/src/cli/install.rs`'s test module that runs the install
  flow against a `tempfile::TempDir` (no `RecordingActions` mock for
  this assertion — use `RealSystemActions` so the actual file is
  created on disk), then asserts that immediately after `execute_inner`
  returns, `std::fs::metadata(&secrets_path)?.permissions().mode() &
  0o777 == 0o600`. If the test infrastructure can't easily call
  `execute_inner` against the real filesystem (it currently uses
  mocks), assert instead that the new file-creation helper itself
  produces a 0600 file.
- [x] 2.2 Add a regression test asserting that — when the install
  re-runs against an existing world-readable `secrets.env` (simulating
  a pre-fix install that the operator is now upgrading) — the
  upgraded install path still ends with the file at `0o600`. Even
  if the fix above only affects fresh creates, the operator-upgrade
  path should also be covered so a re-run after a botched install
  cleans up the permission leak.

## 3. Documentation

- [x] 3.1 If `docs/SECURITY.md` mentions credential handling, add a
  one-line note under the `secrets.env` section: "the wizard creates
  this file with mode 0600 atomically; no world-readable window
  exists during install."
