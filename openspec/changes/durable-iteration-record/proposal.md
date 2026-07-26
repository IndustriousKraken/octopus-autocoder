# durable-iteration-record

## Why

The status reply's `last iteration:` block is fabricated. No per-iteration record exists, so the status handler renders the newest entry of the per-change failure-counter store as if it were the last iteration — its own comment admits it ("no central record exists yet; the field is populated from the most recent failure-state timestamp"). Successful iterations never touch that store, and an entry whose change completed outside the server's own queue walk (implemented elsewhere and pushed) is never cleared. The observed result on production: a daemon that merged two PRs six hours earlier reported `last iteration: finished: 26d ago` with a long-fixed June error as its "outcome", plus a permanently-"due" next-iteration estimate. The operator gets anti-information: the block actively misleads about daemon health, on exactly the surface meant to answer "is this thing running?"

## What Changes

- Every polling iteration — success, idle, skipped, audit-only, or failed — writes a small durable iteration record (state-dir, atomic overwrite) at its end: finished time, outcome kind, one-line summary.
- The status surfaces source their last-iteration data exclusively from that record; failure-counter residue never populates any last-iteration surface again. With the record written on every iteration including idle ones, a stale `finished:` age becomes a TRUE signal that the polling task has stopped — the diagnostic the block always pretended to be.
- Orphaned failure-state entries — ones naming a change that no longer exists in the workspace — are pruned at pass start, so they cannot linger indefinitely (they currently survive forever when a change completes outside the server's queue walk).

## Impact

- Affected specs: `orchestrator-cli` (two new requirements), `chatops-manager` (Status reply always shows live workspace snapshot)
- Affected code: polling iteration driver (write the record), `autocoder/src/control_socket/handlers.rs` (read it; drop the failure-state shim), `autocoder/src/failure_state.rs` + pass startup (prune orphans), a small record module + `autocoder/src/paths.rs`
