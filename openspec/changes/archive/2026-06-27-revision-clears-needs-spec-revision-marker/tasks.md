# Tasks

OpenSpec: implements the ADDED requirement in `specs/orchestrator-cli/spec.md`.

## 1. Clear the marker on a successful revision

- [x] 1.1 In the revision dispatcher's `Completed` / dirty-tree branch (the path
  that commits + `git::push_force_with_lease`es to the agent branch and posts the
  `✅ Revision applied:` reply — `autocoder/src/revisions.rs` ~the success arm, and
  the underlying push in `autocoder/src/polling/revision_session.rs:608`), AFTER
  the commit + force-push succeed, delete the change's marker
  `<workspace>/openspec/changes/<change>/.needs-spec-revision.json` if it exists.
- [x] 1.2 Reuse the existing marker-delete logic (the `ClearRevisionMarker`
  control-socket action deletes this exact path at
  `autocoder/src/chatops/operator_commands.rs:3131`; the path is built by the
  helper at `autocoder/src/polling/revision_session.rs:49`). Make it best-effort:
  a delete failure is logged at WARN and does NOT fail the revision (the revision
  already succeeded; the marker is non-authoritative runtime state).
- [x] 1.3 Do NOT clear the marker on the other revision outcomes: the clean-tree
  declination (`Completed`, no code change), a substantive `Failed`, a
  precondition-unmet failure, or `AskUser`. No revision was applied in those cases.
- [x] 1.4 Leave the existing behavior intact: the agent is still instructed not to
  delete the marker, and the daemon still unstages it so it is never committed
  (`revision_session.rs:399`, `:903`). This new clear is a separate daemon-side
  filesystem delete after the push succeeds, not an agent action and not a commit.

## 2. Tests

- [x] 2.1 A dirty-tree `Completed` revision with a `.needs-spec-revision.json`
  present: after the dispatcher processes it, the marker file is gone (assert the
  filesystem state, not message wording). Drive via the existing revision-dispatch
  test seams (no real subprocess / no real push).
- [x] 2.2 A clean-tree declination (`Completed`, no change), a `Failed`, and a
  precondition-unmet revision each LEAVE a present `.needs-spec-revision.json`
  marker in place.
- [x] 2.3 A dirty-tree `Completed` revision with NO marker present is a no-op (no
  error) — the clear is conditional on the marker existing.

## 3. Validation

- [x] 3.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [x] 3.2 `openspec validate revision-clears-needs-spec-revision-marker --strict`
  from the repo root.
