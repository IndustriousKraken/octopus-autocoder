# Implementation tasks

## 1. Executor denylist policy (`sandbox.rs`)

- [ ] 1.1 For the executor, replace `--tmpfs <home>` with: keep `--ro-bind / /`, bind `$HOME` read-write, then mask each mask-list entry with a `--tmpfs` (or inaccessible bind) over it. systemd-run equivalent: `ProtectHome` not blanket-tmpfs — bind home rw and `InaccessiblePaths=`/`TemporaryFileSystem=` the mask-list entries.
- [ ] 1.2 Ship the default mask-list: credential paths (`~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.netrc`, cloud-token dirs, the other CLIs' stores, `~/.cargo/credentials.toml`, `~/.npmrc`, `~/.pypirc`, `~/.gem/credentials`) AND shell-init/persistence paths (`~/.bashrc`, `~/.profile`, `~/.ssh/authorized_keys`, autostart/cron). Apply masks even inside otherwise-exposed tool trees (deny-overrides-allow).

## 2. Read-only-role + strict-mode allowlist (folds a012)

- [ ] 2.1 Keep the home-masked allowlist for read-only roles: bind the read-only workspace + the role's own store, plus the **resolved CLI binary** (`which` + follow symlinks) AND its home-resident dependency closure, read-only/executable. (This is the folded `a012` binding — read-only roles still need the binary even under the mask.)
- [ ] 2.2 Add a strict-mode flag that runs the executor under the same allowlist (home masked) — opt-in, not default.

## 3. macOS provider (folds a73)

- [ ] 3.1 Add the `sandbox-exec` mechanism: generate a Seatbelt profile realizing the policy — `(allow file-read*/file-write*)` for `$HOME` minus `(deny … (subpath <mask entry>))` for the executor; the allowlist (`(deny default)` + allows) for read-only roles. Deny the macOS analogs of the capability drops (process-info, raw networking, privilege elevation). Run via `sandbox-exec -f <profile> -- <cli>`.
- [ ] 3.2 Generalize the mechanism gate to platform-appropriate: Linux systemd-run/bwrap, macOS sandbox-exec; fail closed when none is available unless the unsandboxed opt-in is set.

## 4. Config

- [ ] 4.1 Mask-list config: the default set + per-repo/global additions and removals; removing a default entry emits a startup relaxed-posture WARN. `os_hide` continues to govern the other-CLI-store subset.
- [ ] 4.2 Strict-mode config flag (executor); read-only roles always allowlist.

## 5. Tests

- [ ] 5.1 The executor reads `~/.cargo` / `~/.pyenv` and writes a tool cache, while `~/.ssh` and `~/.cargo/credentials.toml` are masked (read fails).
- [ ] 5.2 A masked persistence file write (`~/.bashrc`) does not persist to the real file.
- [ ] 5.3 A read-only role runs under the home-masked allowlist with its CLI binary bound (resolved from `~/.local/bin`, symlinks followed); a workspace write fails.
- [ ] 5.4 Removing a default mask entry exposes it AND emits a startup relaxed-posture WARN naming it.
- [ ] 5.5 Strict mode masks all of home for the executor.
- [ ] 5.6 The macOS gate is satisfied via `sandbox-exec`; the Seatbelt profile is derived from the policy (denylist for executor, allowlist for read-only roles).

## 6. Acceptance gate

- [ ] 6.1 `cargo test` passes for the autocoder crate.
- [ ] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 6.3 `openspec validate a013-sandbox-denylist-model --strict` passes.
