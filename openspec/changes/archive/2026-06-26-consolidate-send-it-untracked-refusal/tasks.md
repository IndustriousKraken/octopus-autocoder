# Tasks

## 1. Add the canonical dispatcher requirement

- [x] 1.1 Add `Inbound listener dispatches send it by thread context AND refuses untracked threads` to the `chatops-manager` spec as the single owner of: the four-set lookup order (audit → brownfield-survey → issue-candidate → revision, at most one match), the untracked-thread refusal with its verbatim text, AND the top-level `send it` → `?` fallback.

## 2. Slim the per-context routing requirements

- [x] 2.1 `chatops-manager` brownfield-survey routing: drop the restated four-set lookup prose AND the untracked-thread scenario; keep ONLY the brownfield-survey positive branch (the `BrownfieldBatchAction` submission AND the already-running guard); cite the dispatcher requirement for the lookup order AND refusal.
- [x] 2.2 `chatops-manager` spec-revision routing: drop the duplicate untracked-thread scenario; keep the revision positive branch (run the executor) AND the advisor-routing scenario; cite the dispatcher requirement.
- [x] 2.3 `chatops-manager` issue-candidate routing: drop the restated four-set lookup prose AND the untracked-thread scenario; keep the issue-candidate positive branch (promote / already-promoted); cite the dispatcher requirement.
- [x] 2.4 `orchestrator-cli` audit `send it` requirement: drop the "Send-it in untracked thread is politely refused" scenario (its verbatim text now lives in the dispatcher requirement); keep every audit-specific scenario (tracked-open schedules triage, stale, already-acted, TriageFailed re-attempts); cite the dispatcher requirement for the cross-context dispatch AND refusal.

## 3. Verify no behavior change

- [x] 3.1 Confirm the listener's existing dispatch code already implements the single path the dispatcher requirement now describes (the four-set lookup order + the verbatim untracked refusal). This consolidation is spec-only; if the code diverges from the consolidated spec, that divergence is a separate finding, NOT introduced here.
- [x] 3.2 Confirm no remaining canonical requirement restates the four-set lookup OR the untracked-thread refusal text outside the new dispatcher requirement (grep the specs).
