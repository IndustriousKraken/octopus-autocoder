#!/usr/bin/env bash
# autocoder unattended binary updater. See docs/DEPLOYMENT.md "Unattended updates via cron".
# detect_target_triple is kept in sync with install.sh.
set -euo pipefail

OWNER="IndustriousKraken"
REPO="octopus-autocoder"
STEP="init"
trap 'echo "update.sh failed during step: ${STEP}" >&2' ERR

VERSION=""; DRY_RUN=0; CONFIG_DIR=""
while (( $# )); do
  case "$1" in
    --version) VERSION="$2"; shift 2;;
    --dry-run) DRY_RUN=1; shift;;
    --config-dir) CONFIG_DIR="$2"; shift 2;;
    --) shift; break;;
    *) echo "update.sh: unknown argument: $1" >&2; exit 2;;
  esac
done

# Keep in sync with install.sh's detect_target_triple.
detect_target_triple() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  [[ "$arch" == "arm64" ]] && arch="aarch64"
  case "${os}/${arch}" in
    Linux/x86_64) echo "x86_64-unknown-linux-gnu";;
    Linux/aarch64) echo "aarch64-unknown-linux-gnu";;
    Darwin/aarch64) echo "aarch64-apple-darwin";;
    *) echo "no pre-built binary for ${os}/${arch}; build from source per README" >&2; exit 1;;
  esac
}

SUDO=""; if [[ $EUID -ne 0 ]] && command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi

current_version() { autocoder --version 2>/dev/null | head -n1 | awk '{print $NF}'; }

target_version() {
  if [[ -n "$VERSION" ]]; then echo "$VERSION"; return; fi
  curl -fsSL "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1
}

# Resolve the daemon's config path (a011 task 3): an explicit --config-dir
# always wins and skips the unit lookup; otherwise the installed systemd unit's
# ExecStart is parsed for the flag the daemon is launched with (--config <file>,
# or --config-dir <dir> from which the file is <dir>/config.yaml); absent a unit
# or a recorded path, fall back to the default server-mode location.
resolve_config_path() {
  if [[ -n "$CONFIG_DIR" ]]; then echo "${CONFIG_DIR}/config.yaml"; return; fi
  local exec_line cfg dir
  exec_line="$(systemctl show autocoder.service -p ExecStart 2>/dev/null || true)"
  # Prefer an explicit --config <file> recorded in the unit.
  cfg="$(echo "$exec_line" | sed -n 's/.*--config[= ]\([^ ;]*\).*/\1/p' | head -n1)"
  if [[ -n "$cfg" ]]; then echo "$cfg"; return; fi
  # Accept a --config-dir <dir> form and derive <dir>/config.yaml.
  dir="$(echo "$exec_line" | sed -n 's/.*--config-dir[= ]\([^ ;]*\).*/\1/p' | head -n1)"
  if [[ -n "$dir" ]]; then echo "${dir%/}/config.yaml"; return; fi
  # Fall back to the default-path resolution.
  echo "/etc/autocoder/config.yaml"
}

run_preflight() {
  local new_binary="$1" config_path="$2" rc=0
  # Smoke test catches GLIBC mismatch / missing .so / arch mismatch / corrupted
  # download — load failures the dynamic linker rejects before check-config runs.
  local smoke_err
  if ! smoke_err="$("$new_binary" --version 2>&1 >/dev/null)"; then
    echo "update.sh: new binary failed smoke test:" >&2
    echo "$smoke_err" >&2
    echo "update.sh: not swapping; daemon continues on $(current_version)." >&2
    exit 1
  fi
  set +e; "$new_binary" check-config --config "$config_path" --json; rc=$?; set -e
  case "$rc" in
    0) ;;
    1) echo "update.sh: preflight returned warnings; proceeding." >&2;;
    2) echo "update.sh: preflight failed; not swapping. Daemon continues on $(current_version)." >&2; exit 1;;
    *) echo "update.sh: preflight returned unexpected exit code ${rc}; aborting." >&2; exit 1;;
  esac
}

swap_binary() {
  local new_path="$1" current_path="$2"
  ${SUDO} mv -f "$current_path" "${current_path}.previous"
  ${SUDO} install -m 755 "$new_path" "$current_path"
}

wait_for_active() {
  local timeout_secs="${1:-30}" i=0
  while (( i < timeout_secs )); do
    if systemctl is-active --quiet autocoder; then return 0; fi
    sleep 1; i=$((i + 1))
  done
  return 1
}

STEP="detect"; TRIPLE="$(detect_target_triple)"

STEP="versions"
CURRENT="$(current_version || true)"
TARGET="$(target_version)"
[[ -n "$TARGET" ]] || { echo "update.sh: could not resolve target version" >&2; exit 1; }
if [[ -n "${CURRENT#v}" && "${CURRENT#v}" == "${TARGET#v}" ]]; then
  echo "autocoder is already on ${TARGET}; nothing to do"
  exit 0
fi

STEP="download"
TMP="$(mktemp -d)"
BASENAME="autocoder-${TARGET}-${TRIPLE}"
URL="https://github.com/${OWNER}/${REPO}/releases/download/${TARGET}/${BASENAME}"
curl -fsSL -o "${TMP}/${BASENAME}" "${URL}"
curl -fsSL -o "${TMP}/${BASENAME}.sha256" "${URL}.sha256"

STEP="verify"
if command -v sha256sum >/dev/null 2>&1; then SUMCHECK="sha256sum -c"; else SUMCHECK="shasum -a 256 -c"; fi
if ! ( cd "${TMP}" && ${SUMCHECK} "${BASENAME}.sha256" ); then
  echo "update.sh: checksum verification failed; tempdir preserved at ${TMP}" >&2
  exit 1
fi
chmod +x "${TMP}/${BASENAME}"

STEP="preflight"
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
run_preflight "${TMP}/${BASENAME}" "$CONFIG_PATH"

(( DRY_RUN )) && { echo "[dry-run] Would swap to ${TARGET}"; exit 0; }

STEP="swap"
BINARY_PATH="${AUTOCODER_BINARY_PATH:-/usr/local/bin/autocoder}"
swap_binary "${TMP}/${BASENAME}" "$BINARY_PATH"

STEP="restart"
${SUDO} systemctl restart autocoder
if ! wait_for_active 30; then
  echo "update.sh: new binary did not become active within 30s; rolling back" >&2
  swap_binary "${BINARY_PATH}.previous" "$BINARY_PATH"
  ${SUDO} systemctl restart autocoder
  echo "update.sh: new binary failed to start; rolled back to ${CURRENT:-previous}. Check journalctl -u autocoder." >&2
  exit 1
fi

STEP="log"
SUMMARY="autocoder updated ${CURRENT:-unknown} → ${TARGET}"
echo "$SUMMARY"
if command -v logger >/dev/null 2>&1; then logger -t autocoder-update "$SUMMARY"; fi

exit 0
