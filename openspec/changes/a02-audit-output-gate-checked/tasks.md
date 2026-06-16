# Tasks

## 1. Invoke the [in]/[canon] checks from the audit write-loop

- [ ] 1.1 Expose the verifier framework's `[in]` and `[canon]` contradiction checks as a function the audit harness can call against a single just-written change directory (reuse the existing `agentic_run`-based check, its prompts, and the `submit_contradictions` / `submit_canon_contradictions` MCP tools). The check returns the structured findings (empty = clean) or a gate-failure error.
- [ ] 1.2 In `specs_writing.rs`, after a spec-lane change passes `validate_change(--strict)` for a unit, run the enabled gate checks (`[in]` when `change_internal_contradiction_check` is enabled, `[canon]` when `change_canonical_contradiction_check` is enabled) against that unit. Treat a non-empty finding the same as a `--strict` failure: record it in `failures` and feed its narrative into the next attempt's prompt addendum.
- [ ] 1.3 Honor the gate-failure (could-not-run) posture: a gate that fails to run (transport/parse/no-submission) is NOT treated as "clean"; surface it and do not commit the unit on a fail-to-run (consistent with the gates' fail-closed posture). Do not silently proceed.

## 2. Self-heal resolutions and re-routing

- [ ] 2.1 The retry addendum SHALL present the contradiction findings and the permitted resolutions (align to canon, legible `MODIFIED`, or convert to an issue) so the rewrite is directed. Reuse the existing delete-and-rewrite per-attempt cleanup.
- [ ] 2.2 Support lane re-routing within the loop: a unit the agent converts from a change to an issue is re-checked under the issue contract-change check on the next attempt; a unit converted from an issue to a change is re-checked under `--strict` + `[in]` + `[canon]`.

## 3. Issue-lane contract-change check

- [ ] 3.1 Implement the authoring-time contract-change check: an `agentic_run` read-only session that reads `issue.md` and the relevant canon and submits a verdict on whether implementing the issue requires a contract change. Run it for each issue-lane unit when `change_canonical_contradiction_check` is enabled.
- [ ] 3.2 On a positive finding, drive the re-route to the spec lane through the retry loop; on an unresolved finding at budget exhaustion, do not commit the unit.
- [ ] 3.3 Reuse the existing issue-flavored canon-verification prompt framing where practical so the authoring-time check and the implement-time kick-back judge by the same criteria.

## 4. Fail-closed outcome

- [ ] 4.1 On budget exhaustion with an unresolved contradiction (spec lane) or unresolved contract-change (issue lane), resolve that unit to `AuditOutcome::DidNotComplete` with the found-but-could-not-persist cause; do not commit it; surface via the existing audit-failure chatops path. A clean unit produced in the same run still commits.

## 5. Tests

- [ ] 5.1 A written change with a seeded `[canon]` contradiction drives a retry with the finding in the addendum; a rewrite that aligns to canon passes and commits (behavior: the committed unit no longer contradicts; assert via the gate result, not prompt wording).
- [ ] 5.2 Budget exhaustion on an unresolved contradiction yields `DidNotComplete` and no commit; a clean sibling unit in the same run still commits.
- [ ] 5.3 With the `[canon]` gate disabled, neither the spec-lane `[canon]` check nor the issue contract-change check runs at authoring time; commits follow the a01 structural rules.
- [ ] 5.4 An issue whose fix would require a contract change is re-routed to the spec lane (the committed unit is a `openspec/changes/<slug>/`, not an `issues/<slug>/`); an honest issue commits as an issue.
- [ ] 5.5 A gate that fails to run (could-not-run) does not commit the unit and surfaces the failure (fail-closed), distinct from a clean empty result.

## 6. Docs

- [ ] 6.1 Update `docs/OPERATIONS.md` (audit + contradiction-check sections) to note that the spec-writing audits run the enabled `[in]`/`[canon]` checks at authoring time and self-heal, that issues are checked for hidden contract changes, and that an unresolved contradiction fails the unit closed rather than committing it.
