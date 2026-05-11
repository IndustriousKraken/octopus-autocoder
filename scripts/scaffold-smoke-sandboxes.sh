#!/usr/bin/env bash
# Scaffolds two empty GitHub sandbox repos with the OpenSpec change material
# required for the autocoder smoke test
# (docs/foundation-smoke-test.md).
#
# Prerequisites:
#   - Two repos already created on GitHub. They MUST be empty (no README,
#     no initial commit) — the script seeds the main branch itself.
#   - Your local `git` is configured globally with user.name and user.email
#     (the script does not set these per-repo).
#   - You can already push to those repos (e.g. SSH key registered with
#     github.com).
#
# Usage:
#   ./scripts/scaffold-smoke-sandboxes.sh <repo-1-url> <repo-2-url>
#
# Example:
#   ./scripts/scaffold-smoke-sandboxes.sh \
#     git@github.com:you/orch-smoke-1.git \
#     git@github.com:you/orch-smoke-2.git

set -euo pipefail

if [[ $# -ne 2 ]]; then
    cat <<EOF >&2
Usage: $0 <repo-1-url> <repo-2-url>

Both repositories must already exist on GitHub and must be empty (no README,
no initial commit). The script aborts if either remote already has a main
branch, to avoid overwriting your work.
EOF
    exit 1
fi

repo_1="$1"
repo_2="$2"

scaffold() {
    local url="$1"
    local change_name="$2"
    local why_line="$3"
    local task_text="$4"
    local artifact_name="$5"
    local artifact_text="$6"

    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    echo "→ scaffolding $url"
    echo "  workdir: $tmpdir"

    if git ls-remote --heads "$url" main 2>/dev/null | grep -q "refs/heads/main"; then
        echo "  ✗ $url already has a main branch — refusing to clobber." >&2
        echo "    Delete and recreate the repo as empty, or push manually." >&2
        return 1
    fi

    git -C "$tmpdir" init -q -b main
    git -C "$tmpdir" remote add origin "$url"

    cat >"$tmpdir/README.md" <<EOF
# Orchestrator Smoke Sandbox

This repository is a sandbox for autocoder's end-to-end smoke test.
See \`docs/foundation-smoke-test.md\` in the autocoder repository for
the full procedure.

The agent's job is described under \`openspec/changes/$change_name/\`.
EOF

    mkdir -p "$tmpdir/openspec/changes/$change_name"
    cat >"$tmpdir/openspec/changes/$change_name/proposal.md" <<EOF
## Why
$why_line

## What Changes
- $task_text

## Impact
None — this is a smoke-test fixture.
EOF

    cat >"$tmpdir/openspec/changes/$change_name/tasks.md" <<EOF
## 1. Trivial smoke task

- [ ] 1.1 Create a file at the repo root named \`$artifact_name\`
  containing the literal text \`$artifact_text\` (no trailing newline,
  no surrounding quotes).
EOF

    git -C "$tmpdir" add -A
    git -C "$tmpdir" commit -q -m "scaffold: smoke sandbox with change \`$change_name\`"
    git -C "$tmpdir" push -u -q origin main

    echo "  ✓ pushed main with change \`$change_name\`"
}

# Repo 1: short ## Why line. Exercises the basic happy path:
# clone → checkout → executor → commit → push → PR.
scaffold "$repo_1" \
    "add-greetings-file" \
    "Smoke-test fixture: confirm autocoder applies a trivial change end to end." \
    "Create a file named \`GREETINGS\` containing the text \`hello world\`." \
    "GREETINGS" \
    "hello world"

# Repo 2: deliberately long ## Why line. Exercises the commit-subject
# truncation rule (72 chars total subject including \"<change>: \" prefix).
scaffold "$repo_2" \
    "add-farewell-file" \
    "Smoke-test fixture with an intentionally very long Why line so that autocoder's 72-character commit-subject truncation is observable in the resulting commit message on the agent branch." \
    "Create a file named \`FAREWELL\` containing the text \`goodbye world\`." \
    "FAREWELL" \
    "goodbye world"

cat <<'EOF'

✓ Both sandboxes scaffolded.

Next steps:

  1. Write a `config.yaml` (do NOT commit it) pointing at both repo URLs.
     Use distinct poll_interval_sec values, e.g. 60 and 180.
     See docs/foundation-smoke-test.md for the schema.

  2. Export your GitHub token:
       export GITHUB_TOKEN=ghp_yourtokenhere

  3. Build and run:
       cargo build --release
       RUST_LOG=info ./target/release/autocoder run --config config.yaml

  4. After both sandboxes have an open PR on agent-q, send SIGINT (Ctrl-C)
     and verify the pass criteria in docs/foundation-smoke-test.md.
EOF
