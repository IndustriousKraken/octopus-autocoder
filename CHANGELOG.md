# Changelog

All notable changes to autocoder are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v1.3.2] - 2026-07-15

This release fixes the changelog generator's accuracy, adds a full-rebuild mode,
and cleans up a stray agent-sandbox marker.

### Highlights

- **Accurate changelog extraction** — a `--to <tag>` run with no `--since` no longer resolves to an empty range and reports a real release as "no changes", and the extractor now harvests issues-lane corrections and out-of-lane commits, not just OpenSpec changes — so a release whose substance was bugfixes is no longer reported as empty.
- **Full changelog rebuild** — regenerate every section from history under the current coverage rules, so changelogs written before these fixes can be repaired in one verb instead of by hand.
- **No runaway changelog revisions** — a changelog PR's revision state survives the agent-branch prune, so a single `@<bot> revise` comment is no longer re-dispatched every polling iteration.

### Added

- Add a changelog rebuild mode that regenerates every section from history under the current, more complete coverage rules.

### Fixed

- Fix the extractor's degenerate `(vX .. vX]` empty range when `--to` names a tag and `--since` is unset, and broaden harvesting to include issues-lane corrections and out-of-lane commits so bugfix-only releases aren't reported as empty.
- Stop a changelog PR's per-PR revision state from being pruned every polling iteration, so one `@<bot> revise` comment stops being re-dispatched indefinitely.
- Sanitize, exclude, and clean the `ask_user` fallback marker so a verifier-gate or audit role no longer fabricates a phantom change directory, and the marker can't be committed or trip the dirty-workspace check.

## [v1.3.1] - 2026-07-14

This is primarily a security release: it closes the boundary between the
sandboxed, prompt-injectable agent and the host — deployment credentials, the
control socket, and canonical-RAG file reads — and gates the changelog revise
trigger. It also replaces the one-shot `propose` verb with a conversational
`discuss` loop and adds a lightweight roadmap lane.

### Highlights

- **Isolate the agent from the host's secrets** — a prompt-injected agent can no longer read the deployment's GitHub PAT, LLM API keys, or chatops bot token; the three paths that exposed them are closed.
- **Authorize the control socket** — the socket that exposes the full operator-action and gate-verdict surface (and is bridged into every sandbox) now requires authorization instead of accepting any connection.
- **Conversational `discuss`** — replaces one-shot `propose` with a back-and-forth chat loop that refines a request in-thread before it is queued.
- **Roadmap lane** — a lightweight home for speculative or deferred feature ideas that don't warrant an issue or a full OpenSpec change.

### Security

- Prevent a sandboxed agent from reading deployment credentials (GitHub PAT, LLM API keys, chatops bot token) across the three paths that exposed them.
- Require authorization on the daemon control socket, which previously exposed every operator action and gate-verdict submission with no authentication or per-connection identity.
- Stop canonical-RAG spec indexing from following symlinks, so a committed `spec.md` symlink can no longer make the daemon read and hand back arbitrary host files (secrets, SSH keys, `/etc/passwd`).
- Require authorization on `@<bot> revise` comments on bot-created `changelog-*` PRs, closing an external, unauthenticated path to the LLM executor.

### Added

- Add the `discuss` verb — a conversational proposal loop that supersedes the one-shot `propose`.
- Add a roadmap construct for early or deferred ideas that are neither a code defect (issue) nor a specified behavior change.

### Changed

- Park a change on its spec-revision PR: once the revision PR is open it is the blocking signal, so the redundant `.needs-spec-revision.json` marker is cleared and the change waits on the PR.

### Fixed

- Report a clear, deterministic error when a renamed upstream repo's fork name no longer matches, instead of a misleading 60-second "fork not reachable" timeout with a non-existent `reload` remedy hint.
- Refuse an `XDG_RUNTIME_DIR` the current user doesn't own, so `autocoder reload` after `su` without `-` fails legibly instead of with an opaque "Permission denied".

## [v1.3.0] - 2026-07-01

This release completes the migration of every LLM step onto a provider-swappable,
CLI-wrapped "agentic" session, adds the verifier-gate framework
(`[in]`/`[canon]`/`[rules]`/`[out]`) that fails closed, brings first-class GitLab
and GitHub Enterprise support, an OS-level agent sandbox, and the issues lane for
behavior-preserving corrections.

### Highlights

