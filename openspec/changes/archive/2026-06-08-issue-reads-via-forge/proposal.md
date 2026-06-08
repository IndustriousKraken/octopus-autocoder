## Why

PR creation, pushes, comments, reviews, and authorization all go through the `Forge` trait using the repository's configured token (`Authorization: Bearer <PAT>`, per-owner-routed). Open-issue reading is the one holdout: the scout handler AND the hybrid issue-ingestion triage shell out to the `gh` CLI (`gh api .../issues?state=open`), which authenticates against `gh`'s OWN credential store (`gh auth login` / `GH_TOKEN`) and ignores the PAT autocoder already has.

The operator-visible cost: issue ingestion silently does nothing until the operator runs a separate `gh auth login` on the host — undiscoverable, and inconsistent with every other forge operation. It also violates the forge abstraction's single-source-of-truth principle ("no forge call outside the forge module"): the `gh api` read is a forge operation that bypasses the trait and its credential.

## What Changes

Open-issue reading moves into the `Forge` trait, using the same authenticated API and the same configured credential as the rest of the trait. `GithubForge` lists open issues via the GitHub REST API with the repository's PAT (excluding the pull-request entries the issues endpoint interleaves); `GitlabForge` via the GitLab issues API with its token. The scout handler's issue input AND the issue-ingestion triage obtain issues through this trait operation; the `gh` CLI is no longer used for issue reading, so an operator who configured the PAT for PRs needs no separate `gh auth login`.

Graceful degradation is preserved: a forge issue-read failure logs a WARN and the caller continues with an empty issue list, exactly as the `gh`-failure path did.

## Impact

- **Affected specs:**
  - `git-workflow-manager` — ADD `Forge provider lists open issues via the authenticated API`.
  - `orchestrator-cli` — MODIFY `Scout polling-iteration handler produces a triage list AND persists ScoutRunState` (issue input via the forge, not `gh api`); MODIFY `features.scout config schema` (`include_issues` description: forge, not `gh api`).
- **Affected code:** the `Forge` trait gains an open-issue-listing method; `GithubForge` / `GitlabForge` implement it via REST with the configured token; `polling/scout.rs`'s `fetch_open_issues_json` (the single shared issue read, used by both scout and ingestion) routes through the forge instead of `Command::new("gh")`.
- **Operator-visible behavior:** issue ingestion AND scout issue input work with only the configured GitHub token — no separate `gh auth login`. Identical results; identical graceful degradation on failure.
- **Non-goals:** the issues lane's enablement, promotion (`send it`), and quarantine are unchanged; this is purely the credential/transport for reading issues. The dependency preflight (a011) MAY later drop `gh` from the *required* set for issue features now that the read no longer needs it — out of scope here.
- **Dependencies:** builds on `Forge provider abstraction` (the trait) AND `Hybrid issue ingestion with maintainer promotion` (which "reuses scout's issue read" and so inherits the forge read automatically). No unmerged dependencies.
- **Acceptance:** `cargo test` passes; `openspec validate issue-reads-via-forge --strict` passes. Tests: `GithubForge` lists open issues with the configured token AND excludes pull requests; a forge issue-read error degrades to an empty list with a WARN (no panic); the shared issue read no longer spawns `gh`. Docs updated: `docs/CONFIG.md` (`features.scout.include_issues`) and the issues-lane setup notes drop the `gh auth login` step.
