# git-workflow-manager — delta for issue-reads-via-forge

## ADDED Requirements

### Requirement: Forge provider lists open issues via the authenticated API
The `Forge` trait surface SHALL include listing a repository's open issues, so issue reads use the SAME authenticated forge API AND the SAME configured credential as the rest of the trait (PR/MR lifecycle, comments, reviews, authorization) — NOT a separate CLI with its own credential store.

`GithubForge` SHALL list open issues via the GitHub REST API (`GET /repos/<owner>/<repo>/issues?state=open`, paginated) authenticated with the repository's configured token — the same per-owner-routed PAT used for PR creation — AND SHALL exclude pull-request entries (the GitHub issues endpoint interleaves PRs, marked by a `pull_request` object). `GitlabForge` SHALL list open issues via the GitLab issues API authenticated with its configured token.

Every daemon code path that reads a repository's open issues — the scout handler's issue input AND the hybrid issue-ingestion triage — SHALL obtain them through this trait operation, preserving the trait's single-source-of-truth principle (no forge call outside the forge module). The standalone `gh` CLI SHALL NOT be required for issue reading: an operator who has configured the GitHub token for PR operations SHALL NOT need a separate `gh auth login`.

A failure of the forge open-issue listing (auth, rate limit, network) SHALL be surfaced to the caller as an error so the caller can degrade gracefully (the scout/ingestion callers log a WARN AND continue with an empty issue list), NOT panic or abort the iteration.

#### Scenario: Open-issue listing uses the configured forge credential
- **WHEN** the daemon reads a repository's open issues
- **THEN** the request goes through the `Forge` trait using the same configured token as PR operations
- **AND** no separate CLI credential (e.g. `gh auth login`) is required

#### Scenario: Pull requests are excluded from the issue list
- **WHEN** the GitHub issues endpoint returns pull-request entries interleaved with issues
- **THEN** `GithubForge` excludes the pull-request entries from the returned open-issue list

#### Scenario: Issue reads route through the trait
- **WHEN** the codebase is searched after this change
- **THEN** the scout handler AND the issue-ingestion triage obtain open issues through the `Forge` trait
- **AND** no open-issue read shells out to the `gh` CLI

#### Scenario: A forge issue-read failure is non-fatal
- **WHEN** the forge's open-issue listing fails (auth, rate limit, network)
- **THEN** the listing returns an error to the caller
- **AND** the caller logs a WARN naming the failure AND continues with an empty issue list
