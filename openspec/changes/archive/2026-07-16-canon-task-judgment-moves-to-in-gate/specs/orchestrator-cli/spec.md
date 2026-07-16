## REMOVED Requirements

### Requirement: Pre-flight rejects a change whose tasks direct edits to the canonical specs
**Reason**: The detection this requirement mandates is mechanical token-matching (mutation verb anywhere in the task + a canon-target token anywhere in the task), which cannot bind a verb to its object. The requirement's own precision promise — "a read-only mention of canon for context (no mutation verb) is NOT flagged" — is structurally unkeepable, because every real task carries a mutation verb, so ANY mention of canon in task prose flags. Three confirmed false positives (July 2026) each blocked a valid change and cost an operator round-trip; one was misdiagnosed as gate-model quality because nothing surfaces that this path is a keyword matcher. Semantic judgment about what a sentence directs belongs to the agentic gate that already reads the change at this exact pipeline position.

**Migration**: The check's function moves into the `[in]` gate (see the modified "Change-internal contradiction pre-flight check (opt-in)" requirement): the gate's session also reads `tasks.md` and reports tasks that direct canonical-spec edits, riding the same marker / alert / halt flow — so a true positive produces the identical operator experience, caught pre-executor. Deployments running with the `[in]` gate disabled retain the archive-time backstop: `openspec archive` aborts on a duplicate requirement at fold time, which is a mechanical fact rather than a prose heuristic, and the parked change surfaces through the existing perma-stuck alerting.

## MODIFIED Requirements

### Requirement: Change-internal contradiction pre-flight check (opt-in)
autocoder SHALL provide an opt-in pre-flight check that detects semantic contradictions among the requirements WITHIN a single OpenSpec change before the executor is invoked. The check runs a CLI-wrapped agentic session through the shared `agentic_run` primitive (a56) in a read-only sandbox that reads the change's spec-delta files AND its `tasks.md` on demand AND returns a structured listing of findings via the `submit_contradictions` MCP tool: contradictions (requirements that cannot all hold simultaneously) AND, optionally, canon-editing tasks — tasks whose EDIT TARGET is the canonical specs under `openspec/specs/`. The implementer implements code and tests only; a change's spec delta is folded into the canonical specs by `openspec archive`, so a task directing the implementer to apply the delta to `openspec/specs/` would make the archive abort on a duplicate requirement. Judging whether a task DIRECTS such an edit is semantic: a task that merely MENTIONS the canonical specs as context or justification (e.g. citing which requirement motivates a docs edit), or that references the change's OWN delta under `openspec/changes/<slug>/specs/`, is NOT a finding. On non-empty findings of either kind, autocoder SHALL write `.needs-spec-revision.json` with `revision_suggestion` populated from the findings narrative (canon-editing-task findings state that the implementer implements code and tests only and the delta is folded at archive), post the existing `AlertCategory::SpecNeedsRevision` chatops alert, AND halt the queue walk for this iteration. The executor SHALL NOT be invoked when findings are present.

The check SHALL be gated by `executor.change_internal_contradiction_check` (`disabled` default, `enabled` opt-in). The model is configured via `executor.change_internal_contradiction_check_llm` (parallel to the `reviewer:` config block — provider, model, api_key source, optional api_base_url), which a56's CLI strategy translates into the wrapped CLI's model-selection mechanism. The `claude` strategy reaches only Anthropic-shaped endpoints; a model whose provider resolves to a CLI with no registered strategy makes the check FAIL CLOSED (per the fail-closed posture below) until that strategy is registered.  Enabling the check without configuring the model SHALL fail at daemon startup with a fail-fast validation error.

The check SHALL FAIL CLOSED (gatekeepers-fail-closed standard): an agentic-session error (spawn, timeout, OR a resolved CLI strategy that is not registered), a schema-rejected submission the agent never corrects, a session that ends with no submission, OR any other could-not-run failure SHALL NOT be treated as "no contradictions found." It SHALL log a WARN AND hold the change in an explicit failed-to-run state: write `.needs-spec-revision.json` with a structured `gate_error` population (the gate label AND the cause) distinct from a findings-based revision, post a distinct "gate FAILED TO RUN — change held" chatops alert (under `AlertCategory::SpecNeedsRevision`), AND halt the queue walk. The change is held because it was NOT evaluated — NOT because a problem was found; an operator clears the marker (after fixing the gate) to retry. A schema-invalid `submit_contradictions` call mid-session is a correctable tool error the agent can retry (a56). A successful session that returns empty findings is a clean result AND proceeds to the executor.

The check runs AFTER `a17`'s mechanical archivability check AND BEFORE the executor. The two checks are layered: `a17` catches structural defects (header mismatches); this check catches semantic ones — self-contradictions AND tasks that direct canon edits. Most clean changes pass both with no LLM cost beyond the contradiction check's own.

