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

- [ ] 3.1 In `AuditRegistry` startup registration, remove `architecture_brightline` and `architecture_consultative`; add `architecture_advisor`. The pre-existing `documentation_audit` and the two canon audits stay registered, so the registered set is now seven (`architecture_advisor`, `drift_audit`, `missing_tests_audit`, `security_bug_audit`, `documentation_audit`, `canon_contradiction_audit`, `canon_consolidation_audit`).
- [ ] 3.2 Update `validate_audit_type_names`' known-slug list to the seven slugs; the two removed slugs (plus `dependency_update_triage`) are rejected at startup with the existing error.
- [ ] 3.3 Update `config.example.yaml` audit defaults: drop the two slugs, add `architecture_advisor` with a sensible cadence and its settings keys.
- [ ] 3.4 Update the README audit table: one `architecture_advisor` row (advisory, recommendation-based, issues-by-default) replacing the two old rows.

## 4. Triage: issues-lane routing

- [ ] 4.1 In the audit-triage completion handler, widen the keep-rule to retain either `openspec/changes/<derived-slug>/` OR `issues/<derived-slug>/` (whichever the run produced) and revert everything else by the existing per-path strategy. Commit with the lane-appropriate subject; open one PR (spec PR or issue PR).
- [ ] 4.2 Treat "content in either subtree" as success; only "no content in either subtree" flips the audit-thread to `TriageFailed`.
- [ ] 4.3 Update the `audit-triage.md` prompt: issue-by-default routing for behavior-preserving work (write `issues/<slug>/` with `issue.md` + `tasks.md`, no `specs/`); spec only for a contract change or a genuine new capability; and the guard that triage SHALL NOT author a requirement whose content is an audit metric/threshold. Permit `issues/<slug>/` in the prompt's scope restriction.

## 5. Name-purge: code changes behind the peripheral spec deltas

The spec deltas in this change already purge the two slugs from canon (the
`orchestrator-cli` cadence-schema, subprocess-timeout, install-wizard,
validate-proposal, proposal-created-notification, and `audit`-verb requirements;
the `chatops-manager` emoji top-line + doc-audit-emoji requirements; the removed
`chatops-manager` stale-ignore-clause and `project-documentation`
`.brightline-ignore` requirements). These tasks are the corresponding code:

- [ ] 5.1 Subprocess-timeout: the CLI-spawning audit set in code now includes `architecture_advisor` and drops `architecture_consultative`; the timeout error/log names `architecture_advisor`.
- [ ] 5.2 Install wizard: the audit defaults/flags offer `architecture_advisor` (one slug) in place of `architecture_brightline` + `architecture_consultative`; the `--audit-architecture-advisor` flag replaces the two old flags; the fast-path enables five audits (spec-sync + four LLM-driven).
- [ ] 5.3 Validate-proposal + proposal-created-notification: `architecture_advisor` is advisory (writes no change dir), so it is excluded from both the validate list and the `🔍 created proposal` list; `architecture_consultative` is removed from both.
- [ ] 5.4 `audit`-verb substring matching: the registered-name list and ambiguity/unknown replies reflect the seven-slug set (including `documentation_audit`); `arch` no longer matches two architecture audits.
- [ ] 5.5 Chatops top-line formatter: `architecture_advisor` uses the `🏛 … <N> refactor recommendation(s)` form; the `📐` brightline and `📋` consultative forms and the stale-ignore clause are removed.
- [ ] 5.6 Advisory-audit MCP transport: `architecture_advisor` advertises `submit_findings` with the architecture finding schema (`{subject, body, anchor, severity}`, cap 5) under `ORCH_MCP_ROLE = architecture_advisor`, replacing `architecture_consultative` in the advisory-role set; a clean run submits an empty array (→ `Reported(vec![])`), no submission is still a failure.
- [ ] 5.7 Code reviewer: the size-observation thresholds reference the `Source files and functions stay within a size budget` requirement's configured values, not "the values the architecture-brightline audit applies"; reviewer behavior (advisory, non-blocking) is unchanged.
- [ ] 5.8 Docs/standards prose: the `Source files and functions stay within a size budget` and `The orchestrator polling loop is decomposed by responsibility` requirements now name the `architecture_advisor` (+ drift / code review) as the surfacing mechanism; update any code comments / doc generators that echo the old audit names. The size budget remains the single advisory, non-gating home — the new "Audit findings do not mint new canonical metric requirements" guard defers to it rather than forbidding it.

## 5a. Deliberately out of scope (noted, not done here)

- [ ] 5a.1 `chatops-manager` "Status reply always shows live workspace snapshot" uses `architecture_consultative` only as an illustrative example log filename inside one of its ~15 scenarios. It is cosmetic (the scenario exercises status-line parsing, not the audit's existence) and reproducing that large requirement to swap one example string is not worth it here; swap the example slug opportunistically in a later docs/cleanup pass.
- [ ] 5a.2 The "Install wizard configures periodic audits" requirement is independently drifted from the registered set (it offers the unregistered `spec_sync_audit` and omits `canon_contradiction_audit` / `canon_consolidation_audit`). This change only purges the two architecture slugs it deletes; the wizard's broader drift is its own cleanup.

## 6. Docs

- [ ] 6.1 Replace the `architecture_brightline` / `architecture_consultative` sections in `docs/OPERATIONS.md` and `docs/CHATOPS.md` with one `architecture_advisor` section: what it selects, that it recommends rather than counts, and that `send it` produces an issue by default.

## 7. Tests

- [ ] 7.1 Selector: the advisor picks only the longest files over the threshold, capped; a short file with a long-ish function is not separately flagged (no function-length metric); the line count never appears as a finding.
- [ ] 7.2 Output: findings are capped at 5, carry an anchor, and a clean run returns `Reported(vec![])` with the examined set logged.
- [ ] 7.3 Registry: startup registers exactly the seven slugs (`documentation_audit` remains registered); each removed slug (`architecture_brightline`, `architecture_consultative`, `dependency_update_triage`) fails `validate_audit_type_names`.
- [ ] 7.4 Triage routing: a behavior-preserving refactor triage keeps `issues/<slug>/` and opens an issue PR with no `specs/`; a contract-changing cleanup keeps `openspec/changes/<slug>/`; "no content in either subtree" flips to `TriageFailed`; the out-of-scope revert mechanics are unchanged (regression on the existing scenarios).
- [ ] 7.5 No build/test references the removed `architecture_brightline` / `architecture_consultative` / `.brightline-ignore` symbols.
