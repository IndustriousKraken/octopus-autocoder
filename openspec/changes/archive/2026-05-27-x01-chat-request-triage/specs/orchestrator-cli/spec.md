## ADDED Requirements

### Requirement: `propose` chatops verb queues a chat-driven triage request
The chatops listener SHALL recognize `@<bot> propose <repo-substring> <free-form text>` as the `ProposeRequest` command. The repo-substring follows the established case-insensitive substring-matching rules. The free-form text is everything after the substring (trimmed of leading/trailing whitespace, line breaks preserved internally, capped at 10,000 characters). On a unique repo match, the dispatcher SHALL: generate a `request_id`, post a one-line ack that includes the trailing phrase "Follow along in this thread.", capture the ack message's `ts` as the request's lifecycle `thread_ts`, write a `ProposalRequestState` file with `status: Pending`, AND submit a `queue_proposal_request` control-socket action so the next polling iteration picks up the request.

#### Scenario: Happy-path queueing with thread creation
- **WHEN** an operator posts `@<bot> propose myrepo add a /healthz endpoint` AND `myrepo` uniquely resolves to a configured repo
- **THEN** the bot posts a top-level ack message containing `✓ Queued proposal request for <repo_url>. The next polling iteration will run it (~Nm). Follow along in this thread.`
- **AND** the ack's `ts` becomes the request's `thread_ts`
- **AND** a `ProposalRequestState` file is written with `status: Pending`
- **AND** the per-repo `pending_proposal_requests` queue gains an entry

#### Scenario: Missing request text is rejected
- **WHEN** an operator posts `@<bot> propose myrepo` (no free-form text after the substring)
- **THEN** the bot replies `✗ propose: missing request text. Usage: @<bot> propose <repo> <free-form description>`
- **AND** no state file is written

#### Scenario: Repo substring ambiguity surfaces the candidate list
- **WHEN** the repo-substring matches multiple configured repos
- **THEN** the bot replies with the existing `match_repo`-style "be more specific" list
- **AND** no state file is written

### Requirement: Triage prompt classifies the request as DIRECTIVE, QUESTION, or AMBIGUOUS before acting
The triage-mode prompt for chat-driven requests (`prompts/chat-request-triage.md`) SHALL begin with a classification step. The LLM decides:

- **DIRECTIVE**: the input asks for a specific action a reasonable engineer could build. The LLM proceeds to explore the codebase, classify what needs to be done as fix-vs-spec, apply fixes, create spec proposals.
- **QUESTION**: the input asks for analysis, opinion, or exploration of options. The LLM writes its response to `<workspace>/.chat-reply.md` and STOPS. No source-file modifications.
- **AMBIGUOUS**: the request might be a directive but the LLM cannot pin down what to build. The LLM SHALL use the `ask_user` MCP tool to ask the operator for clarification. The existing chatops escalation posts the question in the request's thread and resumes the executor with the operator's answer.

