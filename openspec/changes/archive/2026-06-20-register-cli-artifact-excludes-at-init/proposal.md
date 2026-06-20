# Register per-run CLI artifact excludes at workspace init, not only before commit

## Why

The agentic CLIs drop auto-discovered config files in the workspace root when
they run — most visibly opencode's `opencode.json` (and its `.opencode/`
scratch). `a16` made the daemon register these in `.git/info/exclude` so they
never get committed, but it wired that registration into `git add -A`
(`git::add_all`) — the commit path.

Advisory audits (`WritePolicy::None`) never commit. They run a wrapped CLI,
then the scheduler checks the workspace is clean. Because the exclude is only
registered before staging, an audit that never stages never gets it — so the
CLI's generated `opencode.json` shows as `?? opencode.json`, the
clean-workspace check flags a write-policy violation, the run's findings are
discarded, and the audit re-triggers (cadence state is not advanced). This
hits EVERY audit that launches such a CLI, not one audit type: the file is the
CLI's own startup config, independent of the audit's logic or tool grants.
`.git/info/exclude` is also clone-local and reset by a re-clone, so a workspace
that was protected by an earlier committing pass loses the exclude after
eviction until the next commit — which an audit-first repo may not reach.

## What Changes

- The workspace-manager requirement "Per-run CLI config artifacts are excluded
  from commits" is broadened: the artifacts SHALL be registered in
  `.git/info/exclude` at workspace INITIALIZATION (alongside the daemon's own
  root-level bookkeeping markers), not only before `git add -A`. Registration
  re-applies on every init so a re-cloned workspace is covered before its first
  pass. The pre-staging registration stays as a defensive backstop.
- `workspace::ensure_initialized` registers `WORKSPACE_CLI_ARTIFACT_EXCLUDES`
  (the strategies' own filename list) via the existing
  `git::ensure_local_excludes`, next to where it already registers
  `.failure-state.json`, `.audit-state.json`, etc.

## Impact

- Affected specs: `workspace-manager` (MODIFY the per-run-CLI-artifact-excludes
  requirement — register at init, survive re-clone, cover the audit
  dirty-check).
- Affected code: `workspace.rs` (`ensure_initialized` — one reuse call to
  `git::ensure_local_excludes`). No new mechanism; `ensure_local_excludes`
  already exists, is idempotent, handles the `.opencode/` dir pattern, and
  no-ops on a non-repo workspace. The `git::add_all` call is kept as a
  defensive backstop.
- Fixes the recurring `architecture_advisor`/`drift`/`documentation_audit`
  (and any opencode-strategy audit) `WritePolicy::None` violations on workspaces
  running the opencode CLI, recovering the discarded advisory findings. No
  sandbox or tool-grant change — the sandbox is not the problem.
