## 1. Verb parser + action submission

- [ ] 1.1 In `autocoder/src/chatops/slack.rs` (or wherever the inbound verb table lives), add `changelog` to the recognized verb set. Parse the form `@<bot> changelog <repo-substring> [<args>]` where `<args>` is a free-form remainder string.
- [ ] 1.2 Submit a `ChangelogAction { repo_url, raw_args, channel, thread_ts }` over the control socket. The control-socket handler stamps a `ChangelogRequest` state file AND returns the resolved repo URL + ETA for the bot's ack.
- [ ] 1.3 Polite refusals:
  - Missing repo-substring → `✗ changelog: missing repo-substring.` (threaded reply, no state file).
  - Repo substring matches nothing → `✗ changelog: no repo matched '<sub>'; configured: <list>` (threaded reply, no state file).
  - Multiple repo substring matches → standard "be more specific" reply with candidates.
  - chatops backend unconfigured → `✗ changelog: chatops backend not configured.` (this verb needs the backend to ack).
  - `post_notification` for the ack fails → `✗ changelog: could not post ack to chat: <reason>` AND NO state file is written (request idempotent on retry).
- [ ] 1.4 Tests via the existing inbound-listener test harness:
  - Valid verb → action submitted; ack message text matches the expected format.
  - Each refusal case → no state file written; reaction or thread reply matches expected.

## 2. Request state file

- [ ] 2.1 New module `autocoder/src/state/changelog_request.rs`:
  ```rust
  #[derive(Serialize, Deserialize, Debug, Clone)]
  pub struct ChangelogRequest {
      pub request_id: String,            // ULID or UUID
      pub repo_url: String,
      pub raw_args: String,
      pub channel: String,
      pub lifecycle_thread_ts: String,
      pub status: ChangelogStatus,
      pub submitted_at: DateTime<Utc>,
  }
  #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ChangelogStatus { Pending, InFlight, Acted, Failed }
  ```
- [ ] 2.2 On-disk location: `<state_dir>/changelog-requests/<request_id>.json`. Mode 0640, owner matches daemon user.
- [ ] 2.3 7-day pruning at iteration start. Logic parallels `audit-thread-state` and `proposal-request-state` pruning. Pruned files emit one INFO log line per request_id.
- [ ] 2.4 Tests covering serialize + deserialize round-trip, AND pruning behavior (write a fixture state with `submitted_at: now - 8 days` → pruning removes it; with `now - 6 days` → preserved).

## 3. Argument parsing for `raw_args`

- [ ] 3.1 New helper `parse_changelog_args(raw: &str) -> Result<ParsedChangelogArgs>` accepting the same flag surface as `autocoder changelog`: `--since <tag>`, `--to <tag>`, `--workspace <path>` (the last is unusual via chatops AND should refuse unless the operator is in a trust-elevated context; default-deny in chatops parsing AND log a WARN).
- [ ] 3.2 Bad flags → bail with a clear error AND post the error as a threaded reply: `✗ changelog: bad arg: <text>`.
- [ ] 3.3 Tests: each flag combination parses correctly; bad flags surface descriptive errors.

## 4. Polling-iteration handler

- [ ] 4.1 New module `autocoder/src/orchestrator/changelog_triage.rs`. Entry point:
  ```rust
  pub async fn run_changelog_request(
      request: &ChangelogRequest,
      workspace: &Path,
      executor: &dyn Executor,
      git: &dyn GitOps,
      chatops: Option<&dyn ChatOpsBackend>,
      ...
  ) -> Result<ChangelogOutcome>;
  ```
