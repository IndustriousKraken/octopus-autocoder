## ADDED Requirements

### Requirement: Revision prompt is constructed from PR-sourced material; no degraded-prompt fallback is permitted
The executor's revision-mode prompt builder SHALL construct its prompt body solely from material sourced from the PR being revised. The pre-`a20a5` approach — calling `openspec instructions apply --change <X>` against the workspace's current state to load "the original change material" — SHALL be removed entirely. The workspace's current state at the moment the revise dispatcher runs is the agent branch's tip, which by the canonical "Implementer prompt template loading" requirement's instruction (`openspec archive is denied in this sandbox. Leave the working tree dirty — autocoder will commit your diff and archive on success.`) always contains the post-archive layout where `openspec/changes/<X>/` does not exist. The `openspec instructions apply` call therefore could never succeed for any change that had ever been in a PR — the placeholder fallback the pre-`a20a5` code fired in this case constituted a degraded-prompt path operating in 100% of production revise invocations.

The revision prompt template (`prompts/implementer-revision.md`) SHALL define five placeholders, all required:

- `{{pr_body}}` — the PR's full body text verbatim. Contains the `## Code Review` section (when the reviewer is enabled) AND the "Changes implemented in this pass" section.
- `{{pr_change_list}}` — newline-separated change slugs extracted from the PR body via the existing `extract_change_list_from_pr_body` helper.
- `{{agent_implementation_notes}}` — concatenated `## Agent implementation notes` issue-comment bodies from the PR, in posted order, separated by `\n\n---\n\n`. These are the canonical implementer-summary comments mandated by the `Implementer-summary PR comment` requirement; one is posted per change in multi-change passes.
- `{{revision_diff}}` — the PR's unified diff (existing field; unchanged). Contains the spec deltas via the archive moves.
- `{{revision_request}}` — the operator's revision text from the triggering PR comment (existing field; unchanged).

The template's prose SHALL instruct the LLM to:

- Identify which change(s) in `{{pr_change_list}}` the operator's `{{revision_request}}` targets. If the request names a specific slug, target that change. If the request is generic (does not name a slug), apply the revision to the change(s) whose content matches the request.
- Use `{{revision_diff}}` as the implementation already in flight; the revision modifies that diff rather than producing a fresh implementation.
- Use `{{agent_implementation_notes}}` to understand what the original implementer claimed to do, which is the gap the operator is closing.
- Use the code review portion of `{{pr_body}}` (when present) to understand what the reviewer flagged.

The builder SHALL NOT substitute placeholder text, fallback strings, OR "best-effort" content for any of the five placeholders. If the caller cannot provide all five inputs as non-error values, the caller SHALL NOT invoke the builder; the dispatcher refusal path defined in `orchestrator-cli` handles that case. This invariant — **no degraded-prompt path is permitted for missing required input** — applies to every prompt builder in autocoder, not only revision-mode. Future prompt builders SHALL inherit the same discipline at their construction sites.

#### Scenario: Builder substitutes all five placeholders from RevisionContext
- **WHEN** `build_revision_prompt` is called with a `RevisionContext` carrying populated `pr_body`, `pr_change_list`, `agent_implementation_notes`, `pr_diff`, AND `revision_text` fields
- **THEN** the rendered prompt contains the verbatim content of all five fields in their template positions
- **AND** the rendered prompt contains NO instance of the pre-`a20a5` placeholder string `_(original change material unavailable — ...)_`
- **AND** the rendered prompt contains NO instance of the pre-`a20a5` `{{change_body}}` placeholder name

#### Scenario: Builder does not invoke openspec
- **WHEN** an automated test wraps `build_revision_prompt` with a process-spawn observer
- **THEN** no `openspec` subprocess is spawned during prompt construction
- **AND** no `Command::new("openspec")` call is reachable from the revision-prompt code path

#### Scenario: Template documents the multi-change resolution rule
- **WHEN** a maintainer reads `prompts/implementer-revision.md`
- **THEN** the template's prose explicitly instructs the LLM on the multi-change resolution: name-match the operator's request to a slug, OR apply the request to all listed changes if no slug is named
- **AND** the template instructs the LLM to leave the workspace dirty for autocoder to commit; the LLM does NOT invoke `git` or `openspec archive` directly

#### Scenario: Operator-override revision templates inherit the new placeholder set
- **WHEN** an operator configures `executor.implementer_revision.prompt_path` (per `a24`'s uniform PromptLoader pattern) pointing at a custom revision-prompt template
- **AND** that template contains the new five placeholders
- **THEN** the builder substitutes them per the standard substitution rules
- **AND** operators migrating from pre-`a20a5` templates see a clear documentation pointer in `docs/CONFIG.md`'s Prompt overrides table (`a24`) naming the placeholder migration

### Requirement: Prompt construction is gated by an explicit availability check at the caller
For every embedded prompt template the daemon ships (revision-mode, implementer-mode, audit-triage, chat-request-triage, brownfield-draft, scout, documentation-audit, sentinel emission), the call site that invokes `build_X_prompt(...)` SHALL first verify that every required input is available as a non-error value. Missing-input cases SHALL be handled by the caller — typically by posting an operator-facing message via the appropriate channel (PR comment, chatops post, control-socket reply) AND refusing to invoke the executor — NOT by the builder substituting placeholder content.

This requirement is the architectural invariant that prevents the `a20a5`-fixed bug class from recurring. The construction-site discipline mirrors the `a20a4` head-qualifier pattern: explicit checks at the site where the dependency is consumed, no silent fallback inside the helper.

#### Scenario: Future prompt builder rejects placeholder fallback
- **WHEN** a future change introduces a new prompt builder (e.g. `build_scout_prompt`, `build_brownfield_survey_prompt`)
- **THEN** the builder's contract documents that every required input must be provided AS a non-error value
- **AND** the builder does NOT contain any "best-effort," "fall back to placeholder," OR "substitute stub" code path for missing required input
- **AND** every call site of the builder is preceded by explicit availability checks for every required input

#### Scenario: Code review surfaces violations of the construction-site discipline
- **WHEN** a future change introduces code that mutates a prompt builder to accept a `None` for what was previously a required input
- **THEN** the reviewer (per the canonical code-reviewer flow) flags the change as violating this requirement
- **AND** the canonical reference to "no degraded-prompt path" appears in the review feedback so the maintainer can locate the architectural reason
