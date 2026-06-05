# git-workflow-manager — delta for a008-gitlab-forge

## ADDED Requirements

### Requirement: GitLab forge provider
autocoder SHALL provide a `GitlabForge` implementation of the `Forge` trait so a GitLab-hosted repository is first-class for the daily loop. `GitlabForge` SHALL implement the trait against GitLab's API:

- **`parse_repo`** extracts the GitLab host AND the URL-encoded `namespace/project` path, supporting nested groups (e.g. `group/subgroup/project`).
- **MR lifecycle.** `open_pr` creates a merge request; `list_open_prs` lists open merge requests; `find_pr_by_head` matches an MR by its source branch; `set_pr_draft` toggles GitLab's `Draft:` title prefix (GitLab marks an MR draft via that prefix, not a flag).
- **Comments.** `list_comments_since` AND `post_comment` operate on MR notes.
- **Reviews.** `post_review` maps the verdict onto GitLab: approve → MR approval; request-changes AND comment → an MR note (GitLab has no distinct request-changes state).
- **Authorization.** `authorize` maps the commenter's GitLab member access level: Developer, Maintainer, AND Owner are authorized; Reporter AND Guest are not. This mirrors the GitHub `author_association` gate.
- **`branch_url`** produces the GitLab MR-create hint (`glab mr create` / MR web URL) for the push-only path.

#### Scenario: Merge-request lifecycle round-trips
- **WHEN** the daily loop opens, lists, and looks up a merge request for a GitLab repository
- **THEN** `GitlabForge` creates the MR, lists it among open MRs, AND finds it by its source branch

#### Scenario: Draft toggles via the title prefix
- **WHEN** `set_pr_draft(true)` then `set_pr_draft(false)` is called for a GitLab MR
- **THEN** the MR title gains the `Draft:` prefix AND then has it removed

#### Scenario: Review verdict maps to GitLab
- **WHEN** `post_review` is called with an approve verdict
- **THEN** the MR is approved
- **AND** a request-changes or comment verdict instead posts an MR note (GitLab has no request-changes state)

#### Scenario: Authorization by access level
- **WHEN** `authorize` evaluates a commenter whose GitLab access level is Developer, Maintainer, or Owner
- **THEN** the commenter is authorized
- **AND** a commenter at Reporter or Guest is not authorized

#### Scenario: Repository parsing handles nested groups
- **WHEN** `parse_repo` is given a GitLab URL with a nested-group path
- **THEN** it returns the GitLab host AND the URL-encoded `namespace/project` path covering the nested groups
