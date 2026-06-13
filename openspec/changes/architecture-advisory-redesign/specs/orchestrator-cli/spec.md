## REMOVED Requirements

### Requirement: Architecture-brightline audit

**Reason:** Replaced by the judgment-based `architecture_advisor`. The
pure-metric checks (function length, duplicate signature, duplicate body)
produced high-volume, frequently-incorrect findings — the function-length
brace-matcher is meaningless for indentation-delimited languages, and file
length already subsumes function length — so the metrics generated double noise
to make one point. The surviving whole-file line count is retained as the
advisor's internal selector (not a finding). The `.brightline-ignore`
suppression file is removed with the duplicate-signature/body metrics that were
its only consumer.

### Requirement: Architecture consultative audit

**Reason:** Replaced by `architecture_advisor`, which keeps the read-only
judgment pass but ends in an actionable recommendation rather than an
unactionable question, and is bounded by a cheap selector rather than
free-scanning the whole tree.

### Requirement: Consultative audit prioritizes oversized, low-cohesion code

**Reason:** Folded into `architecture_advisor`'s judgment criteria (reason about
cohesion rather than raw size; flag a low-cohesion file, leave a large-but-
cohesive file alone). The standalone consultative audit this requirement refined
is removed.

## MODIFIED Requirements

### Requirement: Registered periodic audits
autocoder SHALL register exactly the following audits in its `AuditRegistry` at startup, identified by their `audit_type()` slug: `architecture_advisor`, `drift_audit`, `missing_tests_audit`, `security_bug_audit`, `canon_contradiction_audit`, `canon_consolidation_audit`. The slugs `dependency_update_triage`, `architecture_brightline`, AND `architecture_consultative` SHALL NOT be registered. Each registered audit's cadence is independently configurable under `audits.defaults` and per-repo `repositories[].audits` overrides; an unregistered slug present in either location SHALL fail config validation at startup with the existing "unknown audit type" error message that lists the registered slugs.

This enumeration is the canonical contract for which audits exist. Future changes that add or remove an audit MUST update this requirement in the same commit so the spec and the registered set never drift. The `validate_audit_type_names` startup check enforces the spec/code consistency at runtime: an operator's YAML naming an unregistered slug is a startup-time failure with a clear list of valid slugs.

#### Scenario: Startup with default config registers the canonical set
- **WHEN** autocoder starts with a config whose `audits:` block is
  absent OR present but with all-`disabled` cadences
- **THEN** the in-memory `AuditRegistry` contains exactly the six
  audits enumerated above
- **AND** no audit runs (all are `Disabled` by effective cadence),
  preserving prior daemon behavior

#### Scenario: Operator configures a registered audit
- **WHEN** an operator sets a non-`disabled` cadence under
  `audits.defaults.<slug>` for any of the six registered slugs
  OR under `repositories[].audits.<slug>`
- **THEN** config validation succeeds AND the scheduler invokes
  that audit per its cadence on the appropriate iteration

#### Scenario: Operator configures a removed audit slug
- **WHEN** an operator's `audits.defaults` (or
  `repositories[].audits`, or `audits.settings`) contains a slug
  that was registered in an earlier version of autocoder but has
  since been removed (`dependency_update_triage`,
  `architecture_brightline`, OR `architecture_consultative`)
- **THEN** `validate_audit_type_names` fails at startup with an
  error naming the unknown slug AND listing the registered slugs
  so the operator knows what to use
- **AND** the daemon does NOT start (consistent with the existing
  behavior for typos in audit slugs); the operator must remove the
  entries from their YAML to recover

#### Scenario: Adding or removing an audit requires updating this requirement
- **WHEN** an implementing agent ships a change that registers a
  new audit (extending the registry list) or removes one (deleting
  a registration)
- **THEN** the change's spec delta MUST update this requirement's
  enumeration so the canonical list reflects the new state
- **AND** the change's commit SHOULD also update the
  `validate_audit_type_names` known-slug list, the README audit
  table, and `config.example.yaml` so all four artifacts (spec,
  validator, README, example) stay aligned

