## 1. Marker placement in the MCP server

- [x] 1.1 In `autocoder/src/mcp_askuser_server.rs`, compute the marker path at write time: if `<workspace>/openspec/changes/<change>/` exists, keep the in-directory `.askuser-pending.json`; otherwise write `<workspace>/.askuser-pending-<sanitized-key>.json` (key filtered to `[A-Za-z0-9_-]`, other chars → `-`), with no directory creation on either path. Keep the atomic tempfile + rename write.
- [x] 1.2 Unit tests: role-keyed session writes the root marker and creates nothing under `openspec/changes/`; existing-change session keeps the in-dir marker; a key containing `/` or `..` produces a sanitized root filename inside the workspace.

## 2. Daemon-side exclusion

- [x] 2.1 In `autocoder/src/workspace.rs`, add `.askuser-pending*` to the patterns registered via `ensure_git_info_excluded` at workspace initialization (alongside the existing marker and per-run CLI config patterns).
- [x] 2.2 Unit test: after workspace init, an untracked root or in-dir ask-user marker is ignored by `git status --porcelain` and survives the pattern's idempotent re-registration.

## 3. Verify-side registration and cleanup

- [x] 3.1 In `autocoder/src/cli/verify.rs`, at run start, idempotently register the per-run artifact patterns (including `.askuser-pending*`) in the target repo's `.git/info/exclude`, reusing `workspace::ensure_git_info_excluded`.
- [x] 3.2 At verify exit cleanup, delete any `.askuser-pending*` files at the workspace root and in the verified change's directory, alongside the existing transient-artifact cleanup.
- [x] 3.3 Integration-style test: a simulated gate run that drops a root marker ends with the marker gone after a completed run; with cleanup skipped (simulated interrupt), the marker is present but not staged by `git add -A` thanks to the exclude.

## 4. Verification

- [x] 4.1 Run the full `cargo test` suite; confirm existing marker/exclude tests and the verify subcommand tests still pass.
