## Why

The deterministic `autocoder changelog` subcommand from `a05` is correct, but the output reads as a mechanical extraction. The first paragraph of `## Why` is written for spec readers (motivation context, sometimes incident references, often multi-sentence). Release notes are a different register: terse one-liners, headline items at the top, "Also included" at the bottom, ruthless about dropping internal-only changes.

Closing that register gap is exactly what LLMs do well — and the existing triage infrastructure (`propose`, `send it`, the PR-comment revision loop) is already the right mechanic for "ask the LLM to produce a doc, open a PR, iterate on it via chat." A chat verb that wraps the deterministic extractor as the LLM's input source AND opens a PR with a polished CHANGELOG.md draft inherits all the existing review affordances.

The non-obvious benefit is the operator's revisions becoming durable. When the operator says `@<bot> revise leave out the refactors`, the LLM can do two things: (1) drop the refactors from this release's draft, AND (2) propose `changelog: skip` frontmatter edits to the source proposals. The second is the difference between "filtered for this release" and "permanently classified as not changelog-worthy." Future releases inherit the decision automatically.

## What Changes

**New chatops verb `@<bot> changelog <repo-substring> [<args>]`.** Parsed by the Slack inbound listener alongside `propose`, `send it`, `audit`, `revise`, and the recovery verbs. Optional args mirror the `autocoder changelog` subcommand's flags: `--since <tag>`, `--to <tag>`, `--workspace <path>`. The repo substring resolves to a managed workspace; the workspace's archive is the source.

**Ack message + lifecycle thread.** The bot acks in the channel with a top-level message:

```
✓ Queued changelog request for <repo_url>. The next polling iteration will run it. Follow along in this thread.
```

The ack's `ts` becomes the changelog-request's lifecycle thread. Same shape as `propose`'s lifecycle thread.

**Polling iteration runs the changelog-stylist flow.** On the next polling iteration after the verb is queued:

