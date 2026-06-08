#!/usr/bin/env bash
# Integration tests for update.sh. Builds a sandboxed PATH that mocks
# curl / sha256sum / autocoder / systemctl, then runs the real update.sh
# against synthesised inputs and asserts on stderr + exit code.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPDATE_SH="${REPO_ROOT}/update.sh"

FAIL=0
TESTS=0

run_case() {
  local name="$1"; shift
  TESTS=$((TESTS + 1))
  if "$@"; then
    echo "ok - $name"
  else
    echo "not ok - $name"
    FAIL=$((FAIL + 1))
  fi
}

# Build a sandboxed PATH for one scenario.
#   $1 = scratch dir (pre-created)
#   env: NEW_BIN_VERSION_RC, NEW_BIN_VERSION_STDERR,
#        NEW_BIN_CHECKCONFIG_RC,
#        MOCK_EXECSTART (string returned by `systemctl show -p ExecStart`)
make_sandbox() {
  local dir="$1"
  mkdir -p "${dir}/bin"

  # Mock curl: GitHub `/releases/latest` returns a tag; `.sha256` URLs
  # write a sha256-style line referencing the basename; binary URLs copy
  # the fake "new binary" into the output path.
  cat > "${dir}/bin/curl" <<'CURL_EOF'
#!/usr/bin/env bash
out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) out="$2"; shift 2;;
    -fsSL|-fsSLI|-fLsS|-sSL|-fsS|-f|-s|-S|-L) shift;;
    *) url="$1"; shift;;
  esac
done
if [[ "${url:-}" == */releases/latest ]]; then
  printf '%s\n' '"tag_name": "vTEST"'
  exit 0
fi
if [[ "${url:-}" == *.sha256 ]]; then
  # Write a sha256sums-style line; the sha256sum mock ignores the digest.
  basename_part="$(basename "${url%.sha256}")"
  if [[ -n "$out" ]]; then
    printf 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  %s\n' "$basename_part" > "$out"
  fi
  exit 0
fi
# Binary download: copy the configured fake binary into place.
if [[ -n "$out" && -n "${FAKE_NEW_BINARY:-}" ]]; then
  cp "$FAKE_NEW_BINARY" "$out"
fi
CURL_EOF
  chmod +x "${dir}/bin/curl"

  # Mock sha256sum: `-c` always succeeds. Other invocations are unused.
  cat > "${dir}/bin/sha256sum" <<'SUM_EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "-c" ]]; then exit 0; fi
echo "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  $1"
SUM_EOF
  chmod +x "${dir}/bin/sha256sum"

  # Mock the on-host `autocoder` invoked by current_version().
  cat > "${dir}/bin/autocoder" <<'AC_EOF'
#!/usr/bin/env bash
echo "autocoder ${MOCK_CURRENT_VERSION:-vCURRENT}"
AC_EOF
  chmod +x "${dir}/bin/autocoder"

  # Mock systemctl: `show -p ExecStart` returns MOCK_EXECSTART; restart and
  # is-active default to success but are unreachable in these tests
  # because we exit during preflight.
  cat > "${dir}/bin/systemctl" <<'SC_EOF'
#!/usr/bin/env bash
case " $* " in
  *" show "*"-p ExecStart"*)
    printf '%s\n' "${MOCK_EXECSTART:-}"
    exit 0;;
  *" is-active "*) exit 0;;
  *" restart "*) exit 0;;
esac
exit 0
SC_EOF
  chmod +x "${dir}/bin/systemctl"

  # Build a configurable fake "new binary". --version respects
  # NEW_BIN_VERSION_RC + NEW_BIN_VERSION_STDERR; check-config respects
  # NEW_BIN_CHECKCONFIG_RC.
  cat > "${dir}/fake-new-binary" <<'FB_EOF'
#!/usr/bin/env bash
case "${1:-}" in
  --version)
    if [[ -n "${NEW_BIN_VERSION_STDERR:-}" ]]; then
      printf '%s\n' "$NEW_BIN_VERSION_STDERR" >&2
    fi
    exit "${NEW_BIN_VERSION_RC:-0}";;
  check-config)
    exit "${NEW_BIN_CHECKCONFIG_RC:-0}";;
