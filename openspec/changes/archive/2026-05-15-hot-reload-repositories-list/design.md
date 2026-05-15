# Design notes

## Identity by URL, not by index

The reload handler diffs the new repository list against the current task set by `url` only. Two consequences:

- Reordering the list in YAML does nothing — the diff sees the same URLs and applies no changes.
- Changing the `url` of an existing entry is treated as "remove old URL, add new URL." The old task is cancelled (workspace at its old derived path stays on disk but is no longer polled), a fresh task is spawned for the new URL (which derives its own workspace path).

Alternative considered: identity by `local_path` (when explicitly set). Rejected because `local_path` is optional and derived — operators don't necessarily think of it as the stable key, and changing the URL while keeping `local_path` the same would create a workspace that points at a different remote, which is the kind of subtle misconfiguration that's harder to debug than just "old workspace + new workspace."

If an operator genuinely wants to relocate a repo (point the same workspace at a new URL), they should do it in two steps: remove the entry, reload, add the new entry with the same `local_path`, reload again. The intermediate state has no polling task for that workspace, which is exactly what "I'm migrating this" should look like.

## Per-task config holder, single-snapshot-per-iteration

Each polling task holds an `Arc<ArcSwap<RepositoryConfig>>`. At the top of every iteration (just before `workspace::ensure_initialized`), the task calls `holder.load()` to grab an `Arc<RepositoryConfig>` for the duration of that iteration. The rest of the iteration uses that snapshot exclusively — `base_branch`, `agent_branch`, `poll_interval_sec`, etc. all come from it.

Why single-snapshot: if the reload handler swapped the config in the middle of an iteration, the daemon could end up reading `base_branch` from the new value but `agent_branch` from the old value, leading to a corrupt state (e.g. push to the new agent branch from a pull off the old base). Single-snapshot eliminates this entire class of race.

The cost: per-iteration the new config takes effect on the *next* iteration, not the current one. For changes that operators care about (token rotation, branch switch), this is fine — it just means "the next poll uses the new values."

## Per-repo cancellation tokens, hierarchically composed with the global token

The daemon used to share a single `CancellationToken` across all polling tasks. With per-repo lifecycle, each task gets its own token derived via `parent.child_token()`. The parent is the global shutdown token (so SIGTERM still cancels everything); the child is per-repo so the reload handler can cancel one task without affecting others.

`tokio_util::sync::CancellationToken::child_token()` produces a token that fires when EITHER the child is explicitly cancelled OR the parent is cancelled. This composes correctly with the existing graceful-shutdown logic — no special-casing needed for "is this a per-repo cancel or a global shutdown?" from the task's perspective.

The daemon's per-repo state map needs to be keyed by URL and accessible from both the reload handler and the shutdown signal handler. It lives behind an `Arc<Mutex<HashMap<String, RepoTaskHandle>>>`. Writes are infrequent (only on reload or task-completion cleanup), so contention is negligible.

## Spawning a new task vs returning early on duplicate spawn

When the reload handler decides "add this URL as a new task," it must check the map under the lock to ensure the URL isn't already mapped (e.g. from a concurrent reload). If it is, treat the second add as a no-op rather than spawning a duplicate task.

The map's lock provides the serialization point. The only writers are the reload handler and the task-completion cleanup hook (which removes its entry when the task exits). Both must hold the lock for the duration of their map mutations.

## What happens when a removed-then-re-added URL straddles a reload

If a repo is removed in reload A, then re-added in reload B before the original task has finished its in-flight iteration:

- Reload A cancels the old task's token. The task continues its current iteration (push + PR if commits were produced), then exits at the inter-poll sleep boundary.
- Reload B sees the URL is not in the current task map (the task may still be running but the map removal happened synchronously when reload A cancelled it... let me restate).

Actually the cleaner semantics: the map removal happens when the TASK EXITS, not when the cancel is triggered. So mid-shutdown of repo X, the map still contains X's entry. Reload B sees X in the map. Treats it as "change in place" — but the in-flight task is already on its way out, so swapping the holder is pointless.

Cleanest fix: the task's exit hook removes itself from the map AND drops its `Arc<ArcSwap<RepositoryConfig>>` strong reference. If reload B happened while the task was shutting down, the held strong reference of the holder is dropped, the task exits, and the next reload-or-poll cycle sees no entry for X. Reload B's "is X in the map?" check just needs to look up the map state at the time of decision and act accordingly:

- X is in the map AND its token is NOT cancelled: hot-swap the holder, mark `applied`.
- X is in the map AND its token IS cancelled: treat as a transient state, no-op on this reload's repository step for X. Next reload (or the next time the user runs `autocoder reload`) sees X gone and can re-add.

Operators reading this and reading the response see "X was in the previous list, X is in the new list, X reported as `unchanged`" — slightly misleading. We log a WARN line on this transient case so operators have a paper trail if they hit it.

In practice this only happens when a removal and an addition land within seconds of each other while a Claude iteration is running. The condition resolves itself within one iteration's duration.

## In-flight iteration safety

A task that has been cancelled must NOT start a new iteration. The cancellation check is in the inter-poll `tokio::select!`:

```rust
tokio::select! {
    biased;
    () = cancel.cancelled() => break,
    () = sleep(Duration::from_secs(iteration_snapshot.poll_interval_sec)) => {}
}
```

The `poll_interval_sec` value used in the sleep is from the iteration snapshot, not from the current swap-holder value. Rationale: if the operator just changed the poll interval, we want this iteration to use the snapshot's interval and the NEXT iteration to use the swap holder's new interval. That's consistent with the single-snapshot rule.

## What is NOT in scope here

- Hot-reload of the executor. Still requires restart. The executor's lifecycle (one shared instance, complex internal state for the sandbox config and templates) doesn't compose well with mid-flight swap; we can revisit if it becomes a pain point.
- A daemon-side mechanism to *batch* reloads (e.g. "stop accepting requests, swap, resume" coordination across signals). Out of scope. Reload requests serialize through the map's mutex naturally.
- Operator-facing visibility into per-task health (when did this task last poll? has it failed recently?). Could be a future `autocoder status` command on the same control socket. Out of scope here.
