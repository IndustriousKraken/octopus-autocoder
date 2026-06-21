## ADDED Requirements

### Requirement: Issue-lane passes are code-reviewed like change passes
A polling pass that produces issue-lane commits — a worked issue whose fix-plus-archive commit rides the pass's push and PR — SHALL run the code reviewer over that work, exactly as a pass that processed a change does. An issue pass SHALL NOT be treated as an audit-only pass (one that only writes spec proposals, validated separately, and is not code-reviewed): an issue carries real code changes, so its PR SHALL receive an automatic code review like any change PR, without requiring an operator to trigger it.

The reviewer step's skip SHALL be scoped to passes with no reviewable code: a pass that processed NEITHER a change NOR an issue (a genuinely audit-only pass, or one that produced nothing) skips the reviewer as before. A pass that processed an issue — alone OR alongside a change — runs the reviewer. This upholds the principle of the `Per-change review falls back to bundled when the change set is empty` requirement (a PR whose context carries a non-empty diff reaches the reviewer rather than skipping the call; no verdict is produced without a completed review) and the gatekeepers-fail-closed posture (code does not ship unreviewed by default).

For an issue pass, the reviewer's brief — the role the archived-change brief plays for a change pass — SHALL be the worked issue's `issue.md` AND `tasks.md`, read from the issue's archive entry, so the reviewer has the issue's intent AND acceptance criteria as context alongside the diff and changed files. When a pass processes both a change AND an issue, the reviewer's context SHALL include both the change brief(s) AND the issue brief(s).

This requirement adds the issue case to the reviewer's scope. It does NOT redefine the change-pass behavior, the reviewer transport (oneshot OR agentic), the verdict/concern handling, the `submit_review` contract, `reviewer.mode` dispatch, the per-PR caps, OR the fail-closed no-submission handling — those are exactly as the existing reviewer requirements specify, applied to the issue pass.

#### Scenario: An issue-only pass is reviewed, not skipped
- **WHEN** a pass produces only issue-lane commits (a worked issue, no processed change)
- **THEN** the reviewer runs over the pass's diff AND changed files (a completed reviewer invocation occurs), via the configured transport
- **AND** the emitted verdict comes from that review, NOT skipped as if the pass were audit-only

#### Scenario: The reviewer's context carries the issue's brief
- **WHEN** the reviewer runs for an issue pass
- **THEN** its context includes the worked issue's `issue.md` AND `tasks.md`, read from the issue's archive entry, as the brief the reviewer would otherwise build from a change's archive entry
- **AND** the issue's diff AND changed files reach the reviewer as for any reviewed pass

#### Scenario: A pass with no reviewable code still skips the reviewer
- **WHEN** a pass processed neither a change nor an issue (audit-only, or nothing)
- **THEN** the reviewer is skipped (there is no implementer-produced code to review)

#### Scenario: A mixed change-and-issue pass reviews both
- **WHEN** a pass produces both a processed change AND a worked issue
- **THEN** the reviewer runs AND its context includes both the change brief(s) AND the issue brief(s)
- **AND** the verdict is derived from a completed review of the pass's combined diff
