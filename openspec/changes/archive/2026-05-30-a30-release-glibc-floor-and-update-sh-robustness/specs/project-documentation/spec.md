## ADDED Requirements

### Requirement: Linux gnu release binaries pin a GLIBC floor of 2.17 (or equivalent broad compatibility)
The release workflow's `x86_64-unknown-linux-gnu` AND `aarch64-unknown-linux-gnu` build steps SHALL produce binaries that load AND execute on Linux hosts with GLIBC `2.17` or newer. The recommended mechanism is `cargo-zigbuild` with target-triple suffix `.2.17` (e.g. `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17`); the spec mandates the OUTCOME (binaries SHALL be loadable on every mainstream Linux distro currently in vendor support — RHEL 7+, Ubuntu 16.04+, Debian 9+, AND all newer releases) rather than the specific tooling. Alternative mechanisms (pinning `runs-on` to an older Ubuntu image, switching to the `musl` target, OR a `manylinux2014`-style containerized build) are acceptable provided the loadability guarantee is preserved.

The build host's own GLIBC version SHALL be irrelevant to the resulting binary's compatibility floor — operators who upgrade their build infrastructure SHALL NOT accidentally narrow the runtime compatibility surface. A post-build verification step SHALL inspect each Linux gnu binary's required-symbols list AND fail the workflow if the maximum-required GLIBC version exceeds `2.17` (or whatever floor the implementing change pins).

The `aarch64-apple-darwin` target is unaffected by this requirement — Apple's libsystem versioning is handled separately via the macOS deployment-target setting.

#### Scenario: Linux gnu binary loads on a mainstream older distro
- **WHEN** a release workflow run completes for tag `vX.Y.Z`
- **AND** an operator downloads `autocoder-vX.Y.Z-x86_64-unknown-linux-gnu` to a host running Ubuntu 22.04 (GLIBC 2.35), Debian 12 (GLIBC 2.36), OR RHEL 9 (GLIBC 2.34)
- **THEN** `./autocoder --version` prints the version successfully
- **AND** no `GLIBC_<version> not found` dynamic-linker error is emitted

#### Scenario: Build-host GLIBC upgrade does not narrow the compatibility floor
- **WHEN** GitHub's `ubuntu-latest` runner image moves from one Ubuntu release to a newer one (e.g. 24.04 → 26.04)
- **AND** the same release workflow runs on the newer image
- **THEN** the resulting Linux gnu binaries continue to load on hosts with GLIBC `2.17`+
- **AND** no spec change is required to maintain the compatibility floor

#### Scenario: Post-build verification catches glibc-floor regressions
- **WHEN** a build step somehow produces a binary requiring GLIBC `> 2.17` (e.g. a dependency added a newer-glibc-only symbol)
- **THEN** the workflow's post-build verification step (inspecting required symbols via `objdump -T` OR equivalent) detects the regression
- **AND** the workflow fails before publishing the release
- **AND** the failure message names the offending GLIBC version AND points the maintainer at the relevant dependency for investigation

#### Scenario: macOS target unaffected
- **WHEN** the release workflow runs for tag `vX.Y.Z`
- **THEN** the `aarch64-apple-darwin` build step uses its existing toolchain (no zigbuild involvement)
- **AND** the macOS binary's deployment-target setting (already pinned in the workflow) governs its compatibility floor

