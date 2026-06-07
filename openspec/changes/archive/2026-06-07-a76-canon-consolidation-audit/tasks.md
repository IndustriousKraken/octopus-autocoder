# Implementation tasks

## 1. Register the audit

- [x] 1.1 Add `canon_consolidation_audit` to the `AuditRegistry` with `audit_type()` slug `canon_consolidation_audit`, `requires_head_change = true`, `WritePolicy::OpenSpecOnly`, and an OpenSpec-only sandbox (`Read`/`Glob`/`Grep` plus `Write`/`Edit`). Default cadence `monthly`.
- [x] 1.2 Add the slug to the `validate_audit_type_names` known-slug list (now seven, built on a75's six), the README audit table, and `config.example.yaml`.

## 2. Overlap-detection driver

- [x] 2.1 Run the audit through `agentic_run` (a56) with the embedded `prompts/canon-consolidation-audit.md` (override at `audits.canon_consolidation_audit.prompt_path`). Share the enumerate → retrieve → focused-judgment driver with a75 (overlap vs conflict differs only in prompt + disposition).
- [x] 2.2 Enumerate canonical requirements across `openspec/specs/*/spec.md`. When a21 RAG is enabled, retrieve nearest requirements per requirement via `query_canonical_specs` and judge each bundle for redundancy. Retrieval breadth is a tunable setting with a sensible default.
- [x] 2.3 When RAG is not configured, degrade to a best-effort direct read and log that coverage is best-effort.

## 3. Consolidation change drafting

- [x] 3.1 For a redundant cluster (same invariant, different titles), draft one `consolidate-`-prefixed change under `openspec/changes/`: MODIFY the surviving requirement into the merged general form; REMOVE the redundant requirement(s) via `## REMOVED Requirements`; preserve every non-redundant scenario.
- [x] 3.2 Enforce the general-vs-specific guard in the prompt: never merge a project-wide prescription with a feature-specific implementation; err toward not proposing when a merge would erase a broader rule.
- [x] 3.3 Compose the proposal to state the before/after scenario count and list any scenario dropped as redundant with its reason.
- [x] 3.4 Validate the draft with `openspec validate <name> --strict` before commit; discard an invalid draft with a WARN and no commit. Bound proposals per run by `max_proposals_per_run` (default 1).
- [x] 3.5 Return `AuditOutcome::SpecsWritten(names)`; an empty result is silent (no commit, no chatops).

## 4. Prompt

- [x] 4.1 Write `prompts/canon-consolidation-audit.md`: define the target as redundant requirements expressing the same invariant under different titles; scope to near-duplicate consolidation (no broad restructuring); enforce the general-vs-specific guard with the relational/PostgreSQL worked example; require scenario preservation and the loss summary; instruct the agent to use `query_canonical_specs` when available and to write a `consolidate-` change that validates `--strict`.

## 5. Tests

- [x] 5.1 The audit registers OpenSpecOnly and runs agentic; default config leaves it disabled.
- [x] 5.2 With RAG enabled the driver calls `query_canonical_specs`; with RAG off it proceeds best-effort and logs the degradation.
- [x] 5.3 A near-duplicate cluster yields one `consolidate-` change (MODIFY survivor + REMOVE redundant) preserving non-redundant scenarios; `SpecsWritten` names it.
- [x] 5.4 A general+compatible-specific pair is NOT merged.
- [x] 5.5 The drafted proposal carries the before/after scenario count and the dropped-as-redundant list.
- [x] 5.6 An invalid draft is discarded without commit; an empty result is silent; the per-run cap is honored.
- [x] 5.7 `validate_audit_type_names` accepts the new slug and still rejects an unknown one, listing the seven registered slugs.

## 6. Acceptance gate

- [x] 6.1 `cargo test` passes for the autocoder crate.
- [x] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 6.3 `openspec validate a76-canon-consolidation-audit --strict` passes.
