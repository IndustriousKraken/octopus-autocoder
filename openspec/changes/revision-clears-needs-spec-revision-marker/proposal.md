# A successful revision clears its needs-spec-revision marker

## Why

When a gate, preflight, or review contradiction flags a change, autocoder writes
a `.needs-spec-revision.json` marker that HOLDS the change. When the operator then
runs an `@<bot> revise` and the daemon applies it to the open PR, the spec in need
of revision has been revised — but the marker persists until the operator
remembers to run `clear-revision`. Operators routinely forget, because the
situation already feels resolved (the revision happened, and the open PR already
parks the repo), so the change sits held behind a now-redundant marker.

The marker is transient runtime state — it lives in `.git/info/exclude`, is never
committed, and is lost on any re-clone; the durable source of truth for "this spec
needs revision" is the gate/preflight that writes it. Once a revision is applied
to an open PR, the open-PR park owns the hold, so the marker's blocking role is
redundant. Clearing it automatically on a successful revision removes the toil
without weakening the guard.

## What Changes

- When the revision dispatcher applies a revision to an open PR with the
  dirty-tree `Completed` outcome (a real change committed and force-pushed to the
  agent branch), the daemon ALSO clears that change's local
  `.needs-spec-revision.json` marker, if present.
- The marker is NOT cleared on a clean-tree declination (the agent verified the
  request and made no change) or a failed/precondition-unmet revision — no
  revision was applied, so a flagged concern may still stand.
- The operator `clear-revision` verb is unchanged; it remains for markers that
  never reach a revision (e.g. an operator-must-edit `SpecNeedsRevision` flag) and
  as a manual override.

## Impact

- Affected capability: `orchestrator-cli` (the revision-execution flow).
- Affected code: the revision dispatcher (`autocoder/src/revisions.rs` /
  `autocoder/src/polling/revision_session.rs`) — on the dirty-tree `Completed`
  success path, after the commit + `--force-with-lease` push succeeds, delete
  `<workspace>/openspec/changes/<change>/.needs-spec-revision.json` if it exists
  (best-effort; mirror the existing `ClearRevisionMarker` delete).
- Safe under close-without-merge: if the operator later closes the revision PR
  without merging, the gate/preflight re-flags the still-un-revised spec on the
  next pass, re-writing the marker — so clearing on revision cannot strand an
  un-revised change.
