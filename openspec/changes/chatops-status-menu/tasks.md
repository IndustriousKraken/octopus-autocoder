## 1. Parser: recognise bare `status`

- [ ] 1.1 In `autocoder/src/chatops/operator_commands.rs`, add a new variant to the `OperatorCommand` enum:
  ```rust
  pub enum OperatorCommand {
      // existing variants...
      StatusMenu,
  }
  ```
- [ ] 1.2 Extend `parse_command` to recognize `@<bot> status` (mention + verb, no further arguments) as `OperatorCommand::StatusMenu`. The existing `@<bot> status <repo-substring>` continues to parse as `OperatorCommand::Status { repo_substring }`. The check is on argument count after the verb token: zero args → `StatusMenu`; one arg → `Status`; two-or-more args → existing "invalid" error (the per-repo `status` accepts only one positional arg).
- [ ] 1.3 Whitespace tolerance: `@<bot> status` with trailing whitespace AND `@<bot>  status` with extra inter-token whitespace both parse as `StatusMenu`. Case-insensitivity on the verb: `STATUS`, `Status`, `status` all work — same rule as every other verb.
- [ ] 1.4 Tests:
  - `@<bot> status` parses as `StatusMenu`.
  - `@<bot> Status   ` (trailing whitespace + caps) parses as `StatusMenu`.
  - `@<bot> status myrepo` parses as `Status { repo_substring: "myrepo" }`.
  - `@<bot> status myrepo extra` returns the existing "invalid" error (no silent fallback to StatusMenu).

## 2. Dispatcher: aggregate per-repo state

- [ ] 2.1 In `OperatorCommandDispatcher::handle_message`, add a match arm for `OperatorCommand::StatusMenu` that:
  1. Iterates the passed-in `&[RepoIdentity]` slice.
  2. For each entry, submits a `repo_status` action via the existing `ActionSubmitter` (the production submitter routes to the control socket; tests use `FakeSubmitter`).
  3. Collects each `RepoStatusResponse` into a `Vec`. A submitter call that errors (control-socket failure, repo-not-found, etc.) is recorded as an `UnavailableEntry { url, error }` and the menu still ships every other repo's section.
  4. Calls `format_status_menu_reply(&responses, &unavailable)` and returns `Some(Reply::Sync(text))`.
- [ ] 2.2 New control-socket action `repo_status_all` that returns aggregated `RepoStatusResponse` values in one call (one round trip rather than N). Construct by iterating the live `repo_tasks` registry, calling `build_repo_status` per entry, catching per-repo errors and recording them as `Err` variants in the response. The dispatcher uses this action by default; if the daemon does not support it (older builds), the dispatcher falls back to issuing N individual `repo_status` calls.
- [ ] 2.3 Tests:
  - Three-repo fixture with all three returning Ok: dispatcher returns `Some(Reply::Sync(text))` containing all three sections.
  - Three-repo fixture with one submitter call returning Err: the menu reply contains two normal sections + one `(unavailable: <error>)` section.
  - Empty configured-repos slice: returns `Some(Reply::Sync("📊 No repositories configured."))` rather than an empty menu.

## 3. Formatter

- [ ] 3.1 Add `pub fn format_status_menu_reply(responses: &[RepoStatusResponse], unavailable: &[UnavailableEntry]) -> String`. Emits the documented shape:
  ```
  📊 Watching <N> repositories. Reply `@<bot> status <repo-substring>` for details.

    • <url>
      <queue clause> · <busy clause> · <last-iteration clause>

    • <url>
      ...
  ```
  `<N>` includes unavailable entries (they are still "watched"). Unavailable entries render as `(unavailable: <error>)` in place of the summary line.
- [ ] 3.2 Queue clause:
  - When `pending == 0 && waiting == 0 && excluded == 0`: render `empty queue`.
  - Otherwise: render `<N> pending (<list>), <M> waiting (<list>), <K> excluded`. Each list truncates after 5 entries with ` …+N more`; zero-count entries render as `<N> waiting` (no parenthetical for empty lists). Change names pass through the existing `slack_escape` helper.
- [ ] 3.3 Busy clause:
  - `currently_busy == None`: render `idle`.
  - `currently_busy == Some(BusySummary { change, started_at })`: render `working on <change> (started <age> ago)`. The `<change>` field passes through `slack_escape`.
- [ ] 3.4 Last-iteration clause:
  - `last_iteration == Some(LastIteration { finished_at, .. })`: render `last iteration <age> ago`.
  - `last_iteration == None`: render `no iteration yet`.
- [ ] 3.5 Tests:
  - Three-repo fixture with mixed states (idle empty queue, idle non-empty queue, working) produces the documented shape (snapshot test).
  - Six-pending-entry repo truncates to `(a01, a02, a03, a04, a05 …+1 more)`.
  - All-zero queue collapses to `empty queue`.
  - `last_iteration: None` renders `no iteration yet`.
  - Unavailable entry renders `(unavailable: <error excerpt>)` in place of the summary line; the URL line is still present.
  - Empty responses + empty unavailable produces `📊 No repositories configured.` (handled by the dispatcher, but the formatter accepts both empty slices and returns this string for symmetry).
  - Change name containing `<` survives the slack-escape pass and renders as `&lt;`.

## 4. Help-verb update

- [ ] 4.1 Update the `help` verb's reply text to include a line about bare `status`: `\`@<bot> status\` (no repo) — list every watched repository with queue summary, busy state, and last-iteration time. Use `@<bot> status <repo>` for the per-repo detail.`
- [ ] 4.2 The existing test asserting `help` mentions every verb in the current set continues to pass — `status` is already in the list. Add a new assertion that the reply mentions bare `status` produces the menu.

## 5. README documentation update

- [ ] 5.1 In `docs/CHATOPS.md`'s "ChatOps operator commands" section, add a paragraph after the verb table explaining that bare `@<bot> status` returns the per-repo menu. Include a one-paragraph example of the menu reply shape.
- [ ] 5.2 Update the `status` row of the verb table to mention bare-status menu, e.g. append "When called without `<repo-substring>`, returns a per-repo menu listing every watched repository."

## 6. Spec delta

- [ ] 6.1 The ADDED requirement in `openspec/changes/chatops-status-menu/specs/chatops-manager/spec.md` codifies: the `StatusMenu` parser variant, the per-repo aggregation contract, the menu reply shape (queue clause / busy clause / last-iteration clause), the partial-degradation rule when individual repo state cannot be assembled, and the help-verb mention.

## 7. Verification

- [ ] 7.1 `cargo test` passes (new + existing).
- [ ] 7.2 `openspec validate chatops-status-menu --strict` passes.
- [ ] 7.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