esac
exit 0
FB_EOF
  chmod +x "${dir}/fake-new-binary"
  export FAKE_NEW_BINARY="${dir}/fake-new-binary"
}

assert_contains() {
  local needle="$1" haystack="$2" label="$3"
  if [[ "$haystack" == *"$needle"* ]]; then return 0; fi
  printf '  FAIL: %s\n    expected substring: %s\n    actual: %s\n' \
    "$label" "$needle" "$haystack" >&2
  return 1
}

assert_not_contains() {
  local needle="$1" haystack="$2" label="$3"
  if [[ "$haystack" != *"$needle"* ]]; then return 0; fi
  printf '  FAIL: %s\n    forbidden substring: %s\n    actual: %s\n' \
    "$label" "$needle" "$haystack" >&2
  return 1
}

# ---------- Smoke test: GLIBC mismatch caught before swap ----------
test_smoke_glibc_failure() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local config_dir="${sandbox}/etc"
  mkdir -p "$config_dir"; touch "${config_dir}/config.yaml"

  local stderr_file rc
  stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    NEW_BIN_VERSION_RC=1 \
    NEW_BIN_VERSION_STDERR="/lib/x86_64-linux-gnu/libc.so.6: version \`GLIBC_2.39' not found" \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --config-dir "$config_dir" --dry-run \
    > /dev/null 2> "$stderr_file"
  rc=$?
  local stderr; stderr="$(cat "$stderr_file")"
  rm -rf "$sandbox" "$stderr_file"

  [[ "$rc" -eq 1 ]] || { echo "  FAIL: exit code: expected 1, got $rc" >&2; return 1; }
  assert_contains "new binary failed smoke test:" "$stderr" "smoke-test header" || return 1
  assert_contains "GLIBC_2.39' not found" "$stderr" "captured loader error" || return 1
  assert_contains "not swapping; daemon continues on" "$stderr" "daemon-continues line" || return 1
}

# ---------- Smoke OK + check-config warnings: proceeds past preflight ----------
test_smoke_ok_warnings_proceeds() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local config_dir="${sandbox}/etc"
  mkdir -p "$config_dir"; touch "${config_dir}/config.yaml"

  local stderr_file stdout_file rc
  stderr_file="$(mktemp)"; stdout_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    NEW_BIN_VERSION_RC=0 \
    NEW_BIN_CHECKCONFIG_RC=1 \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --config-dir "$config_dir" --dry-run \
    > "$stdout_file" 2> "$stderr_file"
  rc=$?
  local stderr stdout
  stderr="$(cat "$stderr_file")"; stdout="$(cat "$stdout_file")"
  rm -rf "$sandbox" "$stderr_file" "$stdout_file"

  [[ "$rc" -eq 0 ]] || { echo "  FAIL: exit code: expected 0, got $rc" >&2; return 1; }
  assert_contains "preflight returned warnings; proceeding." "$stderr" "warnings line" || return 1
  assert_contains "[dry-run] Would swap to vTEST" "$stdout" "dry-run swap message" || return 1
  assert_not_contains "new binary failed smoke test" "$stderr" "no spurious smoke-fail" || return 1
}

# ---------- Smoke OK + check-config OK: happy path reaches swap ----------
test_happy_path() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local config_dir="${sandbox}/etc"
  mkdir -p "$config_dir"; touch "${config_dir}/config.yaml"

  local stderr_file stdout_file rc
  stderr_file="$(mktemp)"; stdout_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    NEW_BIN_VERSION_RC=0 \
    NEW_BIN_CHECKCONFIG_RC=0 \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --config-dir "$config_dir" --dry-run \
    > "$stdout_file" 2> "$stderr_file"
  rc=$?
  local stderr stdout
  stderr="$(cat "$stderr_file")"; stdout="$(cat "$stdout_file")"
  rm -rf "$sandbox" "$stderr_file" "$stdout_file"

  [[ "$rc" -eq 0 ]] || { echo "  FAIL: exit code: expected 0, got $rc; stderr=$stderr" >&2; return 1; }
  assert_contains "[dry-run] Would swap to vTEST" "$stdout" "dry-run swap message" || return 1
  assert_not_contains "preflight returned warnings" "$stderr" "no warnings noise" || return 1
  assert_not_contains "new binary failed smoke test" "$stderr" "no spurious smoke-fail" || return 1
}

