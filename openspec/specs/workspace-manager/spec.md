# workspace-manager Specification

## Purpose
TBD - created by archiving change orchestrator-foundation. Update Purpose after archive.
## Requirements
### Requirement: Deterministic workspace path derivation
The workspace manager SHALL derive a per-repository workspace path deterministically from the configured URL, so that restarting the daemon reuses existing local clones rather than creating new ones.

#### Scenario: Path derivation is stable
- **WHEN** the manager derives a path for a given URL
- **THEN** invoking the same derivation a second time with the same URL returns a path equal by `==` to the first
- **AND** the path is rooted at `/tmp/workspaces/`

#### Scenario: Distinct URLs produce distinct paths
- **WHEN** the manager derives paths for two URLs that differ in host, owner, or repo name
- **THEN** the resulting paths are not equal
- **AND** repeated derivations preserve the inequality

### Requirement: Cross-repository path collision detection at startup
The orchestrator SHALL detect any two configured repositories that resolve to the same workspace path and refuse to start, naming both URLs and the shared path in the error message.

#### Scenario: Two repos derive to the same path
- **WHEN** the orchestrator loads a config containing two repositories whose URLs sanitize to the same workspace path (or whose explicit `local_path` overrides collide)
- **THEN** the orchestrator emits a startup error whose text contains BOTH conflicting URLs verbatim AND the shared path
- **AND** no polling tasks are spawned for either repository
- **AND** the process exits non-zero within 5 seconds of config load

### Requirement: Idempotent workspace initialization
The workspace manager SHALL ensure a repository is locally cloned before each polling iteration begins, performing a clone if absent and a fetch if present, without losing existing local state.

#### Scenario: First-time clone
- **WHEN** the polling task begins an iteration AND the workspace path does not exist on disk
- **THEN** the manager runs `git clone <url> <workspace_path>`
- **AND** the resulting path contains a `.git` directory verifiable via filesystem inspection

#### Scenario: Re-initializing an existing workspace
- **WHEN** the polling task begins an iteration AND the workspace path exists on disk
- **THEN** the manager runs `git fetch origin` inside the workspace and does NOT run a fresh clone
- **AND** any pre-existing local branches in the workspace are preserved (the set returned by `git branch --list` is unchanged before and after `ensure_initialized`)

#### Scenario: Workspace exists but is not a git repository
- **WHEN** the workspace path exists but does not contain a `.git` directory
- **THEN** `ensure_initialized` returns an error naming the path and the missing `.git` marker
- **AND** the manager does NOT delete or modify the existing path

