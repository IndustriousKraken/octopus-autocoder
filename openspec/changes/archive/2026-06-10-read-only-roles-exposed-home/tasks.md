# Tasks

## 1. Spec

- [x] 1.1 MODIFY `Every agentic subprocess runs inside an OS-level sandbox` (executor): read-only roles use the exposed-home denylist (home read-write, mask-list masked, workspace read-only); the masked-home allowlist is strict-mode-only.
- [x] 1.2 MODIFY `Sandbox credential-protection config — toggles, precedence, and relaxed-posture logging` (orchestrator-cli): correct the clause "read-only roles always use the allowlist" → read-only roles use the exposed-home denylist; only strict mode uses the masked-home allowlist.

## 2. Code (`sandbox.rs`)

- [x] 2.1 `uses_denylist()` → `!strict_mode` (read-only roles now use the denylist).
- [x] 2.2 `systemd_run_argv` / `bwrap_argv`: bind home read-write under the denylist for every role (the workspace posture already differs by `workspace_writable`).
- [x] 2.3 `seatbelt_profile`: deny workspace writes for read-only roles under the denylist (`(allow default)` would leave it writable).
- [x] 2.4 Update the module doc + the `uses_denylist` / `FsPolicy` docs to the new model.
- [x] 2.5 Project-scratch overlay: `cli_workspace_scratch(cli)` (opencode → `.opencode`); `SandboxPlan.workspace_scratch` + `RunSandbox::workspace_scratch_dirs`; overlay it writable+ephemeral on the read-only workspace in all three mechanisms (bwrap `--tmpfs`, systemd `TemporaryFileSystem`, seatbelt `allow file-write*`); pre-create the host mountpoint in `wrap_command`. Fixes opencode's `EROFS` on `<workspace>/.opencode/.gitignore`.
- [x] 2.6 Litter cleanup: `ScratchCleanup` RAII guard in `agentic_run` removes the pre-created host mountpoint dir on every exit path (`remove_dir`, empty-only — the tmpfs content is freed by namespace teardown; a tracked/non-empty dir is preserved).

## 3. Tests

- [x] 3.1 `build_plan_selects_policy_by_role_and_strict_mode`: read-only role → `Denylist` + read-only workspace.
- [x] 3.2 `os_hide_controls_other_store_presence_across_policies`: os_hide governs the mask-list for a read-only (denylist) role AND `extra_ro_stores` under strict mode.
- [x] 3.3 `enforced_readonly_role_exposes_home_masks_creds_blocks_workspace_write`: home readable + writable, the credential mask-list masked, the workspace read-only.
- [x] 3.4 `read_only_opencode_role_gets_writable_ephemeral_scratch` (argv-level) + `enforced_readonly_role_scratch_is_writable_repo_stays_readonly` (real kernel): the `.opencode` scratch is writable while repo files stay read-only.

## 4. Docs

- [x] 4.1 `docs/SECURITY.md` §9: update the read-only-role row (exposed-home denylist, read-only workspace; masked allowlist is strict-mode-only).

## 5. Acceptance

- [x] 5.1 `cargo test` passes (sandbox policy + enforcement tests assert the new posture; full suite green).
- [x] 5.2 `openspec validate read-only-roles-exposed-home --strict` passes.
