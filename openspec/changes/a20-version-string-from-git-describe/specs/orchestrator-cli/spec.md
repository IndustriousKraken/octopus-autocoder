## MODIFIED Requirements

### Requirement: Daemon emits a startup version notification on every successful boot
After `autocoder run`'s startup pipeline completes (configs validated, chatops backend constructed, repositories enumerated) AND before the first polling iteration begins, the daemon SHALL post a one-line notification to chatops naming the binary version AND the count of configured repositories. The version string SHALL come from `env!("AUTOCODER_VERSION")` — populated at build time by `build.rs` running `git describe --tags --always --dirty` — NOT from `env!("CARGO_PKG_VERSION")`. The notification SHALL fire on every successful startup — not only after an `update.sh`-driven restart — because every restart is a meaningful operator signal. The notification SHALL be suppressed when no chatops backend is configured AND SHALL NOT be gated by any flag under `chatops.notifications.*` (those flags govern per-change and per-event signals; the startup line is a daemon-lifecycle signal).

#### Scenario: Startup notification fires once per boot with version and repo count
- **WHEN** the daemon starts up against a config with `chatops.provider: slack` AND 3 configured repositories
- **THEN** exactly one `post_notification` call fires to the resolved default channel
- **AND** the message contains the literal `🆙` prefix
- **AND** the message contains `autocoder v<X>` where `<X>` matches `env!("AUTOCODER_VERSION")` verbatim
- **AND** the version string is the `git describe --tags --always --dirty` output (e.g. `v1.1.1` at a clean tag, `v1.1.1-23-g4abc123` at a development commit past the tag, OR the Cargo.toml fallback when `.git/` is absent)
- **AND** the message contains `3 repository(ies) configured`
- **AND** the notification fires before any polling iteration begins

#### Scenario: No chatops backend suppresses the notification
- **WHEN** the daemon starts up against a config with no `chatops:` block
- **THEN** no `post_notification` call fires
- **AND** the daemon emits an INFO log line `startup version: v<X>; <N> repositories` to journalctl as the fallback signal (using the same `env!("AUTOCODER_VERSION")` source)
- **AND** the daemon proceeds to the polling loop without error

#### Scenario: Notification is not gated by `notifications.*` flags
- **WHEN** the daemon starts up against a config with `chatops.notifications.start_work: false` AND `chatops.notifications.failure_alerts: false` AND `chatops.notifications.pr_opened: false`
- **THEN** the startup version notification STILL fires (those flags do not apply to lifecycle signals)
- **AND** an operator who silenced per-change signals still sees the once-per-boot version line

#### Scenario: Notification failure is non-fatal
- **WHEN** the chatops backend's `post_notification` call errors (network blip, channel renamed, scope revoked)
- **THEN** the daemon logs a WARN naming the error AND proceeds to the polling loop
- **AND** no startup is blocked by a notification failure

## ADDED Requirements

### Requirement: Binary version string is derived from `git describe` at build time
The autocoder binary SHALL embed a version string at build time via a `build.rs` script that runs `git describe --tags --always --dirty` AND exposes the output as `env!("AUTOCODER_VERSION")`. The build script SHALL fall back to `env!("CARGO_PKG_VERSION")` (Cargo.toml's `version =` field) when `git describe` cannot run OR returns empty — typical of tarball builds without `.git/` AND of `cargo install` from crates.io. The fallback chain SHALL ALWAYS produce a non-empty string; the build SHALL NEVER fail because of version-string resolution. The `build.rs` SHALL register `.git/HEAD`, `.git/index`, AND `.git/refs/tags` as rerun-if-changed inputs so dev builds reflect the working commit.

Every operator-facing version-string surface in the autocoder binary SHALL read `env!("AUTOCODER_VERSION")`, NOT `env!("CARGO_PKG_VERSION")`. Surfaces include: the `🆙` startup notification (per the modified requirement above), `autocoder --version` (clap's `#[command(version = ...)]` override), AND any future log lines OR PR-body footers that surface version.

The Cargo.toml `version =` field SHALL be operator-bumped only at semver-meaningful releases (major / minor / patch). Per-commit AND per-tag version bumps are NOT required — `git describe` provides the delta-past-tag info automatically.

#### Scenario: Build at a clean tag commit produces the tag string verbatim
- **WHEN** the daemon is built from a commit that has a `vX.Y.Z` tag pointing directly at it AND the working tree is clean
- **THEN** `git describe --tags --always --dirty` returns `vX.Y.Z` (no suffix)
- **AND** `env!("AUTOCODER_VERSION")` resolves to `vX.Y.Z`
- **AND** `autocoder --version` outputs `vX.Y.Z`

#### Scenario: Build past a tag produces the tag + commit-count + SHA
- **WHEN** the daemon is built from a commit that is N commits past the most recent tag `vX.Y.Z` AND the working tree is clean
- **THEN** `git describe` returns `vX.Y.Z-N-g<short-sha>` (e.g. `v1.1.1-23-g4abc123`)
- **AND** `env!("AUTOCODER_VERSION")` resolves to that string
- **AND** the `🆙` startup notification AND `autocoder --version` both show the development-build format

#### Scenario: Build with uncommitted local changes adds `-dirty` suffix
- **WHEN** the daemon is built from a commit AND the working tree has uncommitted modifications to tracked files
- **THEN** `git describe --tags --always --dirty` appends `-dirty` to the output
- **AND** the operator-visible version string AND `🆙` notification surface the `-dirty` suffix
- **AND** operators see clearly that the running binary was built from an in-progress local state

#### Scenario: Build with no `.git/` falls back to Cargo.toml
- **WHEN** the daemon is built from a source location with NO `.git/` directory (e.g., `cargo install autocoder` from crates.io, OR an unpacked source tarball)
- **THEN** `git describe` fails OR returns empty
- **AND** `env!("AUTOCODER_VERSION")` resolves to `env!("CARGO_PKG_VERSION")` (Cargo.toml's version)
- **AND** the build still succeeds
- **AND** the operator-visible version string is the Cargo.toml version verbatim

#### Scenario: Build with no `git` binary on PATH falls back to Cargo.toml
- **WHEN** the daemon is built on a host where the `git` binary is not on PATH
- **THEN** the build script's `Command::new("git")` fails to spawn
- **AND** the fallback to `env!("CARGO_PKG_VERSION")` kicks in
- **AND** the build still succeeds

#### Scenario: build.rs rerun inputs catch dev-build commit changes
- **WHEN** a developer makes a commit AND runs `cargo build` without modifying any source file
- **THEN** cargo re-runs `build.rs` because `.git/HEAD` (or `.git/index`) changed
- **AND** `env!("AUTOCODER_VERSION")` reflects the new commit's `git describe` output

#### Scenario: Clap `--version` override produces the embedded string
- **WHEN** an operator runs `autocoder --version`
- **THEN** the output is the `env!("AUTOCODER_VERSION")` value verbatim (with clap's standard `<binary-name> <version>` formatting)
- **AND** the output is NOT the Cargo.toml `version =` field (unless the fallback path fired at build time)

#### Scenario: Binary-release builds embed clean tag strings
- **WHEN** the GitHub Actions release workflow builds the daemon from a commit that has a `vX.Y.Z` tag (the workflow runs against the tagged commit)
- **THEN** the embedded version string is `vX.Y.Z` (no `-N-gSHA` suffix; no `-dirty` suffix)
- **AND** operators installing via `update.sh` see clean semver versions in their `🆙` notifications AND `--version` output