### Requirement: `update.sh` smoke-tests the new binary's loadability via `--version` before invoking `check-config`
`update.sh`'s `run_preflight()` function SHALL execute the new binary's `--version` subcommand as a smoke test BEFORE invoking `check-config`. The smoke test captures stderr (loader errors print to stderr regardless of the binary's own behavior) AND checks the exit code. On any non-zero exit, the script SHALL print:

```
update.sh: new binary failed smoke test:
<captured stderr from the loader>
update.sh: not swapping; daemon continues on <current-version>.
```

AND exit 1. The swap step SHALL NOT execute.

Only after the smoke test passes does the script invoke `check-config`. The existing `check-config` exit-code mapping (0 = OK, 1 = warnings; proceeding, 2 = preflight failed; not swapping, else = unexpected; aborting) is UNCHANGED for the case where the binary IS loadable.

The smoke test catches load-time failures — GLIBC version mismatch, missing `.so` dependency, architecture mismatch, corrupted download — that the dynamic linker rejects before `check-config`'s code can run. The dynamic linker exits non-zero on these failures, which would otherwise be misread as `check-config` exit code `1` AND mapped to `preflight returned warnings; proceeding.`

#### Scenario: GLIBC mismatch caught before swap
- **WHEN** the operator runs `update.sh --version <tag>` AND the downloaded binary requires a GLIBC version newer than the host's
- **THEN** `$new_binary --version` exits non-zero with stderr containing `version 'GLIBC_X.Y' not found`
- **AND** the script prints `update.sh: new binary failed smoke test:` followed by the captured stderr
- **AND** the script prints `update.sh: not swapping; daemon continues on <current-version>.`
- **AND** the script exits 1
- **AND** the binary at `/usr/local/bin/autocoder` is unchanged
- **AND** the daemon continues running on the previous binary

#### Scenario: Successful smoke test proceeds to check-config
- **WHEN** the operator runs `update.sh --version <tag>` AND the downloaded binary loads cleanly (`$new_binary --version` exits 0)
- **THEN** the script proceeds to invoke `check-config --config <path> --json`
- **AND** the existing exit-code mapping fires per its canonical scenarios (0 / 1 / 2 / *)

#### Scenario: Smoke test failure preserves the rollback artifact
- **WHEN** the smoke test fails
- **THEN** the script exits BEFORE the `swap_binary` step
- **AND** no `/usr/local/bin/autocoder.previous` artifact is created OR overwritten this run
- **AND** any prior `.previous` artifact (from an earlier successful upgrade) is untouched

#### Scenario: Corrupted binary caught before swap
- **WHEN** the downloaded binary's bytes are corrupted (e.g. transient network corruption between the checksum verify AND the smoke test — unlikely but possible)
- **AND** the binary fails to load with `cannot execute binary file` OR similar loader error
- **THEN** the smoke test catches the failure per the GLIBC scenario above
- **AND** the script exits 1 without attempting the swap

### Requirement: `update.sh` distinguishes config-path-not-resolved from config-not-readable from config-not-present in its preflight error messages
Before invoking `run_preflight`, `update.sh` SHALL evaluate the resolved `CONFIG_PATH` against three failure modes in order of specificity AND print a distinct, operator-actionable message for each. The branches:

1. **`CONFIG_PATH` is empty.** The script could not resolve a config path: no `--config-dir` flag was passed AND `systemctl show autocoder.service -p ExecStart` produced no `--config` argument. Print:
   ```
   update.sh: could not resolve config path; pass --config-dir <path> if your install is non-standard
   ```
   AND exit 1.

2. **`CONFIG_PATH` is set, the path EXISTS, BUT is NOT readable by the calling user.** The common case: config owned by the autocoder service user with mode `0600`, AND the script is being run as a non-root user. Detect via `[[ -e "$CONFIG_PATH" && ! -r "$CONFIG_PATH" ]]`. Print:
   ```
   update.sh: config at <path> is not readable by <user>; try running with sudo
   ```
   AND exit 1. The `<user>` placeholder is resolved at runtime via `$(id -un)` so the message names the specific user lacking permission.

3. **`CONFIG_PATH` is set BUT the path does NOT exist** (`! -f "$CONFIG_PATH"`). Either the `--config-dir` flag points at the wrong directory OR the resolved systemd unit's `--config` argument points at a stale path. Print:
   ```
   update.sh: no config file at <path>; check --config-dir or the systemd unit's --config argument
   ```
   AND exit 1.

The order matters: empty-path first (resolution failure precedes any path-based checks); unreadable second (a permission denial against an existing file is more specific than its absence); missing third (the most general failure).

#### Scenario: Empty config path produces resolution-failed message
- **WHEN** the operator runs `update.sh` WITHOUT `--config-dir` AND the host's `systemctl show autocoder.service` returns no `ExecStart` line with a `--config` argument (e.g. on a host where autocoder is not installed as a systemd service)
- **THEN** the script prints `update.sh: could not resolve config path; pass --config-dir <path> if your install is non-standard`
- **AND** exits 1

#### Scenario: Unreadable config produces permission-denied message naming the user
- **WHEN** the operator runs `update.sh --config-dir /etc/autocoder` as user `gilgamesh` (no sudo)
- **AND** `/etc/autocoder/config.yaml` exists AND is owned by the `autocoder` user with mode `0600`
- **THEN** the script prints `update.sh: config at /etc/autocoder/config.yaml is not readable by gilgamesh; try running with sudo`
- **AND** exits 1
- **AND** the message names `gilgamesh` (the calling user), NOT a generic placeholder

#### Scenario: Missing config produces path-mismatch message
- **WHEN** the operator runs `update.sh --config-dir /home/autocoder/wrong-path` AND `/home/autocoder/wrong-path/config.yaml` does NOT exist
- **THEN** the script prints `update.sh: no config file at /home/autocoder/wrong-path/config.yaml; check --config-dir or the systemd unit's --config argument`
- **AND** exits 1

#### Scenario: Each branch exits before run_preflight
- **WHEN** ANY of the three branches fires
- **THEN** the script exits BEFORE the `run_preflight` invocation
- **AND** no `check-config` call OR smoke test is attempted (those depend on a usable config path)
- **AND** the binary at `/usr/local/bin/autocoder` is unchanged

#### Scenario: Generic "cannot find config" message is no longer emitted
- **WHEN** any config-resolution failure occurs in `update.sh`
- **THEN** the script's stderr does NOT contain the pre-spec generic text `cannot find config; pass --config-dir <path> if your install is non-standard`
- **AND** instead emits one of the three specific messages above
