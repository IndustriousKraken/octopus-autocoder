## Why

autocoder's per-iteration archive step uses `std::fs::rename` in Rust and never invokes `openspec archive`. Every archive operation bypasses openspec entirely, including the canonical-spec merge step openspec performs as part of its archive command. Over time this produces drift between the requirements documented in archived changes' delta files and the requirements present in the canonical `openspec/specs/<capability>/spec.md` files: as of this change, 89 unique `### Requirement:` titles appear in archived `## ADDED Requirements` blocks across this repository, but only 59 are present in canonical specs. The 30-requirement gap includes the entire periodic-audit framework, the install subcommand, the release pipeline, perma-stuck detection, throttled failure alerts, and other significant work.

A previous change (`archived-spec-sync-audit`) attempted to solve this by implementing a delta-merge in Rust and applying it via an opt-in periodic audit. That approach added ~1000 lines of code that duplicated functionality the openspec CLI already provides. The correct fix is to delegate the archive step (including its canonical-spec merge) to `openspec archive` itself, removing the need for autocoder to maintain its own merge logic.

`openspec archive` performs both the file move AND the canonical-spec merge in a single operation when the host's openspec profile has the `sync` workflow enabled (via `openspec config profile`). The merge is byte-reasonable (with minor blank-line normalization), aborts atomically on validation errors (so the repo is never left in a half-applied state), and creates missing canonical capabilities with a placeholder Purpose when needed.

## What Changes

**1. Replace `queue::archive`'s rename with an `openspec archive` subprocess call.** The polling loop's flow becomes: executor returns `Completed` → autocoder commits the working-tree changes → autocoder runs `openspec archive <change> -y` in the workspace → on success the change is both moved AND synced. Error handling: if openspec archive aborts (validation error in the rebuilt spec), autocoder treats the change as Failed for the iteration with the openspec stderr as the reason. The `--yes` flag skips the confirmation prompt that would otherwise block non-interactive use.

**2. Delete the spec-sync audit and merge module.** Specifically:
- Remove `autocoder/src/spec_sync.rs` (the merge primitives — 500+ lines).
- Remove `autocoder/src/audits/spec_sync.rs` (the audit wrapper).
- Remove `pub mod spec_sync;` from `autocoder/src/audits/mod.rs`.
- Remove `SpecSyncAudit` registration in `autocoder/src/cli/run.rs`.
- Remove `spec_sync_audit` from the `validate_audit_type_names` recognized-slugs list.
- Remove `WritePolicy::CanonicalSpecMerge` variant (no other audit uses it; keeps the WritePolicy surface narrow).
- Remove the audit's README table row + `config.example.yaml` entries.
- Replace the README's openspec-config section with a neutral setup-prerequisite note: "the autocoder host needs the openspec `sync` workflow enabled (one-time `openspec config profile`). Without it, `openspec archive` will move the change directory but won't merge deltas into canonical specs — autocoder iterations will succeed but drift will accumulate."

**3. Install path documents the openspec-sync prerequisite.** The install script's existing optional steps (system deps, Claude CLI) gain a new step:

> "After installing the openspec CLI, autocoder needs the `sync` workflow enabled in your openspec profile so `openspec archive` does the canonical-spec merge. Run `openspec config profile` once on this host to enable it."

Optional automation: pipe predetermined answers to `openspec config profile`'s TUI. Fragile (breaks if openspec changes the prompts), so defer to manual operator step unless openspec exposes a `--workflows` non-interactive flag in the future.

## Impact

- Affected specs: `orchestrator-cli` — one REMOVED requirement ("Archived-spec-sync audit") + one ADDED requirement ("autocoder invokes openspec archive"). The REMOVED entry rolls back the requirement added by the previous `archived-spec-sync-audit` change.
- Affected code:
  - `autocoder/src/queue.rs` — `archive()` function changes from `fs::rename` to subprocess invocation of `openspec archive`. The collision-check (`archive_collision_path`, `would_collide_on_archive`) stays — it's a pre-flight that prevents wasted executor runs on conflicting dates, which still applies.
  - `autocoder/src/polling_loop.rs` — archive call site unchanged at the surface (still calls `queue::archive(...)`) but error handling adapts to subprocess failures.
  - `autocoder/src/spec_sync.rs` — DELETED.
  - `autocoder/src/audits/spec_sync.rs` — DELETED.
  - `autocoder/src/audits/mod.rs` — `pub mod spec_sync;` line removed.
  - `autocoder/src/cli/run.rs` — `SpecSyncAudit` registration removed.
  - `autocoder/src/config.rs` — `spec_sync_audit` removed from `validate_audit_type_names`'s known list.
  - `autocoder/src/audits/scheduler.rs` — `WritePolicy::CanonicalSpecMerge` variant removed.
  - README — openspec-config section rewritten as a neutral prerequisite. Audit table row dropped.
  - `config.example.yaml` — `spec_sync_audit` entries dropped.
- Operator-visible behavior:
  - autocoder hosts need `openspec config profile` to have `sync` enabled. The install path documents this. On a host without sync configured, autocoder iterations will succeed at the file-move level but won't sync canonical specs.
  - Operators who had configured `spec_sync_audit: daily` will get a startup error from `validate_audit_type_names` with the now-missing slug. The fix is to remove the entry; the released audit is gone.
- Backfill of existing drift is a separate concern handled by the companion `rebuild-canonical-specs-from-archive` change. This change is scoped to "stop creating new drift" only.
- Breaking: minor. The `spec_sync_audit` slug is removed from the recognized list. Operators with it configured need to remove their config entry.

## Acceptance

- `cargo test` passes (with the audit's tests deleted).
- `openspec validate autocoder-uses-openspec-archive --strict` passes.
- Manual: running an autocoder iteration in a test workspace produces a change directory at `openspec/changes/archive/<date>-<slug>` AND updates the corresponding canonical spec(s) — both in one openspec archive subprocess call.