- **Provider-swappable agentic fleet** — every LLM step (reviewer, audits, contradiction checks, and optionally the implementer) now runs through a CLI-wrapped session behind a shared model registry, with `claude`, `opencode` (OpenAI-compatible / Ollama / OpenRouter), and `antigravity` (Google) strategies. The reviewer is agentic by default and reads the diff and files on demand.
- **Verifier gates that fail closed** — four gates — `[in]` (change self-consistency), `[canon]` (change vs. canon), `[rules]` (global engineering rules), and `[out]` (code implements spec) — run around each change and treat "couldn't run" as a hold, never a pass. Run them locally with `autocoder verify <slug>`.
- **First-class GitLab and GitHub Enterprise** — a `Forge` abstraction adds autonomous merge-request creation, review, and triggers for GitLab, and GitHub Enterprise via a self-hosted `api_base`.
- **OS-level agent sandbox** — agentic subprocesses run under a kernel-enforced sandbox (masked home, isolated workspaces, masked credentials) so a model can't read the host's secrets or leak a key into a commit.
- **Issues lane** — a second work path for behavior-preserving corrections that carry no spec delta, with curated entries, maintainer-promoted public GitHub-issue ingestion, and a per-issue perma-stuck gate. On by default.

### Added

**Agentic fleet**

- Extract a single agentic-run primitive and migrate the advisory audits, the code reviewer, and the contradiction check off stdout/HTTP-JSON scraping onto in-session structured submission.
- Add `opencode` and `antigravity` (`agy`) CLI strategies so non-Anthropic providers (OpenAI-compatible, Ollama, OpenRouter, Google) are first-class, and make the implementer strategy-agnostic too.
- Add a shared model registry that drives strategy/model selection for the executor, reviewer, gates, and audits; make an agentic role's API key always optional, falling back to the CLI's own session.
- Make the agentic reviewer the default; it reads the diff and touched files on demand instead of dumping every file into one budget-bounded prompt.

**Verifier-gate framework**

- Add the `[in]`, `[canon]`, `[rules]`, and `[out]` gates and the shared framework that names and positions them around each change.
- Add `autocoder verify <slug>` to run the pre-executor gates locally against the working tree.
- Add a tool-capability probe and a path-less-`api_base` warning so a model that can't emit tool calls, or a misconfigured endpoint, surfaces at config time instead of holding every change with an opaque error.
- Have the `[out]` gate flag stub implementations, and persist each gate's full session log for diagnosis.

**Multi-forge**

- Add a `Forge` provider abstraction with GitHub and GitLab implementations (GitHub Enterprise for free via `api_base`), and route open-issue reads through the forge PAT instead of the `gh` CLI's own credentials.

**Issues lane**

- Add the issues lane: curated `issues/<slug>` corrections and maintainer-promoted ingestion of public GitHub issues, where reporting is not triggering — promotion is the authorization gate.
- Let the bug/gap audits file issues rather than only spec changes, review issue PRs like change PRs, support single-file issues, and add a per-issue perma-stuck gate.

**Security and sandbox**

- Run agentic subprocesses under an OS-level (bubblewrap) sandbox with an exposed-home denylist for the executor and the correct policy for read-only roles.
- Stop plaintext credentials from ever reaching a model or a committed file, bind the control socket into the sandbox, and exclude per-run CLI config artifacts from commits.
- Activate the host's language toolchains (pyenv, nvm, rbenv, …) inside the agent environment so builds resolve the right interpreter.

**Operator recovery**

