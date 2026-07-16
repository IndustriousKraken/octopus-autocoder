## 1. Enumeration returns exclusions

- [ ] 1.1 In `autocoder/src/lanes/issues.rs`, extend `list_ready` (or add an enriched sibling both callers use) to return excluded units with reasons — locked (lock age) or parked (marked-at, detail) — instead of silently dropping them; log each exclusion at INFO per pass from the walker path.
- [ ] 1.2 Add stale-lock recovery in the same enumeration: a lock whose mtime age exceeds `executor.busy_marker_stale_threshold_secs` is removed with a WARN naming the slug and age, a chatops alert is posted, and the unit is treated as ready on that same pass. Fresh locks exclude as before; park markers are never auto-removed.
- [ ] 1.3 Unit tests: fresh lock excludes and reports `locked` with age; stale lock is removed, alerted, and the unit returns to ready; parked unit reports `parked` with marked-at and survives any age; existing ready-list tests pass unchanged.

## 2. Status surface

- [ ] 2.1 Add issues-lane fields to `RepoStatusResponse` (ready slugs, locked entries with age, parked entries with marked-at/detail) and populate them in `build_repo_status` from the enriched enumeration, honoring the lane's feature gate (fields absent when disabled).
- [ ] 2.2 Render the section in `format_status_reply`: full lists when units exist, a one-liner when the enabled lane is empty, omitted when the lane is disabled; marker-read failures degrade the entry with a WARN, never the reply.
- [ ] 2.3 Unit tests: reply shows a parked issue with reason, a locked issue with age, alphabetical ready order; empty-lane one-liner; disabled-lane omission; unreadable marker degrades gracefully.

## 3. Marker-clear covers both lanes

- [ ] 3.1 In `autocoder/src/control_socket/handlers.rs`, extend the `clear-perma-stuck` sweep (`sweep_marker_clear` / `SweepMarkerKind::PermaStuck`) to enumerate issue-lane park markers in both forms (in-directory and single-file sibling) alongside change markers, labeling each cleared entry with its lane in the response; `clear-revision` enumeration is unchanged.
- [ ] 3.2 Extend the exact-target `clear_perma_stuck_marker` path to fall back to issue units (exact or prefix over issue slugs, honoring the unit's form) when no change matches, with the reply naming the lane.
- [ ] 3.3 Unit tests: a repo whose only park marker is on an issue is swept (not "nothing to clear"), both issue forms clear correctly, an exact-target slug matching only an issue clears it, a changes-lane match still wins when both lanes share a slug, and `clear-revision` never touches issue units.

## 4. Docs and verification

- [ ] 4.1 Update `docs/CHATOPS.md`'s status-reply and clear-perma-stuck documentation (the project-documentation canon requires the status reply's sections and verb behaviors to be documented there).
- [ ] 4.2 Run the full `cargo test` suite; confirm changes-lane status rendering, existing marker-clear tests, and existing issues-lane tests are unchanged.
