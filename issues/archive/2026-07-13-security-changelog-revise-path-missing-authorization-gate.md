# Changelog-revision PR-comment path is missing the authorization gate

## Symptom

Any GitHub user — including one with `author_association: NONE` on a public repo
— can comment `@<bot> revise <arbitrary text>` on a bot-created `changelog-*` PR
and cause the daemon to run the LLM executor on that text and force-push to the
changelog branch. No authorization check is applied. This is a fully external,
unauthenticated trigger (no prompt injection required to reach it), and it chains
into the sandbox credential/control-socket issues once the executor is running.

## Why

The primary PR-comment revise path gates every non-automatic verb with
`is_comment_authorized(...)`:

```
// src/revisions/process_pr.rs:281-287
if parses_as_verb
    && !is_trusted_automatic
    && !is_comment_authorized(comment, &self.github_cfg.command_authorization)
{
    self.drop_unauthorized_verb(comment, state, latest_seen).await?;
    return Ok(CommentFlow::Continue);
}
```

The changelog-revision loop is a copy of that shape
(`changelog_triage.rs` module doc says "Mirrors the shape of
`revisions::process_revision_requests`") but the gate was dropped in the copy. In
`process_one_changelog_pr_revision` (`changelog_triage.rs:~820-840`) the only
filter before dispatch is the bot-self loop-prevention skip
(`comment.user_login().eq_ignore_ascii_case(bot_username) && !starts_with(MARKER)`);
a non-bot commenter is never skipped. It then calls `parse_revision_trigger` and
goes straight to `re_run_stylist_and_force_push` with the attacker-controlled
`revision_text` — `is_comment_authorized` is never called on this path.

The blast radius of the *committed* result is bounded by the scope check in
`re_run_stylist_and_force_push` (out-of-scope files trigger `reset_hard` +
refuse-to-commit), but the executor still RUNS on attacker input: unauthorized
compute/LLM-cost abuse on demand, plus prompt injection into a repo-write-capable
agent that (per the companion sandbox issues) can then read `secrets.env` and
exfiltrate the GitHub PAT. That elevates this from "annoying" to a genuine
unauthenticated entry point.

## Tasks

- [x] In `process_one_changelog_pr_revision`, after `parse_revision_trigger`
  returns a trigger and before `re_run_stylist_and_force_push`, drop the comment
  when `!is_comment_authorized(&comment, &github_cfg.command_authorization)` —
  mirroring `process_pr.rs:281-287`. Respect `decline_comment` (silent by
  default) and advance `last_seen_comment_at` so the drop is at-most-once.
- [x] Audit for any other copy of the revise/verb-dispatch shape that may have
  dropped the same gate (grep for `parse_revision_trigger` call sites and confirm
  each is preceded by an authorization check or is a trusted-internal path).

## Tests

- [x] An unauthorized commenter's `@<bot> revise ...` on a `changelog-*` PR is
  dropped: the executor is NOT invoked and no force-push occurs.
- [x] An authorized commenter's `@<bot> revise ...` on the same PR still runs the
  stylist and force-pushes (no regression).
