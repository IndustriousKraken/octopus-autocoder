## Why

The companion `autocoder-uses-openspec-archive` change switches the per-iteration archive step to invoke `openspec archive` (so canonical specs stay in sync going forward, assuming the host has openspec sync configured). It does NOT address pre-existing drift in repos that have been autocoder-managed since before that fix landed. This autocoder repo has ~30 unsynced requirements accumulated over the past few weeks; myrepo likely has a similar gap; the operator's repos (whenever he onboarded) too; any future repo onboarded from a host where openspec sync wasn't enabled at the time of past archives will arrive with drift.

Investigation on 2026-05-24 surfaced an important property: incremental backfill (re-running `openspec archive` only on changes whose deltas aren't yet in canonical) is **fundamentally unreliable when drift is mid-history** rather than end-of-history. If change B never synced but changes C and D did, the canonical state was built from A + C + D in that order. Re-archiving B today applies B's deltas on top of canonical-with-C-and-D-already-merged — but the chronologically-correct merge would have B between A and C. Subsequent MODIFIED requirements in C may have been built assuming B's contribution existed. The incremental result is silently incorrect.

The only safe answer for any non-trivial drift is **full rebuild from scratch**: clear canonical specs, iterate every archived change in chronological order, re-archive each so openspec applies its deltas in the correct order. The result is canonical specs that exactly equal what they would be if every archive had synced correctly the first time.

The cost of full rebuild is low — no LLM calls, just file I/O and openspec invocations. The complexity of clever-incremental-backfill (skip-already-synced, partial-history reasoning, conflict resolution) buys nothing reliable. Drop the cleverness; ship the safe thing.

A secondary practical concern: when the rebuild runs on a live daemon-managed repo, it must coordinate with the polling loop so the rebuild's commits don't collide with normal queue work. Two options: schedule the rebuild for the next iteration boundary (safe; default), or interrupt the current iteration and run immediately (faster but disruptive; opt-in via `--immediate`).

For operator convenience, expose the rebuild through both the autocoder CLI (operator SSHes to the daemon host, or runs against a local clone) AND a chatops verb (matches the existing operator-commands pattern). The chatops verb only supports the safe scheduled mode; `--immediate` requires SSH access by design (you should not be remotely killing executor invocations from chat).

## What Changes

**1. New CLI subcommand: `autocoder sync-specs --rebuild --workspace <path>`.**

- Rebuilds ALL canonical specs in the workspace from archive history. No `--capability` flag — keep it simple, rebuild everything.
- Flow:
  1. Enumerate `<workspace>/openspec/changes/archive/*` directories. Sort by name (chronological because of the YYYY-MM-DD prefix).
  2. `rm -rf <workspace>/openspec/specs/*/` (clear every existing canonical spec; the archives are the source of truth).
  3. For each archived change in chronological order:
     - Record the original dated directory name (e.g. `2026-05-15-foo-bar`).
     - `mv archive/<original-name> changes/<slug>` (strip the date prefix to get the slug).
     - `openspec archive <slug> -y` (openspec rebuilds canonical from this change's deltas, possibly creating new capability files with placeholder Purpose).
     - On success: openspec produced a new dated dir `archive/<today>-<slug>`. Rename in-place: `mv archive/<today>-<slug> archive/<original-name>` to preserve the original date.
     - On failure (openspec exits non-zero): log ERROR with the openspec stderr, leave the change at the active path, continue to the next change. Subsequent changes may also fail if they depend on the failed one; that's expected — the operator inspects the report at the end.
  4. Print a summary: total changes processed, successful, failed, with a list of failure reasons.
  5. Exit 0 if all changes succeeded, non-zero if any failed.
- Implementation in `autocoder/src/cli/sync_specs.rs`. Pure orchestration — no merge logic, no openspec internals. ~200 lines including the in-place rename machinery and the summary report.

**2. `--immediate` flag** (CLI only; not chatops).

- Without `--immediate`: when run against a workspace under daemon management, the rebuild waits for the current iteration to finish before starting (operator's CLI invocation blocks, polling progress logged so the operator can see what's happening). Once the iteration releases the busy marker, rebuild acquires it and proceeds.
- With `--immediate`: forcibly cancel the current iteration first. SIGTERM the executor subprocess via the busy marker's recorded pid, wait up to 30 seconds for cleanup, then run rebuild. The cancelled iteration's partial workspace state gets cleaned by the existing dirty-workspace recovery on the rebuild's first git-status check.
- Outside daemon context (local clone, no daemon running): `--immediate` is a no-op — there's no iteration to interrupt.

**3. Control-socket action: `RebuildSpecs { url, immediate: bool }`**.

- Resolves the workspace path from `url` (via the configured `repositories[]` map).
- If `immediate`: same as the CLI's `--immediate` path.
- Otherwise: sets a `pending_rebuild` flag on the per-repo polling task's in-memory state.
- Polling loop checks this flag at iteration start; if set, runs the rebuild instead of the normal queue walk for that iteration. The rebuild's commits land on agent-q via the existing iteration scaffolding (busy marker, git add/commit/push, PR creation).
- The flag is cleared once the rebuild iteration starts (so a second `RebuildSpecs` arriving mid-rebuild waits for the NEXT iteration after this one).

**4. Chatops verb: `@autocoder rebuild-specs <repo-substring>`.**

- Parses via the existing operator-commands infrastructure (from the `chatops-operator-commands` change).
- Resolves repo by substring against `repositories[].url`.
- Submits `RebuildSpecs { url, immediate: false }` to the control socket.
- Replies in-channel: `✓ rebuild scheduled for <repo> — will run after current iteration finishes`.
- The eventual PR-opened notification fires through the existing flow when the rebuild iteration completes.

**Chatops does NOT support `--immediate`.** Killing a running executor mid-iteration is a foot-loaded gun; chat-driven invocation makes it too easy to fire accidentally. Operators wanting `--immediate` SSH to the daemon host and run the CLI directly.

**5. Polling-loop coordination.**

- Per-repo in-memory state gains a `pending_rebuild: bool` field.
- At iteration start: check `pending_rebuild`. If true: clear it, mark this iteration as a rebuild iteration, run rebuild logic instead of the normal queue walk + executor flow. The rebuild produces commits via the existing git wrappers; push + PR creation use the existing iteration plumbing.
- Concurrency: only the polling task itself reads/writes its `pending_rebuild` field, so no synchronization is needed beyond the existing iteration loop's natural serialization.
- The rebuild iteration's PR has a recognizable title (e.g. `spec rebuild: <N> capability(ies) rebuilt from archive history`) so operators can distinguish rebuild PRs from normal queue-work PRs at a glance.

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement: "Rebuild canonical specs from archive."
- Affected code:
  - `autocoder/src/cli/sync_specs.rs` — NEW. CLI subcommand + the rebuild orchestration.
  - `autocoder/src/main.rs` — `SyncSpecs(SyncSpecsArgs)` clap variant added.
  - `autocoder/src/control_socket.rs` — new `RebuildSpecs` action variant + handler. Handler sets the pending-rebuild flag on the named repo's polling task (or signals + waits for immediate mode).
  - `autocoder/src/polling_loop.rs` — per-repo state gains `pending_rebuild: bool`; iteration start checks the flag and branches to rebuild logic; rebuild logic shells out to the same orchestration code as the CLI subcommand (so logic is shared, not duplicated).
  - `autocoder/src/chatops/operator_commands.rs` — new `OperatorCommand::RebuildSpecs { repo_substring }` variant + parser support.
  - Each chatops backend's incoming-message handler — recognize the new verb (handled centrally via the parser, so individual backends inherit it automatically).
  - README — new subsection under "Operating Notes" titled "Rebuilding canonical specs" describing when to use rebuild (drift detected, repo onboarded with pre-existing drift, etc.), the CLI invocation, the chatops verb, the `--immediate` caveat.
- Operator-visible behavior:
  - New CLI subcommand `autocoder sync-specs --rebuild --workspace <path>` for operator local use.
  - New chatops verb `@autocoder rebuild-specs <repo>` for in-channel triggering.
  - Rebuild PRs land on the agent branch with a distinctive title; operator reviews + merges like any other autocoder PR.
- Breaking: no.
- Acceptance: `cargo test` passes (new tests). `openspec validate rebuild-canonical-specs-from-archive --strict` passes. Manual: run the rebuild against this autocoder repo (which has ~30 unsynced requirements). Expect: all canonical spec files regenerated, the archive directory unchanged (date prefixes preserved via in-place rename), `git diff openspec/specs/` shows the merged requirements, every archived change appears in chronological order in canonical history.
