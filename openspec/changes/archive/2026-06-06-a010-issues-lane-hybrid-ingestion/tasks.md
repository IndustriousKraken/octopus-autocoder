# Implementation tasks

## 1. Read-only ingestion + triage

- [x] 1.1 Read reported GitHub issues read-only, reusing scout's existing issue read (`scout.rs` / the scout issue-read opt-in).
- [x] 1.2 Classify each report via the chat-request-triage primitive (`build_chat_triage_prompt`).
- [x] 1.3 Dedup the report against open issues AND `issues/archive/`; a duplicate produces no candidate.
- [x] 1.4 Draft a candidate `issues/<slug>/` (`issue.md` + `tasks.md`) for a bug-classified report; post it to chatops WITHOUT writing to `issues/` or queuing.

## 2. Maintainer promotion

- [x] 2.1 Reuse the audit "send it" pattern: a maintainer "send it" on a posted candidate writes `issues/<slug>/` AND queues it. Absent promotion, nothing is written or queued.

## 3. Triage routing

- [x] 3.1 Route by classification: Bug → issues-lane candidate; Behavior change → the changes lane as a proposal (reusing the propose/triage path), not an issue; Question / invalid / duplicate → declined or deduped, no work queued.

## 4. Prompt quarantine (`executor`)

- [x] 4.1 For a public-origin issue, embed the body as untrusted DATA inside a robust delimiter (not a markdown fence the body can break) with an explicit untrusted-report framing; source the task and scope from the maintainer-approved classification, never the body.
- [x] 4.2 Rely on single-pass substitution (`a002`) so `{{token}}`-looking text in the body is not expanded during prompt construction.

## 5. Tests

- [x] 5.1 A triaged public issue posts a candidate to chatops and queues nothing.
- [x] 5.2 A maintainer "send it" writes `issues/<slug>/` and queues it; an unpromoted candidate does neither.
- [x] 5.3 A report duplicating an open or archived issue is deduped (no candidate).
- [x] 5.4 A behavior-change report routes to `changes/` as a proposal, not an issue.
- [x] 5.5 A public-origin body is placed in the untrusted-data region; instruction-like text in the body does not become the task (task derives from the classification).
- [x] 5.6 `{{token}}`-looking text in an issue body is not expanded.

## 6. Acceptance gate

- [x] 6.1 `cargo test` passes for the autocoder crate.
- [x] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 6.3 `openspec validate a010-issues-lane-hybrid-ingestion --strict` passes.