- [ ] 4.2 Body:
  - Parse `request.raw_args` via `parse_changelog_args`.
  - Call the `a05` extractor directly: `autocoder::cli::changelog::extract(workspace, &parsed_args, ChangelogFormat::Json).await?`. Note: factor the extractor's data-producing path into a callable function in `a05`; the chatops handler should NOT shell out to its own binary.
  - Serialize the structured data to a string.
  - Build the prompt: stylist template + the JSON data + a directive to read `<workspace>/CHANGELOG.md` if it exists. The prompt also instructs the executor to read selected proposal documents from the archive when more context is needed.
  - Invoke the executor in triage-mode (or a parallel "one-shot doc-writer" mode if triage's contract is too narrow — verify by reading the existing triage handler).
  - Validate the resulting diff: must touch `CHANGELOG.md` (required) AND MAY touch `openspec/changes/archive/<slug>/proposal.md` (frontmatter edits). Reject diffs that touch other paths — bail with `✗ changelog: LLM produced out-of-scope diff; refusing to commit. See <log-path>.`
  - Commit the diff to a `changelog-<short-hash>` branch. Push. Open a single PR.
  - Post a threaded reply in the lifecycle thread: `✓ Changelog draft ready at <PR-URL>. Review on GitHub; revise via @<bot> revise <text>.`
- [ ] 4.3 Tests:
  - Fixture workspace with archive entries + an existing `CHANGELOG.md` → handler invokes the (mocked) executor, captures a (mocked) diff, validates path scope, commits, opens (mocked) PR.
  - Fixture workspace with NO existing `CHANGELOG.md` → handler still invokes the executor; the mock returns a fresh Keep-a-Changelog-formatted file; commit + PR proceed.
  - Mocked executor returns a diff touching `src/foo.rs` → handler rejects with the out-of-scope error.
  - Mocked executor returns a diff touching `CHANGELOG.md` AND multiple `proposal.md` files → handler accepts.

## 5. Prompt template

- [ ] 5.1 Create `prompts/changelog-stylist.md` per the excerpt in the proposal. Full template should include:
  - Role: "You are writing release notes for a project that uses OpenSpec."
  - Input description: "JSON listing the archived changes shipped in this release window. Read individual `proposal.md` files via the Read tool for fuller context."
  - **Critical existence check**: "Before writing the changelog, check whether `CHANGELOG.md` exists in the workspace root. If it does, read it AND match its established style. If it does NOT exist, create one in the Keep a Changelog v1.1.0 format with a top-level project heading, an `## [Unreleased]` placeholder, AND this release's section starting with `## [<version>] - <YYYY-MM-DD>`."
  - Register guidance: "Write release notes, not motivation paragraphs. One sentence per entry, two if non-obvious. Lead with the user-visible verb."
  - Grouping guidance: "Group thematically, not strictly by capability. Related changes that span capabilities cluster together."
  - Headline guidance: "Top of the section gets 3-5 lead items. Long tail goes under `### Also included`."
  - Internal-only handling: "Pure refactors / test-only / doc-only changes belong in `### Also included` OR you may propose `changelog: skip` frontmatter for them. If you propose frontmatter, edit the relevant `openspec/changes/archive/<slug>/proposal.md` file in the same commit."
  - Output contract: "Write the polished changelog to `CHANGELOG.md` (creating or updating). MAY also edit `proposal.md` frontmatter files. Do NOT touch any other path."
- [ ] 5.2 Embed at compile time via `include_str!("../../prompts/changelog-stylist.md")`. Override via `executor.changelog_stylist_prompt_path` config (parallel to other prompt overrides).
- [ ] 5.3 Tests: the embedded template loads; the override config field accepts a path; the override file's contents replace the embedded template.

## 6. PR + revision-loop integration

- [ ] 6.1 The handler's `git_workflow_manager` interaction creates the branch + commit + PR via the same surfaces `propose` / `send it` use — no new git plumbing. PR body includes a short description naming this as a changelog draft AND a hint at the revision loop.
- [ ] 6.2 The PR's branch (`changelog-<short-hash>`) participates in the existing PR-comment revision dispatcher from `a01-pr-comment-revision-loop`. The dispatcher invokes the revision handler against the changelog branch the same way it does against any other autocoder-opened PR.
- [ ] 6.3 The revision handler, on a changelog PR, re-invokes `run_changelog_request` with the previous draft + the operator's instruction. The deterministic data layer is unchanged between revisions; only the stylist's rendering changes.
- [ ] 6.4 Tests:
  - PR is opened with the expected branch name AND body shape.
  - Revision-loop fixture: an existing changelog PR receives an `@<bot> revise leave out the refactors` comment → next iteration re-runs the stylist with that constraint → force-push to the PR branch.

## 7. Docs

- [ ] 7.1 In `docs/CHATOPS.md`, extend the `Chat-driven workflows` section to add a `### Generating a changelog: \`changelog\`` subsection. Include:
  - The verb syntax (`@<bot> changelog <repo> [<flags>]`).
  - The flag surface (mirrors `autocoder changelog`).
  - The PR output shape (single PR; participates in the revision loop).
  - Frontmatter propagation: revisions like "leave out refactors" may include source-proposal frontmatter edits in the same PR.
  - Polite-refusal cases.
- [ ] 7.2 In `docs/CLI.md`'s `## \`changelog\`` section (from `a05`), add a cross-link footer: `For an LLM-styled draft that opens a PR for review, use the \`@<bot> changelog\` chatops verb instead. See [CHATOPS.md → Generating a changelog](CHATOPS.md#generating-a-changelog-changelog).`

## 8. Spec deltas

- [ ] 8.1 `openspec/changes/a06-chat-driven-changelog/specs/orchestrator-cli/spec.md` ADDs one requirement covering the chat verb's polling-iteration handler, the deterministic-extract → stylist-prompt → single-PR flow, the diff scope validation, AND the revision-loop participation.
- [ ] 8.2 `openspec/changes/a06-chat-driven-changelog/specs/chatops-manager/spec.md` ADDs one requirement extending the inbound listener's verb table to recognize `changelog` AND submit a `ChangelogAction`.
- [ ] 8.3 `openspec/changes/a06-chat-driven-changelog/specs/project-documentation/spec.md` ADDs one requirement covering the CHATOPS.md subsection AND the CLI.md cross-link.

## 9. Verification

- [ ] 9.1 `cargo test` passes (new + existing).
- [ ] 9.2 `openspec validate a06-chat-driven-changelog --strict` passes.
- [ ] 9.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
