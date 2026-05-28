## ADDED Requirements

### Requirement: Change-internal contradiction pre-flight check (opt-in)
autocoder SHALL provide an opt-in pre-flight check that detects semantic contradictions among the requirements WITHIN a single OpenSpec change before the executor is invoked. The check uses a configurable LLM to read the change's spec-delta files AND produce a structured JSON listing of contradictions (requirements that cannot all hold simultaneously). On non-empty findings, autocoder SHALL write `.needs-spec-revision.json` with `revision_suggestion` populated from the contradictions narrative, post the existing `AlertCategory::SpecNeedsRevision` chatops alert, AND halt the queue walk for this iteration. The executor SHALL NOT be invoked when contradictions are found.

The check SHALL be gated by `executor.change_internal_contradiction_check` (`disabled` default, `enabled` opt-in). The LLM is configured via `executor.change_internal_contradiction_check_llm` (parallel to the `reviewer:` config block — provider, model, api_key source, optional api_base_url). Enabling the check without configuring the LLM SHALL fail at daemon startup with a fail-fast validation error.

The check SHALL fail-open: LLM transport errors, parse failures, OR malformed responses log a WARN AND treat the check as "no contradictions found." The daemon does NOT gate work on a failed check — operators see the WARN in journalctl AND can investigate; the executor proceeds.

The check runs AFTER `a17`'s mechanical archivability check AND BEFORE the executor. The two checks are layered: `a17` catches structural defects (header mismatches), `a19` catches semantic ones (self-contradictions). Most clean changes pass both with no LLM cost beyond the contradiction check's own.

#### Scenario: Default-disabled produces no LLM call
- **WHEN** `executor.change_internal_contradiction_check` is unset (default `disabled`)
- **AND** any change reaches the pre-executor pipeline
- **THEN** no LLM call is made for the contradiction check
- **AND** the executor is invoked normally (assuming `a17`'s archivability check passed)

#### Scenario: Enabled mode invokes the LLM with the change's deltas
- **WHEN** `executor.change_internal_contradiction_check: enabled` AND the LLM config is set
- **AND** a change passes `a17`'s archivability check
- **THEN** the pipeline invokes the configured LLM with the embedded `prompts/change-contradiction-check.md` prompt + the change's concatenated spec-delta files
- **AND** parses the response as JSON conforming to `{ contradictions: [{ requirement_a, requirement_b, summary }] }`

#### Scenario: Empty contradictions array proceeds to executor
- **WHEN** the LLM returns `{"contradictions": []}`
- **THEN** the pipeline proceeds to the executor
- **AND** no marker is written
- **AND** no chatops alert fires

#### Scenario: Non-empty contradictions array writes marker and skips executor
- **WHEN** the LLM returns one or more contradictions
- **THEN** the pipeline writes `.needs-spec-revision.json` with `revision_suggestion` text populated from the contradictions narrative (per the documented format)
- **AND** the marker's `unarchivable_deltas` AND `unimplementable_tasks` arrays are empty (this case is semantic, not structural)
- **AND** the chatops alert under `AlertCategory::SpecNeedsRevision` fires (subject to the 24h throttle)
- **AND** the executor is NOT invoked for this change OR any subsequent change in this iteration

#### Scenario: LLM call failure fails open
- **WHEN** the LLM call returns Err (network, rate-limit, transport)
- **THEN** the pipeline logs a WARN naming the error
- **AND** treats the check as "no contradictions found"
- **AND** proceeds to the executor
- **AND** the daemon does NOT gate iteration progress on the failed check

#### Scenario: Malformed LLM response fails open
- **WHEN** the LLM returns a response that doesn't parse as the expected JSON shape
- **THEN** the pipeline logs a WARN naming the response excerpt (truncated to 200 chars)
- **AND** proceeds to the executor (same fail-open posture)

#### Scenario: Enabled without LLM config fails fast at startup
- **WHEN** `config.yaml` sets `executor.change_internal_contradiction_check: enabled`
- **AND** `executor.change_internal_contradiction_check_llm` is unset
- **THEN** daemon startup fails with the error `executor.change_internal_contradiction_check is enabled but executor.change_internal_contradiction_check_llm is not configured`
- **AND** the daemon does NOT begin polling
- **AND** the operator sees the error message on stderr AND in journalctl

#### Scenario: Prompt override replaces the embedded default
- **WHEN** `executor.change_internal_contradiction_check_prompt_path` points to an override file
- **THEN** the pipeline reads the override file AND uses its contents as the prompt template
- **AND** an empty override file produces an error at use time (the daemon does not feed an empty prompt to the LLM)

#### Scenario: Marker `revision_suggestion` enumerates findings clearly
- **WHEN** the LLM returns 2 contradictions
- **THEN** the marker's `revision_suggestion` text contains both findings numbered 1 AND 2, each with `requirement_a`, `requirement_b`, AND `summary` fields
- **AND** the text ends with operator guidance (`Edit the conflicting requirements... clear via @<bot> clear-revision`)

#### Scenario: Operator clearing the marker without spec edits is permitted
- **WHEN** the operator assesses the LLM's findings as a false positive AND runs `@<bot> clear-revision <repo> <change>` without editing the spec
- **THEN** the next polling iteration retries the change AND re-runs the contradiction check
- **AND** the operator's tolerance for false positives shapes their decision to enable the check OR keep it disabled
