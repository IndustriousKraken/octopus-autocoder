# Design

## D1 — The invariant, and what "addressed" / "silent" mean

The standard is a cross-surface UX invariant, sibling to `gatekeepers-fail-closed`.
Two definitions make it precise and keep it from forcing the bot to react to
ordinary chatter:

- **Addressed** — the message directs a command at the bot: the bot mention is the
  leading token of a command attempt (the same "first token is `@<bot>`" rule the
  `revise` trigger and the chatops parser already use). An incidental mention
  ("cc @<bot>") or a message with no mention is NOT addressed.
- **Silent** — no acknowledgment of any kind. A `?` reaction IS an acknowledgment.
  The invariant requires *some* signal when addressed-and-unactionable, not
  necessarily a text reply.

The acknowledgment scales to the surface: a reaction where the surface supports one
(chatops `?`), a reply where it does not (GitHub PR comments have no reaction
affordance for a bot, so a one-time text reply).

## D2 — Chatops is already compliant; the fix there is spec-vs-code

`chatops-manager`'s dispatcher contract already maps `None` (unrecognized) to a `?`
reaction, and the code does this. The only defect is the `orchestrator-cli`
operator-commands requirement's stale "unrecognized verbs SHALL be silently
ignored" clause and its "Unknown verbs are silently ignored" scenario, which
contradict that contract. This change rewords them to the `?` acknowledgment,
preserving two existing behaviors: (a) the AskUser-reply fall-through is tried
before the `?` fires (an answer to an open question is still handled), and (b) a
message that does not address the bot still gets no reaction (the original "no
negative feedback for typos in normal channel chat" intent, now scoped to
non-addressing messages).

## D3 — GitHub PR comments are the genuinely-silent surface

`process_revision_requests` drops a non-matching comment via `None => continue`. The
realization: when a PR comment's first token is `@<bot>` but the verb is not
`revise` or `code-review`, post a one-time affordance reply listing the recognized
commands. Constraints that keep it safe and loop-free:

- **One-time:** deduplicated by comment id (reuse the existing comment-notification
  dedup keying), so the every-pass comment fetch doesn't re-post it.
- **No self-reply / no loop:** bot-authored comments are already filtered before
  parsing; the affordance never replies to the bot's own comments. Its example
  syntax is not placed as the reply's first line, so even a re-parse couldn't match
  the `@<bot> revise` trigger.
- **Authorized only:** gated to authorized commenters (the same authorization the
  `revise` trigger uses), so a public PR can't turn the bot into a reply machine
  for arbitrary mentions. An unauthorized addressed-but-unknown comment is ignored
  as today.
- **Incidental mentions exempt:** only a comment whose *first* token is `@<bot>`
  qualifies; "cc @<bot> fyi" (mention not first) is not an addressed command and is
  ignored.

## D4 — Future surfaces

The standard is stated for "any operator surface," so a future GitLab (or other)
integration inherits the invariant: when it adds a `@<bot>` command surface, it
must acknowledge addressed-but-unactionable requests (reaction or reply). No
per-surface requirement is added for surfaces that do not yet exist.
