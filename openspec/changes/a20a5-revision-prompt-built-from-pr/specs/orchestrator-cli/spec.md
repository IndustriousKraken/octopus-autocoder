## ADDED Requirements

### Requirement: Revise dispatcher refuses to invoke the executor when PR-context assembly fails
The PR-comment revision dispatcher SHALL assemble the executor's `RevisionContext` from PR-sourced material (per the `executor` capability's `Revision prompt is constructed from PR-sourced material` requirement) BEFORE invoking the executor. The assembly step SHALL fetch:

- The PR body (one `GET /repos/{owner}/{repo}/pulls/{n}` call OR via the existing PR-list response if already in scope).
- The PR's issue comments via `list_issue_comments_since(..., since=None)` — fetching every comment, then filtering to those whose body starts with the canonical `## Agent implementation notes` heading.

When any of these fetches returns an `Err`, the dispatcher SHALL:

1. Post a clear failure comment to the PR naming the assembly failure:
   ```
   ✗ Cannot revise: failed to fetch PR context: <truncated-error-message>. The daemon will retry on the next polling iteration. If this persists, check journalctl for the daemon's GitHub API errors AND verify the bot's token has Read access on this repo.
   ```
2. NOT advance the comment-seen marker (`last_seen_comment_ts` in the per-PR state file). This guarantees the revise comment is NOT lost; the next iteration's dispatcher pass re-attempts the assembly. Transient API errors (rate limits, brief outages, 5xx) self-recover.
3. NOT invoke the executor. The placeholder fallback is removed entirely; there is no degraded-prompt path.

When the assembly succeeds, the dispatcher SHALL pass the populated `RevisionContext` to the executor's revision-mode entry point. The executor's `Completed` / `Failed` / `AskUser` outcomes are handled by the existing canonical revise mechanisms (per the canonical "Revising an open PR via comment" requirement).

#### Scenario: Successful assembly proceeds to the executor
- **WHEN** an operator's `@<bot> revise <text>` comment is detected on an open PR
- **AND** the dispatcher fetches the PR body successfully AND fetches the PR's issue comments successfully
- **THEN** the dispatcher constructs a `RevisionContext` with all five fields populated (`pr_body`, `pr_change_list`, `agent_implementation_notes`, `pr_diff`, `revision_text`)
- **AND** invokes the executor's revision-mode entry point with that context
- **AND** the executor's outcome is handled per the canonical revise mechanism

#### Scenario: PR body fetch failure produces refusal comment AND preserves marker
- **WHEN** the PR body fetch (`GET /repos/{owner}/{repo}/pulls/{n}`) returns an `Err` (HTTP 5xx, network timeout, etc.)
- **THEN** the dispatcher posts a comment beginning with `✗ Cannot revise: failed to fetch PR context:` AND naming the error
- **AND** the per-PR state file's `last_seen_comment_ts` is NOT advanced
- **AND** the executor is NOT invoked
- **AND** the next polling iteration's dispatcher re-attempts the assembly (the operator's revise comment is preserved)

#### Scenario: PR comments fetch failure has identical handling
- **WHEN** the PR-comments fetch returns an `Err`
- **THEN** the same refusal comment is posted, the marker is NOT advanced, AND the executor is NOT invoked

#### Scenario: Persistent assembly failure surfaces visibly
- **WHEN** the PR-body OR PR-comments fetch fails on N consecutive polling iterations (N > 1)
- **THEN** N refusal comments accumulate on the PR (one per iteration)
- **AND** the comment timestamps make the persistent-failure pattern visible to the operator on the PR page
- **AND** the operator can investigate via journalctl AND the GitHub API status pages

#### Scenario: PR has no `Agent implementation notes` comments
- **WHEN** the dispatcher fetches PR comments AND none match the `## Agent implementation notes` heading (e.g., revise was posted within the same iteration the PR was created, before the implementer-summary comment landed)
- **THEN** the assembly succeeds with an empty `agent_implementation_notes` field
- **AND** the executor is invoked with the empty field
- **AND** the LLM still has spec deltas (via `pr_diff`), the PR body, the change list, AND the revision request — sufficient to attempt the revision
- **AND** no refusal comment is posted (this is not a failure)

#### Scenario: No fallback placeholder is ever rendered
- **WHEN** the dispatcher invokes the executor's revision-mode entry point in any code path
- **THEN** the `RevisionContext` it passes has all five fields populated from PR-sourced material
- **AND** no field contains the pre-`a20a5` placeholder string `_(original change material unavailable — ...)_` OR any analogous "best-effort" stub
- **AND** the rendered prompt the executor sees is full-fidelity for the revision task
