# Extract the operator_commands `send it` cascade into its own module

## Problem

`autocoder/src/chatops/operator_commands.rs` (~11,700 lines) holds the
four-context `send it` cascade and its audit/survey state-machine logic inline.
The file is the largest in the crate; the cascade is a self-contained unit that
does not need to live in the catch-all operator-commands module. This is a
maintainability signal, not a defect.

## Desired end state

The `send it` cascade and its state-machine logic live in their own module, out of
`operator_commands.rs`. The canon-mandated context resolution order is preserved
EXACTLY: audit → survey → issue-candidate → revision → canonical refusal. Chatops
reply text and `send it` context ordering are identical.

## Tasks

- [ ] Extract the `send it` cascade and its audit/survey state-machine logic
  (`dispatch_send_it_on_audit`, `try_send_it_on_survey`,
  `try_send_it_on_issue_candidate`, `try_send_it_on_revision`, and the audit-thread
  lookup) into its own module, out of `operator_commands.rs`. Re-locate via the
  function NAMES — line numbers have drifted.
- [ ] Preserve the context resolution order exactly: audit → survey →
  issue-candidate → revision → canonical refusal. The extraction must not reorder
  or drop any context.
- [ ] Verify: `cargo build` and the existing suite pass; chatops replies and the
  `send it` ordering are unchanged.

## Constraints (behavior-preserving refactor)

- No observable contract change — chatops reply text and context ordering stay
  identical. This is reorganization, not a feature change. No spec delta.
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
