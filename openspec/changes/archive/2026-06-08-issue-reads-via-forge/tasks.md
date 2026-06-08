# Tasks

## 1. Forge trait gains open-issue listing

- [x] 1.1 Add a `list_open_issues` method to the `Forge` trait returning the open issues (number, title, body, author association), with a forge-neutral issue type.
- [x] 1.2 `GithubForge::list_open_issues`: `GET /repos/<owner>/<repo>/issues?state=open` (paginated) with `Authorization: Bearer <configured token>`; filter out entries carrying a `pull_request` object. Reuse the existing github.rs request/auth shape.
- [x] 1.3 `GitlabForge::list_open_issues`: the GitLab issues API with its configured token.

## 2. Route issue reads through the forge

- [x] 2.1 Replace `polling/scout.rs`'s `fetch_open_issues_json` (`Command::new("gh") api ...`) with a call through the resolved `Forge` for the repo — the single shared read used by BOTH the scout handler AND the issue-ingestion triage (`lanes/ingestion.rs` reuses it).
- [x] 2.2 Preserve graceful degradation: a forge error → WARN + empty issue list (callers unchanged).
- [x] 2.3 Confirm no open-issue read shells out to `gh` anymore (the path-literals/boundary tests, and a grep for `Command::new("gh")` in the issue path).

## 3. Tests

- [x] 3.1 `GithubForge::list_open_issues` returns open issues with the configured token AND excludes pull-request entries (mockito, as the other github.rs tests do).
- [x] 3.2 A forge issue-read error degrades to an empty list with a WARN (no panic), preserving the prior `gh`-failure behavior.
- [x] 3.3 The shared issue read routes through the forge — no `gh` spawn in the scout/ingestion issue path.

## 4. Documentation

- [x] 4.1 `docs/CONFIG.md`: `features.scout.include_issues` — fetch via the forge API, not `gh`.
- [x] 4.2 Issues-lane setup notes (OPERATIONS.md / README): drop the separate `gh auth login` step — the configured GitHub token covers issue reads.

## 5. Acceptance

- [x] 5.1 `cargo test` passes.
- [x] 5.2 `openspec validate issue-reads-via-forge --strict` passes.
