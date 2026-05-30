## 1. Release workflow: pin Linux gnu GLIBC floor

- [x] 1.1 In `.github/workflows/release.yml`, for the matrix entries targeting `x86_64-unknown-linux-gnu` AND `aarch64-unknown-linux-gnu`:
  - Add a step before the build step that installs `cargo-zigbuild` AND zig: `pip install cargo-zigbuild` (or the appropriate apt/snap/curl-based install) AND any required zig setup. The recommended approach is to use the official `goto-bus-stop/setup-zig` action (or the equivalent) to install zig, then `cargo install cargo-zigbuild` via the existing Rust toolchain.
  - Replace `cargo build --release --target <triple>` with `cargo zigbuild --release --target <triple>.2.17`. The `.2.17` suffix is the zigbuild GLIBC-floor pin.
- [x] 1.2 Verify the resulting binaries' GLIBC floor: a post-build step runs `objdump -T <binary> | grep -oE 'GLIBC_[0-9.]+' | sort -V | tail -n1` AND asserts the maximum-required GLIBC version is `<= 2.17`. If a higher version slips in (a dependency that uses a newer-glibc-only symbol), the workflow fails AND the implementer investigates which dependency leaked.
- [x] 1.3 The macOS target (`aarch64-apple-darwin`) build step is UNCHANGED. zigbuild is Linux-only in this workflow.
- [x] 1.4 Test gate (existing `cargo test --release`) is UNCHANGED. Test execution does not require the cross-build toolchain.
- [x] 1.5 Asset naming (`autocoder-<tag>-<triple>` AND `.sha256`) is UNCHANGED. The binaries are still emitted to the same paths; `update.sh`'s download URLs continue to resolve.
- [ ] 1.6 Manual verification on a downstream Ubuntu 22.04 host: download the newly-built `autocoder-<tag>-x86_64-unknown-linux-gnu` binary, run `./autocoder --version`. Expected: prints the version cleanly. Pre-spec behavior: prints `GLIBC_2.39 not found`.

## 2. `update.sh`: smoke-test the new binary

- [x] 2.1 In `update.sh`'s `run_preflight()` function, add a new prelude block BEFORE the existing `check-config` invocation:
  ```bash
  # Smoke test: can the binary even execute? Catches GLIBC mismatch,
  # missing .so dependency, arch mismatch, corrupted download — any
  # case where the binary can't load at all. The dynamic linker exits
  # non-zero on load failure, which the check-config exit-code mapping
  # would otherwise treat as "warnings; proceeding."
  local smoke_err
  if ! smoke_err="$("$new_binary" --version 2>&1 >/dev/null)"; then
    echo "update.sh: new binary failed smoke test:" >&2
    echo "$smoke_err" >&2
    echo "update.sh: not swapping; daemon continues on $(current_version)." >&2
    exit 1
  fi
  ```
- [x] 2.2 The capture pattern (`smoke_err="$(... 2>&1 >/dev/null)"`) redirects stdout to /dev/null AND captures stderr to the variable, so loader errors land in `$smoke_err` for the operator. If the binary somehow prints to stdout instead (unusual), the redirection still preserves stderr capture.
- [x] 2.3 The existing `check-config` invocation AND its exit-code mapping (0 / 1 / 2 / *) remain UNCHANGED. The smoke test runs FIRST; if it passes, the existing behavior follows.
- [x] 2.4 Tests:
  - A mocked `$new_binary` that exits 1 on `--version` AND prints `GLIBC_2.39 not found` to stderr: the script's stderr output contains that text, the swap is not attempted, exit code is 1.
  - A mocked `$new_binary` that exits 0 on `--version` (smoke OK) AND exits 1 on `check-config` (config warnings): the existing "warnings; proceeding" path fires AND the swap proceeds.
  - A mocked `$new_binary` that exits 0 on `--version` AND exits 0 on `check-config` (happy path): both checks pass; swap proceeds.

## 3. `update.sh`: config-resolution error messages

