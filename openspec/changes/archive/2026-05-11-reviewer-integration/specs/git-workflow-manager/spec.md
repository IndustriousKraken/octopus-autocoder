## MODIFIED Requirements

### Requirement: Monolithic PR at end of pass
The git workflow manager SHALL push the agent branch and create a single Pull Request via the GitHub REST API at the end of each polling iteration that produced at least one commit. **When the code-reviewer is enabled, the PR body SHALL include the reviewer's report under a `## Code Review` heading, and a `Block` verdict SHALL cause the PR to be created as a draft (with a `do-not-merge` label fallback if the host rejects drafts).**

#### Scenario: Opening a PR with a passing review
- **WHEN** an iteration completes AND the agent branch contains at least one commit ahead of base AND `reviewer.enabled` is true AND `code_reviewer.review` returns `Ok(ReviewReport { verdict: Pass, .. })`
- **THEN** the manager pushes with `--force-with-lease` and POSTs to the GitHub PR API with `draft: false` and a body whose final section is `## Code Review` followed by the reviewer's `markdown`

#### Scenario: Opening a PR with a Block verdict
- **WHEN** an iteration completes AND the reviewer returns `Ok(ReviewReport { verdict: Block, .. })`
- **THEN** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: true`
- **AND** the PR body's final section is `## Code Review` followed by the reviewer's `markdown`

#### Scenario: Reviewer disabled or absent
- **WHEN** the `reviewer` config block is absent OR `reviewer.enabled` is false
- **THEN** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: false` and a body that does NOT include a `## Code Review` section
- **AND** no LLM API call is made

#### Scenario: Reviewer failure
- **WHEN** `reviewer.enabled` is true AND `code_reviewer.review` returns `Err(_)`
- **THEN** the manager logs `"reviewer failed: {error}"` naming the reason
- **AND** the manager pushes the agent branch and POSTs to the GitHub PR API with `draft: false`
- **AND** the PR body's `## Code Review` section contains only the line `(reviewer failed: <reason>)`

#### Scenario: Draft creation falls back to label
- **WHEN** `Block` verdict requires `draft: true` AND the GitHub API rejects the draft flag (specific GitHub error indicating drafts are not supported on this repo)
- **THEN** the manager retries the PR creation request with `draft: false`
- **AND** on success, the manager POSTs to `https://api.github.com/repos/<owner>/<repo>/issues/<pr_number>/labels` with body `{ "labels": ["do-not-merge"] }`
- **AND** the manager logs `"draft unsupported; applied do-not-merge label as fallback"`