# ---------- Config branch 1: empty resolved path ----------
test_config_branch_empty() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"

  local stderr_file rc
  stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    MOCK_EXECSTART="" \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --dry-run \
    > /dev/null 2> "$stderr_file"
  rc=$?
  local stderr; stderr="$(cat "$stderr_file")"
  rm -rf "$sandbox" "$stderr_file"

  [[ "$rc" -eq 1 ]] || { echo "  FAIL: exit code: expected 1, got $rc" >&2; return 1; }
  assert_contains "no config file at /etc/autocoder/config.yaml" "$stderr" "branch-1 message" || return 1
  assert_not_contains "cannot find config" "$stderr" "no pre-spec generic text" || return 1
}

# ---------- Config branch 2: file exists but is unreadable ----------
test_config_branch_unreadable() {
  if [[ $EUID -eq 0 ]]; then
    # Running as root, can read everything; skip the branch-2 readability
    # assertion since root bypasses mode bits. The branch's logic is
    # exercised by the other branches; we treat root invocation as a
    # benign skip.
    echo "  SKIP: running as root; -r always true"
    return 0
  fi
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local config_dir="${sandbox}/etc"
  mkdir -p "$config_dir"
  echo "secret" > "${config_dir}/config.yaml"
  chmod 0000 "${config_dir}/config.yaml"

  local stderr_file rc
  stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --config-dir "$config_dir" --dry-run \
    > /dev/null 2> "$stderr_file"
  rc=$?
  local stderr; stderr="$(cat "$stderr_file")"
  chmod 0644 "${config_dir}/config.yaml" 2>/dev/null || true
  rm -rf "$sandbox" "$stderr_file"

  [[ "$rc" -eq 1 ]] || { echo "  FAIL: exit code: expected 1, got $rc; stderr=$stderr" >&2; return 1; }
  assert_contains "is not readable by $(id -un)" "$stderr" "branch-2 names current user" || return 1
  assert_contains "try running with sudo" "$stderr" "branch-2 sudo hint" || return 1
  assert_not_contains "cannot find config" "$stderr" "no pre-spec generic text" || return 1
}

# ---------- Config branch 3: --config-dir points at missing path ----------
test_config_branch_missing() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"

  local stderr_file rc
  stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" \
    AUTOCODER_BINARY_PATH="${sandbox}/install/autocoder" \
    bash "$UPDATE_SH" --version vTEST --config-dir "${sandbox}/nope" --dry-run \
    > /dev/null 2> "$stderr_file"
  rc=$?
  local stderr; stderr="$(cat "$stderr_file")"
  rm -rf "$sandbox" "$stderr_file"

  [[ "$rc" -eq 1 ]] || { echo "  FAIL: exit code: expected 1, got $rc" >&2; return 1; }
  assert_contains "no config file at ${sandbox}/nope/config.yaml" "$stderr" "branch-3 message" || return 1
  assert_not_contains "cannot find config" "$stderr" "no pre-spec generic text" || return 1
}

run_case "smoke-test catches GLIBC failure before swap"     test_smoke_glibc_failure
run_case "smoke-OK + check-config warnings proceeds"         test_smoke_ok_warnings_proceeds
run_case "smoke-OK + check-config OK reaches dry-run swap"   test_happy_path
run_case "config branch 1: empty resolved path"              test_config_branch_empty
run_case "config branch 2: unreadable existing config"       test_config_branch_unreadable
run_case "config branch 3: missing config file"              test_config_branch_missing

echo "1..${TESTS}"
if (( FAIL > 0 )); then
  echo "FAIL: ${FAIL} of ${TESTS} cases failed" >&2
  exit 1
fi
echo "PASS: all ${TESTS} cases"
