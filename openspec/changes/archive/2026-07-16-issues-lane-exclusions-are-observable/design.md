## Context

`lanes/issues.rs::list_ready` (`issues.rs:395-404`) excludes locked and parked units with bare `continue` — no log, no counter, no status surface. The markers live only in the server workspace and are `.git/info/exclude`d, so an operator's clone shows nothing. `RepoStatusResponse` (`chatops/operator_commands.rs:1721`) carries pending/waiting/perma-stuck fields for the changes lane only. Production evidence: `fix-setup-swallows-admin-activation-error` on coterie sat excluded for ~3 weeks while alphabetically-later issues (`rate-limit…`, `signup…`) ran — proving exclusion, with no way to see the reason remotely. Parking alerts once at park time (per the parking requirement's fail-loud clause) and never again; a stale `.in-progress` lock never alerts at all and has no recovery path, unlike the repo-level busy marker which has stale/dead-PID recovery.

## Goals / Non-Goals

**Goals:**
- Excluded-issue state is visible in the journal every pass and in the chat `status` reply on demand.
- A crash-leftover `.in-progress` lock recovers automatically after the existing staleness threshold.

**Non-Goals:**
- Auto-unparking `.perma-stuck.json` (operator-owned by spec; only visibility changes).
- New configuration — staleness reuses `executor.busy_marker_stale_threshold_secs`.
- Changing selection semantics; ready/locked/parked behavior is unchanged except stale-lock removal.

## Decisions

- **Log at INFO once per excluded unit per pass.** Exclusions are rare (a handful of units, ~6 passes/hour), so the journal cost is a few lines per hour per excluded issue — cheap for the diagnostic value. State-tracking to log only on transitions was rejected as bookkeeping for a non-problem.
- **Stale-lock threshold = the busy-marker threshold.** The `.in-progress` lock is the per-unit analog of the per-repo busy marker; any age past one full iteration-plus-session is a leftover. Reusing the threshold means one staleness concept, no new knob. Recovery removes the lock and alerts — mirroring busy-marker stale recovery's shape (WARN + chatops).
- **`list_ready` returns exclusions instead of a second walk.** The enumeration already visits every unit and knows each reason; returning `(ready, excluded)` (or an enriched entry list) lets both the walker's logging and `build_repo_status` consume one source. `repo_status` must not re-implement the skip logic — that is how the changes lane's status stays truthful (`queue::list_marker_excluded` pattern).
- **Status section mirrors the changes-lane rendering.** Same marked-at/detail annotations operators already read for perma-stuck changes; issues add lock age. Omit-when-disabled keeps replies for lane-off repos unchanged.
- **`clear-perma-stuck` learns the second lane; `clear-revision` doesn't.** Production evidence (coterie, 2026-07-15): `clear-perma-stuck coterie *` replied "nothing to clear" while `issues/fix-setup-swallows-admin-activation-error/.perma-stuck.json` sat on disk — `SweepMarkerKind::PermaStuck.list_marked` enumerates `openspec/changes/` only, and the exact-target path resolves slugs via the changes-scoped `resolve_change_prefix`. The sweep gains the issues enumeration (both unit forms); the exact-target path falls back to issue slugs only when no change matches, so existing changes-lane behavior is byte-identical and a rare cross-lane slug collision deterministically favors the changes lane (the wildcard clears both anyway). `clear-revision` is untouched because `.needs-spec-revision.json` cannot exist for a unit that carries no spec delta.

## Risks / Trade-offs

- [Removing a stale lock while the unit is genuinely still being worked] → The threshold is the same one that governs busy-marker recovery, which already bounds how long legitimate work can hold markers; a session outliving it has already tripped repo-level recovery. Same trade-off, already accepted project-wide.
- [Reply length growth on repos with many issues] → Ready list is slugs-only; locked/parked lists are as long as the problem they describe. The empty case is a one-liner.
