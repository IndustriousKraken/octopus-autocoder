# Tasks

## 1. Register CLI-artifact excludes at workspace init

- [x] 1.1 In `workspace::ensure_initialized`, after the daemon root-level marker excludes (`.failure-state.json`, `.audit-state.json`, …), register `crate::agentic_run::WORKSPACE_CLI_ARTIFACT_EXCLUDES` via `crate::git::ensure_local_excludes`. Best-effort: a failure logs a WARN and does not abort init (matching the existing marker registrations). Keep the existing `git::add_all` registration as a defensive backstop.

## 2. Tests

- [x] 2.1 A freshly initialized workspace has the per-run CLI artifacts (`opencode.json`, `.opencode/`, `.mcp.json`, `mcp_config.json`) in `.git/info/exclude` — before any commit.
- [x] 2.2 With the excludes registered, an UNTRACKED `opencode.json` in the workspace root does NOT appear in `git status --porcelain` (the dirty check is blind to it), so an advisory audit's clean-workspace check passes.
- [x] 2.3 Registration is idempotent across repeated init (no duplicate entries), and re-applies after a re-clone (a fresh `.git/info/exclude` is re-populated on init).
