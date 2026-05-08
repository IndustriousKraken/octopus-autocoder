## Why

Once an agent's commits land on the agent branch, they need a quality review BEFORE the PR is human-merged. This change adds an automated AI code-review step focused exclusively on code quality (security, maintainability, smells, language idioms). It is intentionally NOT a spec-compliance check — that is the verifier's job and lives in its own future change.

## What Changes

- Add `code-reviewer` capability: extracts the diff between `base_branch` and `agent_branch`, sends it to a configured LLM API, parses the response into a structured `ReviewReport { verdict, markdown }`, returns it.
- Modify `git-workflow-manager`: when the reviewer is enabled, the PR body includes the report under a `## Code Review` heading; on a `Block` verdict the PR is created as a draft (with a `do-not-merge` label fallback if the host rejects drafts).
- Implementation: add `src/llm.rs` (provider abstraction), `src/code_reviewer.rs` (the reviewer itself), a `reviewer:` config block, and a default prompt template at `prompts/code-review-default.md` that explicitly scopes the review to code quality.

## Capabilities

### New Capabilities
- `code-reviewer`: AI-driven code-quality review of the diff between base and agent branches; produces a structured verdict and markdown report.

### Modified Capabilities
- `git-workflow-manager`: PR body composition now includes the reviewer report when enabled; Block verdict produces a draft PR.

## Impact

The orchestrator's PRs now ship with a structured code-quality review. Block-verdict PRs cannot be merged accidentally without a human first un-drafting them (or removing the fallback label). The reviewer is opt-in via the config block; setups that omit it run unchanged from phase-1 + multi-repo.