### Requirement: Completed triage splits into one or two PRs by content path
After the triage executor returns `Completed`, the daemon SHALL inspect the working tree's changed paths AND keep ONLY paths inside the triage's output subtree for this run — `openspec/changes/<derived-slug>/` for a spec change OR `issues/<derived-slug>/` for a behavior-preserving issue. Each path outside the kept subtree (code fixes, doc edits, ANY other content) SHALL be reverted to its committed (HEAD) state BEFORE the PR commit, by a strategy chosen by where the path lives: a tracked path PRESENT in HEAD (a modification, deletion, type-change, OR the source side of a rename) is restored — BOTH the index AND the worktree — via `git checkout HEAD -- <path>`, so a code edit the executor staged with `git add` cannot survive; a tracked path ABSENT from HEAD (a brand-new file the executor created AND staged — porcelain `A ` — OR a rename destination) is unstaged via `git reset HEAD -- <path>` AND removed from disk; an untracked addition is removed from disk via `std::fs::remove_file` / `remove_dir_all`. The not-in-HEAD case SHALL NOT be reverted with `git checkout HEAD` / `git restore --source=HEAD`, which abort with a "pathspec did not match any file(s) known to git" error for a path absent from HEAD on some git versions — exactly the common case where the executor `git add`ed a new code file. If any out-of-scope write cannot be reverted or removed, the daemon SHALL abort before the PR commit rather than allow the write to leak into the PR. At most ONE PR is created per triage run — the spec PR OR the issue PR. Code fixes flow through the standard implementer pipeline on a subsequent polling iteration after the operator merges the PR.

