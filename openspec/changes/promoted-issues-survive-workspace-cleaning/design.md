## Context

`handle_promote_issue_candidate` (`control_socket/handlers.rs:1329`) writes the issue unit as untracked files into the daemon's workspace; the doc comment states "writing the unit IS the queue." Nothing commits or otherwise persists the unit. Observed failure (coterie, 2026-07-15): promotion at 16:18Z, destroyed at ~16:28Z when the changelog revision path's out-of-scope failure cleanup restored a clean workspace — `git clean` removes untracked files. The maintainer received a success reply and the issue vanished without any error naming it. The same window is open to dirty-tree recovery, `wipe_workspace`, and re-clones, on every repo.

The candidate store (`lanes/ingestion.rs:503`, `<state_dir>/issue-candidates/<id>.json`) already persists the complete drafted unit — `issue_md`, `tasks_md`, `report_body`, slug, origin, repo URL, status — and records are never pruned. Durability requires no new state, only treating that record as authoritative.

## Goals / Non-Goals

**Goals:**
- A maintainer's "send it" guarantees the issue eventually runs (or is visibly parked), regardless of workspace hygiene events in between.
- Units destroyed before this change ships are healed automatically on upgrade.

**Non-Goals:**
- Committing/pushing at promotion time. The promotion handler is a synchronous control-socket action; git work there would race the busy-marker serializer, and in fork-PR mode there is no writable upstream to push loose issue commits to. The issue files reach git exactly as today: committed by the walker as part of the fix PR.
- Protecting arbitrary untracked operator files from workspace cleaning — only the daemon's own queued units.
- A tombstone marker format. Deleting the record file IS the tombstone; no new marker type.

## Decisions

- **Record-as-queue, workspace-as-cache.** Reconciliation (re-materialize missing units from `Promoted` records) runs once per iteration, before `list_ready`. It is a directory scan of `issue-candidates/` filtered by repo URL plus two existence checks per record — trivially cheap at real candidate counts.
- **Archive check is a slug-suffix match over `issues/archive/` entries** (`<UTC-date>-<slug>` or `<UTC-date>-<slug>.md`), the archive layout the lane already writes. In-flight work needs no special case: while the walker works the unit, the unit exists in `issues/`, so reconciliation skips it.
- **Re-materialization reuses `promote_candidate`'s unit-writing** (extracted so both call sites share it), guaranteeing the resurrected unit is byte-identical to the original promotion — same form decision (curated single-file vs public-origin directory), same quarantine handling.
- **WARN on every resurrection.** A disappeared unit means something cleaned the workspace between promotion and pickup; the operator should see that, and on upgrade the WARN log doubles as the audit trail of historically lost issues.
- **No status field beyond `Promoted`.** A "completed" status flip was considered (walker updates the record at archive time) but rejected: the archive directory already encodes completion, and a second writer to the candidate record adds a coupling the archive check makes unnecessary.

## Risks / Trade-offs

- [An operator deletes a unit from the workspace intending to cancel it; reconciliation resurrects it] → Intentional: workspace files are a cache, and cache deletion isn't a durable operator decision. The documented tombstone (delete the record) is one file removal; the resurrection WARN tells the operator where to do it.
- [A slug collision between an old archived issue and a new promotion suppresses re-materialization] → The archive check matches date-prefixed entries ending in the slug; a re-promoted same-slug issue that legitimately needs to run again would be masked by its archived predecessor. Accepted for now: candidate ids are keyed by (repo, source issue), and re-reports of a fixed issue are deduped upstream. The WARN-less skip is visible in the reconciler's debug logging.
- [Candidate store grows unboundedly] → Pre-existing behavior, unchanged by this design; records are small JSON. A pruning policy can be its own change if it ever matters.
