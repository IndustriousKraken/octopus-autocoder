## ADDED Requirements

### Requirement: On-demand code review of a PR, commit, or target
The orchestrator SHALL let an operator request a code review on demand — outside the normal per-pass flow — from one management surface, available BOTH as a CLI subcommand AND as a chatops verb (`@<bot> review <repo-substring> <target>`), resolving the repository by the same selector rule the other operator commands use. The review SHALL run the existing agentic reviewer (its sandbox, `submit_review`, AND reads-on-demand behavior unchanged) AND report the resulting verdict + concerns back to the operator: as a reply in the originating chat channel, AND — when the target is a PR — optionally as a comment on that PR.

The target SHALL be one of:

- `pr <number>` — review the pull request's diff. The diff is resolved from the repository's local clone (the PR's base..head range); the review surface is that diff + its changed files.
- `commit <sha>` — review a single commit's diff (`git show <sha>`).
- `files <path> [<path> ...]` — review the current content of the named files (a TARGET review: no diff; the reviewer reads the files on demand).
- a free-text description — review the area the description names; the reviewer locates the relevant files itself (via `Glob`/`Grep`) AND reviews them (a TARGET review).

The on-demand review SHALL be advisory AND read-only: it SHALL NOT modify code, open a revision, NOR change any marker — it reports findings; the operator decides what to do with them (e.g. roll the implementation back, OR spec/issue a fix). A review whose session fails to produce a valid verdict SHALL surface that failure (per the gatekeepers-fail-closed standard) rather than reporting a clean pass.

A target spanning many files (a broad area, OR the whole codebase) SHALL be SCOPED rather than forced into a single oversized prompt: the orchestrator SHALL chunk the target (e.g. per file or per module) into multiple reviewer sessions AND aggregate their findings into one report, so a large review degrades into bounded sessions rather than overflowing the model's context. When a target is bounded enough for one session, a single session is used.

#### Scenario: Review a PR posts a verdict
- **WHEN** an operator requests `@<bot> review <repo> pr <N>`
- **THEN** the orchestrator resolves the PR's diff from the local clone AND runs the agentic reviewer over it
- **AND** it reports the verdict + concerns to the operator (a chat reply, AND optionally a PR comment)
- **AND** it opens no revision AND modifies no code or marker

#### Scenario: Review a commit
- **WHEN** an operator requests `@<bot> review <repo> commit <sha>`
- **THEN** the reviewer reviews that commit's diff AND the verdict is reported back

#### Scenario: Review a file-set with no diff
- **WHEN** an operator requests `@<bot> review <repo> files src/a.rs src/b.rs`
- **THEN** the reviewer runs a TARGET review over the current content of those files (no diff) AND reports the verdict

#### Scenario: Review a described area, reviewer finds the files
- **WHEN** an operator requests a review by free-text description of functionality
- **THEN** the reviewer locates the relevant files itself (via `Glob`/`Grep`) AND reviews them
- **AND** the verdict names the files it actually reviewed so the operator can see the scope it chose

#### Scenario: A large target is chunked and aggregated, not one giant prompt
- **WHEN** the target spans more files than fit one bounded session (a broad area or the whole codebase)
- **THEN** the orchestrator runs multiple reviewer sessions over chunks (per file or per module) AND aggregates their findings into one report
- **AND** no single session is handed an oversized prompt

#### Scenario: On-demand review is advisory and read-only
- **WHEN** any on-demand review completes with findings
- **THEN** it reports them to the operator AND does NOT open a revision, modify code, or change any marker
- **AND** a session that fails to produce a valid verdict surfaces the failure rather than reporting a clean pass
