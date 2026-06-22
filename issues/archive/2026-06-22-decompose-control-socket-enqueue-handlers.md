# Decompose the control_socket enqueue handlers and dispatch table

## Problem

`autocoder/src/control_socket.rs` (~6,600 lines) has ~10 `handle_queue_*` enqueue
handlers that share a near-identical body — require `url` + `request_id`,
`find_repo`, look up the live `RepoTaskHandle` pending queue, de-dup by
`request_id`, push, and return `{ok, url, request_id, poll_interval_sec}` — and a
`dispatch_request` router that is one ever-growing `match action.as_str()`. This is
a maintainability signal (a "junk drawer" of duplicated arms), not a defect.

## Desired end state

The enqueue handlers share ONE helper (each supplies only its queue selector and
its request-record constructor); the action router is table-driven (action string
→ handler) so adding an action no longer extends a giant match; and the handler
bodies move behind that table into a submodule, bringing the file under the size
budget. Control-socket request/response JSON is byte-identical; `dispatch_request`
and `ControlState` stay reachable at their current paths.

## Tasks

- [x] Factor the shared enqueue body out of the `handle_queue_*` handlers
  (`handle_queue_proposal_request`, `_changelog_`, `_brownfield_`, `_scout_`,
  `_spec_it_`, `_sync_upstream_`, `_brownfield_survey_`, `_brownfield_batch_`, and
  the matching clear/revision handlers) into one generic helper; each handler then
  supplies only its queue selector and request-record constructor. Re-locate via
  the function NAMES — line numbers have drifted.
- [x] Replace the hand-maintained `match action.as_str()` arm list in
  `dispatch_request` with a table-driven dispatch (action → handler).
- [x] Move the handler bodies behind the table into a submodule (e.g. a
  `control_socket/` directory module or `control_socket/handlers.rs`), keeping
  `dispatch_request` and `ControlState` at their current paths.
- [x] Verify: `cargo build` and the existing suite pass; control-socket JSON
  responses unchanged.

## Constraints (behavior-preserving refactor)

- No observable contract change — control-socket request/response JSON stays
  identical. This is reorganization, not a feature change. No spec delta.
- Keep public call sites compiling by re-exporting moved items (`pub(crate) use`)
  from their original module path.
- Moved unit tests go to a sibling test module, not a fresh inline
  `#[cfg(test)] mod tests` in the new file.
- Match the surrounding hand-formatting; do NOT run `cargo fmt` (this crate is
  intentionally not rustfmt-clean).
- Do not author or restate any size/duplication threshold as a spec requirement —
  the line counts are audit selectors, not contracts.
- Verify against a reliably-green test suite — a behavior-preserving refactor
  checked by a flaky suite proves nothing.
