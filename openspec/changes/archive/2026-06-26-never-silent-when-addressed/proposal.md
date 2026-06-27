# The bot is never silent on a request addressed to it

## Why

When an operator addresses the bot — `@<bot>` in chatops, a `@<bot>` PR comment on
GitHub, and any future surface (GitLab, etc.) — and the bot can't act on the
request, it should say so, not sit silent. Today the behavior is inconsistent and
in one place self-contradictory:

- **Chatops:** the dispatcher contract already acknowledges an unrecognized verb
  with a `?` reaction (`chatops-manager`: `None → ? reaction`), and the code does
  this. But the `orchestrator-cli` operator-commands requirement still says
  "unrecognized verbs SHALL be silently ignored" — a stale clause that contradicts
  the `?`-reaction contract.
- **GitHub PR comments:** genuinely silent. A comment that addresses the bot but
  isn't a recognized command (`revise`, `code-review`) — e.g. a forgotten or
  mistyped `revise` — is dropped with no reply. The operator stares at an
  unchanging PR, not realizing they missed a point of syntax.

The fix is a single cross-surface invariant plus the realizations that bring each
surface in line.

## What Changes

- **New standard (`project-documentation`):** when the bot is *addressed* on any
  operator surface and cannot act on the request, it SHALL emit a non-silent
  acknowledgment — a reaction where the surface supports reactions (the chatops
  `?`), or a reply naming what it can do where it does not (a PR comment). A
  message that does NOT address the bot (incidental mention, ordinary chatter) is
  exempt — the bot does not react to normal conversation. "Silent" means no
  acknowledgment of any kind; a `?` reaction is an acknowledgment, not silence.
- **GitHub PR comments (`orchestrator-cli`):** a comment whose first token is
  `@<bot>` but whose verb is not `revise` or `code-review` gets a one-time
  command-affordance reply listing the recognized commands (deduplicated by comment
  id; bot-authored comments are still filtered; gated to authorized commenters to
  avoid public-PR reply abuse). This replaces the current silent drop.
- **Chatops reconciliation (`orchestrator-cli`):** the stale "silently ignored"
  clause is corrected to match the existing `?`-reaction contract — an
  addressed-but-unrecognized message (that also matches no open AskUser question)
  gets the `?` acknowledgment, not silence; a non-addressing message is still
  ignored entirely. The AskUser-reply fall-through is preserved.

## Impact

- Affected specs: `project-documentation` (the new standard), `orchestrator-cli`
  (the PR-comment-affordance requirement, and the operator-commands requirement's
  silent-ignore reconciliation). `chatops-manager`'s `?`-reaction contract is
  already consistent and is referenced, not modified.
- Affected code: the PR-comment dispatcher (`revisions.rs`) gains the affordance
  reply (dedup via the existing comment-notification keying); the chatops listener
  already emits `?` (the change is spec wording, not behavior, on that side).
- Independent change; the PR-comment affordance is the only new behavior — the
  chatops side is a spec-vs-code reconciliation.
