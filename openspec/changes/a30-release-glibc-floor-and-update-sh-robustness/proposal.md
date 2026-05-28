## Why

A real upgrade attempt against a downstream operator's host surfaced three production-relevant defects in the release pipeline AND `update.sh`:

1. **GLIBC version mismatch silently bricks upgrades.** The release workflow's Linux gnu builds run on `runs-on: ubuntu-latest`, which GitHub Actions resolves to Ubuntu 24.04 (Noble; GLIBC 2.39) as of early 2024. The Rust toolchain links against the build host's libc, so every released `x86_64-unknown-linux-gnu` binary since the runner switch requires GLIBC 2.39+ at runtime. Operators on older mainstream distros (Ubuntu 22.04 / Jammy with GLIBC 2.35, RHEL 9 with GLIBC 2.34, Debian 12 with GLIBC 2.36) see:
   ```
   /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found
   ```
   The binary cannot load AT ALL on the target host. This affects every operator with a mainstream non-bleeding-edge Linux deployment.

2. **`update.sh`'s preflight cannot distinguish "binary unloadable" from "config has warnings."** `run_preflight()` invokes `$new_binary check-config --config <path> --json` AND maps exit code 1 to `preflight returned warnings; proceeding.` But the dynamic linker ALSO exits 1 when it refuses to load a binary (GLIBC mismatch, missing `.so` dependency, arch mismatch, corrupted download) — `check-config`'s code never runs. The script confuses load-time failure with config warnings, swaps the unloadable binary into place, watches `systemctl restart autocoder` fail to reach `active`, AND falls through to the rollback path. The rollback succeeds (good), but the operator's daemon was down for the restart-window AND the swap attempt was wasted.

3. **The "cannot find config" message misleads operators with permission issues.** When `update.sh` is invoked without sudo AND the resolved `config.yaml` exists but is unreadable by the calling user (the common case: config owned by the autocoder service user with mode `0600`), the `! -f "$CONFIG_PATH"` check returns true. The script then prints `cannot find config; pass --config-dir <path>` — but the operator MAY have already passed `--config-dir`. The actual issue is permission-denied, not path-not-resolved. Operators waste time re-checking their flag invocation when they should be reaching for sudo.

All three are independent but bundle naturally as release-pipeline robustness fixes.

## What Changes

**Pin a GLIBC floor for Linux gnu release binaries via `cargo-zigbuild`.** The release workflow's Linux gnu build steps SHALL use `cargo-zigbuild` with a target triple that pins the GLIBC version: `x86_64-unknown-linux-gnu.2.17` AND `aarch64-unknown-linux-gnu.2.17`. The `.2.17` suffix is a `cargo-zigbuild` extension that tells the linker to emit binaries compatible with GLIBC 2.17 or newer (RHEL 7 / Ubuntu 14.04-era — covers every mainstream Linux distro currently in support). Build hosts continue to run `ubuntu-latest`; the zigbuild toolchain handles the glibc-floor pinning regardless of the build host's own libc.

Alternative mechanisms acceptable for the implementer if `cargo-zigbuild` proves operationally awkward: pinning the matrix entry to `runs-on: ubuntu-22.04` (GLIBC 2.35, covers most operators but not RHEL 9) OR switching the Linux gnu target to `x86_64-unknown-linux-musl` (statically-linked, glibc-independent, but tradeoffs around DNS-under-load AND malloc behavior). The spec mandates the outcome (binaries SHALL load on GLIBC 2.17+ hosts), not the mechanism.

The macOS target (`aarch64-apple-darwin`) is unaffected; Apple's libsystem versioning is handled separately by the deployment target setting (already pinned in the workflow). The aarch64 Linux target gains the same glibc-floor treatment as x86_64.

**`update.sh` runs a `--version` smoke test before invoking `check-config`.** A new step at the top of `run_preflight()` SHALL execute `$new_binary --version` AND capture both stdout AND stderr:

