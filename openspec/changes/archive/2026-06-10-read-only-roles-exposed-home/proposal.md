## Why

Read-only roles (audits, the agentic reviewer, the verifier gates) ran under a **masked-home allowlist**: `$HOME` was replaced with an empty tmpfs and only the workspace, the role's own CLI store, and "the resolved CLI binary + its symlink dependency closure" were bound back. That model is wrong for any CLI whose runtime lives under `$HOME` — which is most of them. opencode is a Node app at `~/.opencode/bin/opencode` running on `~/.nvm/.../node` with its JS + `node_modules` under the home; the symlink-closure binding can't reach the interpreter or modules, so `bwrap` failed with `execvp opencode: No such file or directory` and the gate held every change. Claude survives only because the **executor** uses the exposed-home denylist; the read-only roles never got that.

The fix: read-only roles use the same **exposed-home denylist** as the executor — `$HOME` present and writable (so the CLI finds AND can write its toolchain runtime, session, and caches), the credential mask-list still masked, the capability drops unchanged — with the **workspace read-only**. The workspace, not the home, is the meaningful "read-only": a gate/reviewer must not modify the repo, specs, or PR branch, but it may read the home and write its own caches (exactly what the executor already does). The fragile CLI-binary-binding allowlist is retained for **strict mode only** (the explicit high-compliance opt-in, which accepts that a toolchain-heavy CLI may not start under the mask).

## What Changes

- **Read-only roles move from the masked-home allowlist to the exposed-home denylist** (home read-write, mask-list masked), keeping the workspace read-only. `RunSandbox::uses_denylist()` becomes `!strict_mode` (was `workspace_writable && !strict_mode`).
- **Home is read-write for every denylist role**, including read-only roles — so CLIs can write session/cache under `$HOME`. The three argv builders (`systemd_run_argv`, `bwrap_argv`, `seatbelt_profile`) bind home read-write under the denylist regardless of role; the workspace posture (read-write vs read-only) is the only role difference. Seatbelt's denylist gains a workspace write-deny for read-only roles.
- **The masked-home allowlist is now strict-mode-only.** Its CLI-binary-binding + self-store/extra-store logic is unchanged but reached only when `strict_mode` is set.
- No change to: the executor's policy, the mask-list contents, `os_hide`/`engine_deny`, capability drops, `/proc` restriction, egress (still open), or the control-socket bind.

## Impact

- **Affected specs:** `executor` — MODIFY `Every agentic subprocess runs inside an OS-level sandbox` (the read-only-role filesystem policy + its scenarios).
- **Affected code:** `sandbox.rs` — `uses_denylist`, the denylist branches of all three argv builders, module + policy docs, and three policy tests.
- **Affected docs:** `docs/SECURITY.md` §9 (the read-only-role row of the OS-level sandbox).
- **Security posture:** read-only roles can now READ the operator's home (minus the masked credential set) and WRITE their own home caches/session — strictly more permissive than the old mask, strictly less than the executor (which also writes the repo). The credential mask-list still bounds the sensitive set; the read-only workspace still prevents repo/spec/PR tampering. Operators wanting the tighter masked home opt into `strict_mode` (and accept its CLI-runtime limitation).
- **Acceptance:** `cargo test` (the sandbox policy + enforcement tests assert the new posture) + `openspec validate read-only-roles-exposed-home --strict`.
