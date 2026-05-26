## Why

`@<bot> status <repo-substring>` is the per-repo deep-dive — branches, last commit per branch, latest PR, currently-busy state, queue snapshot. It assumes the operator already knows which repo they care about. `@<bot> status` with no repo argument currently parses as an unknown verb and produces only a `?` reaction (the silent-unrecognized path from `chatops-slack-inbound-listener`), so an operator who doesn't remember the exact substring of a configured repo gets no help.

The natural complement is a menu: bare `@<bot> status` returns a compact one-liner per configured repo with the queue summary, current busy state, and last-iteration timestamp. From that menu the operator picks a repo and re-issues `@<bot> status <substring>` for the full detail. This is a small but high-frequency discovery affordance — the kind of thing operators use weekly without thinking.

Implementation cost is low: the per-repo `RepoStatusResponse` already carries every field the menu needs (queue counts, busy state, last-iteration timestamp). The menu is a thin aggregation across the configured-repos list plus a compact reply formatter.

## What Changes

**Parser recognises bare `status`.** `parse_command` SHALL accept `@<bot> status` with no further arguments as `OperatorCommand::StatusMenu`. The existing `@<bot> status <repo-substring>` continues to parse as `OperatorCommand::Status { repo_substring }`. No ambiguity — the variant is chosen by argument presence.

**Dispatcher returns a per-repo aggregate.** For `StatusMenu`, the dispatcher iterates the live `RepoIdentity` list (the same hot-reloadable slice the per-repo status uses), builds a `RepoStatusResponse` per entry by calling the same handler (`build_repo_status`) the per-repo path uses, and formats the aggregated result via a new `format_status_menu_reply(responses: &[RepoStatusResponse]) -> String`.

**Reply shape.** One leading line announcing the menu and giving the drilldown instruction, followed by one section per repo (two lines each — URL on top, summary on the next line). Example:

```
📊 Watching 3 repositories. Reply `@<bot> status <repo-substring>` for details.

  • git@github.com:acme/widgets.git
    2 pending (a06-foo, a07-bar), 0 waiting, 0 excluded · idle · last iteration 3m ago

  • git@github.com:org-b/another.git
    empty queue · idle · last iteration 5m ago

  • git@github.com:personal/foo.git
    5 pending (a01, a02, a03, a04, a05 …+2 more), 1 waiting (a07-bar), 0 excluded · working on a05-foo (started 2m ago)
```

Compact rules:
- Pending-list truncates to the first 5 entries with ` …+N more` when N > 5.
- When `pending == 0 && waiting == 0 && excluded == 0`, the queue clause collapses to `empty queue` (cuts repetitive zero noise).
- Otherwise the queue clause lists non-zero categories: e.g. `2 pending (a06, a07), 0 waiting, 0 excluded` (the zero entries are kept for symmetry so the operator can confirm the daemon actually checked rather than skipped).
- The busy clause is `idle` OR `working on <change> (started <age> ago)` — identical to the per-repo reply's `currently:` line content.
- The last-iteration clause is `last iteration <age> ago` OR `no iteration yet` (fresh-startup daemon that hasn't run a polling pass on this repo).

**Partial-degradation under failure.** Same contract as the per-repo status: a repo whose state cannot be fully assembled (busy-marker read fails, last-iteration timestamp unavailable) renders with `(unavailable)` in the affected field and a WARN is logged. The menu reply ships every other repo's section regardless.

**Slack-escape.** Repo URLs come from config and are not author-controlled. Change names (in pending / waiting lists) ARE operator-supplied via the OpenSpec change directory naming, but the parser's existing argument-sanitization regex already bounds them to `[a-zA-Z0-9_-]{1,64}`, so they can't contain Slack-special characters. Belt-and-braces: pass them through the existing `slack_escape` helper anyway, consistent with `chatops-status-enrichment`.

**Help verb update.** The `help` verb's reply SHALL mention that bare `@<bot> status` returns the per-repo menu. One-line addition to the existing `help` synopsis.

## Impact

- **Affected specs:** `chatops-manager` — one ADDED requirement covering the bare-status command shape and the menu reply contract.
- **Affected code:**
  - `autocoder/src/chatops/operator_commands.rs` — add `OperatorCommand::StatusMenu` variant, extend `parse_command` to recognize `@<bot> status` with no args, extend the dispatcher to handle the variant by aggregating per-repo responses, add `format_status_menu_reply` formatter, update the `help` verb's text.
  - `autocoder/src/control_socket.rs` — extend the control-socket action surface with a new `repo_status_all` action that aggregates `build_repo_status` across the live `repo_tasks` registry. The aggregation reads each repo's workspace path and config in a single iteration; failures per repo are caught and recorded as partial-degradation rather than failing the whole call.
  - Tests:
    - Parser: bare `@<bot> status` parses as `StatusMenu`; `@<bot> status myrepo` continues to parse as `Status { repo_substring }`; `@<bot> status  ` (trailing whitespace) parses as `StatusMenu`.
    - Formatter: three-repo fixture with mixed states (one idle empty, one idle with pending, one working) produces the documented shape; six-pending-entries fixture truncates with ` …+1 more`; one repo with `(unavailable)` field still ships every other repo's section.
    - Help verb: assert reply contains a phrase mentioning bare `status` returns the menu.
    - Slack-escape: change name containing `<` (despite the parser's allowlist) survives the escape pass.
    - End-to-end: full dispatcher → submitter → control-socket → formatter cycle with a fake submitter and a fixture `RepoIdentity` list.

- **Operator-visible behavior:** bare `@<bot> status` now returns the menu instead of a `?` reaction. Per-repo `@<bot> status <substring>` is unchanged. `@<bot> help` mentions the menu form.
- **Breaking:** no. The current behavior (bare-status → `?` reaction) was a fallthrough of the unrecognized-verb path, not an intentional contract. Operators currently typing `@<bot> status` get an upgrade from "silent ?" to "useful menu."
- **Acceptance:** `cargo test` passes (new + existing). An operator typing `@<bot> status` against a daemon configured with N repos receives a threaded reply containing N two-line sections within ~1 second, with the docfeatured aggregate shape.
