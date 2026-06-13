# Tasks

## 1. The `architecture_advisor` audit

- [ ] 1.1 Add an `ArchitectureAdvisorAudit` (`audit_type()` = `architecture_advisor`, `requires_head_change = true`, `WritePolicy::None`). Reuse the surviving whole-file line-count scan (lift `check_file_size`'s line-counting from `brightline.rs`) as the candidate SELECTOR only: rank scanned files by line count, keep those over a configurable threshold, cap at a configurable candidate count. Do NOT emit the count as a finding.
- [ ] 1.2 For each selected candidate, invoke the agent CLI read-only with a new `architecture-advisor` prompt that directs the agent to read the file (+ context to judge cohesion/placement) and return a ranked, anchored recommendation per the spec: what's wrong, why it matters, the concrete action, grounded in the project's language/architecture/patterns. Forbid snark and generic lecturing. Cap findings at 5.
- [ ] 1.3 Return `AuditOutcome::Reported(findings)`; on a clean run return `Reported(vec![])` and record the examined candidates + no-recommendation conclusion in the audit-run log.
- [ ] 1.4 Add the audit's settings keys (selector threshold, candidate cap) under its slug in `audits.settings`, with compile-time defaults.

## 2. Remove the two old audits

- [ ] 2.1 Delete `audits/brightline.rs` and `audits/brightline/ignore.rs` (and the `.brightline-ignore` schema/loader). Preserve only the file-line-count logic needed by 1.1 (move it into the advisor or a shared helper).
- [ ] 2.2 Delete the `architecture_consultative` audit implementation and its prompt.
- [ ] 2.3 Remove `.brightline-ignore` handling end-to-end (loader, stale-entry validation, chatops stale-ignore clause). No successor ignore mechanism in this change.

## 3. Registry, validator, config, README

- [ ] 3.1 In `AuditRegistry` startup registration, remove `architecture_brightline` and `architecture_consultative`; add `architecture_advisor`. The registered set is now six.
- [ ] 3.2 Update `validate_audit_type_names`' known-slug list to the six slugs; the two removed slugs (plus `dependency_update_triage`) are rejected at startup with the existing error.
- [ ] 3.3 Update `config.example.yaml` audit defaults: drop the two slugs, add `architecture_advisor` with a sensible cadence and its settings keys.
- [ ] 3.4 Update the README audit table: one `architecture_advisor` row (advisory, recommendation-based, issues-by-default) replacing the two old rows.

## 4. Triage: issues-lane routing

- [ ] 4.1 In the audit-triage completion handler, widen the keep-rule to retain either `openspec/changes/<derived-slug>/` OR `issues/<derived-slug>/` (whichever the run produced) and revert everything else by the existing per-path strategy. Commit with the lane-appropriate subject; open one PR (spec PR or issue PR).
- [ ] 4.2 Treat "content in either subtree" as success; only "no content in either subtree" flips the audit-thread to `TriageFailed`.
- [ ] 4.3 Update the `audit-triage.md` prompt: issue-by-default routing for behavior-preserving work (write `issues/<slug>/` with `issue.md` + `tasks.md`, no `specs/`); spec only for a contract change or a genuine new capability; and the guard that triage SHALL NOT author a requirement whose content is an audit metric/threshold. Permit `issues/<slug>/` in the prompt's scope restriction.

## 5. Peripheral canonical name-purge (remove the two slugs / name the advisor)

These canonical requirements reference the removed slugs and must be updated in
the same change so canon stays internally consistent (the spec deltas in this
change cover the load-bearing requirements; these are the mechanical
follow-through):

- [ ] 5.1 `orchestrator-cli` "Audit cadence config schema" — the example scenario uses `architecture_brightline`; reword to a surviving slug.
- [ ] 5.2 `orchestrator-cli` "Periodic audits enforce their per-audit subprocess timeout" — the audit list names `architecture_consultative_audit`; replace with `architecture_advisor` and update the matching scenario.
- [ ] 5.3 `orchestrator-cli` "Install wizard configures periodic audits" — the wizard's audit defaults/flags reference both slugs; replace with `architecture_advisor`.
- [ ] 5.4 `orchestrator-cli` "LLM-driven audits validate their proposal" (the `openspec validate <slug> --strict` requirement) — drop `architecture_consultative` from the list; `architecture_advisor` is advisory and writes no change dir, so it is not added.
- [ ] 5.5 `orchestrator-cli` "Audit proposal-created notification" — drop `architecture_consultative`/`architecture_brightline` from the proposal-creating list.
- [ ] 5.6 `orchestrator-cli` audit-substring-match scenarios (the `arch` ambiguity + the "registered:" list) — update to the six-slug set; `arch` no longer matches two architecture audits.
- [ ] 5.7 `chatops-manager` — remove the `architecture_brightline` `📐` top-line, the brightline stale-ignore-clause requirement, and the `📋` consultative entry from the emoji conventions; add the advisor's notification form.
- [ ] 5.8 `project-documentation` — remove the "OPERATIONS.md describes the `.brightline-ignore` file" requirement; replace the OPERATIONS.md/CHATOPS.md architecture-audit sections with the advisor.

## 6. Docs

- [ ] 6.1 Replace the `architecture_brightline` / `architecture_consultative` sections in `docs/OPERATIONS.md` and `docs/CHATOPS.md` with one `architecture_advisor` section: what it selects, that it recommends rather than counts, and that `send it` produces an issue by default.

## 7. Tests

- [ ] 7.1 Selector: the advisor picks only the longest files over the threshold, capped; a short file with a long-ish function is not separately flagged (no function-length metric); the line count never appears as a finding.
- [ ] 7.2 Output: findings are capped at 5, carry an anchor, and a clean run returns `Reported(vec![])` with the examined set logged.
- [ ] 7.3 Registry: startup registers exactly the six slugs; each removed slug (`architecture_brightline`, `architecture_consultative`, `dependency_update_triage`) fails `validate_audit_type_names`.
- [ ] 7.4 Triage routing: a behavior-preserving refactor triage keeps `issues/<slug>/` and opens an issue PR with no `specs/`; a contract-changing cleanup keeps `openspec/changes/<slug>/`; "no content in either subtree" flips to `TriageFailed`; the out-of-scope revert mechanics are unchanged (regression on the existing scenarios).
- [ ] 7.5 No build/test references the removed `architecture_brightline` / `architecture_consultative` / `.brightline-ignore` symbols.