When the discard step drops non-empty out-of-scope paths (the agent wrote code despite the prompt's restriction), the daemon SHALL emit a WARN log naming the dropped paths AND post a chatops reply in the audit-thread naming the dropped paths AND directing the operator to capture the dropped fixes as `tasks.md` items if they were load-bearing.

When the discard step leaves NO content in EITHER `openspec/changes/<derived-slug>/` OR `issues/<derived-slug>/` (the agent wrote only code, or nothing), NO PR is created AND the daemon posts a chatops reply in the audit-thread naming `no spec or issue content produced; retry with a clearer directive`. The audit-thread's `status` flips to `TriageFailed`.

When the discard step leaves content in the kept subtree, the daemon SHALL create the branch off the same base, commit the kept paths with a lane-appropriate subject (`audit-triage spec proposal from <audit_type>` for a spec change, `audit-triage issue from <audit_type>` for an issue), push the branch, AND open the PR via the existing PR-creation helpers. On merge of an issue PR, the issues-lane walker picks up `issues/<derived-slug>/` and implements it through the standard pipeline. PR-body text describes the content AND does NOT cross-link to any fixes PR (there is no fixes PR).

#### Scenario: Mixed diff produces one spec PR; code paths are discarded with chatops warning
- **GIVEN** the triage executor's Completed working tree contains BOTH new files in `openspec/changes/audit-fix-x/` AND modifications to `src/foo.rs`
- **WHEN** the audit-triage completion handler runs
- **THEN** `src/foo.rs` is reverted to its base-branch (HEAD) state — BOTH the index AND the worktree — BEFORE the commit (via `git checkout HEAD -- src/foo.rs`, since it exists in HEAD; a not-in-HEAD addition would instead be unstaged via `git reset HEAD --` AND removed from disk), so a code edit the executor staged with `git add` cannot survive into the spec commit
- **AND** the working tree's `src/foo.rs` reverts to the base-branch state
- **AND** a WARN log fires naming the audit type, the derived slug, AND `src/foo.rs` as the dropped path
- **AND** the daemon creates a spec branch + PR with ONLY `openspec/changes/audit-fix-x/` paths
- **AND** the PR body does NOT mention a companion fixes PR
- **AND** the daemon posts a chatops reply in the audit-thread naming `src/foo.rs` as dropped AND explaining that code fixes go through the standard implementer pipeline; the spec PR has been opened; if the dropped fixes were load-bearing, revise the spec to capture them as tasks.md items
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: A staged brand-new code file is discarded without a pathspec error
- **GIVEN** the triage executor's Completed working tree contains new files in `openspec/changes/audit-fix-x/` AND a brand-new file `src/new.rs` the executor created AND staged with `git add` (porcelain `A `, absent from HEAD)
- **WHEN** the audit-triage completion handler runs
- **THEN** `src/new.rs` is unstaged via `git reset HEAD -- src/new.rs` AND removed from disk, NOT reverted with `git checkout HEAD` / `git restore --source=HEAD` (which would abort with a pathspec error for a path absent from HEAD)
- **AND** the discard step does NOT error AND the triage flow proceeds to open the PR
- **AND** `src/new.rs` is named among the dropped paths in both the WARN log AND the chatops reply
- **AND** the PR's diff contains ONLY `openspec/changes/audit-fix-x/` paths

#### Scenario: A refactor triage produces one issue PR
- **GIVEN** the triage acted on an architecture_advisor recommendation AND the executor's Completed working tree contains new files in `issues/<derived-slug>/` (`issue.md` AND `tasks.md`, no `specs/` directory) AND modifications to `src/foo.rs`
- **WHEN** the audit-triage completion handler runs
- **THEN** `src/foo.rs` is reverted by the same per-path strategy AND only `issues/<derived-slug>/` is kept
- **AND** the daemon commits with subject `audit-triage issue from architecture_advisor` AND opens ONE issue PR
- **AND** no `openspec/changes/` spec proposal is created
- **AND** on merge the issues-lane walker picks up `issues/<derived-slug>/`

#### Scenario: Spec-only triage produces one spec PR with no warning
- **GIVEN** the triage executor's Completed working tree contains ONLY new files in `openspec/changes/audit-fix-x/`
- **WHEN** the audit-triage completion handler runs
- **THEN** the discard step finds no paths to drop AND emits NO WARN log
- **AND** the spec branch + PR is created with the spec content
- **AND** NO chatops warning is posted (the agent followed the restriction)
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: Code-only triage produces NO PR; chatops reply explains no content
- **GIVEN** the triage executor's Completed working tree contains ONLY modifications to `src/foo.rs` (no `openspec/changes/<derived-slug>/` AND no `issues/<derived-slug>/` content)
- **WHEN** the audit-triage completion handler runs
- **THEN** the discard step restores `src/foo.rs` to the base-branch state
- **AND** no branch is created AND no PR is opened
- **AND** the daemon posts a chatops reply in the audit-thread naming `no spec or issue content produced; retry with a clearer directive`
- **AND** the audit-thread's `status` flips to `TriageFailed`

#### Scenario: Empty-diff triage posts a no-action reply
- **GIVEN** the triage executor returns `Completed` but the working tree's diff is empty (the LLM decided nothing was actionable)
- **WHEN** the audit-triage completion handler runs
- **THEN** no PRs are created
- **AND** the bot posts a reply in the audit thread containing the LLM's final-summary text explaining the decision
- **AND** the audit-thread's `status` flips to `Acted`

#### Scenario: Slug collision is suffixed
- **GIVEN** the derived slug `<audit-type>-<hash>` already exists as the kept subtree (`openspec/changes/<slug>/` OR `issues/<slug>/`)
- **WHEN** the audit-triage completion handler builds the output dir
- **THEN** the daemon increments a suffix (`-2`, `-3`, ...) until it finds a free path
- **AND** the resulting directory uses the suffixed slug

## ADDED Requirements

### Requirement: Architecture advisory audit
autocoder SHALL register an `architecture_advisor` audit in the periodic-audit framework, replacing `architecture_brightline` AND `architecture_consultative`. The audit is `requires_head_change = true` AND `WritePolicy::None`.

The audit SHALL select a bounded set of candidate files using a cheap, language-agnostic signal — whole-file line count — taking ONLY the longest files whose line count exceeds a configurable pain threshold, up to a configurable candidate cap; NOT every file over the threshold. The line count is a SELECTOR ONLY: it determines which files the audit examines AND SHALL NOT be emitted as a finding. There is no function-length, duplicate-signature, or duplicate-body metric, AND no `.brightline-ignore` file.

For each selected candidate the audit SHALL invoke the wrapped agent CLI with a read-only sandbox AND a prompt directing the agent to read the file (AND the surrounding context needed to judge cohesion AND placement) AND return a professional recommendation: whether the file warrants refactoring, the nature of the problem (oversized, a low-cohesion "junk drawer", a single oversized function, OR a monolith better wrapped than decomposed), AND a concrete recommended action grounded in the project's own language, architecture, AND patterns. The prompt SHALL forbid snark AND generic best-practice lecturing, AND SHALL require each recommendation to carry a `file` or `file:line-range` anchor.

The audit SHALL return findings as `AuditOutcome::Reported(findings)`, ranked, capped at a small maximum (5). A run that examines its candidates AND finds none worth refactoring SHALL return `AuditOutcome::Reported(vec![])`; the audit-run log SHALL record the candidates examined AND the no-recommendation conclusion so the run carries evidence it looked.

#### Scenario: The audit selects only the longest files over the pain threshold
- **WHEN** the audit runs against a repository with many files of varying length
- **THEN** it examines only the longest files whose line count exceeds the configured threshold, up to the configured candidate cap
- **AND** a file's line count selects it for examination AND is never emitted as a finding

#### Scenario: Each recommendation is actionable and anchored
- **WHEN** the audit judges a selected file to warrant refactoring
- **THEN** the finding states what is wrong, why it matters, AND the recommended action
- **AND** it carries a `file` or `file:line-range` anchor
- **AND** it is phrased as a recommendation, not a question, with no snark

#### Scenario: A large but cohesive file is not flagged
- **WHEN** a selected file exceeds the size threshold but implements a single cohesive responsibility
- **THEN** the audit does NOT recommend splitting it on size alone
- **AND** that file does not consume one of the capped recommendation slots

#### Scenario: A clean run records what it examined and stays quiet
- **WHEN** the audit examines its candidates AND none warrant refactoring
- **THEN** it returns `AuditOutcome::Reported(vec![])`
- **AND** the audit-run log records the candidates examined AND the no-recommendation conclusion
- **AND** no chatops post is sent unless `notify_on_clean: true` is set

### Requirement: Audit triage routes behavior-preserving work to the issues lane
When an operator acts on an advisory audit finding via `send it`, the audit-reply triage SHALL choose its output lane by the nature of the work. A behavior-preserving correction or refactor — one that changes no observable contract — SHALL be drafted as an issue (`issues/<derived-slug>/` containing `issue.md` AND `tasks.md`, with NO `specs/` directory). A spec change (`openspec/changes/<derived-slug>/`) SHALL be produced ONLY when the work requires altering an observable contract (public API, serialized/wire format, CLI surface) OR surfaces a new capability decision that belongs in canon. For architecture-advisor refactor findings the default SHALL be an issue.

#### Scenario: A refactor recommendation becomes an issue
- **WHEN** triage acts on an `architecture_advisor` recommendation to decompose an oversized file AND no contract change is required
- **THEN** it drafts `issues/<derived-slug>/` with `issue.md` AND `tasks.md` AND no `specs/` directory
- **AND** it does NOT create an `openspec/changes/` spec proposal

#### Scenario: A contract-changing cleanup becomes a spec change
- **WHEN** the recommended cleanup cannot be done without changing a public API, serialized/wire format, OR CLI surface
- **THEN** triage drafts an `openspec/changes/<derived-slug>/` spec proposal instead of an issue
- **AND** the issues lane is not used for that finding

### Requirement: Audit metrics do not become canonical requirements
No audit, AND no triage acting on an audit's findings, SHALL author a canonical requirement (a `SHALL`-bearing spec requirement) whose content is an audit's own selection or detection metric — a file-size threshold, a function-length threshold, a duplication count, OR a similar heuristic. Such metrics are signals for where to apply judgment, not contracts. Engineering discipline of this kind, where the project chooses to record it at all, belongs as advisory guidance in project documentation, NOT as an enforceable requirement that the change-vs-canonical gate AND future audits treat as binding.

#### Scenario: An audit threshold is not promoted to a requirement
- **WHEN** an audit surfaces files or functions exceeding a size threshold AND an operator acts on the finding
- **THEN** no resulting spec requirement states that files or functions SHALL stay within that threshold
- **AND** the threshold remains a heuristic the audit uses to select candidates

#### Scenario: Recorded discipline lives in documentation, not canon
- **WHEN** the project wants to record a size or organization convention
- **THEN** it is captured as advisory guidance in project documentation
- **AND** it is NOT written as a `SHALL` requirement enforced against future changes
