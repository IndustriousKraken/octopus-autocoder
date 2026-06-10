# Tasks

## 1. Spec

- [x] 1.1 ADD `Per-run CLI config artifacts are excluded from commits` (workspace-manager): the daemon adds the strategies' auto-discovered config artifacts to `.git/info/exclude` before `git add -A`; untracked-only, derived from the strategy filename constants.

## 2. Code

- [x] 2.1 `agentic_run.rs`: `WORKSPACE_CLI_ARTIFACT_EXCLUDES` (built from `OPENCODE_CONFIG_FILENAME` / `ANTIGRAVITY_MCP_CONFIG_FILENAME` + `.mcp.json` + `.opencode/`).
- [x] 2.2 `git.rs`: `ensure_local_excludes(workspace, patterns)` (idempotent append to `.git/info/exclude`; no-op when `.git` is not a directory); call it from `add_all` before `git add -A` (best-effort).

## 3. Tests (`git.rs`)

- [x] 3.1 `ensure_local_excludes_is_idempotent` — no duplicate entries on a second call.
- [x] 3.2 `add_all_excludes_untracked_cli_artifacts` — the change file commits; `opencode.json` / `.mcp.json` / `.opencode/` do not.
- [x] 3.3 `add_all_exclude_does_not_hide_a_tracked_config` — a force-tracked `opencode.json`'s later edit still commits.

## 4. Acceptance

- [x] 4.1 `cargo test` passes (the exclude tests + full suite green).
- [x] 4.2 `openspec validate exclude-cli-artifacts-from-commits --strict` passes.
