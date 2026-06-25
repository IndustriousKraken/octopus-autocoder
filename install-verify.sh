#!/usr/bin/env bash
# autocoder check-only installer (the `verify` spec-box).
#
# Fetches the SAME pre-built autocoder binary the daemon ships in (so the
# `verify` subcommand runs the identical gate-check logic the server runs),
# places it on PATH, and writes a MINIMAL config carrying only what `verify`
# needs: the three verifier-gate model blocks, their enabled flags, and the
# global-rule corpus location. No repos, chatops, reviewer, or daemon config
# is written — the spec-authoring machine never builds from source and never
# runs the daemon.
#
# Usage:
#   ./install-verify.sh [--version vX.Y.Z] [--config PATH]
#
# Environment:
#   AUTOCODER_VERSION   pin the release tag (default: latest)
#   ANTHROPIC_API_KEY   the gate CLIs self-authenticate from their own login;
#                       no key is written into the config.
set -euo pipefail

OWNER="IndustriousKraken"
REPO="octopus-autocoder"
STEP="init"
trap 'echo "install-verify.sh failed during step: ${STEP}" >&2' ERR

VERSION="${AUTOCODER_VERSION:-}"
# Default to the STANDARD discovery location autocoder auto-resolves (the same
# path `run`/`verify` find when `--config` is omitted), so a check-only install
# makes `autocoder verify <change-slug>` work with NO `--config` flag. A check-
# only config is an ordinary autocoder config — just a minimal subset — so it
# does not warrant a distinct filename.
CONFIG_PATH="${HOME}/.config/autocoder/config.yaml"
# Tracks whether the operator pointed `--config` somewhere explicit; when they
# did, the post-install summary suggests the matching `--config <path>` form.
CONFIG_OVERRIDDEN=0
while (( $# )); do
  case "$1" in
    --version) VERSION="$2"; shift 2;;
    --config) CONFIG_PATH="$2"; CONFIG_OVERRIDDEN=1; shift 2;;
    *) echo "unknown argument: $1" >&2; exit 2;;
  esac
done

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

STEP="detect"; TRIPLE="$(detect_target_triple)"

STEP="version"
if [[ -z "$VERSION" ]]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  [[ -n "$VERSION" ]] || { echo "could not resolve latest release tag" >&2; exit 1; }
fi

STEP="download"
TMP="$(mktemp -d)"
BASENAME="autocoder-${VERSION}-${TRIPLE}"
URL="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${BASENAME}"
curl -fsSL -o "${TMP}/${BASENAME}" "${URL}"
curl -fsSL -o "${TMP}/${BASENAME}.sha256" "${URL}.sha256"

STEP="verify"
if command -v sha256sum >/dev/null 2>&1; then SUMCHECK="sha256sum -c"; else SUMCHECK="shasum -a 256 -c"; fi
if ! ( cd "${TMP}" && ${SUMCHECK} "${BASENAME}.sha256" ); then
  echo "checksum verification failed; tempdir preserved at ${TMP}" >&2
  exit 1
fi

STEP="install-binary"
# Check-only install is always a user-PATH install (no daemon, no root).
DEST="${HOME}/.local/bin/autocoder"
mkdir -p "$(dirname "${DEST}")"
install -m 755 "${TMP}/${BASENAME}" "${DEST}"

STEP="write-config"
mkdir -p "$(dirname "${CONFIG_PATH}")"
if [[ -e "${CONFIG_PATH}" ]]; then
  echo "config ${CONFIG_PATH} already exists; leaving it untouched" >&2
else
  # Minimal config: only the gate model blocks, their enabled flags, the
  # unified agentic-session timeout, and the global-rule corpus location.
  # `repositories` is empty (verify operates on the cwd repo, not a managed
  # repo). The model nicknames default to a small Anthropic model the gate
  # CLI authenticates for from its own login; edit `model:` to taste.
  cat > "${CONFIG_PATH}" <<'YAML'
# Check-only config for `autocoder verify` (the spec-box).
# verify operates on the current working directory's repo; no managed
# repositories are listed here.
repositories: []

executor:
  kind: claude_cli
  command: claude
  # The single timeout governing every agentic gate session.
  agentic_session_timeout_secs: 3600

  # [in] gate — change-internal contradictions.
  change_internal_contradiction_check: enabled
  change_internal_contradiction_check_llm:
    provider: anthropic
    model: claude-sonnet-4-5

  # [canon] gate — change vs. this project's canonical specs.
  change_canonical_contradiction_check: enabled
  change_canonical_contradiction_check_llm:
    provider: anthropic
    model: claude-sonnet-4-5

  # [rules] gate — change vs. the portable global-rule corpus.
  global_rules_check: enabled
  global_rules_check_llm:
    provider: anthropic
    model: claude-sonnet-4-5
  global_rules:
    # Path to a local rule-corpus directory, OR a git URL autocoder clones.
    # Edit to point at your corpus before running the [rules] gate.
    corpus: ~/.config/autocoder/global-rules

github: {}
YAML
  echo "wrote minimal verify config to ${CONFIG_PATH}"
fi

# Suggest the next-step invocation that MATCHES where the config landed: the
# flagless form when it went to the standard discovery path (auto-discovered,
# no `--config` needed), or the explicit `--config <path>` form when the
# operator directed it elsewhere — so the printed command always resolves the
# config this installer just wrote.
if (( CONFIG_OVERRIDDEN )); then
  NEXT_STEP="autocoder verify <change-slug> --config ${CONFIG_PATH}"
else
  NEXT_STEP="autocoder verify <change-slug>"
fi

cat <<EOF

autocoder (verify-only) installed:
  binary:  ${DEST}
  config:  ${CONFIG_PATH}

Ensure ${HOME}/.local/bin is on your PATH, then in a repository run:
  ${NEXT_STEP}

verify is the LOCAL accelerator: it runs the same gate checks the server
runs, so you learn whether a change would pass before pushing. The server
gates remain the enforcement (they run against fresher canon at implement
time and cover every contributor).
EOF