1. The deterministic extractor (`autocoder changelog --workspace <path> --format json`) emits the structured archive data.
2. The output is passed to the wrapped agent CLI with a `prompts/changelog-stylist.md` system prompt.
3. The agent reads any existing `CHANGELOG.md` (to match the project's established style, OR to create a Keep-a-Changelog-formatted file from scratch if none exists), reads selected proposal documents from the archive (when summarization needs more context than the JSON provides), AND writes its polished draft to `<workspace>/CHANGELOG.md`.
4. The agent MAY additionally propose `changelog:` frontmatter edits to source proposals — but only the operator can confirm those via the revision loop (the LLM proposes; the human disposes).
5. autocoder commits the diff to a new branch (`changelog-<short-hash>`), pushes, AND opens a PR.

**Two-PR shape is NOT used.** Unlike `propose` / `send it`, the changelog flow produces a single PR. The reason: CHANGELOG.md is the only output artifact. Frontmatter edits to source proposals live in the same PR — they're part of "what this release's changelog work decided," not a separable concern.

**The PR participates in `a01-pr-comment-revision-loop`.** Reviewer comments of the form `@<bot> revise <text>` trigger a re-run. Examples:

- `@<bot> revise leave out the refactors from this changelog` — the LLM drops them AND (in a follow-up commit on the same branch) adds `changelog: skip` frontmatter to their proposals.
- `@<bot> revise the top section needs a one-sentence release headline` — the LLM adds a `## v<X.Y.Z>` overview paragraph above the per-capability sections.
- `@<bot> revise group the chat-driven changes thematically rather than by capability` — the LLM restructures.

Each revision re-invokes the stylist with the previous draft + the operator's instruction in context. The deterministic data layer doesn't change between revisions (the archive is fixed); only the LLM's rendering changes.

**Polite-refusal cases.** Same shape as `propose`:

- `✗ changelog: missing repo-substring.` — no first arg.
- `✗ changelog: no repo matched '<substring>'; configured: <list>` — substring doesn't resolve.
- `✗ changelog: chatops backend not configured` — the verb needs the backend to ack.
- `✗ changelog: could not post ack to chat: <reason>` — ack post fails.

**7-day staleness rule.** Changelog-request state files are pruned after 7 days regardless of terminal status. Same as `propose` / `send it`.

**The stylist prompt template.** A new `prompts/changelog-stylist.md` is embedded at compile time (overridable via a config knob, parallel to other prompt templates). Excerpt:

> You are writing release notes for a project that uses OpenSpec. Your input is a JSON object listing the archived changes shipped in this release window AND the corresponding `proposal.md` files (read them with the `Read` tool when you need fuller context than the JSON summary provides).
>
> **Read the existing CHANGELOG.md first.** It lives at the workspace root. If there IS one, match its style — heading hierarchy, item phrasing register, grouping convention, presence or absence of dates and PR links. If there is NO existing CHANGELOG.md, create one in the Keep a Changelog v1.1.0 format: top-level project heading, `## [Unreleased]` placeholder, then this release's section starting with `## [<version>] - <YYYY-MM-DD>`.
>
> **Write release notes, not motivation paragraphs.** Each entry is one sentence (two if the motivation is genuinely non-obvious). Lead with the user-visible verb ("Adds X", "Fixes Y", "Changes Z behavior"). Drop incident references, ticket numbers, and "we got burned last quarter when..." context — those belong in proposals, not in the operator-facing release notes.
>
> **Group thematically, not strictly by capability.** Related changes that span capabilities should cluster ("Chat-driven workflows" rather than splitting between `chatops-manager` and `orchestrator-cli`). Internal-only changes (pure refactors, test-coverage-only, doc-only) belong under a footer `### Also included`. If 3+ entries are internal-only, propose `changelog: skip` frontmatter for them and group accordingly.
>
> **Headline the release.** The top of the section gets 3-5 lead items — the changes operators most want to know about. The long tail goes under `### Also included` or analogous footer.

The full prompt template ships in `prompts/changelog-stylist.md` and is the source of truth; the proposal excerpt above is illustrative.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one ADDED requirement: `changelog chatops verb queues an LLM-styled CHANGELOG.md update via the standard triage path`. Names the polling-iteration flow (deterministic extract → stylist prompt → single-PR output), the prompt template's responsibilities, the participation in the revision loop, the 7-day staleness rule, the polite-refusal cases.
  - `chatops-manager` — one MODIFIED requirement (or ADDED — depending on whether the listener's verb table is currently expressed as one requirement enumerating all verbs OR each verb has its own): extends the inbound listener's verb-parsing surface to recognize `changelog` AND submit a `queue_changelog_action` to the daemon control socket.
  - `project-documentation` — one ADDED requirement: `CHATOPS.md and CLI.md document the changelog chatops verb AND the changelog-stylist prompt`.
- **Affected code:**
  - `autocoder/src/chatops/slack.rs` (or wherever the inbound verb parser lives): add `changelog` to the verb table; submit a `ChangelogAction { repo_url, since, to, workspace_override }` over the control socket.
  - `autocoder/src/control_socket.rs` (or equivalent dispatcher): handle the new action by stamping a `ChangelogRequest` state file under `<state_dir>/changelog-requests/<request_id>.json`.
  - `autocoder/src/orchestrator/changelog_triage.rs` (new) — module hosting the polling-iteration handler:
    - `run_changelog_request(request: &ChangelogRequest, workspace: &Path, ...) -> Result<ChangelogOutcome>`:
      - Runs `autocoder changelog --workspace <path> --format json --since <since> --to <to>` (calls the `a05` function directly, no subprocess).
      - Constructs the prompt: the JSON data + the stylist template + a directive to read `<workspace>/CHANGELOG.md` if it exists.
      - Invokes the existing executor in a one-shot mode (similar to triage mode).
      - Captures the diff. Validates that the diff touches `CHANGELOG.md` AND optionally `openspec/changes/<slug>/proposal.md` files (for frontmatter edits). Reject diffs that touch other paths — the LLM's scope is the changelog, not code.
      - Commits the diff to a `changelog-<short-hash>` branch.
      - Pushes AND opens a single PR.
  - `prompts/changelog-stylist.md` (new) — the full prompt template per the excerpt above.
  - `autocoder/src/state/changelog_request.rs` (new) — request-state schema, on-disk format, 7-day pruning logic (parallel to `audit-thread-state` and `proposal-request-state`).
  - `docs/CHATOPS.md` — extend the `Chat-driven workflows` section to add a `### Generating a changelog: \`changelog\`` subsection.
  - `docs/CLI.md` — the existing `## \`changelog\`` entry (from `a05`) gets a cross-link to the chatops verb at the end ("For an LLM-styled draft that opens a PR for review, use the `@<bot> changelog` chatops verb instead").
- **Operator-visible behavior:**
  - `@<bot> changelog coterie` queues the work; the LLM writes a polished `CHANGELOG.md` update to a `changelog-*` branch; the bot opens a PR.
  - Operators iterate via PR comments: `@<bot> revise <text>` re-runs the stylist with the additional constraint.
  - When the operator's revision is structural (e.g. "leave out refactors"), the LLM may also propose `changelog: skip` frontmatter edits to the source proposals — visible in the same PR's diff.
  - First-time runs against a repo without an existing CHANGELOG.md produce a freshly-created file in Keep a Changelog format; the operator can review the formatting choice in the PR before merging.
- **Breaking:** no. The verb is additive. Existing chatops verbs and the deterministic `autocoder changelog` subcommand continue to work unchanged.
- **Acceptance:** `cargo test` passes; `openspec validate a06-chat-driven-changelog --strict` passes. A new integration test using `MockChatOpsBackend` AND a fixture executor exercises the full chat-verb → deterministic-extract → LLM-stylist → PR-open path against a tempdir.