#### Scenario: Default-disabled produces no contradiction-check session
- **WHEN** `executor.change_internal_contradiction_check` is unset (default `disabled`)
- **AND** any change reaches the pre-executor pipeline
- **THEN** no contradiction-check session is spawned (no LLM cost)
- **AND** the executor is invoked normally (assuming `a17`'s archivability check passed)

#### Scenario: Enabled mode runs an agentic session over the change's deltas
- **WHEN** `executor.change_internal_contradiction_check: enabled` AND the model config is set
- **AND** a change passes `a17`'s archivability check
- **THEN** the pipeline runs an `agentic_run` session (a56) in a read-only sandbox (`Read`/`Glob`/`Grep`, `ORCH_MCP_ROLE = contradiction_check`, the `submit_contradictions` MCP tool) with the embedded `prompts/change-contradiction-check.md` prompt (OR the configured override)
- **AND** the agent reads the change's spec-delta files AND `tasks.md` on demand AND returns findings by calling `submit_contradictions` with `{ contradictions: [{ requirement_a, requirement_b, summary }], canon_editing_tasks: ["<task text>"] }` (the second field optional and empty when there are none)

#### Scenario: Empty findings submission proceeds to executor
- **WHEN** the agent calls `submit_contradictions` with an empty `contradictions` array AND no canon-editing-task findings
- **THEN** the pipeline proceeds to the executor
- **AND** no marker is written
- **AND** no chatops alert fires

#### Scenario: Non-empty contradictions submission writes marker and skips executor
- **WHEN** the agent submits one or more contradictions
- **THEN** the pipeline writes `.needs-spec-revision.json` with `revision_suggestion` text populated from the contradictions narrative (per the documented format)
- **AND** the marker's `unarchivable_deltas`, `unimplementable_tasks`, AND `gate_error` populations are empty (this case is semantic findings, not structural AND not a gate error)
- **AND** the chatops alert under `AlertCategory::SpecNeedsRevision` fires (subject to the 24h throttle)
- **AND** the executor is NOT invoked for this change OR any subsequent change in this iteration

#### Scenario: A task directing a canon edit is reported and holds the change
- **WHEN** the change's `tasks.md` contains a task such as `Apply the ADDED Requirements block from specs/<cap>/spec.md to openspec/specs/<cap>/spec.md`
- **THEN** the agent reports it in `canon_editing_tasks`
- **AND** the pipeline writes `.needs-spec-revision.json` whose `revision_suggestion` names the offending task AND states that the implementer implements code and tests only (the delta is folded by `openspec archive`)
- **AND** the alert fires AND the executor is NOT invoked

#### Scenario: Task prose that mentions canon without directing an edit is not a finding
- **WHEN** a task directs an edit to a non-canon target while citing the canonical specs as context or justification (e.g. `Update docs/CHATOPS.md's verb documentation (the project-documentation requirements say the verbs are documented there)`), OR references the change's own delta under `openspec/changes/<slug>/specs/`
- **THEN** the agent does NOT report it as a canon-editing task
- **AND** the change proceeds normally to the executor (absent other findings)

#### Scenario: Session failure holds the change (fail closed)
- **WHEN** the agentic session fails (spawn error, timeout, OR the resolved CLI strategy is not registered — e.g. a non-`claude` command whose strategy has not been added)
- **THEN** the pipeline logs a WARN (carrying the `[in]` label) naming the cause
- **AND** writes `.needs-spec-revision.json` with a structured `gate_error` (gate label + cause), NOT a "no contradictions found" result
- **AND** posts a distinct "gate FAILED TO RUN — change held" chatops alert
- **AND** the executor is NOT invoked — the change is held until an operator clears the marker

#### Scenario: No valid submission holds the change (fail closed)
- **WHEN** the session ends with no schema-valid `submit_contradictions` call (the agent never submits, OR every submission is schema-rejected and never corrected)
- **THEN** the pipeline logs a WARN (carrying the `[in]` label) with a truncated session-output excerpt
- **AND** writes the `.needs-spec-revision.json` marker with a `gate_error` population AND halts the iteration (the same fail-closed hold)

#### Scenario: Enabled without model config fails fast at startup
- **WHEN** `config.yaml` sets `executor.change_internal_contradiction_check: enabled`
- **AND** `executor.change_internal_contradiction_check_llm` is unset
- **THEN** daemon startup fails with the error `executor.change_internal_contradiction_check is enabled but executor.change_internal_contradiction_check_llm is not configured`
- **AND** the daemon does NOT begin polling
- **AND** the operator sees the error message on stderr AND in journalctl

#### Scenario: Prompt override replaces the embedded default
- **WHEN** `executor.change_internal_contradiction_check_prompt_path` points to an override file
- **THEN** the pipeline reads the override file AND uses its contents as the prompt template
- **AND** an empty override file produces an error at use time (the daemon does not feed an empty prompt to the session)

#### Scenario: Marker `revision_suggestion` enumerates findings clearly
- **WHEN** the agent submits 2 contradictions
- **THEN** the marker's `revision_suggestion` text contains both findings numbered 1 AND 2, each with `requirement_a`, `requirement_b`, AND `summary` fields
- **AND** the text ends with operator guidance (`Edit the conflicting requirements... clear via @<bot> clear-revision`)

#### Scenario: Operator clearing the marker without spec edits is permitted
- **WHEN** the operator assesses the findings as a false positive AND runs `@<bot> clear-revision <repo> <change>` without editing the spec
- **THEN** the next polling iteration retries the change AND re-runs the contradiction check
- **AND** the operator's tolerance for false positives shapes their decision to enable the check OR keep it disabled
