#!/usr/bin/env bash
# Integration tests for install-verify.sh (the check-only installer). Builds a
# sandboxed PATH that mocks curl / sha256sum and supplies a fake "release
# binary", points HOME at a scratch dir, then runs the real install-verify.sh
# and asserts WHERE the minimal config lands AND what the post-install summary
# suggests as the next-step command.
#
# Covers the `check-only-install-writes-default-config` change:
#   - a flagless install writes to the standard discovery path
#     (~/.config/autocoder/config.yaml), NOT verify.yaml;
#   - the summary's next-step command is flagless `autocoder verify <slug>`;
#   - a pre-existing config.yaml at that path is left untouched;
#   - an explicit `--config <path>` overrides discovery (writes there) AND the
#     summary shows the matching `--config <path>` invocation.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_SH="${REPO_ROOT}/install-verify.sh"

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

# Build a sandboxed PATH + fake release binary for one scenario.
#   $1 = scratch dir (pre-created)
# Exports FAKE_RELEASE_BINARY so the curl mock can copy it into place.
make_sandbox() {
  local dir="$1"
  mkdir -p "${dir}/bin"

  # Mock curl: `/releases/latest` returns a tag; `.sha256` URLs write a
  # sha256sums-style line; binary URLs copy the fake release binary into the
  # requested output path.
  cat > "${dir}/bin/curl" <<'CURL_EOF'
#!/usr/bin/env bash
out=""
url=""
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
  basename_part="$(basename "${url%.sha256}")"
  if [[ -n "$out" ]]; then
    printf 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  %s\n' "$basename_part" > "$out"
  fi
  exit 0
fi
# Binary download: copy the configured fake release binary into place.
if [[ -n "$out" && -n "${FAKE_RELEASE_BINARY:-}" ]]; then
  cp "$FAKE_RELEASE_BINARY" "$out"
fi
exit 0
CURL_EOF
  chmod +x "${dir}/bin/curl"

  # Mock sha256sum: `-c` always succeeds (the curl mock writes a placeholder
  # digest the installer never truly verifies in these tests).
  cat > "${dir}/bin/sha256sum" <<'SUM_EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "-c" ]]; then exit 0; fi
echo "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  ${2:-$1}"
SUM_EOF
  chmod +x "${dir}/bin/sha256sum"

  # A throwaway "release binary": its contents are irrelevant — the installer
  # only `install -m 755`s it onto PATH.
  printf '#!/usr/bin/env bash\nexit 0\n' > "${dir}/fake-release-binary"
  chmod +x "${dir}/fake-release-binary"
  export FAKE_RELEASE_BINARY="${dir}/fake-release-binary"
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

# ---------- Default install: config lands at the discovery path ----------
test_default_writes_discovery_path() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local home="${sandbox}/home"; mkdir -p "$home"

  local stdout_file stderr_file rc
  stdout_file="$(mktemp)"; stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" HOME="$home" \
    bash "$INSTALL_SH" --version vTEST \
    > "$stdout_file" 2> "$stderr_file"
  rc=$?
  local stdout stderr; stdout="$(cat "$stdout_file")"; stderr="$(cat "$stderr_file")"

  local discovery="${home}/.config/autocoder/config.yaml"
  local legacy="${home}/.config/autocoder/verify.yaml"
  local ok=0
  [[ "$rc" -eq 0 ]] || { echo "  FAIL: exit code: expected 0, got $rc; stderr=$stderr" >&2; ok=1; }
  [[ -f "$discovery" ]] || { echo "  FAIL: config not written to discovery path $discovery" >&2; ok=1; }
  [[ ! -e "$legacy" ]] || { echo "  FAIL: legacy verify.yaml should not be written ($legacy)" >&2; ok=1; }
  assert_contains "config:  ${discovery}" "$stdout" "summary prints resolved config path" || ok=1
  # The next-step command is flagless (no --config) when the config went to the
  # discovery path.
  assert_contains "autocoder verify <change-slug>" "$stdout" "summary shows verify command" || ok=1
  assert_not_contains "verify <change-slug> --config" "$stdout" "summary omits --config" || ok=1

  rm -rf "$sandbox" "$stdout_file" "$stderr_file"
  return $ok
}

# ---------- Pre-existing config.yaml is left untouched ----------
test_existing_config_untouched() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local home="${sandbox}/home"; mkdir -p "$home/.config/autocoder"
  local discovery="${home}/.config/autocoder/config.yaml"
  printf 'PRE-EXISTING SENTINEL\n' > "$discovery"

  local stdout_file stderr_file rc
  stdout_file="$(mktemp)"; stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" HOME="$home" \
    bash "$INSTALL_SH" --version vTEST \
    > "$stdout_file" 2> "$stderr_file"
  rc=$?
  local stderr contents; stderr="$(cat "$stderr_file")"; contents="$(cat "$discovery")"

  local ok=0
  [[ "$rc" -eq 0 ]] || { echo "  FAIL: exit code: expected 0, got $rc; stderr=$stderr" >&2; ok=1; }
  assert_contains "PRE-EXISTING SENTINEL" "$contents" "pre-existing config preserved" || ok=1
  assert_contains "already exists; leaving it untouched" "$stderr" "guard message printed" || ok=1

  rm -rf "$sandbox" "$stdout_file" "$stderr_file"
  return $ok
}

# ---------- Explicit --config overrides discovery ----------
test_explicit_config_overrides() {
  local sandbox; sandbox="$(mktemp -d)"
  make_sandbox "$sandbox"
  local home="${sandbox}/home"; mkdir -p "$home"
  local explicit="${sandbox}/custom/my-verify.yaml"

  local stdout_file stderr_file rc
  stdout_file="$(mktemp)"; stderr_file="$(mktemp)"
  PATH="${sandbox}/bin:${PATH}" HOME="$home" \
    bash "$INSTALL_SH" --version vTEST --config "$explicit" \
    > "$stdout_file" 2> "$stderr_file"
  rc=$?
  local stdout stderr; stdout="$(cat "$stdout_file")"; stderr="$(cat "$stderr_file")"

  local discovery="${home}/.config/autocoder/config.yaml"
  local ok=0
  [[ "$rc" -eq 0 ]] || { echo "  FAIL: exit code: expected 0, got $rc; stderr=$stderr" >&2; ok=1; }
  [[ -f "$explicit" ]] || { echo "  FAIL: config not written to explicit path $explicit" >&2; ok=1; }
  [[ ! -e "$discovery" ]] || { echo "  FAIL: discovery path should be untouched under --config ($discovery)" >&2; ok=1; }
  # Under an explicit override the next-step command shows the matching
  # --config <path> so the suggested invocation resolves the same file.
  assert_contains "autocoder verify <change-slug> --config ${explicit}" "$stdout" "summary shows --config form" || ok=1

  rm -rf "$sandbox" "$stdout_file" "$stderr_file"
  return $ok
}

run_case "default install writes to ~/.config/autocoder/config.yaml" test_default_writes_discovery_path
run_case "pre-existing config.yaml left untouched"                   test_existing_config_untouched
run_case "explicit --config overrides discovery"                     test_explicit_config_overrides

echo "1..${TESTS}"
if (( FAIL > 0 )); then
  echo "FAIL: ${FAIL} of ${TESTS} cases failed" >&2
  exit 1
fi
echo "PASS: all ${TESTS} cases"