- [x] 3.1 Replace the existing single check:
  ```bash
  CONFIG_PATH="$(resolve_config_path || true)"
  if [[ -z "$CONFIG_PATH" || ! -f "$CONFIG_PATH" ]]; then
    echo "update.sh: cannot find config; pass --config-dir <path> if your install is non-standard" >&2
    exit 1
  fi
  ```
  with three branches in order of specificity:
  ```bash
  CONFIG_PATH="$(resolve_config_path || true)"
  if [[ -z "$CONFIG_PATH" ]]; then
    echo "update.sh: could not resolve config path; pass --config-dir <path> if your install is non-standard" >&2
    exit 1
  fi
  if [[ -e "$CONFIG_PATH" && ! -r "$CONFIG_PATH" ]]; then
    echo "update.sh: config at ${CONFIG_PATH} is not readable by $(id -un); try running with sudo" >&2
    exit 1
  fi
  if [[ ! -f "$CONFIG_PATH" ]]; then
    echo "update.sh: no config file at ${CONFIG_PATH}; check --config-dir or the systemd unit's --config argument" >&2
    exit 1
  fi
  ```
- [x] 3.2 The order matters: (1) empty path means resolution failed; (2) path exists but unreadable is the perm-denied case; (3) path resolves but doesn't exist is the configuration mismatch case.
- [x] 3.3 The `id -un` call resolves to the calling user's name at runtime (e.g., `gilgamesh`, `autocoder`, `root`), so the error message names the specific user who lacks permission.
- [x] 3.4 Tests:
  - Branch 1 (empty path): `resolve_config_path` returns empty → script prints the resolution-failed message.
  - Branch 2 (unreadable): a fixture file with mode `0600` owned by a different user → script prints the permission message naming `$(id -un)`.
  - Branch 3 (missing): `--config-dir /nonexistent` → script prints the file-missing message naming the path.

## 4. `update.sh` line-count compliance

- [x] 4.1 The canonical "Bounded size and complexity" scenario requires `update.sh` to be `≤ 150 lines including comments`. Estimate: the smoke-test addition is ~10 lines; the config-resolution rewrite is ~8 net new lines. Pre-change line count: 140 (verified). Post-change estimate: ~158, over the cap.
- [x] 4.2 Resolution path: trim redundant lines in the existing script to fit. Candidates: collapse the trailing block-comment headers, inline single-use variables, AND/OR remove the explanatory comments inside `run_preflight` AND keep only the smoke-test comment (which carries the diagnostic load).
- [x] 4.3 If trimming the existing script proves impossible to reach ≤ 150 lines WITHOUT removing diagnostic value, the implementer opens a follow-on change `aNN-relax-update-sh-line-cap` modifying the canonical scenario from `≤ 150` to `≤ 175` (or whatever the post-fit line count is). Do NOT include the line-cap relaxation in `a30`; keep this change focused on the operator-facing fixes.
- [x] 4.4 Verification: `wc -l update.sh` reports `≤ 150` lines.

## 5. Spec deltas

- [x] 5.1 `openspec/changes/a30-release-glibc-floor-and-update-sh-robustness/specs/project-documentation/spec.md` ADDs three requirements covering the release-glibc-floor, smoke-test, AND config-error-messages behaviors.

## 6. Verification

- [x] 6.1 `cargo test` passes (any new unit tests for update.sh logic — if implemented as bats / bash integration tests — plus existing tests).
- [x] 6.2 `openspec validate a30-release-glibc-floor-and-update-sh-robustness --strict` passes.
- [x] 6.3 `wc -l update.sh` reports `≤ 150`.
- [ ] 6.4 Manual end-to-end verification:
  - After the post-spec release workflow has run AND produced a tagged release, download `autocoder-<tag>-x86_64-unknown-linux-gnu` on an Ubuntu 22.04 host. Confirm `./autocoder --version` runs cleanly.
  - On the same host, run `./update.sh --version <tag> --config-dir /etc/autocoder` WITHOUT sudo. Expected: one of the three specific error messages, NOT the generic "cannot find config" text.
  - On the same host, run `sudo ./update.sh --version <tag>`. Expected: smoke test passes, check-config preflight passes, swap completes, daemon reaches `active`.
  - Simulate a load-time failure: edit the downloaded binary to corrupt a few bytes near the ELF header, then run `sudo ./update.sh --version <tag>`. Expected: smoke test catches it, swap is not attempted, exit 1 with the captured loader error in stderr.
