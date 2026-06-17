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
autocoder SHALL register exactly the following audits in its `AuditRegistry` at startup, identified by their `audit_type()` slug: `architecture_advisor`, `drift_audit`, `missing_tests_audit`, `security_bug_audit`, `documentation_audit`, `canon_contradiction_audit`, `canon_consolidation_audit`. The slugs `dependency_update_triage`, `architecture_brightline`, AND `architecture_consultative` SHALL NOT be registered. `spec_sync_audit` — the deterministic, no-LLM spec-sync audit — is configurable under `audits.defaults` and `repositories[].audits` but is NOT an `AuditRegistry` entry: it runs via the spec-sync rebuild path rather than the LLM audit framework, so it is a recognized audit slug, not an unknown one. Each registered audit's cadence is independently configurable under `audits.defaults` and per-repo `repositories[].audits` overrides; a slug that is neither a registered audit NOR the recognized `spec_sync_audit` present in either location SHALL fail config validation at startup with the existing "unknown audit type" error message that lists the valid slugs.

This enumeration is the canonical contract for which audits exist. Future changes that add or remove an audit MUST update this requirement in the same commit so the spec and the registered set never drift. The `validate_audit_type_names` startup check enforces the spec/code consistency at runtime: an operator's YAML naming a slug that is neither a registered audit nor the deterministic `spec_sync_audit` is a startup-time failure with a clear list of valid slugs.

#### Scenario: Startup with default config registers the canonical set
- **WHEN** autocoder starts with a config whose `audits:` block is
  absent OR present but with all-`disabled` cadences
- **THEN** the in-memory `AuditRegistry` contains exactly the seven
  audits enumerated above
- **AND** no audit runs (all are `Disabled` by effective cadence),
  preserving prior daemon behavior

#### Scenario: Operator configures a registered audit
- **WHEN** an operator sets a non-`disabled` cadence under
  `audits.defaults.<slug>` for any of the seven registered slugs
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