- Add code rollback/recovery to re-implement merged-but-ungated work under the controls, unconditional rollback as an emergency override that preempts in-flight work, and defer/resume for a unit you want to set aside intact.
- Unify the destructive-confirm interface to one `confirm` verb, add bulk marker clearing across the fleet, and let operators reorder the change queue without renaming directories.
- Add on-demand code review (point the reviewer at a PR, commit, file, or described area) and review-survival provenance (which of a past change's edits still live in current code).

### Changed

- Make "gatekeepers fail closed" a project-wide standard — an inability to run a control is a non-passing state, never a pass — and bring the verifier gates and audits into conformance.
- Drain operator chat requests between changes so a long queue walk can't starve them, and never stay silent when addressed: reply even when the bot can't act on the request.
- Ground the spec-revision `send it` loop in the current contradiction, persist its incremental progress across rounds, hold the busy marker while it runs, clear the revision marker on success, and nudge decomposition when a large change won't converge.
- Make the human `@<bot> revise` cap opt-in (unlimited by default), and stop burning a revision slot when the subprocess never started.
- Add a unified, configurable agentic-session timeout, workspace-cache eviction so build trees don't fill the disk, and one unified rotated daemon log.
- Commit `OCTOPUS.md` to a managed repo (issues, OpenSpec, canon/archive, and gate rules in one place) and document local `verify`.

### Fixed

- Fail the `[out]` gate closed when the change's delta was already archived — it had been silently passing everything — and make the reviewer's failure visible in the PR instead of rendering no review section.
- Rate a credential-leak reviewer finding as `Block`, not `Concerns`, and aggregate reviewer revisions so duplicate findings don't each spend an executor run and a cap slot.
- Degrade gracefully when one repo's fork setup fails instead of crash-looping the whole daemon, and preserve completed work when a branch push is rejected.
- Fix a UTF-8-boundary panic in the issue-triage parser, expand `~` in the global-rules corpus path, and resolve in-change `RENAMED` headers in the archivability pre-flight.
- Serialize workspace git ops so a rollback can't corrupt the index of a concurrent pass, and make the check-only install write a config the binary can discover.
- Surface an open-PR park in `status` (it had shown idle), name the offending paths in a write-policy audit alert, and persist the on-demand audit queue across restarts.

### Also included

- Decompose the 17,943-line `polling_loop.rs` (the project's worst structural-bloat offender) and add a file-size-discipline escalation to the architecture audit.
- Redesign the architecture advisory audit, add canon self-contradiction and canon-consolidation audits, carry full audit-finding bodies through triage, and make audits fail closed and report.
- Deprecate the redundant `executor.command` knob; fall back to a bundled review when a per-change split yields no sub-contexts; improve executor-failure legibility with retry backoff.

## [v1.2.1] - 2026-06-04

This release makes autocoder operable from chat — a full inbound ChatOps command
surface — and closes the PR-comment revision loop end to end. It also hardens the
spec-archive and executor-outcome paths against silent failure and moves daemon
state off `/tmp`.

### Highlights

- **Operate autocoder from chat** — a full inbound command surface (`@<bot> status`, `revise`, `send it`, `audit`, `code-review`, `wipe-workspace`, `clear-perma-stuck`, …) with a status menu, enriched healthy-repo status, threaded audit findings, and at-least-once event dedup.
- **PR-comment revision loop** — `@<bot> revise <text>` on any autocoder-opened PR runs a revision iteration, reviewer-initiated revisions flow through the same path, and the agent now pushes back on requests that would damage the code.
- **On-demand audits and triage** — trigger any audit from chat, and `send it` in an audit thread turns findings into a fixes PR plus a spec PR.
- **Durable state off `/tmp`** — daemon state, markers, and bookkeeping move to a persistent state directory so a reboot no longer wipes them.
- **Structured executor outcomes** — the implementer signals success / needs-revision / another-iteration through MCP tools instead of fragile stdout sentinels.

### Added

**ChatOps command surface**

- Add an inbound Slack command listener with a parser, dispatcher, and control-socket handlers for operator verbs (`status`, `clear-perma-stuck`, `wipe-workspace`, …), documented in the README.
- Add `@<bot> status` with no repo argument to list configured repositories instead of returning `?`, and enrich `@<bot> status <repo>` so a healthy repo reports what the daemon is doing instead of collapsing to one line.
- Deduplicate Slack's at-least-once redeliveries and resolve both user-style (`U…`) and bot-style (`B…`) mentions so mobile and desktop commands both parse.
- Post audit findings in per-audit threads with clear separators, trigger any audit on demand, and run `send it` in an audit thread to open a fixes PR and a spec PR.
- Re-run the reviewer on a PR with `@<bot> code-review`, and notify the chat channel across the whole revise lifecycle, not just on the PR.

**Revision loop**

- Add the `@<bot> revise <text>` PR-comment revision dispatcher and route reviewer-initiated revisions through the same plumbing.
- Add separate caps for operator-initiated and automatic reviewer-marked revisions, and surface the revision's summary in the PR comment it posts.

**Onboarding existing codebases**

- Add the `brownfield` verb to generate canonical specs for an existing capability, plus a survey-and-batch mode that proposes which capabilities to spec and in what order.
- Add a `scout` verb that answers "what's worth looking at?" on an unfamiliar codebase.
- Add OSS fork-contribution support (`spec_storage` for out-of-tree specs, fork-mode PR routing) so autocoder can land targeted PRs on repos the operator doesn't own.

**Install, config, and diagnostics**

- Detect pre-wizard installs so re-running the installer no longer clobbers a hand-written systemd unit or config, and re-run just one wizard section instead of the whole thing.
- Add a config-validation subcommand to check a config against the binary without starting the daemon, and an `inspect` diagnostic subcommand for agent activity and RAG context.
- Add `update.sh` with a startup version notification derived from `git describe`, plus the `autocoder changelog` subcommand and a chat-driven changelog stylist that rewrites it as release notes.
- Add a shared model registry and unify the per-role LLM provider config so a model tuple is declared once, and attribute LLM-produced output to the model that generated it.

**Executor and spec integrity**

- Feed the implementer the canonical specs via an MCP RAG surface so changes are built against the existing contract.
- Replace stdout outcome sentinels with MCP outcome tools, add an "I need another iteration" outcome for honest scope overflow, and recover when an agent exits without signaling any outcome.
- Stream executor output incrementally so a timeout-kill captures partial work instead of a 0-byte log, and split the per-change log into separate prompt/actions/answer/stderr files.
- Add a semantic change-internal contradiction pre-flight and a spec-delta archivability pre-flight before a change reaches the executor.

### Changed

- Run pending changes before audits and bound how many audits run per iteration, so an audit storm can't monopolize the daemon.
- Move daemon state, markers, and workspace bookkeeping off `/tmp` into a persistent state directory, with consistent path resolution across every code path.
- Write `secrets.env` at `0600` atomically instead of chmod-after-write, and gate external GitHub-comment triggers so only authorized users can fire billed work.
- Classify a SIGTERM-killed executor (exit 143 on `systemctl restart`) as a restart, not a failure, and make chatops recovery verbs tolerant of backtick-wrapped change slugs copied from alerts.
- Ship language-neutral default prompts so agents on non-Rust projects don't run `cargo`.

### Fixed

- Harden the spec-rebuild and archive paths against openspec's "exits 0 but archived nothing" silent skip, and order the rebuild by dependency rather than alphabetically.
- Fix the busy marker bricking a repo whose daemon was killed mid-iteration, and stop permanently skipping a repo for the daemon's lifetime on a transient clone/fetch failure.
- Self-heal a workspace that exists but has no `.git/` by re-cloning instead of failing forever.
- Process `@<bot> revise` on fork-PR repos (the head qualifier now respects the fork owner), stop double-processing a single revise comment, and build the revision prompt from the open PR rather than the archived change state.
- Push and open a PR from audit-only iterations so audit-authored proposals stop vanishing.
- Fix the reviewer's auto-revise never firing, a git-fetch pipe deadlock on large fetches, and a scope check rejecting the changelog stylist's own `changelog: skip` edit.
- Skip audits and their writes against a workspace with no `.git/`, coordinate `wipe-workspace` with the in-flight iteration instead of killing it mid-file-op, and fall back to the active path for a change's `proposal.md` when assembling a PR body.

### Also included

- Reviewer: configurable prompt budget with a per-change review mode (honored on re-review too), single-pass prompt substitution, and tests that assert behavior instead of verbatim prompt wording.
- Uniform prompt-override surface across the embedded templates, audit logs that carry the repo URL, and audit-generated changes marked as such in the start-of-work notification and self-validated at authoring time.
- Tests use per-test tempdirs instead of live daemon paths, daemon paths are threaded through APIs instead of a process global, and prompts link to upstream OpenSpec docs.

## [v1.1.1] - 2026-05-24

First tagged release. autocoder is an autonomous, multi-repository daemon that
works through an OpenSpec change queue: it drives a swappable AI executor to
implement each queued change, opens a reviewed pull request per polling pass,
and escalates to a human over chat when it needs a decision.

### Highlights

- **Autonomous multi-repo orchestrator** — a Rust daemon polls every configured repository, drives an AI executor through the OpenSpec change queue, and opens a pull request per pass, with no human in the loop.
- **Fork-based PRs with automated code review** — autocoder pushes to a fork and opens the PR upstream (no upstream write access required), and every PR gets an AI code-quality review before a human merges.
- **ChatOps across Slack, Discord, Teams, Mattermost, and Matrix** — the daemon escalates agent questions to chat, survives restarts mid-conversation, announces opened PRs, and accepts operator recovery commands from the channel.
- **Periodic repository audits** — scheduled drift, missing-tests, security/bug, architecture, and spec-sync audits surface findings and can file new OpenSpec changes.
- **One-command install and pre-built binaries** — `autocoder install` runs a setup wizard, and a GitHub Actions pipeline publishes tagged release binaries.

### Added

**Core orchestrator**

- Add the orchestrator daemon and its `orchestrator-cli`, `openspec-queue-engine`, `executor`, and `git-workflow-manager` capabilities: a backend-agnostic executor implements each queued change while the daemon owns queue, git, ChatOps, and recovery.
- Run multiple repositories concurrently from a single config and one daemon instance.
- Add the `rewind` subcommand (with a `--repo` selector) to roll back an agent branch and recover a repository.
- Add a `spec-needs-revision` executor outcome so the agent can flag tasks it cannot perform from its sandbox instead of failing or faking completion.

**Pull requests and code review**

- Add fork-and-PR mode so autocoder operates without push access to upstream repositories.
- Add an automated AI code-quality review step on each agent branch before human merge.
- Include the implementer agent's own summary in the PR body.
- Cap how many changes are bundled into a single PR (`max_changes_per_pr`) so reviews stay manageable.

**ChatOps**

- Add asynchronous ChatOps escalation: agent questions are routed to a human, conversation state is persisted to disk and resumed when an answer arrives, and other changes keep processing in the meantime.
- Add experimental Discord, Microsoft Teams, Mattermost, and Matrix providers alongside Slack.
- Add a ChatOps notification when a PR is opened.
- Add operator recovery commands that run directly from the chat channel.

**Periodic audits**

- Add a periodic-audit framework that runs repository-wide audits on a cadence, reports findings to ChatOps, and can author new OpenSpec changes.
- Add drift, missing-tests, security/bug, architecture-consultative, and archived-spec-sync audits.

**Configuration and operations**

- Add per-owner GitHub token routing so one daemon can manage repos across multiple personal and org accounts.
- Allow secrets to be written inline in `config.yaml` instead of only through environment variables.
- Add a daemon control socket that hot-reloads tokens, reviewer credentials, and ChatOps config without interrupting in-flight runs.
- Extend hot-reload to the `repositories` list, so repos can be added, removed, or retuned without a restart.

**Install and releases**

- Add the `autocoder install` subcommand with a first-run setup wizard.
- Add a GitHub Actions release pipeline that tags releases and publishes pre-built binaries.
- Extend the install wizard to configure audits.

**Recovery and self-healing**

- Add perma-stuck detection that stops re-running a change which repeatedly fails the same way and alerts the operator, naming both the marker file to clear and the run-log path to inspect.
- Self-heal changes whose implementation is already in `HEAD` by archiving them instead of re-running them forever.
- Add an option to recreate the fork from scratch on workspace re-initialization.
- Add a path to rebuild canonical specs from the change archive, repairing pre-existing spec drift.

### Changed

- Rename the project to **autocoder** and refresh operator-facing naming and CLI ergonomics.
- Archive each change with `openspec archive` so canonical specs in `openspec/specs/` stay in sync (replacing the prior in-process file rename).
- Halt the queue walk on the first non-archived outcome (failed or escalated) instead of continuing to later changes.
- Stagger and jitter per-repository polling so simultaneous fetches don't trip intrusion-detection systems.
- Write more informative PR titles and bodies.

### Fixed

- Treat a "completed" outcome that left the workspace unmodified as a failure, instead of archiving an unimplemented change.
- Skip re-implementing changes that already have an open PR, instead of thrashing the PR branch and erroring on duplicate-PR creation.
- Commit the final change's archive in each pass; previously the last archive of a pass was never committed or pushed.
- Detect archive-destination collisions and broaden perma-stuck handling so a colliding change no longer loops through repeated executor runs.
- Recover automatically from a workspace left dirty by a failed or timed-out run, instead of stalling until manual cleanup.
- Raise a ChatOps alert when the workspace is dirty mid-iteration instead of looping silently.
- Fetch the fork's agent branch at workspace init so `git push --force-with-lease` stops misfiring with "stale info".
- Track the spawned agent's own process group so orphan cleanup can terminate stuck agent subprocess trees.

### Also included

- Expand `config.example.yaml` to document every configurable field.
