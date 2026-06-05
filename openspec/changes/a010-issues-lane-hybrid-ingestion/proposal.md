## Why

`a009` gave corrections a curated home — a maintainer commits `issues/<slug>/` directly. This change adds the **public path**: the bot triages reported GitHub issues read-only, dedups, drafts a candidate, and posts it to chatops; a maintainer **"send it"**s to promote it, and only then is `issues/<slug>/` written and queued. The public can **report** but cannot **trigger** code work — promotion is the authorization gate.

Because this lane feeds untrusted issue bodies into a **code-writing** executor (unlike scout, which is read-only), **quarantine is load-bearing** and is a required component of this change. Defense in depth: the promotion gate (untrusted content enters only after maintainer approval), the prompt quarantine (the body is data, the task is not), and human merge (the PR is the final backstop). Net effect: an injected issue body can at worst waste compute — it cannot trigger work and cannot ship code.

## What Changes

**Hybrid ingestion.** The bot triages reported GitHub issues read-only (reusing scout's issue read), classifies and dedups each against open AND archived issues, drafts a candidate `issues/<slug>/`, and posts it to chatops **without queuing it**. A maintainer promotes a candidate with a "send it" (reusing the audit send-it pattern); only on promotion does the daemon write `issues/<slug>/` and queue it. Ingestion is gated behind the existing scout issue-read opt-in.

**Triage routing.** Each report is classified: a **Bug** (code drifted from a correct spec) becomes an issues-lane candidate; a **Behavior change** (wants new or changed behavior) is routed to the changes lane as a proposal, not an issue; a **Question / invalid / duplicate** is declined or deduped.

**Prompt quarantine.** A public-origin issue body is embedded as DATA inside a robust delimiter (not a markdown fence the body can break out of) with an explicit untrusted-report framing. The task and scope come from the lane and the maintainer-approved classification, never from the body. Single-pass substitution (`a002`, archived) prevents `{{token}}` expansion of placeholder-looking text inside the body.

## Impact

- **Affected specs:** `orchestrator-cli` — ADD `Hybrid issue ingestion with maintainer promotion`, `Triage routing classifies each report`. `executor` — ADD `Public issue body is quarantined as untrusted data in the implementer prompt`.
- **Affected code:** the ingestion path (reuse scout's `gh api .../issues` read; classify via the chat-request-triage primitive; dedup against open issues and `issues/archive/`; draft a candidate; post to chatops with no queue); the "send it" promotion (reuse the audit send-it pattern) that writes `issues/<slug>/` and queues it; triage routing that sends behavior-change reports to `changes/` as a proposal; the implementer-prompt quarantine (robust delimiter, untrusted-report framing, body-as-data, relying on `a002` single-pass substitution).
- **Operator-visible behavior:** a reported GitHub issue becomes a candidate posted to chatops; nothing is queued until a maintainer "send it"s it; a behavior-change report is routed to `changes/` instead; duplicates (open or archived) are deduped. Public authors can report but cannot trigger work.
- **Dependencies:** stacks on `a009` (the lane). Builds on the archived `a000` (authorization posture) and `a002` (single-pass substitution — required for safe body ingestion). Reuses scout's issue read and the audit send-it pattern.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a010-issues-lane-hybrid-ingestion --strict` passes. Tests: a triaged public issue posts a candidate and queues nothing; a "send it" writes `issues/<slug>/` and queues it; a candidate that is not promoted queues nothing; a duplicate of an open or archived issue is deduped; a behavior-change report routes to `changes/` as a proposal; a public-origin body is delimited as untrusted data and its embedded instructions do not become the task; `{{token}}`-looking text in a body is not expanded.