- On exit code 0: the binary loaded successfully; proceed to `check-config`.
- On any non-zero exit code: print `update.sh: new binary failed smoke test:` followed by the captured stderr (so the operator sees the actual loader error — `GLIBC_2.39 not found`, `cannot execute binary file`, etc.). Print `update.sh: not swapping; daemon continues on <current-version>.`. Exit 1.

This catches load-time failures BEFORE the script attempts the swap. The existing `check-config` exit-code mapping (0 = OK, 1 = warnings, 2 = hard errors, else = unexpected) remains UNCHANGED for the case where the binary IS loadable.

**`update.sh`'s config-resolution check distinguishes three failure modes.** The current single check `[[ -z "$CONFIG_PATH" || ! -f "$CONFIG_PATH" ]]` is replaced with three ordered checks:

1. `CONFIG_PATH` is empty (resolution failed: no `--config-dir` flag AND `systemctl show` produced no path) → print `update.sh: could not resolve config path; pass --config-dir <path> if your install is non-standard.`
2. `CONFIG_PATH` is set, the path exists, BUT is not readable by the calling user (`-e $path && ! -r $path`) → print `update.sh: config at <path> is not readable by $(id -un); try running with sudo.`
3. `CONFIG_PATH` is set BUT the path does not exist (`! -f $path`) → print `update.sh: no config file at <path>; check --config-dir or the systemd unit's --config argument.`

Each prints its specific message AND exits 1. The new error messages are operator-actionable — they name the specific failure mode AND the resolution.

## Impact

- **Affected specs:**
  - `project-documentation` — three ADDED requirements:
    - `Linux gnu release binaries pin a GLIBC floor of 2.17 (or equivalent broad compatibility)`.
    - `update.sh smoke-tests the new binary's loadability via --version before invoking check-config`.
    - `update.sh distinguishes config-path-not-resolved from config-not-readable from config-not-present in its preflight error messages`.
- **Affected code:**
  - `.github/workflows/release.yml` — Linux gnu build steps gain a `cargo-zigbuild` install step AND switch from `cargo build --release --target <triple>` to `cargo zigbuild --release --target <triple>.2.17`. Other workflow steps (test gate, asset upload, checksum generation, publish) unchanged.
  - `update.sh` — `run_preflight()` gains the smoke-test prelude. The config-resolution check before `run_preflight` is split into three branches with specific messages. The script's existing ≤150-line bound is at risk; the implementer trims any redundancy (e.g. consolidating the curl/SUMCHECK lines, removing inline-redundant comments) OR opens a follow-on change to relax the cap if trimming is impractical.
- **Operator-visible behavior:**
  - Operators on GLIBC 2.17+ hosts (every mainstream Linux distro currently in vendor support) can install AND upgrade autocoder via the binary release path. Operators on truly ancient distros (< RHEL 7 / Ubuntu 14.04 era) build from source — same as today.
  - `update.sh` failure modes produce specific, actionable error messages. Load-time failures (GLIBC mismatch, missing .so, etc.) are caught BEFORE the swap, so the daemon never goes down for a known-broken binary.
  - Config-path resolution errors name the specific issue (permission denied vs. file missing vs. resolution failure) AND its resolution.
- **Breaking:** no. Operators on GLIBC 2.39+ hosts see binaries continue to work (the binaries become MORE compatible, not less). Operators on older hosts gain access where they had none. The `update.sh` improvements are pure error-message quality + an additional pre-swap check; the success path is unchanged.
- **Acceptance:** `cargo test` passes; `openspec validate a30-release-glibc-floor-and-update-sh-robustness --strict` passes. Tests:
  - `update.sh` smoke-test path covered: a mocked `$new_binary` that exits non-zero on `--version` causes the script to exit 1 with the expected stderr content; the swap is not attempted.
  - `update.sh` config-resolution branches covered: each of the three branches (empty path, unreadable file, missing file) produces the documented message.
  - Manual verification: a downstream operator on Ubuntu 22.04 OR Debian 12 successfully runs `update.sh --version <new-tag>` after the workflow's first post-spec release, with the binary loading AND `systemctl is-active autocoder` reaching `active` within 30s.
