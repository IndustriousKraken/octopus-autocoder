# git-workflow-manager — delta for a007-forge-provider-abstraction

## ADDED Requirements

### Requirement: Forge provider abstraction
The daemon SHALL route every forge **API** operation through a single `Forge` trait whose concrete implementation is selected per repository by the repository URL's host. The trait surface SHALL cover everything coupled to the forge today: repository-URL parsing; PR/MR lifecycle (open, list-open, find-by-head, set-draft); comment listing-since AND posting; review posting; fork creation; commenter authorization; AND the push-only branch hint. The git operations (clone, fetch, branch, commit, push) are NOT part of the trait — they use the raw URL and the `origin` remote and remain host-neutral.

This change SHALL provide the `GithubForge` implementation, reproducing the current GitHub behavior exactly: today's `github.rs` REST shapes, the `author_association`-based authorization gate, AND the draft-PR handling. It SHALL NOT introduce a second provider or any operator-visible behavior change; it is a behavior-preserving extraction whose correctness is established by the existing GitHub tests passing unchanged through the trait. After the change, no direct forge REST call SHALL exist outside the forge module — every forge call site goes through the trait (single source of truth).

Provider selection SHALL resolve from the repository URL host: a GitHub host resolves to `GithubForge`. A host with no registered forge provider SHALL return a clear error naming the host, AND no forge API operation SHALL proceed for that repository — preserving today's rejection of non-GitHub URLs until a later change registers an additional provider.

#### Scenario: A GitHub repository resolves to the GitHub forge
- **WHEN** the daemon performs a forge operation for a repository whose URL host is GitHub
- **THEN** the operation is served by `GithubForge`
- **AND** it behaves identically to the pre-extraction `github.rs` (same REST shapes AND results)

#### Scenario: Forge API calls have a single source of truth
- **WHEN** the codebase is searched after this change
- **THEN** no forge REST API call exists outside the forge module
- **AND** every PR/MR-lifecycle, comment, review, fork, AND authorization call site goes through the `Forge` trait

#### Scenario: An unsupported forge host returns a clear error
- **WHEN** a repository URL resolves to a host with no registered forge provider
- **THEN** forge resolution returns an error naming the host
- **AND** no forge API operation is attempted for that repository

#### Scenario: Commenter authorization rides the forge
- **WHEN** a forge-sourced command (e.g. `@<bot> revise`) is evaluated for authorization
- **THEN** the selected forge decides the commenter's authorization
- **AND** `GithubForge` applies the GitHub `author_association` gate exactly as before

#### Scenario: Git operations are unchanged
- **WHEN** the daemon clones, fetches, branches, commits, or pushes for any repository
- **THEN** those operations use the raw URL and the `origin` remote
- **AND** they do NOT route through the `Forge` trait
