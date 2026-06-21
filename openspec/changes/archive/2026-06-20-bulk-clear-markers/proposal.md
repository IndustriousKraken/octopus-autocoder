# Marker-clear commands accept wildcard targets

## Why

`clear-perma-stuck` and `clear-revision` require an exact `<repo> <change>`
pair. In practice an operator never has two stuck markers of the same kind in
one repo at once, and a change to the revision or perma-stuck machinery can leave
a marker on many of the configured repositories — currently eleven. Clearing them
means hunting each marker down and issuing one command per repo per change. That
is busywork the operator should not have to do, especially during the exact
recovery moments these commands exist for.

## What Changes

- A new `orchestrator-cli` requirement adds a wildcard target to both
  marker-clear verbs:
  - `clear-<kind> <repo> *` — clear every marker of that kind in one repo.
  - `clear-<kind> *` — clear every marker of that kind across all repos.
- The chatops-manager "Argument sanitization at parser entry" requirement is
  MODIFIED to carve out `*` as a wildcard sentinel for these two verbs only,
  recognized before the change-slug / repo-substring regex (the canonical rule
  otherwise rejects a non-matching argument and forbids it reaching the control
  socket — so the carve-out must live in that requirement, not be asserted around
  it). Every non-`*` argument is still sanitized as before.
- Bulk clearing is fail-loud: the reply enumerates what was cleared per repo and
  per change, and reports "nothing to clear" explicitly rather than replying
  empty; a per-repo failure is reported alongside the successes without aborting
  the sweep.

## Impact

- Affected specs: `orchestrator-cli` (ADD the wildcard-behavior requirement) AND
  `chatops-manager` (MODIFY "Argument sanitization at parser entry" to carve out
  the `*` sentinel for the marker-clear verbs; all existing scenarios preserved,
  one added). The orchestrator-cli "Chatops operator commands" requirement is
  unchanged. Non-`*` argument sanitization is left intact.
- Affected code: the operator-command parser (recognize `*` for these verbs) and
  the control-socket handlers for `ClearPermaStuckMarker` / `ClearRevisionMarker`
  (resolve `*` to all changes in a repo / all repos, enumerate, report).
- No change to what a single exact clear does. Same threat model — channel write
  access is the trust boundary.
