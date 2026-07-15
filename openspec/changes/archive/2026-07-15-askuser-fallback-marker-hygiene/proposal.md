## Why

When the per-execution MCP server's `ask_user` cannot reach the control socket, it writes a fallback marker at `<workspace>/openspec/changes/<ORCH_MCP_CHANGE>/.askuser-pending.json` — creating the directory if needed. Verifier-gate and audit sessions set `ORCH_MCP_CHANGE` to a role name (`canon_contradiction_check`, `global_rules_check`, …), so the fallback fabricates a phantom change directory that `openspec list` reports as a real change. Worse, nothing excludes or cleans the marker: on a spec box (direct clone, partial install, `autocoder verify`), a later broad `git add` sweeps it into a commit — observed: a canon-gate marker from a local verify run was committed, merged to master, and resurfaced days later in another clone's `openspec list`. On the server, the marker isn't in the daemon's `.git/info/exclude` list either, so it can trip the dirty-workspace check.

## What Changes

- The MCP server's fallback marker never creates a directory: sessions whose change key names an *existing* `openspec/changes/<change>/` directory keep the in-directory marker (unchanged); every other session (gates, audits, stylist — any role-named key) writes `<workspace>/.askuser-pending-<sanitized-key>.json` at the workspace root. Same atomic write. This depends only on the workspace root existing, so it behaves identically in a daemon-managed workspace and a directly-cloned spec-box repo.
- The ask-user fallback marker pattern (`.askuser-pending*`) joins the per-run artifact class registered in `.git/info/exclude` — at daemon workspace initialization (server) and, defensively, before staging.
- `autocoder verify` registers the same exclude patterns in the target repo at run start and deletes any ask-user fallback markers its gate sessions leave, as part of its existing transient-artifact cleanup — so a spec box can't commit a marker even after an interrupted run.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `executor`: new requirement — the ask-user fallback marker never fabricates change directories (placement rules for change vs. non-change sessions).
- `workspace-manager`: "Per-run CLI config artifacts are excluded from commits" gains the ask-user fallback marker patterns in its excluded artifact class.
- `orchestrator-cli`: the `verify` subcommand requirement's transient-artifact contract explicitly covers ask-user fallback markers (cleanup on exit) and adds exclude registration in the target repo.

## Impact

- `autocoder/src/mcp_askuser_server.rs`: marker-path computation (existing-dir check, root fallback name, sanitization); no protocol change.
- `autocoder/src/workspace.rs`: one more pattern in the exclude registration.
- `autocoder/src/cli/verify.rs`: exclude registration at start; marker cleanup on exit.
- No state migration: old markers are plain untracked files; the exclude pattern covers both old and new names.
