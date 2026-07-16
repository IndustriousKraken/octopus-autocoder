## Why

Promoting an issue candidate writes `issues/<slug>/` (or `<slug>.md`) as loose untracked files into the daemon's mutable workspace — "writing the unit IS the queue." Any workspace-cleaning path that runs before the issues lane picks the unit up silently destroys it: dirty-tree recovery, `wipe_workspace`, a re-clone, or (observed in production) the changelog revision path's "leave the workspace clean" failure cleanup, which deleted a maintainer-promoted issue ten minutes after promotion while replying only about the changelog. The maintainer saw "✓ Promoted … AND queued" and then nothing, ever, with no error. Queue durability currently depends on nothing cleaning the workspace in the window — luck, not design.

## What Changes

- The promoted candidate record (already persisted at `<state_dir>/issue-candidates/<id>.json` with the full drafted `issue_md`/`tasks_md`/`report_body`) becomes the durable queue entry; the workspace unit is its materialization.
- Each polling iteration reconciles before lane enumeration: a `Promoted` candidate whose unit is absent from both `issues/` (either form) and `issues/archive/` is re-materialized from the record, with a WARN naming the resurrection — so a destroyed unit costs one iteration, not the issue.
- An archived unit (fix landed) is never re-materialized; deleting the candidate record is the operator's way to permanently retire a promoted-but-unwanted issue.
- Retroactive healing: on first run after upgrade, existing `Promoted` records whose units were destroyed in the past are re-materialized automatically — no re-ingestion needed.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "Hybrid issue ingestion with maintainer promotion" requirement gains the durable-record/reconciliation contract.

## Impact

- `autocoder/src/lanes/ingestion.rs`: a reconcile function (scan `Promoted` candidates for a repo, check unit presence in `issues/` and `issues/archive/`, re-run the existing unit-write on absence).
- `autocoder/src/polling_loop/commits.rs` (or the iteration pre-lane step): call the reconciler before `list_ready`.
- No state-shape change: `CandidateState` already carries everything needed; no migration.