#### Scenario: The deterministic spec_sync_audit slug passes validation
- **WHEN** a config (or the install wizard's conservative default)
  sets `audits.defaults.spec_sync_audit` to a non-`disabled` cadence
- **THEN** `validate_audit_type_names` succeeds — `spec_sync_audit`
  is a recognized deterministic audit slug, not an unknown one
- **AND** the daemon starts (the install wizard's conservative
  default does not prevent startup)

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

### Requirement: Audit cadence config schema
autocoder SHALL accept an optional top-level `audits:` block with `defaults:` (global) and per-repository `audits:` overrides. Each entry maps an audit type name to a `Cadence`. The `Cadence` enum SHALL accept the literal strings `disabled`, `daily`, `every-N-days` (where `N` is a positive integer), `weekly`, `monthly`, `quarterly`. Every audit defaults to `disabled` when unset in both global defaults and per-repo overrides.

#### Scenario: Per-repo cadence overrides global default
- **WHEN** `audits.defaults.architecture_advisor: weekly` AND a
  repository sets `audits.architecture_advisor: every-3-days`
- **THEN** the effective cadence for that repository is
  `every-3-days`

#### Scenario: Audit absent from both global and per-repo is disabled
- **WHEN** the operator's config has no entry for an audit type
  in either `audits.defaults` or any `repositories[].audits`
- **THEN** the audit's effective cadence is `disabled` AND the
  framework never invokes it

#### Scenario: every-N-days requires a positive integer
- **WHEN** a config entry uses `every-N-days` where N is `0` OR
  negative OR non-integer
- **THEN** config load fails at startup with an error naming the
  offending field path AND the parsed value

#### Scenario: Unknown audit type names fail config load
- **WHEN** a config entry under `audits.defaults` or
  `audits` (per-repo) uses a name that does not match a
  registered audit type
- **THEN** config load fails at startup with an error naming
  the field path AND the unknown audit type AND listing the
  known audit type names
- **AND** the daemon does NOT start

### Requirement: Periodic audits enforce their per-audit subprocess timeout
Every audit that spawns the wrapped agent CLI as a child process — including `drift_audit`, `architecture_advisor`, `missing_tests_audit`, `security_bug_audit`, AND `documentation_audit` — SHALL kill the child and return `Err(_)` once the elapsed wall-clock time exceeds `executor.timeout_secs`. The error message SHALL name both the audit type and the timeout condition so the operator can tell from a single log line which audit hung and why. The audit log file SHALL record the timeout outcome before the error returns so post-mortem inspection of `/tmp/autocoder/logs/<basename>/audits/<audit_type>-<ts>.log` is conclusive.

#### Scenario: drift_audit subprocess exceeds timeout
- **WHEN** `DriftAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured `executor.command` is a script that sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose `format!("{err:#}")` contains the substring `drift_audit` AND the substring `timeout`
- **AND** the audit log file written via the audit's `AuditLogWriter` contains a `kind: Err` section together with the substring `reason: timeout`
- **AND** the spawned child process does not survive past the call's return (no orphaned `sleep` left behind)

#### Scenario: architecture_advisor subprocess exceeds timeout
- **WHEN** `ArchitectureAdvisorAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured command sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose message contains `architecture_advisor` AND `timeout`
- **AND** the audit log file contains a `kind: Err` / `reason: timeout` section

#### Scenario: specs-writing audit (via missing_tests) subprocess exceeds timeout
- **WHEN** `MissingTestsAudit::run` is invoked with `executor_timeout_secs = 1` AND the configured command sleeps longer than the timeout
- **THEN** the call returns `Err(_)` whose message contains `missing_tests_audit` AND `timeout`
- **AND** no new directory is created under `<workspace>/openspec/changes/` as a side-effect of the timed-out run (defense-in-depth against the spec-writing audit's commit step running on a child that never finished)

### Requirement: Install wizard configures periodic audits
The `autocoder install` wizard SHALL prompt operators about periodic audits during first-time install, after the reviewer prompt and before the config-assembly step. The wizard offers a three-tier UX: (1) inline prompt for `spec_sync_audit` with default ON at daily cadence (cheap, defensive, no LLM cost); (2) a single yes/no gate for the LLM-driven audits (default no — operators who don't want a tour answer once and move on); (3) a fast-path "enable all at recommended cadences" question for operators who answered yes to the gate, with per-audit walk-through as the fallback when the fast path is declined. The non-interactive mode SHALL mirror this with flags whose defaults match the conservative interactive defaults so existing IaC scripts that don't know about the new flags continue to work without behavior change.

#### Scenario: Default interactive path enables spec_sync_audit only
- **WHEN** an operator runs `autocoder install` AND accepts
  every audit-related default (bare-Enter on the spec-sync
  cadence prompt → `daily`; bare-Enter on the LLM-driven
  gate → `no`)
- **THEN** the wizard writes `audits.defaults.spec_sync_audit: daily`
  to config.yaml AND no other audit entries
- **AND** the operator's total interaction with the audits
  section is two prompts (cadence + gate)

#### Scenario: Operator declines spec_sync_audit
- **WHEN** the operator answers `n` (never) to the spec-sync
  cadence prompt
- **THEN** the wizard skips the LLM-driven-audits gate
  AND any subsequent per-audit prompts
- **AND** the rendered config.yaml omits the `audits:`
  block entirely (matching the `Option<AuditsConfig>`
  schema's `None` representation)

#### Scenario: Fast-path enables all five audits
- **WHEN** the operator chose a non-disabled cadence for
  spec-sync AND answered `y` to the LLM-driven-audits gate
  AND accepted the fast-path default `Y` on the "enable all
  at recommended cadences" prompt
- **THEN** config.yaml contains all five audits at their
  recommended cadences:
  - `spec_sync_audit`: per the operator's spec-sync answer
  - `architecture_advisor`: weekly
  - `drift_audit`: weekly
  - `missing_tests_audit`: monthly
  - `security_bug_audit`: weekly
- **AND** total wizard interaction in this branch is three
  prompts (spec-sync cadence + LLM gate + fast-path
  acceptance)

#### Scenario: Individual cadence walk-through after declining fast-path
- **WHEN** the operator answered `y` to the LLM-driven gate
  AND `n` to the fast-path prompt
- **THEN** the wizard prompts for each of the four LLM-driven
  audits individually: slug + description + cadence choice
  (with the recommended cadence as the default)
- **AND** each audit's chosen cadence appears in
  `audits.defaults` UNLESS the operator chose `never`
  (those audits are omitted)
- **AND** the resulting config.yaml's audit count matches
  the operator's non-disabled choices (spec-sync + each LLM
  audit the operator did NOT decline)

#### Scenario: Non-interactive defaults match conservative interactive defaults
- **WHEN** an operator runs `autocoder install --non-interactive`
  with all the existing-spec's required flags AND NO new
  `--audits-*` flags
- **THEN** config.yaml contains exactly
  `audits.defaults.spec_sync_audit: daily` (the
  conservative default matching the interactive default-default)
- **AND** existing IaC scripts (Ansible playbooks, cloud-init,
  etc.) that pre-date this change continue to produce a
  working install without surprise behavior change

#### Scenario: Non-interactive recommended preset
- **WHEN** an operator runs
  `autocoder install --non-interactive --audits-llm-driven recommended`
  with all other required flags
- **THEN** config.yaml contains all five audits at their
  recommended cadences (same as the interactive fast-path)
- **AND** no per-audit `--audit-<slug>` flag is required

#### Scenario: Non-interactive per-audit override within recommended preset
- **WHEN** the operator passes
  `--audits-llm-driven recommended --audit-security-bug-audit disabled`
- **THEN** three of the four LLM-driven audits get their
  recommended cadences AND `security_bug_audit` is omitted
  from config.yaml (treated as disabled)
- **AND** spec-sync follows its own `--audits-spec-sync`
  flag (or default `daily` if unset)

#### Scenario: --audits-llm-driven none master switch overrides per-audit flags
- **WHEN** the operator passes
  `--audits-llm-driven none --audit-architecture-advisor weekly`
- **THEN** architecture_advisor is NOT enabled (the
  master switch wins)
- **AND** the rendered config.yaml has no
  architecture_advisor entry
- **AND** the wizard emits a one-line stdout note explaining
  that the per-audit flag was overridden by the master
  switch (so IaC logs distinguish "operator opted-out

### Requirement: LLM-driven audits validate their generated proposals before committing
Every LLM-driven audit that writes a proposal (currently `missing_tests_audit` AND `security_bug_audit`) SHALL invoke `openspec validate <slug> --strict` against its just-written `openspec/changes/<slug>/` directory before returning success. The advisory audits — `architecture_advisor`, `drift_audit`, AND `documentation_audit` — generate findings rather than a spec proposal AND are unaffected by this requirement. When validation passes, the audit returns its existing outcome variant. When validation fails AND the configured retry budget is not exhausted, the audit SHALL re-invoke its LLM with the validation error appended to the prompt and overwrite the change directory with the new response. When validation fails AND the retry budget IS exhausted, the audit SHALL discard the change directory AND post a chatops failure notification AND return a `ValidationExhausted` outcome.

#### Scenario: Valid proposal on first attempt
- **WHEN** an LLM-driven audit writes a proposal and `openspec validate <slug> --strict` exits 0 on first invocation
- **THEN** the audit returns its existing success outcome with `retries_used == 0`
- **AND** no retry is attempted
- **AND** no chatops failure notification fires

#### Scenario: Validation passes after one retry
- **WHEN** an LLM-driven audit writes an invalid proposal on attempt 0 AND `audits.max_validation_retries` is 1 AND the LLM produces a valid proposal on attempt 1 (with the prior validation error appended to its prompt)
- **THEN** the audit returns its existing success outcome with `retries_used == 1`
- **AND** the chatops notification (when `notify_on_clean=true` for this audit) includes the clause `validated on retry 1 of 1`
- **AND** the change directory at `openspec/changes/<slug>/` contains the second (valid) proposal, not the first

#### Scenario: Retry budget exhausted
- **WHEN** an LLM-driven audit writes invalid proposals on both attempt 0 and attempt 1 with `audits.max_validation_retries == 1`
- **THEN** the audit returns `AuditOutcome::ValidationExhausted { audit_type, retries_attempted: 1, final_error }`
- **AND** the `openspec/changes/<slug>/` directory does NOT exist after the call
- **AND** no commit is made to git
- **AND** a chatops `❌` notification is posted to the repo's resolved channel containing the audit type, the retry count, and a truncated excerpt of the final validation error

#### Scenario: max_validation_retries = 0 disables retries
- **WHEN** an LLM-driven audit writes an invalid proposal on the first attempt AND `audits.max_validation_retries == 0`
- **THEN** the audit returns `ValidationExhausted { retries_attempted: 0, .. }` immediately
- **AND** no second LLM call is made
- **AND** the discard-and-notify path runs the same as the exhausted case above

#### Scenario: Validation retry passes validation error in addendum
- **WHEN** the retry path invokes the LLM on attempt N > 0
- **THEN** the LLM prompt contains an addendum naming the previous attempt's openspec validation error verbatim
- **AND** the LLM's response replaces the change directory entirely (delete-and-rewrite, not patch)

### Requirement: Audit posts a chatops notification when it creates a queue-bound proposal
Every LLM-driven audit that writes a proposal (`missing_tests_audit` AND `security_bug_audit`) SHALL post a chatops notification immediately after `openspec validate <slug> --strict` passes for its just-written proposal AND before the audit function returns to the scheduler. The notification names the audit type, the change slug, and a one-line excerpt of the proposal's `## Why` section, so operators have clear provenance when the next polling iteration begins implementing the change. The notification fires regardless of the audit's `notify_on_clean` setting, since it signals "something was found" rather than "nothing was found." The advisory audits — `architecture_advisor`, `drift_audit`, AND `documentation_audit` — generate findings rather than a proposal AND do not post the `🔍 created proposal` notification.

#### Scenario: Validated proposal fires the notification on first attempt
- **WHEN** an LLM-driven audit's proposal passes `openspec validate <slug> --strict` on the first attempt (`retries_used == 0`)
- **THEN** the audit posts exactly one chatops notification whose text matches `🔍 <repo_url>: <audit_type> created proposal \`<change_slug>\` — <why_excerpt>`
- **AND** the notification text does NOT contain a parenthetical about retries

#### Scenario: Validated proposal after retry includes the retry-count parenthetical
- **WHEN** an LLM-driven audit's proposal passes validation after one or more retries (`retries_used > 0`)
- **THEN** the notification text appends ` (validated on retry <retries_used> of <max_validation_retries>)`

#### Scenario: ValidationExhausted does NOT fire the proposal-created notification
- **WHEN** an LLM-driven audit's proposal fails validation through every retry and the audit returns `ValidationExhausted`
- **THEN** the `🔍 created proposal` notification SHALL NOT fire
- **AND** the existing `❌ <audit-type> produced an invalid proposal` notification (from `a01-audit-proposal-self-validation`) fires instead

#### Scenario: notify_on_clean=false does not suppress this notification
- **WHEN** an LLM-driven audit configured with `notify_on_clean: false` produces a valid proposal
- **THEN** the `🔍 created proposal` notification still fires
- **AND** the existing `notify_on_clean=false` semantics still suppress only the empty-findings success message

#### Scenario: architecture_advisor produces no proposal-created notification
- **WHEN** the `architecture_advisor` audit runs to completion AND produces any number of recommendations
- **THEN** no `🔍 created proposal` notification fires from this audit
- **AND** the audit's existing notification behaviour (if any) is unchanged

#### Scenario: chatops backend absent does not affect audit outcome
- **WHEN** the daemon has no chatops backend configured AND an LLM-driven audit produces a valid proposal
- **THEN** the audit returns its `Reported` outcome normally
- **AND** the missing notification does NOT affect the proposal commit, the queue insertion, or the iteration's overall success

#### Scenario: chatops post_notification failure does not affect audit outcome
- **WHEN** the chatops backend is configured AND `post_notification` returns Err during the `🔍` notification post
- **THEN** the failure is logged at WARN
- **AND** the audit's `Reported` outcome is unaffected
- **AND** the proposal commit proceeds normally

### Requirement: Chatops `audit` verb queues an on-demand audit run for the next polling iteration
The chatops listener SHALL recognize `@<bot> audit <audit-substring> <repo-substring>` as the `AuditNow` command. The audit-substring SHALL be matched case-insensitively against the registered audit-type names by substring (same rule the repo-substring uses against configured repository URLs). The repo-substring SHALL be matched per the existing repo-substring rules. On a unique match in both, the dispatcher SHALL submit a `queue_audit` control-socket action AND post a one-line ack naming the resolved audit-type and repo URL. On ambiguous or no-match, the dispatcher SHALL reply with the candidate list (mirroring the existing `match_repo` reply shapes).

#### Scenario: Unique substring matches queue the audit
- **WHEN** an operator posts `@<bot> audit sec myrepo` AND `sec` uniquely matches `security_bug_audit` AND `myrepo` uniquely matches a configured repo URL
- **THEN** the dispatcher submits a `queue_audit` action with both resolved names
- **AND** the bot posts a threaded reply whose first line is `✓ Queued security_bug_audit for <repo_url>. Will run on the next polling iteration (~Nm).` (where `~Nm` is the per-repo poll interval rounded to minutes, OR `imminently` when the next iteration is <30 seconds away)

#### Scenario: Ambiguous audit substring lists candidates
- **WHEN** an operator posts `@<bot> audit canon myrepo` AND `canon` matches both `canon_contradiction_audit` and `canon_consolidation_audit`
- **THEN** the bot replies `✗ audit substring \`canon\` matches multiple: canon_consolidation_audit, canon_contradiction_audit. Be more specific.`
- **AND** no audit is queued

#### Scenario: Unknown audit substring lists all registered names
- **WHEN** an operator posts `@<bot> audit gibberish myrepo`
- **THEN** the bot replies `✗ no audit matched \`gibberish\`; registered: architecture_advisor, drift_audit, missing_tests_audit, security_bug_audit, documentation_audit, canon_contradiction_audit, canon_consolidation_audit.`
- **AND** no audit is queued

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

### Requirement: Audit findings do not mint new canonical metric requirements
No audit, AND no triage acting on an audit's findings, SHALL author a NEW canonical requirement that re-encodes an audit's selection or detection metric — a file-size threshold, a function-length threshold, a duplication count, OR a similar heuristic — as a binding constraint a future change is measured against. A project's size/structure budget has a SINGLE canonical home: the `Source files and functions stay within a size budget` requirement, which is explicitly advisory AND non-gating (a size finding never blocks a pull request or a change from archiving). Triage that acts on an architectural finding SHALL produce a behavior-preserving refactor (an issue), NOT a spec requirement restating the threshold. The metric remains a signal the audit uses to select candidates, never a contract.

#### Scenario: An audit threshold is not restated as a new requirement
- **WHEN** an audit surfaces files or functions exceeding a size threshold AND an operator acts on the finding
- **THEN** no new spec requirement is authored that states files or functions SHALL stay within that threshold
- **AND** the resulting work is a behavior-preserving refactor (an issue), not a spec encoding the metric

#### Scenario: The size budget keeps a single advisory home
- **WHEN** the project records a size or structure budget
- **THEN** it lives in the single advisory `Source files and functions stay within a size budget` requirement
- **AND** it is NOT duplicated into per-change specs NOR promoted to a pull-request-blocking gate
