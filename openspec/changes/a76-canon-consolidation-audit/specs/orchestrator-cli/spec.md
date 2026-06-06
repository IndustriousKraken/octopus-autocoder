# orchestrator-cli — delta for a76-canon-consolidation-audit

## ADDED Requirements

### Requirement: Canon-consolidation audit
autocoder SHALL register a `canon_consolidation_audit` audit in the periodic-audit framework, following the specs-writing pattern of `missing_tests_audit` / `security_bug_audit`. The audit invokes the wrapped agent CLI through the shared `agentic_run` primitive with a `WritePolicy::OpenSpecOnly` sandbox (`Read`/`Glob`/`Grep` plus `Write`/`Edit`), identifies a redundant cluster of canonical requirements, AND drafts a consolidation change under `openspec/changes/` that merges them. It returns `AuditOutcome::SpecsWritten(names)` so the same iteration's queue walk carries the change to a PR. The audit is `requires_head_change = true`; its default cadence is heavy (`monthly`). It NEVER modifies the canonical specs directly — its only output is a reviewable change.

**RAG-assisted overlap detection, best-effort fallback.** The audit enumerates the canonical requirements across `openspec/specs/*/spec.md`. When `a21`'s canonical-spec RAG is enabled, the agent SHALL use `query_canonical_specs` to retrieve, for each requirement, a bounded set of the most semantically-similar requirements AND judge that focused bundle for redundancy — bounding per-call input AND targeting related requirements, where redundancy lives. When RAG is not configured, the audit SHALL degrade to a best-effort direct read of the canon AND log that coverage is best-effort. The retrieval breadth is an operator-tunable setting with a sensible default.

**Conservative v1 scope.** The audit SHALL propose a merge only where the requirements express the *same invariant* under different titles (e.g. per-provider duplicates of one outbound-call retry rule). Broad capability restructuring AND speculative over-fragmentation merges are OUT of scope for this audit.

**General-vs-specific guard.** The audit SHALL NOT merge a general, project-wide prescription with a feature-specific implementation of it — merging "all data is stored in a relational database" with "make PostgreSQL available" into "all data is stored in PostgreSQL" erases the general rule AND would forbid a later alternative. The prompt SHALL err toward NOT proposing when a merge would erase a broader prescription. This prompt guidance is design intent verified by the drift audit's semantic judgment; it SHALL NOT be pinned by a unit test asserting verbatim substrings of the prompt (per the project-documentation requirement `Tests assert behavior or derivation, never message wording`).

**Consolidation change shape.** The drafted change SHALL name itself with a `consolidate-` prefix; MODIFY the surviving requirement into the merged general form; REMOVE the now-redundant requirement(s) via a `## REMOVED Requirements` delta; AND preserve every non-redundant scenario across the merged result. Because consolidation is subtractive, the proposal SHALL state the before/after scenario count AND list any scenario dropped as redundant, with the reason, so the PR reviewer sees exactly what is consolidated AND can catch silent information loss. The audit SHALL validate the drafted change with `openspec validate <name> --strict` before committing; an invalid draft is discarded with a WARN AND no commit (consistent with the framework's `LLM-driven audits validate their generated proposals before committing` requirement).

**Human arbitration.** The consolidation change is reviewed in a PR; the operator merges, `@<bot> revise`s, or rejects it. A merge that proves wrong after landing is recoverable by removing the change AND rebuilding canon from the archive (`Rebuild canonical specs from archive`). The number of proposals per run is bounded by `audits.canon_consolidation_audit.max_proposals_per_run` (default `1`; consolidation changes are denser to review than additive ones).

#### Scenario: Runs agentic with an OpenSpec-only sandbox and RAG-assisted detection
- **WHEN** the audit runs
- **THEN** autocoder spawns the wrapped agent CLI via `agentic_run` with a `WritePolicy::OpenSpecOnly` sandbox (`Write`/`Edit` allowed alongside the read tools)
- **AND** the prompt is the embedded `prompts/canon-consolidation-audit.md` template OR the operator override at `audits.canon_consolidation_audit.prompt_path`
- **WHEN** `a21`'s RAG is enabled
- **THEN** the agent retrieves the nearest requirements per requirement via `query_canonical_specs`; when RAG is not configured the audit proceeds with a best-effort direct read AND logs that coverage is best-effort

#### Scenario: A near-duplicate cluster yields one consolidation change
- **WHEN** two or more canonical requirements express the same invariant under different titles (e.g. a per-provider retry rule duplicated for Stripe AND PayPal)
- **THEN** the audit drafts one `consolidate-`-prefixed change that MODIFIES the surviving requirement into the merged general form AND REMOVES the redundant one(s)
- **AND** every non-redundant scenario is preserved in the merged result
- **AND** the audit returns `AuditOutcome::SpecsWritten` naming the created change

#### Scenario: A general rule plus a compatible specialization is not merged
- **WHEN** a general project-wide prescription AND a compatible feature-specific implementation of it are candidates (e.g. "all data in a relational database" AND "make PostgreSQL available")
- **THEN** the audit does NOT propose merging them, because the merge would erase the general prescription

#### Scenario: The proposal surfaces the scenario-loss summary
- **WHEN** the audit drafts a consolidation change that reduces the total scenario count
- **THEN** the proposal states the before/after scenario count AND lists each scenario dropped as redundant with the reason
- **AND** the PR reviewer can see exactly what was consolidated

#### Scenario: An invalid draft is discarded without committing
- **WHEN** the drafted consolidation change fails `openspec validate <name> --strict`
- **THEN** the audit deletes the draft AND records a WARN naming the validation error
- **AND** no consolidation change is committed from that run

#### Scenario: No redundancy produces a silent outcome
- **WHEN** the audit finds no redundant cluster within its conservative scope
- **THEN** it returns `AuditOutcome::SpecsWritten(vec![])`
- **AND** no commit is made AND no chatops post is sent (per the framework behavior for spec-writing audits)

## MODIFIED Requirements

### Requirement: Registered periodic audits
autocoder SHALL register exactly the following audits in its `AuditRegistry` at startup, identified by their `audit_type()` slug: `architecture_brightline`, `architecture_consultative`, `drift_audit`, `missing_tests_audit`, `security_bug_audit`, `canon_contradiction_audit`, `canon_consolidation_audit`. The slug `dependency_update_triage` SHALL NOT be registered. Each registered audit's cadence is independently configurable under `audits.defaults` and per-repo `repositories[].audits` overrides; an unregistered slug present in either location SHALL fail config validation at startup with the existing "unknown audit type" error message that lists the registered slugs.

This enumeration is the canonical contract for which audits exist. Future changes that add or remove an audit MUST update this requirement in the same commit so the spec and the registered set never drift. The `validate_audit_type_names` startup check enforces the spec/code consistency at runtime: an operator's YAML naming an unregistered slug is a startup-time failure with a clear list of valid slugs.

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

#### Scenario: Operator configures the removed dependency_update_triage slug
- **WHEN** an operator's `audits.defaults` (or
  `repositories[].audits`, or `audits.settings`) contains the key
  `dependency_update_triage` (a slug that was registered in
  earlier versions of autocoder but has since been removed)
- **THEN** `validate_audit_type_names` fails at startup with an
  error naming `dependency_update_triage` as unknown AND listing
  the registered slugs so the operator knows what to use
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