#### Scenario: Directive proceeds to explore + classify + fix/spec
- **WHEN** the operator's request is `add a /healthz endpoint that returns 200 OK with the daemon's version and uptime`
- **THEN** the LLM classifies as DIRECTIVE
- **AND** proceeds with the explore + classify + fix-or-spec flow
- **AND** the diff after execution contains code changes (and optionally a new `openspec/changes/<derived-slug>/` directory)

#### Scenario: Question writes to .chat-reply.md and stops
- **WHEN** the operator's request is `what would it take to refactor the auth module to use the new error type?`
- **THEN** the LLM classifies as QUESTION
- **AND** writes its analysis to `<workspace>/.chat-reply.md`
- **AND** does NOT modify any other files
- **AND** `git status --porcelain` (after the executor returns) shows only `.chat-reply.md` as new/modified

#### Scenario: Ambiguous request escalates via ask_user
- **WHEN** the operator's request is `something something handler logic` (genuinely unclear)
- **THEN** the LLM classifies as AMBIGUOUS
- **AND** uses the `ask_user` MCP tool to post a clarifying question
- **AND** the existing chatops escalation posts the question in the request's `thread_ts`
- **AND** the operator's reply resumes the executor

### Requirement: `.chat-reply.md` marker drives the discussion-reply path
After the triage executor returns `Completed`, the polling iteration SHALL check for `<workspace>/.chat-reply.md` BEFORE running the diff-split + two-PR creation. The presence of this file means "the LLM classified as QUESTION and wrote its response here." The iteration SHALL: read the file contents, truncate at 35,000 characters with a daemon-log pointer when over, post the contents as a threaded reply in the request's `thread_ts`, delete `<workspace>/.chat-reply.md`, and set the state's `status` to `Discussed`. If `git status --porcelain` reports any OTHER modifications, the iteration SHALL log WARN naming them AND revert via `git reset --hard HEAD; git clean -fd`. No PRs are created.

#### Scenario: Clean discussion reply
- **WHEN** the executor returns Completed AND `.chat-reply.md` is the only modified file
- **THEN** the file contents post as a threaded reply in the request's thread
- **AND** the file is deleted
- **AND** the state's `status` is `Discussed`
- **AND** no PR is created
- **AND** no WARN log fires

#### Scenario: Discussion reply with leaked source modifications is cleaned up
- **WHEN** the executor returns Completed AND `.chat-reply.md` is present AND `git status --porcelain` ALSO shows modifications to other files
- **THEN** the file contents post as a threaded reply normally
- **AND** the state's `status` is `Discussed`
- **AND** a WARN log fires naming the unexpected other modifications
- **AND** the workspace is reverted via `git reset --hard HEAD; git clean -fd` so the next iteration sees a clean tree

#### Scenario: Long reply is truncated with daemon-log pointer
- **WHEN** the `.chat-reply.md` contents exceed 35,000 characters
- **THEN** the posted thread reply is truncated to 35,000 chars
- **AND** ends with `… [truncated; full reply at journalctl -u autocoder | grep request_id=<request_id>]`

### Requirement: Directive triage uses the existing two-PR mechanic; PRs participate in the revision-loop
When the executor returns Completed without a `.chat-reply.md` marker, the polling iteration SHALL run the diff-split + two-PR creation logic from `a01-audit-reply-acts` (using the shared `split_diff_by_spec_dir` helper). The resulting fixes PR and spec PR are structurally identical to PRs spawned by `send it` and by polling-loop processing. Operators commenting `@<bot> revise <text>` on either get revisions through `a01-pr-comment-revision-loop`.

#### Scenario: Mixed diff produces two PRs that cross-link
- **WHEN** the directive's executor returns Completed with both code changes AND a new `openspec/changes/<chat-derived-slug>/`
- **THEN** the daemon creates a fixes branch + PR with the code paths
- **AND** the daemon creates a spec branch + PR with the openspec paths
- **AND** each PR body contains a link to the other
- **AND** the state's `status` is `Acted`

#### Scenario: Code-only directive produces only a fixes PR
- **WHEN** the directive's diff has only code paths
- **THEN** only the fixes PR is created
- **AND** the state's `status` is `Acted`

#### Scenario: Spec-only directive produces only a spec PR
- **WHEN** the directive's diff has only new `openspec/changes/<chat-derived-slug>/` paths
- **THEN** only the spec PR is created
- **AND** the state's `status` is `Acted`

#### Scenario: Empty-diff directive posts a no-action reply
- **WHEN** the directive's executor returns Completed with an empty diff AND no `.chat-reply.md`
- **THEN** no PRs are created
- **AND** the bot posts a reply in the request's thread explaining no action was taken
- **AND** the state's `status` is `Acted`

#### Scenario: Revision comments on a triage PR are processed normally
- **WHEN** a chat-request-spawned PR has an operator comment `@<bot> revise <text>`
- **THEN** the existing revision-loop dispatcher picks up the comment AND processes the revision against the PR's branch
- **AND** the proposal-request state file is not consulted (the revision is its own scope)

### Requirement: Proposal-request state files are pruned after 7 days
The daemon SHALL prune `ProposalRequestState` files whose `submitted_at` is older than 7 days. The prune runs periodically (at iteration start or once per day per the existing housekeeping pattern). Stale entries are removed regardless of `status`.

#### Scenario: Stale entry is removed
- **WHEN** the prune runs AND a `ProposalRequestState` has `submitted_at` more than 7 days in the past
- **THEN** the state file is removed

#### Scenario: Fresh entry is preserved
- **WHEN** the prune runs AND a `ProposalRequestState` has `submitted_at` within the last 7 days
- **THEN** the state file is NOT removed regardless of status
