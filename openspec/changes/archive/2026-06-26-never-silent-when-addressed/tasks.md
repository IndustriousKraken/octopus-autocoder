# Tasks

## 1. GitHub PR-comment affordance (the genuinely-silent surface)

- [x] 1.1 In `revisions.rs::process_revision_requests`, after a comment fails to parse as `revise` (and `code-review`), detect the "addressed but unrecognized" case: the comment's first non-whitespace token (after stripping leading HTML-comment lines) is `@<bot-username>` but the verb is neither `revise` nor `code-review`. Require the commenter be authorized (the same authorization the `revise` trigger uses). Bot-authored comments are already filtered upstream — keep that filter ahead of this check.
- [x] 1.2 On a match, post a one-time affordance reply: a PR comment listing the recognized commands (`@<bot> revise <text>`, `@<bot> code-review`) and noting the command was not recognized. Do NOT place the example syntax as the reply's first line (so a re-parse cannot match the `revise` trigger).
- [x] 1.3 Deduplicate by the originating comment id (reuse the existing comment-notification dedup keying, e.g. a new `CommentNotifKey` variant) so the every-iteration comment fetch posts the affordance at most once per comment.
- [x] 1.4 A comment that does not address the bot as its first token, or whose author is not authorized, falls through to the existing `None => continue` (ignored, no reply).

## 2. Chatops reconciliation (spec-vs-code; code already emits `?`)

- [x] 2.1 No behavior change is required on the chatops side — the dispatcher already maps `None` to a `?` reaction (`chatops-manager`'s contract). Verify the listener applies the `?` reaction for an addressed-but-unrecognized message AFTER the AskUser-reply path declines, and that a non-addressing message gets no reaction. This task is a consistency check against the reconciled spec wording, not new behavior.

## 3. Tests

- [x] 3.1 PR affordance: an authorized `@<bot> looks good` (or a mistyped verb) comment yields exactly one affordance reply naming the recognized commands; a second iteration over the same comment posts nothing further (dedup); a `revise`/`code-review` comment still triggers its action (regression). Assert behavior (a reply was posted once / the recognized commands are named), not the exact phrasing.
- [x] 3.2 PR non-addressing: a comment not beginning with `@<bot>` (and `cc @<bot>` with the mention not first) yields no affordance reply.
- [x] 3.3 PR authorization: an unauthorized addressed-but-unknown comment yields no reply.
- [x] 3.4 Chatops consistency: an addressed-but-unrecognized message that is not an AskUser reply results in the `?` reaction (not silence); a non-addressing message results in no reaction. (Mirror/extend the existing chatops dispatcher tests.)

## 4. Docs

- [x] 4.1 Update `docs/CHATOPS.md` (and the forge-PR-command docs) to state the invariant: addressing the bot with an unrecognized command yields an acknowledgment — a `?` reaction in chatops, an affordance reply on a PR — never silence; incidental mentions and ordinary conversation are not acknowledged.
