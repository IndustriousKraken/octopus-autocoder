# workspace-manager — delta for exclude-cli-artifacts-from-commits

## ADDED Requirements

### Requirement: Per-run CLI config artifacts are excluded from commits
The agentic CLI strategies write per-run, server-specific config files into the workspace ROOT that the wrapped CLI auto-discovers from the project directory (claude's `.mcp.json`, opencode's `opencode.json` plus its `.opencode/` project scratch, agy's `mcp_config.json`). Unlike the claude settings file — which lives under `.git/` where git never stages it — these cannot move out of the working tree, because the CLI only discovers them in the working directory. The daemon SHALL ensure they are NEVER committed: before staging the working tree (`git add -A`), it SHALL add these artifacts to the workspace's `.git/info/exclude`.

`.git/info/exclude` is a LOCAL exclude — it is never itself committed (no `.gitignore` change appears in the repository) — AND it affects only UNTRACKED files. Therefore a repository that legitimately TRACKS one of these files is unaffected: its tracked copy continues to stage and commit normally, and only autocoder's generated (untracked) copy is skipped. The artifact list SHALL derive from the strategies' own filename definitions so it cannot drift from what they write. This extends the `a16` principle — daemon bookkeeping never lives in the managed repository tree — to the agentic CLIs' auto-discovered configs.

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
