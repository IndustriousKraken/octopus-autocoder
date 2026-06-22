# Design

## The marker is a transient trigger-plus-hold, not a record of truth

`.needs-spec-revision.json` is registered in the workspace-local
`.git/info/exclude` — gitignored, never committed, and lost on any re-clone or
wipe. The durable source of truth for "this spec needs revision" is the gate or
preflight that writes it (`preflight_checks.rs`, `queue_walk.rs`,
`spec_archivability.rs`). So the marker is a transient signal that (a) triggers
the operator to revise and (b) holds the change out of `list_pending`.

## Why clearing on a successful revision is safe

A revision is dispatched against an OPEN PR. An open PR already parks the repo, so
the marker's hold is redundant the moment the PR exists. When the revision is
applied (dirty-tree `Completed` → commit + `--force-with-lease` push that updates
the PR), the flagged spec has been revised in that PR. From then on:

- **Merge:** the marker is already gone — the change re-processes against the
  revised base spec, the gate passes, it proceeds. No manual `clear-revision`.
- **Close without merge:** the marker is gone, the park lifts, the change
  re-processes against the still-un-revised spec, and the SAME gate/preflight
  re-flags it → a fresh marker → a fresh revision. It self-heals; nothing is
  stranded un-revised.

The gate is the arbiter at every terminal, which is why clearing the transient
marker early is safe.

## Clear only when a revision was actually applied

The clear fires ONLY on the dirty-tree `Completed` branch (a real change was
committed and pushed). It does NOT fire on:

- a **clean-tree declination** (the agent evaluated the request and made no change
  — per the "Completed with no code change is a reported declination" scenario):
  nothing was revised, so the flagged concern may still stand;
- a **`Failed`** or **precondition-unmet** revision: no revision work landed.

In those cases the marker is retained, and the operator/gate decides.

## ADD, not MODIFY

This is an additive behavior on the revision-execution flow — it does not change
any existing assertion in "Revision execution updates the agent branch and posts a
reply comment", so it ships as a new requirement rather than a MODIFY that would
reproduce that requirement's seven scenarios. The new requirement names the
dirty-tree success branch it hooks into, so the relationship is explicit.

## Implementation note

The clear is a local filesystem delete of
`<workspace>/openspec/changes/<change>/.needs-spec-revision.json`, mirroring the
`ClearRevisionMarker` control-socket action's delete. It is best-effort: a failure
to delete is logged but does not fail the revision (the revision already
succeeded; the marker is non-authoritative). The existing revision behavior —
the agent is instructed not to delete the marker, and the daemon unstages it so it
is never committed — is unchanged; this clear is a separate daemon-side delete
after the push succeeds.
