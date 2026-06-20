## MODIFIED Requirements

### Requirement: Per-run CLI config artifacts are excluded from commits
The agentic CLI strategies write per-run, server-specific config files into the workspace ROOT that the wrapped CLI auto-discovers from the project directory (claude's `.mcp.json`, opencode's `opencode.json` plus its `.opencode/` project scratch, agy's `mcp_config.json`). Unlike the claude settings file — which lives under `.git/` where git never stages it — these cannot move out of the working tree, because the CLI only discovers them in the working directory. The daemon SHALL ensure they are NEVER committed AND never read as an unexpected working-tree change: it SHALL register these artifacts in the workspace's `.git/info/exclude` at workspace INITIALIZATION (the same place the daemon's own root-level bookkeeping markers are registered), AND, defensively, before staging the working tree (`git add -A`).

Registration at initialization is required because some passes inspect the working tree WITHOUT ever staging or committing. In particular, an advisory audit (`WritePolicy::None`) runs a wrapped CLI — which drops its auto-discovered config — and is then checked for a clean workspace; it never reaches `git add -A`, so an exclude registered only before staging would not cover it, and the generated config would be misread as a disallowed write. Registering at init places the excludes before ANY pre-pass or post-run dirty check, for every lane AND every audit.

`.git/info/exclude` is a LOCAL exclude — it is never itself committed (no `.gitignore` change appears in the repository) — AND it affects only UNTRACKED files. Therefore a repository that legitimately TRACKS one of these files is unaffected: its tracked copy continues to stage and commit normally, and only autocoder's generated (untracked) copy is skipped. Because the local exclude is reset by a re-clone, registration SHALL re-apply on every workspace initialization so a freshly (re-)cloned workspace is covered before its first pass. The artifact list SHALL derive from the strategies' own filename definitions so it cannot drift from what they write. This extends the `a16` principle — daemon bookkeeping never lives in the managed repository tree — to the agentic CLIs' auto-discovered configs.

#### Scenario: A generated CLI config is not committed
- **WHEN** a workspace contains an UNTRACKED per-run CLI config (e.g. `opencode.json`, `.mcp.json`, an `.opencode/` scratch dir, or `mcp_config.json`) alongside the change's actual files
- **AND** the daemon stages and commits the working tree
- **THEN** the change's files are committed
- **AND** none of the per-run CLI config artifacts are committed

#### Scenario: A repository that tracks the file is unaffected
- **WHEN** a repository already TRACKS a file of one of these names (it is committed in the repo)
- **AND** that file is modified during a run AND the daemon stages and commits
- **THEN** the tracked file's change IS committed (the exclude affects only untracked files)

#### Scenario: Exclusion is idempotent
- **WHEN** the daemon stages a workspace's working tree more than once across runs
- **THEN** each artifact pattern appears at most once in `.git/info/exclude` (no duplicate entries accumulate)

#### Scenario: Exclusion is registered at workspace initialization
- **WHEN** a workspace is initialized (including a re-initialization after a re-clone)
- **THEN** the per-run CLI config artifacts are registered in `.git/info/exclude` before the workspace's first pass inspects the working tree
- **AND** this does not depend on the pass ever staging or committing

#### Scenario: A CLI config dropped by an advisory audit does not trip the dirty check
- **WHEN** an advisory audit (`WritePolicy::None`) runs a wrapped CLI that auto-generates a per-run config (e.g. `opencode.json`) in the workspace root
- **AND** the audit makes no other change AND never stages or commits
- **THEN** the post-run clean-workspace check sees no unexpected entry (the artifact is excluded)
- **AND** the audit is NOT reported as a write-policy violation
