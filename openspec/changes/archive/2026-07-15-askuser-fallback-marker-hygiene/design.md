## Context

`mcp_askuser_server.rs:109-117` computes the ask-user fallback marker path as `<workspace>/openspec/changes/<ORCH_MCP_CHANGE>/.askuser-pending.json`, creating the directory as needed. Gate/audit sessions pass role names as the change key, so a fallback fabricates a phantom change (`openspec list` showed `canon_contradiction_check` — "No tasks"). The marker is a write-only breadcrumb: nothing in the codebase reads it (only `mcp_askuser_server.rs` references the name), so relocation is behaviorally safe. Nor is it excluded from git anywhere: the daemon's `.git/info/exclude` list (`workspace.rs:157-216`) doesn't cover it, and `verify` registers no excludes at all — which is how a marker written during a local verify run was committed by a broad `git add`, merged to master, and resurfaced in another clone.

The fix must serve two very different environments equally: the **server** (daemon, sandboxed agents, deep workspace paths under `<cache>/workspaces/`, workspace-init hygiene machinery) and a **spec box** (partial check-only install, the repo IS the operator's own clone at an arbitrary path, only `verify` ever runs, no daemon).

## Goals / Non-Goals

**Goals:**
- No fallback marker ever fabricates an `openspec/changes/` entry, in either environment.
- No fallback marker is ever committable, in either environment — including after an interrupted run.
- `verify` leaves the operator's clone exactly as it found it.

**Non-Goals:**
- Making the fallback marker *readable* by anything (it stays a breadcrumb; the gate's fail-closed verdict is the real signal).
- Changing the primary ask_user path (control-socket relay to chatops) or the in-change-dir marker for real implementer sessions.
- Cleaning up historical markers already committed to repos (one-off operator cleanup).

## Decisions

- **Placement key: "does the change directory already exist?"** — not an allowlist of role names. Real change sessions have a directory (created by the queue/authoring flow); role sessions never do. An existence check needs no registry of roles, cannot drift when new gate roles are added, and behaves identically on server and spec box because it only touches the workspace tree. The fallback path (`<workspace>/.askuser-pending-<key>.json`) requires no `mkdir` at all.
- **Sanitize the key to `[A-Za-z0-9_-]`.** The key comes from an env var; filename-joining it unsanitized would let a hostile or malformed value traverse paths. One character-class filter, applied only to the root-marker filename.
- **Hygiene rides the existing exclude machinery.** The daemon side adds one pattern (`.askuser-pending*`) to the already-spec'd per-run-artifact exclusion (workspace-manager requirement) — same registration points (workspace init + pre-staging), same idempotence. Bare `.askuser-pending*` matches both the in-dir and root marker names at any depth, per the same git-pattern reasoning as the existing `*.perma-stuck.json` suffix entries.
- **`verify` gets both halves: register at start, clean at exit.** Cleanup on exit satisfies the existing "transient artifacts" contract; registration at start covers the interrupted-run window where cleanup never executes. Registration reuses `workspace::ensure_git_info_excluded` (idempotent, local-only). This is the spec-box story: the operator's clone gains only local exclude lines, never tracked changes.
- **Sandbox compatibility needs no special-casing.** The MCP child runs inside the agent sandbox, which always grants workspace-root writes (that is where agents work, in both environments); `.git/info/exclude` writes happen from the daemon/verify process outside the sandbox at setup/teardown time.

## Risks / Trade-offs

- [A repo that deliberately tracks a file named like a marker] → The exclude affects untracked files only (existing, spec'd property of `.git/info/exclude`); tracked files keep committing normally.
- [Root markers accumulate if runs keep failing and cleanup keeps being skipped] → They're excluded from git and tiny; the next completed verify run's cleanup sweeps `.askuser-pending*` at the root. Acceptable residue.
- [Existing tooling expecting the old phantom-dir path] → None exists; the marker has no reader in the codebase, and phantom dirs were a bug, not an interface.
