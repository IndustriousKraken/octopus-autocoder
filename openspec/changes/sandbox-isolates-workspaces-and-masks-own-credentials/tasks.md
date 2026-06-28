# Tasks

## 1. Mask the daemon's own config + secrets

- [x] 1.1 Thread the daemon's RESOLVED config file path AND secrets file path
  (loaded at startup) into the sandbox mask construction, so they are added to the
  effective mask-list at runtime in addition to `DEFAULT_MASK_RELATIVE`. Do NOT
  hardcode a location — cover non-standard paths (`~/autocoder/config.yaml`,
  `/etc/autocoder/secrets.env`, etc.), including paths outside `$HOME`.
- [x] 1.2 Apply the masking for BOTH the executor and read-only roles, across all
  three mechanisms (systemd `InaccessiblePaths`, bwrap `--ro-bind /dev/null` /
  tmpfs over the entry, seatbelt deny), matching how `DEFAULT_MASK_RELATIVE`
  entries are applied today.
- [x] 1.3 Correct the `sandbox.rs` module doc once the "autocoder's own config is
  masked" claim is actually true.

## 2. Sibling workspaces read-only, own workspace read-write

- [x] 2.1 Under the denylist policy, bind the workspaces-parent directory
  read-only and re-bind ONLY the role's own workspace read-write (executor) — so
  sibling workspaces are readable but not writable/deletable. On the bwrap path
  this is `--ro-bind <workspaces-parent>` then `--bind <own-workspace>`; map the
  equivalent for systemd (`ReadOnlyPaths`/`ReadWritePaths`) and seatbelt.
- [x] 2.2 Preserve toolchain writability — the rest of the exposed home stays
  writable (caches/sessions); only the sibling-workspace subtree becomes
  read-only.

## 3. Tests

- [x] 3.1 From inside the sandbox, a `Bash` read of the resolved config path and
  the secrets path both fail; a write to the config path fails (mirror the
  credential-mask scenario at `executor/spec.md:1644`).
- [x] 3.2 From inside the sandbox, READING a sibling workspace's file succeeds, but
  WRITING or deleting a sibling workspace file fails; writing within the role's own
  workspace still succeeds.

## Constraints

- Touch only the denylist policy. Do NOT change strict mode, egress, the capability
  drops, or read-only-role workspace handling.
- Match the surrounding hand-formatting; do NOT run `cargo fmt`.
