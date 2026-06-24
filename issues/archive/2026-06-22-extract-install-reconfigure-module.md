# Extract the install --reconfigure subsystem into its own module

## Problem

`autocoder/src/cli/install.rs` (~4,300 lines) embeds the entire `--reconfigure`
subsystem inside the install module. The reconfigure subsystem is a self-contained
unit with its own section plumbing. This is a maintainability signal, not a defect.

## Desired end state

The `--reconfigure` subsystem lives in a new `autocoder/src/cli/reconfigure.rs`
module, registered in `autocoder/src/cli/mod.rs`; `install.rs`'s `execute_inner`
call site calls into it. The `--reconfigure <audits|reviewer|chatops>` flag, its
three-section allowlist, and all printed guidance stay byte-for-byte unchanged.

## Tasks

- [x] Move the `--reconfigure` subsystem (`resolve_existing_config_path`,
  `execute_reconfigure`, `section_label`, `print_restart_guidance`,
  `reconfigure_audits`, `reconfigure_reviewer`, `reconfigure_chatops`,
  `apply_in_place_patch`, `prior_file_mode`, and the `ReconfigureSection` plumbing)
  into a new `autocoder/src/cli/reconfigure.rs`; register it in
  `autocoder/src/cli/mod.rs`. Re-locate via the SYMBOL names — line numbers have
  drifted.
- [x] Update the `execute_inner` call site in `install.rs` to call into the new
  module. The `--reconfigure` flag, its three-section allowlist, and all printed
  guidance must stay byte-for-byte unchanged.
- [x] Verify: `cargo build` and the existing suite pass; the `--reconfigure` CLI
  surface and guidance output are unchanged.

## Constraints (behavior-preserving refactor)

- No observable contract change — the `--reconfigure` CLI surface and printed
  guidance stay identical. This is reorganization, not a feature change. No spec
  delta.
- Keep public call sites compiling by re-exporting moved items (`pub(crate) use`)
  from their original module path.
- Moved unit tests go to a sibling test module, not a fresh inline
  `#[cfg(test)] mod tests` in the new file.
- Match the surrounding hand-formatting; do NOT run `cargo fmt` (this crate is
  intentionally not rustfmt-clean).
- Do not author or restate any size threshold as a spec requirement — the line
  counts are audit selectors, not contracts.
- Verify against a reliably-green test suite — a behavior-preserving refactor
  checked by a flaky suite proves nothing.
